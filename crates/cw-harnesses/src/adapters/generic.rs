use chrono::{DateTime, Utc};
use cw_core::{
    analyze_turn_from_payload,
    ir::{HarnessKind, ProviderKind, Session},
    mutation::apply_mutations_to_payload,
};
use cw_store::CwStore;
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader},
    path::Path,
};

use crate::{auto_heal::check_and_apply_auto_heal, error::HarnessError};

/// Extracts plain text representation from Codex content field.
pub(crate) fn extract_codex_content_text(val: Option<&Value>) -> String {
    let Some(v) = val else {
        return String::new();
    };
    match v {
        Value::String(s) => s.clone(),
        Value::Array(arr) => arr
            .iter()
            .filter_map(|item| {
                item.get("text")
                    .or_else(|| item.get("summary_text"))
                    .and_then(|t| t.as_str())
            })
            .collect::<Vec<_>>()
            .join("\n"),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

/// Imports a Claude Code / Codex Rollout / OpenCode / Pi JSONL session transcript file into `CwStore`.
pub fn import_generic_jsonl_transcript(
    path: &Path,
    harness: HarnessKind,
    store: &CwStore,
    workspace_root: Option<String>,
) -> Result<String, HarnessError> {
    let mut harness = harness;
    let path_str = path.to_string_lossy();
    if harness == HarnessKind::Pi && path_str.contains("/.omp/") {
        harness = HarnessKind::Omp;
    }
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("session");
    // For Codex `rollout-2026-10-19T09-46-31-01a15320-58da-...`, use the UUID suffix so sessions don't collide on `rollout-2026`
    let id_slug = if stem.starts_with("rollout-") && stem.len() > 28 {
        stem.get(28..).unwrap_or(stem)
    } else if let Some((_, uuid_part)) = stem.split_once('_') {
        uuid_part
    } else {
        stem
    };
    let mut session_id = format!(
        "{}_{}",
        harness.as_str(),
        id_slug.chars().take(12).collect::<String>()
    );

    let file = fs::File::open(path)?;
    let file_len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let lines_to_parse: Vec<String> = if file_len > 2_000_000 {
        use std::io::{Read, Seek, SeekFrom};
        let mut head_reader = BufReader::new(&file);
        let mut selected = Vec::with_capacity(600);
        for _ in 0..45 {
            let mut l = String::new();
            if head_reader.read_line(&mut l).unwrap_or(0) == 0 {
                break;
            }
            selected.push(l);
        }
        let mut tail_file = fs::File::open(path)?;
        let _ = tail_file.seek(SeekFrom::Start(file_len.saturating_sub(1_200_000)));
        let mut tail_buf = String::new();
        let _ = tail_file.read_to_string(&mut tail_buf);
        // Skip first partial line after seek
        for l in tail_buf.lines().skip(1) {
            selected.push(l.to_string());
        }
        selected
    } else {
        BufReader::new(file).lines().map_while(Result::ok).collect()
    };

    let mut messages = Vec::new();
    let mut system_chunks: Vec<String> = Vec::new();
    let mut turn_counter = 0u32;
    let mut detected_workspace = workspace_root;
    let mut detected_model = if harness == HarnessKind::Codex {
        "gpt-5-codex".to_string()
    } else {
        "claude-sonnet-4".to_string()
    };
    let mut detected_provider = if harness == HarnessKind::Codex {
        ProviderKind::OpenAI
    } else {
        ProviderKind::Anthropic
    };
    let mut title: Option<String> = None;
    let mut first_ts: Option<DateTime<Utc>> = None;
    let mut last_ts: Option<DateTime<Utc>> = None;
    let mut latest_input_tokens: Option<u32> = None;
    let mut latest_cache_read_tokens: Option<u32> = None;
    let mut latest_output_tokens: u32 = 0;

    for (line_idx, line) in lines_to_parse.into_iter().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<Value>(&line) else {
            continue;
        };

        let ts_str = entry
            .get("timestamp")
            .or_else(|| entry.get("created_at"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        if let Some(ref ts) = ts_str
            && let Ok(dt) = DateTime::parse_from_rfc3339(ts)
        {
            let utc_dt = dt.with_timezone(&Utc);
            if first_ts.is_none() {
                first_ts = Some(utc_dt);
            }
            last_ts = Some(utc_dt);
        }

        if detected_workspace.is_none()
            && let Some(cwd) = entry.get("cwd").and_then(|v| v.as_str())
            && !cwd.is_empty()
        {
            detected_workspace = Some(cwd.to_string());
        }

        let entry_type = entry.get("type").and_then(|v| v.as_str()).unwrap_or("");

        if entry_type == "title" {
            if let Some(t) = entry.get("title").and_then(|v| v.as_str())
                && !t.trim().is_empty()
            {
                title = Some(t.trim().to_string());
            }
            continue;
        }

        // 1. Codex CLI Rollout format (`session_meta`, `turn_context`, `event_msg`, `response_item`)
        if entry_type == "session" {
            if let Some(id) = entry.get("id").and_then(|v| v.as_str())
                && !id.is_empty()
            {
                session_id = format!("{}_{}", harness.as_str(), &id[..id.len().min(12)]);
            }
            if let Some(cwd) = entry.get("cwd").and_then(|v| v.as_str())
                && !cwd.is_empty()
            {
                detected_workspace = Some(cwd.to_string());
            }
            continue;
        }

        if entry_type == "model_change" {
            if let Some(m) = entry.get("model").and_then(|v| v.as_str())
                && !m.is_empty()
            {
                detected_model = m.to_string();
            }
            continue;
        }

        if entry_type == "session_meta" || entry_type == "turn_context" {
            if let Some(p) = entry.get("payload") {
                if let Some(sid) = p.get("session_id").and_then(|v| v.as_str())
                    && !sid.is_empty()
                {
                    session_id = format!("{}_{}", harness.as_str(), &sid[..sid.len().min(12)]);
                }
                if let Some(cwd) = p.get("cwd").and_then(|v| v.as_str())
                    && !cwd.is_empty()
                {
                    detected_workspace = Some(cwd.to_string());
                }
                if let Some(m) = p.get("model").and_then(|v| v.as_str())
                    && !m.is_empty()
                {
                    detected_model = m.to_string();
                }
                if let Some(prov) = p.get("model_provider").and_then(|v| v.as_str()) {
                    if prov.contains("ollama") || prov.contains("local") {
                        detected_provider = ProviderKind::OpenAICompatible;
                    } else if prov.contains("openai") {
                        detected_provider = ProviderKind::OpenAI;
                    }
                }
            }
            continue;
        }

        if entry_type == "compacted" {
            let win_num = entry
                .get("payload")
                .and_then(|p| p.get("window_number"))
                .and_then(|w| w.as_u64())
                .unwrap_or(1);
            let rep_len = entry
                .get("payload")
                .and_then(|p| p.get("replacement_history"))
                .and_then(|r| r.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            messages.clear();
            turn_counter = turn_counter.max(win_num as u32) + 1;
            messages.push(json!({
                "role": "assistant",
                "content": [{
                    "type": "text",
                    "text": format!("[cw:folded_turns 1..{}] Codex Compacted Context Window #{} ({} replacement items retained)", turn_counter, win_num, rep_len)
                }],
                "cw_timestamp": ts_str,
                "cw_turn_seq": turn_counter
            }));
            continue;
        }

        if entry_type == "event_msg" {
            if let Some(p) = entry.get("payload")
                && p.get("type").and_then(|v| v.as_str()) == Some("token_count")
                && let Some(last_usage) = p.get("info").and_then(|i| i.get("last_token_usage"))
            {
                if let Some(inp) = last_usage.get("input_tokens").and_then(|v| v.as_u64()) {
                    latest_input_tokens = Some(inp as u32);
                }
                if let Some(cached) = last_usage
                    .get("cached_input_tokens")
                    .and_then(|v| v.as_u64())
                {
                    latest_cache_read_tokens = Some(cached as u32);
                }
                if let Some(out) = last_usage.get("output_tokens").and_then(|v| v.as_u64()) {
                    latest_output_tokens = out as u32;
                }
            }
            continue;
        }

        if entry_type == "response_item" {
            let Some(p) = entry.get("payload") else {
                continue;
            };
            let item_type = p.get("type").and_then(|v| v.as_str()).unwrap_or("");
            match item_type {
                "message" => {
                    let role = p.get("role").and_then(|v| v.as_str()).unwrap_or("user");
                    let text = extract_codex_content_text(p.get("content"));
                    if text.trim().is_empty() {
                        continue;
                    }
                    if role == "developer"
                        || text.starts_with("# AGENTS.md")
                        || text.starts_with("<INSTRUCTIONS>")
                    {
                        if system_chunks.len() < 4 {
                            system_chunks.push(text);
                        }
                    } else if role == "user" {
                        if !text.starts_with("<codex_internal_context") {
                            turn_counter += 1;
                            if title.is_none() {
                                let one_line =
                                    text.split_whitespace().collect::<Vec<_>>().join(" ");
                                title = Some(one_line.chars().take(78).collect());
                            }
                        } else if turn_counter == 0 {
                            turn_counter = 1;
                        }
                        messages.push(json!({
                            "role": "user",
                            "content": text,
                            "cw_timestamp": ts_str,
                            "cw_turn_seq": turn_counter.max(1)
                        }));
                    } else {
                        messages.push(json!({
                            "role": "assistant",
                            "content": [{"type": "text", "text": text}],
                            "cw_timestamp": ts_str,
                            "cw_turn_seq": turn_counter.max(1)
                        }));
                    }
                }
                "reasoning" => {
                    let summary_txt = extract_codex_content_text(p.get("summary"));
                    let thought = if !summary_txt.trim().is_empty() {
                        summary_txt
                    } else {
                        "[Codex encrypted reasoning trace]".to_string()
                    };
                    messages.push(json!({
                        "role": "assistant",
                        "content": [{"type": "thinking", "thinking": thought}],
                        "cw_timestamp": ts_str,
                        "cw_turn_seq": turn_counter.max(1)
                    }));
                }
                "function_call" | "custom_tool_call" => {
                    let call_id = p
                        .get("call_id")
                        .or_else(|| p.get("id"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| format!("codex_call_{line_idx}"));
                    let name = p.get("name").and_then(|v| v.as_str()).unwrap_or("exec");
                    let raw_args = p
                        .get("arguments")
                        .or_else(|| p.get("input"))
                        .cloned()
                        .unwrap_or(Value::Null);
                    let input_val = match raw_args {
                        Value::String(ref s) => serde_json::from_str::<Value>(s)
                            .unwrap_or_else(|_| json!({ "CommandLine": s })),
                        other => other,
                    };
                    messages.push(json!({
                        "role": "assistant",
                        "content": [{
                            "type": "tool_use",
                            "id": call_id,
                            "name": name,
                            "input": input_val
                        }],
                        "cw_timestamp": ts_str,
                        "cw_turn_seq": turn_counter.max(1)
                    }));
                }
                "function_call_output" | "custom_tool_call_output" => {
                    let call_id = p
                        .get("call_id")
                        .or_else(|| p.get("id"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| format!("codex_res_{line_idx}"));
                    let out_txt = extract_codex_content_text(p.get("output"));
                    messages.push(json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": call_id,
                            "content": out_txt
                        }],
                        "cw_timestamp": ts_str,
                        "cw_turn_seq": turn_counter.max(1)
                    }));
                }
                "agent_message" => {
                    let author = p
                        .get("author")
                        .and_then(|v| v.as_str())
                        .unwrap_or("subagent");
                    let text = extract_codex_content_text(p.get("content"));
                    if !text.trim().is_empty() {
                        messages.push(json!({
                            "role": "assistant",
                            "content": [{"type": "text", "text": format!("[{author}] {text}")}],
                            "cw_timestamp": ts_str,
                            "cw_turn_seq": turn_counter.max(1)
                        }));
                    }
                }
                _ => {}
            }
            continue;
        }

        // 2. Claude Code / OpenCode / Pi message format
        if let Some(msg) = entry.get("message") {
            if let Some(m) = msg.get("model").and_then(|v| v.as_str())
                && !m.is_empty()
            {
                detected_model = m.to_string();
            }
            if let Some(prov) = msg.get("provider").and_then(|v| v.as_str())
                && (prov.contains("machinist") || prov.contains("local") || prov.contains("ollama"))
            {
                detected_provider = ProviderKind::OpenAICompatible;
            }
            if let Some(api) = msg.get("api").and_then(|v| v.as_str())
                && api.contains("openai")
            {
                detected_provider = ProviderKind::OpenAICompatible;
            }
            if let Some(u) = msg.get("usage") {
                let cached = u
                    .get("cache_read_input_tokens")
                    .or_else(|| u.get("cacheRead"))
                    .and_then(|v| v.as_u64());
                if let Some(c) = cached {
                    latest_cache_read_tokens = Some(c as u32);
                }
                let total_prompt = msg
                    .get("contextSnapshot")
                    .and_then(|cs| cs.get("promptTokens"))
                    .and_then(|v| v.as_u64())
                    .or_else(|| {
                        let inp = u
                            .get("input_tokens")
                            .or_else(|| u.get("input"))
                            .and_then(|v| v.as_u64())?;
                        Some(inp + cached.unwrap_or(0))
                    });
                if let Some(inp) = total_prompt {
                    latest_input_tokens = Some(inp as u32);
                }
                if let Some(out) = u
                    .get("output_tokens")
                    .or_else(|| u.get("output"))
                    .and_then(|v| v.as_u64())
                {
                    latest_output_tokens = out as u32;
                }
            }
            let role_val = msg.get("role").and_then(|r| r.as_str()).unwrap_or("");
            if role_val == "user" || role_val == "assistant" {
                turn_counter += 1;
            }
            if role_val == "user" && title.is_none() {
                if let Some(c) = msg.get("content").and_then(|v| v.as_str())
                    && !c.trim().is_empty()
                {
                    let one_line = c.split_whitespace().collect::<Vec<_>>().join(" ");
                    title = Some(one_line.chars().take(78).collect());
                } else if let Some(arr) = msg.get("content").and_then(|v| v.as_array()) {
                    for item in arr {
                        if item.get("type").and_then(|t| t.as_str()) == Some("text")
                            && let Some(t) = item.get("text").and_then(|s| s.as_str())
                            && !t.trim().is_empty()
                        {
                            let one_line = t.split_whitespace().collect::<Vec<_>>().join(" ");
                            title = Some(one_line.chars().take(78).collect());
                            break;
                        }
                    }
                }
            }
            let mut m_clone = msg.clone();
            if let Some(obj) = m_clone.as_object_mut() {
                if let Some(ref ts) = ts_str {
                    obj.insert("cw_timestamp".to_string(), Value::String(ts.clone()));
                }
                obj.insert("cw_turn_seq".to_string(), json!(turn_counter.max(1)));
            }
            messages.push(m_clone);
        } else if entry.get("role").is_some() {
            if entry.get("role").and_then(|r| r.as_str()) == Some("user") {
                turn_counter += 1;
            }
            messages.push(entry);
        }
    }

    if messages.is_empty() {
        return Ok(session_id);
    }

    let created_at = first_ts.unwrap_or_else(Utc::now);
    let updated_at = last_ts.unwrap_or(created_at);
    let system_text = if system_chunks.is_empty() {
        format!("{} Agent Session", harness.as_str())
    } else {
        system_chunks.join("\n\n")
    };

    let payload = json!({
        "cw_analysis_incomplete": file_len > 2_000_000,
        "model": detected_model,
        "max_tokens": 8192,
        "system": system_text,
        "messages": messages
    });

    let session = Session {
        id: session_id.clone(),
        parent_session_id: None,
        harness,
        workspace_root: detected_workspace.clone(),
        model: detected_model,
        provider: detected_provider,
        title: title.or_else(|| Some(format!("{} ({id_slug})", harness.as_str()))),
        created_at,
        updated_at,
    };
    store.upsert_session(&session)?;
    if let Ok(abs_path) = fs::canonicalize(path) {
        let byte_len = fs::metadata(&abs_path).map(|m| m.len()).unwrap_or(0);
        let _ = store.set_session_source_path(&session_id, &abs_path.to_string_lossy(), byte_len);
    }

    let seq = turn_counter.max(1);
    let existing_mutations = store.list_mutations(&session_id).unwrap_or_default();
    let mut mutated_payload_opt = None;
    let mut saved_by_mutations = 0u32;
    if !existing_mutations.is_empty() {
        let mut cloned = payload.clone();
        let rep = apply_mutations_to_payload(
            &mut cloned,
            ProviderKind::Anthropic,
            &existing_mutations,
            seq,
        );
        saved_by_mutations = rep.saved_tokens;
        mutated_payload_opt = Some(cloned);
    }
    let payload_for_analysis = mutated_payload_opt.as_ref().unwrap_or(&payload);

    let turn_id = format!("turn_{}_{}", session_id, seq);
    let (mut turn, file_states) = analyze_turn_from_payload(
        turn_id,
        session_id.clone(),
        None,
        seq,
        payload_for_analysis,
        ProviderKind::Anthropic,
        None,
        latest_cache_read_tokens,
        detected_workspace.as_deref(),
    )?;
    turn.created_at = updated_at;
    turn.token_usage.output_tokens = latest_output_tokens;
    turn.token_usage.saved_by_mutations = saved_by_mutations;
    if let Some(real_inp) = latest_input_tokens
        && real_inp > turn.token_usage.input_tokens
    {
        let adjusted_inp = real_inp.saturating_sub(saved_by_mutations);
        turn.token_usage.input_tokens = adjusted_inp;
        if let Some(cached) = latest_cache_read_tokens {
            turn.token_usage.cached_tokens = cached.min(adjusted_inp);
            turn.kv_cache_report.hit_tokens = cached.min(adjusted_inp);
            turn.kv_cache_report.newly_cached_tokens = adjusted_inp.saturating_sub(cached);
            turn.kv_cache_report.hit_ratio = cached as f32 / adjusted_inp.max(1) as f32;
        }
    }
    store.save_turn(&turn, Some(&payload), mutated_payload_opt.as_ref(), None)?;
    store.save_file_states(&session_id, &file_states)?;
    check_and_apply_auto_heal(
        store,
        &session_id,
        &mut turn,
        &payload,
        seq,
        ProviderKind::Anthropic,
        detected_workspace.as_deref(),
    );
    Ok(session_id)
}
