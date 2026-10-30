use chrono::Utc;
use cw_core::ir::{ContextNode, ContextTurn, ProviderKind};
use cw_store::MutationEffectivenessRecord;
use serde_json::{Value, json};
use std::time::Instant;

use crate::{
    sse::extract_cached_tokens_from_json,
    state::{DaemonState, WsEvent},
};

pub struct Recorder {
    pub state: DaemonState,
    pub turn: Option<ContextTurn>,
    pub provider: ProviderKind,
    pub previous: Option<Vec<ContextNode>>,
    pub started: Instant,
    pub usage: Option<Value>,
    pub sse: bool,
    pub buffer: Vec<u8>,
}

impl Recorder {
    pub fn new(
        state: DaemonState,
        turn: Option<ContextTurn>,
        provider: ProviderKind,
        previous: Option<Vec<ContextNode>>,
        started: Instant,
    ) -> Self {
        Self {
            state,
            turn,
            provider,
            previous,
            started,
            usage: None,
            sse: false,
            buffer: Vec::new(),
        }
    }

    pub fn tap(&mut self, bytes: &[u8]) {
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
                if let Some(data) = line.strip_prefix(b"data:")
                    && let Ok(value) = serde_json::from_slice::<Value>(data)
                {
                    self.read_usage(&value);
                }
            }
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
        let mut turn = self.turn.take()?;
        turn.outcome = Some(outcome.into());
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
        Some(tokio::task::spawn_blocking(move || {
            if let Err(error) = state.store.save_turn(&turn, None, None, None) {
                tracing::error!(%error,"Unable to finalize provider attempt");
                return;
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
            self.finish("cancelled");
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
