use chrono::Utc;
use cw_core::ir::{ContextNode, ContextTurn, ProviderKind};
use cw_store::MutationEffectivenessRecord;
use serde_json::{Value, json};
use std::time::Instant;

use crate::{
    sse::extract_cached_tokens_from_json,
    state::{DaemonState, WsEvent},
};

const MAX_CAPTURED_PROVIDER_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

pub struct Recorder {
    pub state: DaemonState,
    pub turn: Option<ContextTurn>,
    pub provider: ProviderKind,
    pub previous: Option<Vec<ContextNode>>,
    pub started: Instant,
    pub usage: Option<Value>,
    pub sse: bool,
    pub buffer: Vec<u8>,
    pub terminal_outcome: Option<String>,
    pub current_event: Option<String>,
    pub raw_response: Option<Vec<u8>>,
    pub response_capture_truncated: bool,
    pub response_body_complete: bool,
    pub session_terminated: bool,
    pub spend_permit: Option<cw_core::spend_guard::RequestPermit>,
}

fn trim_ascii(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map_or(0, |p| p + 1);
    if start <= end {
        &bytes[start..end]
    } else {
        &[]
    }
}

impl Recorder {
    pub fn new(
        state: DaemonState,
        turn: Option<ContextTurn>,
        provider: ProviderKind,
        previous: Option<Vec<ContextNode>>,
        started: Instant,
    ) -> Self {
        let raw_response = state.capture_wire_response.then(Vec::new);
        Self {
            state,
            turn,
            provider,
            previous,
            started,
            usage: None,
            sse: false,
            buffer: Vec::new(),
            terminal_outcome: None,
            current_event: None,
            raw_response,
            response_capture_truncated: false,
            response_body_complete: false,
            session_terminated: false,
            spend_permit: None,
        }
    }

    pub fn tap(&mut self, bytes: &[u8]) {
        if let Some(raw) = self.raw_response.as_mut() {
            if raw.len().saturating_add(bytes.len()) <= MAX_CAPTURED_PROVIDER_RESPONSE_BYTES {
                raw.extend_from_slice(bytes);
            } else {
                self.raw_response = None;
                self.response_capture_truncated = true;
            }
        }
        if let Some(turn) = self.turn.as_mut()
            && turn.first_body_byte_ms.is_none()
            && !bytes.is_empty()
        {
            turn.first_body_byte_ms = Some(self.started.elapsed().as_millis() as u64);
        }
        if self.buffer.len().saturating_add(bytes.len()) > 1_048_576 {
            self.buffer.clear();
            return;
        }
        self.buffer.extend_from_slice(bytes);
        if self.sse {
            while let Some(end) = self.buffer.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = self.buffer.drain(..=end).collect();
                self.process_sse_line(&line);
            }
        }
    }

    pub fn process_sse_line(&mut self, line: &[u8]) {
        let trimmed = trim_ascii(line);
        if trimmed.is_empty() {
            // An empty line marks the dispatch/end of an SSE event block in RFC 8895.
            // Reset the pending event type for the next block.
            self.current_event = None;
            return;
        }

        if let Some(rest) = trimmed.strip_prefix(b"event:") {
            let event_name = trim_ascii(rest);
            self.current_event = Some(String::from_utf8_lossy(event_name).to_string());
            return;
        }

        if let Some(rest) = trimmed.strip_prefix(b"data:") {
            let data = trim_ascii(rest);
            if data == b"[DONE]" {
                self.record_terminal("completed");
                return;
            }
            if let Ok(value) = serde_json::from_slice::<Value>(data) {
                self.read_usage(&value);
                self.handle_sse_data_json(&value);
            }
        }
    }

    pub fn handle_sse_data_json(&mut self, value: &Value) {
        // 1. Check event name from SSE frame header or top-level "type" field
        let event_type = self
            .current_event
            .as_deref()
            .or_else(|| value.get("type").and_then(Value::as_str));

        match event_type {
            Some("message_stop" | "response.completed" | "response.done") => {
                self.record_terminal("completed");
                return;
            }
            Some("error" | "response.failed") => {
                self.record_terminal("stream_error");
                return;
            }
            Some("response.cancelled" | "response.incomplete") => {
                self.record_terminal("cancelled");
                return;
            }
            _ => {}
        }

        // 2. Check for explicit error payload (e.g. {"error": {...}})
        if value.get("error").is_some() {
            self.record_terminal("stream_error");
            return;
        }

        // 3. Check for Ollama / llama.cpp {"done": true}
        if value.get("done").and_then(Value::as_bool) == Some(true) {
            self.record_terminal("completed");
            return;
        }

        // 4. Check OpenAI Chat completions finish_reason
        if let Some(choices) = value.get("choices").and_then(Value::as_array) {
            for choice in choices {
                if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
                    match reason {
                        "stop" | "tool_calls" | "function_call" | "length" | "end_turn" => {
                            self.record_terminal("completed");
                        }
                        "content_filter" | "error" => {
                            self.record_terminal("stream_error");
                        }
                        _ => {}
                    }
                }
            }
        }

        // 5. Check Gemini candidates finishReason
        if let Some(candidates) = value.get("candidates").and_then(Value::as_array) {
            for candidate in candidates {
                if let Some(reason) = candidate.get("finishReason").and_then(Value::as_str) {
                    match reason {
                        "STOP" | "MAX_TOKENS" => {
                            self.record_terminal("completed");
                        }
                        "SAFETY"
                        | "RECITATION"
                        | "BLOCKLIST"
                        | "PROHIBITED_CONTENT"
                        | "SPII"
                        | "MALFORMED_FUNCTION_CALL"
                        | "OTHER" => {
                            self.record_terminal("stream_error");
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    pub fn record_terminal(&mut self, outcome: &str) {
        if self.terminal_outcome.as_deref() != Some("stream_error") {
            self.terminal_outcome = Some(outcome.to_string());
        }
    }

    pub fn read_usage(&mut self, value: &Value) {
        let usage = value
            .get("usage")
            .or_else(|| value.get("usageMetadata"))
            .or_else(|| value.pointer("/message/usage"))
            .or_else(|| value.pointer("/response/usage"));
        if let Some(usage) = usage.filter(|u| u.is_object()) {
            let merged = self.usage.get_or_insert_with(|| json!({}));
            if let (Some(target), Some(source)) = (merged.as_object_mut(), usage.as_object()) {
                for (k, v) in source {
                    target.insert(k.clone(), v.clone());
                }
            }
        }
    }

    pub fn finish(&mut self, outcome: &str) -> Option<tokio::task::JoinHandle<()>> {
        if !self.sse
            && let Ok(value) = serde_json::from_slice::<Value>(&self.buffer)
        {
            self.read_usage(&value);
        }
        if let Some(permit) = self.spend_permit.take() {
            let model = self
                .turn
                .as_ref()
                .and_then(|t| t.model.clone())
                .unwrap_or_default();
            let turn_id = self
                .turn
                .as_ref()
                .map(|turn| turn.id.as_str())
                .unwrap_or_default();
            let settlement = permit.settle(self.usage.as_ref(), self.provider, &model);
            let (status, settled_usd, error) = match settlement {
                Ok(cost) => ("SETTLED", Some(cost), None),
                Err(error) => {
                    tracing::error!(%error, "SpendGuard settlement failed; killswitch engaged");
                    ("FAILED_CLOSED", None, Some(error.to_string()))
                }
            };
            if let Err(error) = self.state.store.record_spend_settlement(
                turn_id,
                status,
                settled_usd,
                self.usage.as_ref(),
                error.as_deref(),
            ) {
                tracing::error!(%error, "Unable to persist SpendGuard settlement evidence");
            }
        }
        let mut turn = self.turn.take()?;
        let effective_outcome = if let Some(ref term) = self.terminal_outcome {
            term.as_str()
        } else if outcome == "completed" && self.sse {
            "stream_error"
        } else {
            outcome
        };
        turn.outcome = Some(effective_outcome.into());
        turn.latency_ms = Some(self.started.elapsed().as_millis() as u64);
        turn.observed_usage = self.usage.take();
        let cached = turn
            .observed_usage
            .as_ref()
            .and_then(|u| extract_cached_tokens_from_json(&json!({"usage":u})));
        turn.kv_cache_report = cw_core::radix::analyze_kv_cache(
            self.previous.as_deref(),
            &mut turn.nodes,
            self.provider,
            cached,
        );
        turn.token_usage.cached_tokens = cached.unwrap_or(0);
        turn.token_usage.output_tokens = turn
            .observed_usage
            .as_ref()
            .and_then(|u| {
                u.get("output_tokens")
                    .or_else(|| u.get("completion_tokens"))
                    .or_else(|| u.get("candidatesTokenCount"))
            })
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .min(u32::MAX as u64) as u32;
        let state = self.state.clone();
        let raw_response = self.raw_response.take();
        let response_capture_truncated = self.response_capture_truncated;
        let response_body_complete = self.response_body_complete;
        let provider = self.provider;
        let is_session_ended = self.session_terminated
            || state
                .store
                .is_session_terminated(&turn.session_id)
                .unwrap_or(false);
        Some(tokio::task::spawn_blocking(move || {
            if let Err(error) = state.store.save_turn(&turn, None, None, None) {
                tracing::error!(%error,"Unable to finalize provider attempt");
                return;
            }
            if state.capture_wire_response
                && let Err(error) = state.store.save_wire_response(
                    &turn.id,
                    raw_response.as_deref().unwrap_or_default(),
                    response_body_complete,
                    response_capture_truncated,
                )
            {
                tracing::error!(%error, "Unable to persist raw provider response evidence");
            }
            for id in &turn.applied_mutation_ids {
                let record = MutationEffectivenessRecord {
                    mutation_id: id.clone(),
                    session_id: turn.session_id.clone(),
                    turn_id: turn.id.clone(),
                    tokens_saved: 0,
                    kv_cache_hit_rate: turn.kv_cache_report.hit_ratio as f64,
                    ttft_ms: None,
                    created_at: Utc::now(),
                };
                if let Err(error) = state.store.record_mutation_effectiveness(&record) {
                    tracing::error!(%error,"Unable to record mutation evidence");
                }
            }

            // CW-117C: Correlate observed provider outcome with pre-dispatch shadow decisions
            let (turn_provider, turn_model, pricing, _is_verified) =
                state.store.resolve_turn_model_and_pricing(&turn, provider);
            let outcome = cw_core::calibration::ObservedProviderOutcome::from_completed_turn(
                &turn,
                turn_provider,
                &turn_model,
                pricing.as_ref(),
                Utc::now(),
            );

            if let Ok(decisions) = state
                .store
                .list_shadow_decision_evidence(Some(&turn.session_id))
            {
                // 1. Immediate correlation for decisions on this exact request
                for dec in decisions
                    .iter()
                    .filter(|d| d.request_index == turn.sequence_number)
                {
                    let corr =
                        cw_core::calibration::correlate_decision_with_outcome(dec, Some(&outcome));
                    if let Err(err) = state.store.save_correlated_decision_outcome(&corr) {
                        tracing::warn!(%err, "Failed to persist correlated decision outcome");
                    }
                }

                // 2. Horizon continuation tracking for previous decisions within their horizon
                let prior_decisions: Vec<_> = decisions
                    .iter()
                    .filter(|d| {
                        d.request_index < turn.sequence_number
                            && turn.sequence_number <= d.request_index + d.forecast_horizon_turns
                    })
                    .collect();

                if !prior_decisions.is_empty() || is_session_ended {
                    // Fetch existing turns to construct full continuation sequence
                    if let Ok(turns) = state.store.list_turns(&turn.session_id) {
                        let mut all_outcomes = Vec::new();
                        for t in &turns {
                            let (t_provider, t_model, t_pricing, _verified) =
                                state.store.resolve_turn_model_and_pricing(t, provider);
                            let o =
                                cw_core::calibration::ObservedProviderOutcome::from_completed_turn(
                                    t,
                                    t_provider,
                                    &t_model,
                                    t_pricing.as_ref(),
                                    t.created_at,
                                );
                            all_outcomes.push(o);
                        }
                        for dec in prior_decisions {
                            let base_outcome = all_outcomes
                                .iter()
                                .find(|o| o.request_index == dec.request_index);
                            let pho = cw_core::calibration::evaluate_prospective_horizon(
                                dec,
                                base_outcome,
                                &all_outcomes,
                                dec.forecast_horizon_turns,
                                is_session_ended,
                            );
                            if let Err(err) = state.store.save_prospective_horizon_observation(&pho)
                            {
                                tracing::warn!(%err, "Failed to persist prospective horizon observation");
                            }
                        }

                        if is_session_ended {
                            for dec in decisions
                                .iter()
                                .filter(|d| d.request_index == turn.sequence_number)
                            {
                                let pho = cw_core::calibration::evaluate_prospective_horizon(
                                    dec,
                                    Some(&outcome),
                                    &all_outcomes,
                                    dec.forecast_horizon_turns,
                                    true,
                                );
                                if let Err(err) =
                                    state.store.save_prospective_horizon_observation(&pho)
                                {
                                    tracing::warn!(%err, "Failed to persist terminal prospective horizon observation");
                                }
                            }
                        }
                    }
                }
            }

            state.broadcast(WsEvent::TurnCompleted {
                session_id: turn.session_id.clone(),
                turn: Box::new(turn),
            });
        }))
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        if self.turn.is_some() {
            if self.sse && !self.buffer.is_empty() {
                let remaining: Vec<u8> = std::mem::take(&mut self.buffer);
                self.process_sse_line(&remaining);
            }
            let terminal = self.terminal_outcome.clone();
            let outcome = match terminal.as_deref() {
                Some("completed") => "completed",
                Some(term) => term,
                None => "cancelled",
            };
            self.finish(outcome);
        }
    }
}

pub struct InterceptCleanup {
    pub state: DaemonState,
    pub id: String,
}

impl Drop for InterceptCleanup {
    fn drop(&mut self) {
        let state = self.state.clone();
        let id = self.id.clone();
        tokio::spawn(async move {
            if state.intercepts.lock().await.remove(&id).is_some() {
                state.broadcast(WsEvent::FirewallResolved { intercept_id: id });
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cw_core::ir::ContextTurn;
    use cw_store::CwStore;
    use std::time::Duration;

    fn create_test_recorder(provider: ProviderKind) -> (Recorder, CwStore, String) {
        let store = CwStore::open_in_memory().unwrap();
        let state = DaemonState::new(store.clone());
        let turn_id = format!("turn_{}", uuid::Uuid::new_v4());
        let session_id = "test_session".to_string();
        store
            .upsert_session(&cw_core::ir::Session {
                id: session_id.clone(),
                parent_session_id: None,
                harness: cw_core::ir::HarnessKind::GenericProxy,
                workspace_root: None,
                model: "test-model".to_string(),
                provider,
                title: Some("Test".to_string()),
                created_at: Utc::now(),
                updated_at: Utc::now(),
            })
            .unwrap();
        let payload = match provider {
            ProviderKind::Anthropic => json!({
                "model": "claude-3-5-sonnet",
                "max_tokens": 1024,
                "messages": [{"role": "user", "content": "hello"}]
            }),
            _ => json!({
                "model": "gpt-4o",
                "messages": [{"role": "user", "content": "hello"}]
            }),
        };
        let (turn, _) = cw_core::analyze_turn_from_payload(
            turn_id.clone(),
            session_id,
            None,
            1,
            &payload,
            provider,
            None,
            None,
            None,
        )
        .unwrap();
        let mut recorder = Recorder::new(state, Some(turn), provider, None, Instant::now());
        recorder.sse = true;
        (recorder, store, turn_id)
    }

    async fn wait_for_saved_turn(store: &CwStore, turn_id: &str) -> ContextTurn {
        for _ in 0..50 {
            if let Ok(Some(turn)) = store.get_turn(turn_id) {
                return turn;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("Turn {} was not saved to store in time", turn_id);
    }

    #[tokio::test]
    async fn oversized_raw_response_is_marked_incomplete_for_evidence() {
        let (mut recorder, store, turn_id) = create_test_recorder(ProviderKind::OpenAI);
        recorder.state.capture_wire_response = true;
        recorder.raw_response = Some(Vec::new());
        recorder.tap(&vec![b'x'; MAX_CAPTURED_PROVIDER_RESPONSE_BYTES + 1]);
        recorder.response_body_complete = true;
        let task = recorder
            .finish("completed")
            .expect("turn finalization task");
        task.await.expect("turn finalization");

        assert_eq!(
            store.get_wire_response(&turn_id).unwrap(),
            Some((Vec::new(), true, true))
        );
    }

    #[tokio::test]
    async fn test_early_disconnect_with_usage_present_records_cancelled() {
        let (mut recorder, store, turn_id) = create_test_recorder(ProviderKind::Anthropic);

        // Anthropic sends message_start with token usage in the very first event
        let message_start = b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"usage\":{\"input_tokens\":42,\"output_tokens\":0}}}\n\n";
        recorder.tap(message_start);

        assert!(
            recorder.usage.is_some(),
            "Usage must be extracted from message_start"
        );
        assert_eq!(
            recorder.terminal_outcome, None,
            "No terminal event should be seen yet"
        );

        // Client disconnects early (recorder is dropped without terminal event)
        drop(recorder);

        let turn = wait_for_saved_turn(&store, &turn_id).await;
        assert_eq!(
            turn.outcome.as_deref(),
            Some("cancelled"),
            "Early disconnect with usage present must be recorded as cancelled, not completed"
        );
        assert!(
            turn.observed_usage.is_some(),
            "Observed usage should be preserved for token accounting"
        );
    }

    #[tokio::test]
    async fn test_client_disconnect_after_anthropic_message_stop_records_completed() {
        let (mut recorder, store, turn_id) = create_test_recorder(ProviderKind::Anthropic);

        let stream_data = concat!(
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"usage\":{\"input_tokens\":42,\"output_tokens\":0}}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"
        );
        recorder.tap(stream_data.as_bytes());

        assert_eq!(recorder.terminal_outcome.as_deref(), Some("completed"));

        // Client disconnects after message_stop
        drop(recorder);

        let turn = wait_for_saved_turn(&store, &turn_id).await;
        assert_eq!(
            turn.outcome.as_deref(),
            Some("completed"),
            "Disconnect after message_stop must be recorded as completed"
        );
    }

    #[tokio::test]
    async fn test_client_disconnect_after_openai_response_completed_records_completed() {
        let (mut recorder, store, turn_id) = create_test_recorder(ProviderKind::OpenAI);

        let stream_data = concat!(
            "event: response.created\ndata: {\"type\":\"response.created\"}\n\n",
            "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"status\":\"completed\",\"usage\":{\"input_tokens\":100,\"output_tokens\":25}}}\n\n"
        );
        recorder.tap(stream_data.as_bytes());

        assert_eq!(recorder.terminal_outcome.as_deref(), Some("completed"));

        // Client disconnects after response.completed
        drop(recorder);

        let turn = wait_for_saved_turn(&store, &turn_id).await;
        assert_eq!(
            turn.outcome.as_deref(),
            Some("completed"),
            "Disconnect after response.completed must be recorded as completed"
        );
        assert_eq!(turn.token_usage.output_tokens, 25);
    }

    #[tokio::test]
    async fn test_client_disconnect_after_data_done_records_completed() {
        let (mut recorder, store, turn_id) = create_test_recorder(ProviderKind::OpenAI);

        let stream_data = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"test\"},\"finish_reason\":null}]}\n\n",
            "data: [DONE]\n\n"
        );
        recorder.tap(stream_data.as_bytes());

        assert_eq!(recorder.terminal_outcome.as_deref(), Some("completed"));

        drop(recorder);

        let turn = wait_for_saved_turn(&store, &turn_id).await;
        assert_eq!(
            turn.outcome.as_deref(),
            Some("completed"),
            "Disconnect after data: [DONE] must be recorded as completed"
        );
    }

    #[tokio::test]
    async fn test_stream_failure_event_records_stream_error() {
        let (mut recorder, store, turn_id) = create_test_recorder(ProviderKind::Anthropic);

        let stream_data = concat!(
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"usage\":{\"input_tokens\":10}}}\n\n",
            "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Server overloaded\"}}\n\n"
        );
        recorder.tap(stream_data.as_bytes());

        assert_eq!(recorder.terminal_outcome.as_deref(), Some("stream_error"));

        drop(recorder);

        let turn = wait_for_saved_turn(&store, &turn_id).await;
        assert_eq!(
            turn.outcome.as_deref(),
            Some("stream_error"),
            "SSE error event must record stream_error outcome"
        );
    }

    #[tokio::test]
    async fn test_openai_response_failed_event_records_stream_error() {
        let (mut recorder, store, turn_id) = create_test_recorder(ProviderKind::OpenAI);

        let stream_data = "event: response.failed\ndata: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"message\":\"Execution failed\"}}}\n\n";
        recorder.tap(stream_data.as_bytes());

        assert_eq!(recorder.terminal_outcome.as_deref(), Some("stream_error"));

        drop(recorder);

        let turn = wait_for_saved_turn(&store, &turn_id).await;
        assert_eq!(
            turn.outcome.as_deref(),
            Some("stream_error"),
            "OpenAI response.failed event must record stream_error outcome"
        );
    }

    #[tokio::test]
    async fn test_finish_completed_does_not_mask_stream_error() {
        let (mut recorder, store, turn_id) = create_test_recorder(ProviderKind::OpenAI);

        let error_event = "data: {\"error\":{\"message\":\"Rate limit exceeded\",\"type\":\"rate_limit_error\"}}\n\n";
        recorder.tap(error_event.as_bytes());

        assert_eq!(recorder.terminal_outcome.as_deref(), Some("stream_error"));

        // Even if finish is called with "completed" because status code was 200 OK,
        // the terminal failure must take precedence
        recorder.finish("completed");

        let turn = wait_for_saved_turn(&store, &turn_id).await;
        assert_eq!(
            turn.outcome.as_deref(),
            Some("stream_error"),
            "finish(completed) must not mask an observed stream_error"
        );
    }

    #[tokio::test]
    async fn test_anthropic_stop_reason_does_not_mark_completed_before_message_stop() {
        let (mut recorder, store, turn_id) = create_test_recorder(ProviderKind::Anthropic);

        let stream_data = concat!(
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"usage\":{\"input_tokens\":15}}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n",
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":5}}\n\n"
        );
        recorder.tap(stream_data.as_bytes());

        // stop_reason was delivered, but message_stop has NOT yet arrived!
        assert_eq!(
            recorder.terminal_outcome, None,
            "Anthropic stop_reason must NOT mark stream completed before message_stop"
        );

        // Client disconnects before message_stop
        drop(recorder);

        let turn = wait_for_saved_turn(&store, &turn_id).await;
        assert_eq!(
            turn.outcome.as_deref(),
            Some("cancelled"),
            "Disconnect between message_delta and message_stop must remain cancelled"
        );
    }

    #[tokio::test]
    async fn test_incomplete_terminal_sse_frame_does_not_establish_completion() {
        let (mut recorder, store, turn_id) = create_test_recorder(ProviderKind::Anthropic);

        // Truncated frame: only event header arrived, connection broke before data payload
        let truncated_frame = b"event: message_stop\n";
        recorder.tap(truncated_frame);

        assert_eq!(
            recorder.terminal_outcome, None,
            "Incomplete terminal frame must not establish completion"
        );

        drop(recorder);

        let turn = wait_for_saved_turn(&store, &turn_id).await;
        assert_eq!(
            turn.outcome.as_deref(),
            Some("cancelled"),
            "Client disconnect on truncated terminal frame must remain cancelled"
        );
    }

    #[tokio::test]
    async fn test_sse_eof_without_terminal_event_records_stream_error() {
        let (mut recorder, store, turn_id) = create_test_recorder(ProviderKind::OpenAI);

        // Stream sent partial delta chunks but was closed prematurely at EOF without [DONE]
        let partial_chunks = "data: {\"choices\":[{\"delta\":{\"content\":\"unfinished\"}}]}\n\n";
        recorder.tap(partial_chunks.as_bytes());

        assert_eq!(recorder.terminal_outcome, None);

        // Simulating upstream HTTP 200 EOF calling finish("completed")
        recorder.finish("completed");

        let turn = wait_for_saved_turn(&store, &turn_id).await;
        assert_eq!(
            turn.outcome.as_deref(),
            Some("stream_error"),
            "HTTP 200 SSE stream ending without a terminal event must be recorded as stream_error"
        );
    }

    #[tokio::test]
    async fn test_non_sse_response_preserves_completed_behavior() {
        let (mut recorder, store, turn_id) = create_test_recorder(ProviderKind::OpenAI);
        recorder.sse = false; // standard non-streaming JSON response

        let json_body = json!({
            "id": "chatcmpl-test",
            "choices": [{"message": {"role": "assistant", "content": "Done"}}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5}
        });
        recorder.tap(serde_json::to_vec(&json_body).unwrap().as_slice());

        // Full JSON response body received, EOF reached on HTTP 200
        recorder.finish("completed");

        let turn = wait_for_saved_turn(&store, &turn_id).await;
        assert_eq!(
            turn.outcome.as_deref(),
            Some("completed"),
            "Non-SSE response reaching EOF on HTTP 200 must be recorded as completed"
        );
        assert_eq!(turn.token_usage.output_tokens, 5);
    }
}
