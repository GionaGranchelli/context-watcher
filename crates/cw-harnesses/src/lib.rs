use chrono::{DateTime, Utc};
use cw_core::{
    analyze_turn_from_payload,
    ir::{HarnessKind, MutationOp, MutationStatus, NodeKind, ProviderKind, Session},
    mutation::apply_mutations_to_payload,
};
use cw_store::CwStore;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptSyncReport {
    pub session_id: String,
    pub transcript_path: String,
    pub backup_path: String,
    pub steps_compacted: usize,
    pub bytes_before: u64,
    pub bytes_after: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum HarnessError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Store error: {0}")]
    Store(#[from] cw_store::StoreError),
    #[error("Normalize error: {0}")]
    Normalize(#[from] cw_core::normalizer::NormalizeError),
    #[error("No source transcript on disk recorded for session '{0}'")]
    NoTranscriptSource(String),
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

/// Parses an Antigravity (AGY) `transcript.jsonl` file (and hydrates any `truncated_fields`
/// from the matching line in `transcript_full.jsonl`), extracting the real project workspace,
/// human-readable title, step timestamps, and provider KV cache metrics.
pub fn import_agy_transcript(
    transcript_path: &Path,
    store: &CwStore,
    fallback_workspace: Option<String>,
) -> Result<String, HarnessError> {
    let full_path = transcript_path.with_file_name("transcript_full.jsonl");
    let full_lines: Vec<String> = if full_path.exists() {
        BufReader::new(fs::File::open(&full_path)?)
            .lines()
            .map_while(Result::ok)
            .collect()
    } else {
        Vec::new()
    };

    // Derive conversation-id from `<appDataDir>/brain/<conversation-id>/.system_generated/logs/transcript.jsonl`
    let conv_id = transcript_path
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .unwrap_or("agy_session")
        .to_string();
    let session_id = format!("agy_{}", &conv_id[..conv_id.len().min(12)]);

    let reader = BufReader::new(fs::File::open(transcript_path)?);
    let mut messages = Vec::new();
    let mut system_text = String::from(
        "Antigravity Agent Harness Session\n<RULE[AGENTS.md]>\nZero-Drop Proxy Transparency & KV-Cache Awareness.\n</RULE[AGENTS.md]>",
    );
    let mut title: Option<String> = None;
    let mut detected_workspace: Option<String> = None;
    let mut detected_model = String::from("gemini-4-argon");
    let mut first_ts: Option<DateTime<Utc>> = None;
    let mut last_ts: Option<DateTime<Utc>> = None;
    let mut latest_input_tokens: Option<u32> = None;
    let mut latest_cache_read_tokens: Option<u32> = None;
    let mut latest_output_tokens: u32 = 0;

    let mut turn_counter = 0u32;
    let mut pending_call_ids: VecDeque<String> = VecDeque::new();

    for (line_idx, line_res) in reader.lines().enumerate() {
        let mut line = line_res?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(mut step) = serde_json::from_str::<Value>(&line) else {
            continue;
        };

        // If `truncated_fields` is present, read the corresponding line from `transcript_full.jsonl`
        if step
            .get("truncated_fields")
            .and_then(|t| t.as_array())
            .map(|a| !a.is_empty())
            .unwrap_or(false)
            && let Some(full_line) = full_lines.get(line_idx)
            && let Ok(full_step) = serde_json::from_str::<Value>(full_line)
        {
            step = full_step;
            let _ = &mut line;
        }

        let step_ts_str = step
            .get("created_at")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        if let Some(ref ts) = step_ts_str
            && let Ok(parsed_dt) = DateTime::parse_from_rfc3339(ts)
        {
            let utc_dt = parsed_dt.with_timezone(&Utc);
            if first_ts.is_none() {
                first_ts = Some(utc_dt);
            }
            last_ts = Some(utc_dt);
        }

        if let Some(inp) = step.get("input_tokens").and_then(|v| v.as_u64()) {
            latest_input_tokens = Some(inp as u32);
        }
        if let Some(cached) = step.get("cache_read_tokens").and_then(|v| v.as_u64()) {
            latest_cache_read_tokens = Some(cached as u32);
        }
        if let Some(out) = step.get("output_tokens").and_then(|v| v.as_u64()) {
            latest_output_tokens = out as u32;
        }

        let step_type = step.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let content_str = step
            .get("content")
            .map(|c| match c {
                Value::String(s) => s.clone(),
                other => serde_json::to_string(other).unwrap_or_default(),
            })
            .unwrap_or_default();

        // Try to infer workspace root and model from content or tool calls
        if detected_workspace.is_none() {
            detected_workspace = infer_workspace_from_text(&content_str);
        }
        if step_type == "USER_INPUT"
            && let Some(m) = extract_model_from_settings_change(&content_str)
        {
            detected_model = m;
        }

        match step_type {
            "USER_INPUT" => {
                turn_counter += 1;
                let clean_user_req = extract_clean_user_request(&content_str);
                if title.is_none() && !clean_user_req.is_empty() {
                    let one_line = clean_user_req.split_whitespace().collect::<Vec<_>>().join(" ");
                    title = Some(one_line.chars().take(78).collect());
                }
                messages.push(json!({
                    "role": "user",
                    "content": if clean_user_req.is_empty() { content_str } else { clean_user_req },
                    "cw_timestamp": step_ts_str,
                    "cw_turn_seq": turn_counter
                }));
            }
            "PLANNER_RESPONSE" => {
                let mut blocks = Vec::new();
                if let Some(thinking) = step.get("thinking").and_then(|v| v.as_str())
                    && !thinking.is_empty()
                {
                    blocks.push(json!({
                        "type": "thinking",
                        "thinking": thinking
                    }));
                }
                if !content_str.is_empty() {
                    blocks.push(json!({
                        "type": "text",
                        "text": content_str
                    }));
                }
                if let Some(tcs) = step.get("tool_calls").and_then(|v| v.as_array()) {
                    for (tc_i, tc) in tcs.iter().enumerate() {
                        let name = tc
                            .get("name")
                            .or_else(|| tc.get("tool_name"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("tool");
                        let args = tc
                            .get("arguments")
                            .or_else(|| tc.get("args"))
                            .cloned()
                            .unwrap_or(Value::Null);
                        if detected_workspace.is_none() {
                            detected_workspace = infer_workspace_from_tool_args(&args);
                        }
                        let call_id = tc
                            .get("id")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| format!("agy_call_{line_idx}_{tc_i}"));
                        pending_call_ids.push_back(call_id.clone());
                        blocks.push(json!({
                            "type": "tool_use",
                            "id": call_id,
                            "name": name,
                            "input": args
                        }));
                    }
                }
                if !blocks.is_empty() {
                    messages.push(json!({
                        "role": "assistant",
                        "content": blocks,
                        "cw_timestamp": step_ts_str,
                        "cw_turn_seq": turn_counter.max(1)
                    }));
                }
            }
            _ => {
                if !content_str.is_empty() {
                    if step.get("source").and_then(|v| v.as_str()) == Some("SYSTEM")
                        && messages.is_empty()
                    {
                        system_text = content_str;
                    } else {
                        let call_id = pending_call_ids
                            .pop_front()
                            .unwrap_or_else(|| format!("agy_res_{line_idx}"));
                        messages.push(json!({
                            "role": "user",
                            "content": [{
                                "type": "tool_result",
                                "tool_use_id": call_id,
                                "content": content_str
                            }],
                            "cw_timestamp": step_ts_str,
                            "cw_turn_seq": turn_counter.max(1)
                        }));
                    }
                }
            }
        }
    }

    if messages.is_empty() {
        return Ok(session_id);
    }

    let final_workspace = detected_workspace.or(fallback_workspace);
    let created_at = first_ts.unwrap_or_else(Utc::now);
    let updated_at = last_ts.unwrap_or(created_at);

    let synthetic_payload = json!({
        "model": detected_model,
        "max_tokens": 8192,
        "system": system_text,
        "messages": messages
    });

    let session = Session {
        id: session_id.clone(),
        parent_session_id: None,
        harness: HarnessKind::Antigravity,
        workspace_root: final_workspace.clone(),
        model: detected_model,
        provider: ProviderKind::Anthropic,
        title,
        created_at,
        updated_at,
    };
    store.upsert_session(&session)?;

    if let Ok(abs_path) = fs::canonicalize(transcript_path) {
        let byte_len = fs::metadata(&abs_path).map(|m| m.len()).unwrap_or(0);
        let _ = store.set_session_source_path(
            &session_id,
            &abs_path.to_string_lossy(),
            byte_len,
        );
    }

    let seq = turn_counter.max(1);
    let existing_mutations = store.list_mutations(&session_id).unwrap_or_default();
    let mut mutated_payload_opt = None;
    let mut saved_by_mutations = 0u32;
    if !existing_mutations.is_empty() {
        let mut cloned = synthetic_payload.clone();
        let rep = apply_mutations_to_payload(
            &mut cloned,
            ProviderKind::Anthropic,
            &existing_mutations,
            seq,
        );
        saved_by_mutations = rep.saved_tokens;
        mutated_payload_opt = Some(cloned);
    }
    let payload_for_analysis = mutated_payload_opt.as_ref().unwrap_or(&synthetic_payload);

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
        final_workspace.as_deref(),
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

    store.save_turn(
        &turn,
        Some(&synthetic_payload),
        mutated_payload_opt.as_ref(),
        None,
    )?;
    store.save_file_states(&session_id, &file_states)?;
    Ok(session_id)
}

/// Extracts only the human text inside `<USER_REQUEST>...</USER_REQUEST>`, stripping XML tags and metadata.
fn extract_clean_user_request(raw: &str) -> String {
    if let Some(start) = raw.find("<USER_REQUEST>") {
        let after = &raw[start + "<USER_REQUEST>".len()..];
        if let Some(end) = after.find("</USER_REQUEST>") {
            return after[..end].trim().to_string();
        }
    }
    // Strip any trailing <ADDITIONAL_METADATA> or <USER_SETTINGS_CHANGE>
    let without_meta = raw.split("<ADDITIONAL_METADATA>").next().unwrap_or(raw);
    without_meta.trim().to_string()
}

fn extract_model_from_settings_change(raw: &str) -> Option<String> {
    let start = raw.find("<USER_SETTINGS_CHANGE>")?;
    let after_start = &raw[start + "<USER_SETTINGS_CHANGE>".len()..];
    let end = after_start.find("</USER_SETTINGS_CHANGE>")?;
    let block = &after_start[..end];
    if let Some(idx) = block.find("Model Selection` from") {
        let rest = &block[idx..];
        if let Some(to_idx) = rest.find(" to ") {
            let after_to = &rest[to_idx + 4..];
            if let Some(dot) = after_to.find('.') {
                let model_name = after_to[..dot].trim();
                if !model_name.is_empty()
                    && model_name.len() < 48
                    && model_name.chars().all(|c| {
                        c.is_alphanumeric() || matches!(c, ' ' | '-' | '.' | '_' | '(' | ')')
                    })
                {
                    return Some(model_name.to_string());
                }
            }
        }
    }
    None
}

/// Infers project workspace root from file paths in tool arguments (e.g., `/home/user/Development/my-proj/src/lib.rs` -> `/home/user/Development/my-proj`).
fn infer_workspace_from_tool_args(args: &Value) -> Option<String> {
    for key in ["Path", "path", "DirectoryPath", "SearchPath", "CommandLine"] {
        if let Some(val) = args.get(key).and_then(|v| v.as_str())
            && let Some(ws) = extract_project_root_from_path_str(val)
        {
            return Some(ws);
        }
    }
    None
}

fn infer_workspace_from_text(text: &str) -> Option<String> {
    if let Some(idx) = text.find("file:///") {
        let after = &text[idx + 7..];
        let path_part: String = after
            .chars()
            .take_while(|c| !c.is_whitespace() && *c != ')' && *c != ']' && *c != '"' && *c != '#')
            .collect();
        if let Some(ws) = extract_project_root_from_path_str(&path_part) {
            return Some(ws);
        }
    }
    extract_project_root_from_path_str(text)
}

fn extract_project_root_from_path_str(s: &str) -> Option<String> {
    // Match `/home/<user>/<Category>/<ProjectName>` or `/Users/<user>/<Category>/<ProjectName>`
    for token in s.split(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '(' | ')' | '[' | ']'))
    {
        let clean = token.strip_prefix("file://").unwrap_or(token);
        if clean.starts_with("/home/") || clean.starts_with("/Users/") {
            if clean.contains("/.gemini/") || clean.contains("/.claude/") || clean.contains("/.local/")
            {
                continue;
            }
            let parts: Vec<&str> = clean.split('/').filter(|p| !p.is_empty()).collect();
            // e.g. ["home", "gionag", "Development", "contextWatcher", ...]
            if parts.len() >= 4
                && matches!(
                    parts[2],
                    "Development"
                        | "Projects"
                        | "repos"
                        | "work"
                        | "workspace"
                        | "code"
                        | "src"
                        | "git"
                )
            {
                return Some(format!("/{}/{}/{}/{}", parts[0], parts[1], parts[2], parts[3]));
            } else if parts.len() >= 3 {
                return Some(format!("/{}/{}/{}", parts[0], parts[1], parts[2]));
            }
        }
    }
    None
}

/// Imports a Claude Code / Codex Rollout / OpenCode / Pi JSONL session transcript file into `CwStore`.
pub fn import_generic_jsonl_transcript(
    path: &Path,
    harness: HarnessKind,
    store: &CwStore,
    workspace_root: Option<String>,
) -> Result<String, HarnessError> {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("session");
    // For Codex `rollout-2026-10-19T09-46-31-01a15320-58da-...`, use the UUID suffix so sessions don't collide on `rollout-2026`
    let id_slug = if stem.starts_with("rollout-") && stem.len() > 28 {
        &stem[28..]
    } else {
        stem
    };
    let mut session_id = format!("{}_{}", harness.as_str(), &id_slug[..id_slug.len().min(12)]);

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
        BufReader::new(file)
            .lines()
            .map_while(Result::ok)
            .collect()
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

        // 1. Codex CLI Rollout format (`session_meta`, `turn_context`, `event_msg`, `response_item`)
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
                if let Some(cached) = last_usage.get("cached_input_tokens").and_then(|v| v.as_u64())
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
                    let name = p
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("exec");
                    let raw_args = p.get("arguments").or_else(|| p.get("input")).cloned().unwrap_or(Value::Null);
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
                    let author = p.get("author").and_then(|v| v.as_str()).unwrap_or("subagent");
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
            if let Some(u) = msg.get("usage") {
                if let Some(inp) = u.get("input_tokens").and_then(|v| v.as_u64()) {
                    latest_input_tokens = Some(inp as u32);
                }
                if let Some(cached) = u.get("cache_read_input_tokens").and_then(|v| v.as_u64()) {
                    latest_cache_read_tokens = Some(cached as u32);
                }
                if let Some(out) = u.get("output_tokens").and_then(|v| v.as_u64()) {
                    latest_output_tokens = out as u32;
                }
            }
            if msg.get("role").and_then(|r| r.as_str()) == Some("user") {
                turn_counter += 1;
                if title.is_none()
                    && let Some(c) = msg.get("content").and_then(|v| v.as_str())
                    && !c.trim().is_empty()
                {
                    let one_line = c.split_whitespace().collect::<Vec<_>>().join(" ");
                    title = Some(one_line.chars().take(78).collect());
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
    Ok(session_id)
}

fn extract_codex_content_text(val: Option<&Value>) -> String {
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

/// Safely writes committed `contextWatcher` compactions back to the harness's on-disk `.jsonl`
/// transcript file (creating a `.jsonl.bak` backup first) so non-proxy harnesses (AGY, Claude Code,
/// Codex, OpenCode, Pi) immediately benefit from the reduced context on session resume.
pub fn sync_mutations_to_transcript_on_disk(
    store: &CwStore,
    session_id: &str,
) -> Result<TranscriptSyncReport, HarnessError> {
    let Some((transcript_path_str, _)) = store.get_session_source_path(session_id)? else {
        return Err(HarnessError::NoTranscriptSource(session_id.to_string()));
    };
    let transcript_path = PathBuf::from(&transcript_path_str);
    if !transcript_path.exists() {
        return Err(HarnessError::NoTranscriptSource(transcript_path_str));
    }

    let backup_path = PathBuf::from(format!("{transcript_path_str}.bak"));
    if !backup_path.exists() {
        fs::copy(&transcript_path, &backup_path)?;
    }

    let bytes_before = fs::metadata(&transcript_path).map(|m| m.len()).unwrap_or(0);
    let rules = store.list_mutations(session_id)?;
    let committed_rules: Vec<_> = rules
        .into_iter()
        .filter(|r| r.status == MutationStatus::Committed)
        .collect();

    // Collect stubbed/compacted content strings from the latest turn's nodes
    let mut compacted_by_call_id: HashMap<String, String> = HashMap::new();
    let mut folded_turns: Vec<(u32, u32, String)> = Vec::new();
    for r in &committed_rules {
        if let MutationOp::FoldSubtree {
            start_turn,
            end_turn,
            summary_text,
        } = &r.op
        {
            folded_turns.push((*start_turn, *end_turn, summary_text.clone()));
        }
    }

    if let Some(turn) = store.get_latest_turn(session_id)? {
        for node in &turn.nodes {
            if let NodeKind::ToolResult {
                call_id, content, ..
            } = &node.kind
                && (content.starts_with("[cw:pruned")
                    || content.starts_with("[cw:shadow_deduped")
                    || content.starts_with("[cw:folded")
                    || content.contains("[cw:auto_pilot"))
            {
                compacted_by_call_id.insert(call_id.clone(), content.clone());
            }
        }
    }

    let raw_text = fs::read_to_string(&transcript_path)?;
    let mut new_lines = Vec::new();
    let mut steps_compacted = 0usize;
    let mut current_turn = 0u32;

    for (line_idx, line) in raw_text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(mut val) = serde_json::from_str::<Value>(line) else {
            new_lines.push(line.to_string());
            continue;
        };

        let step_type = val
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if step_type == "USER_INPUT"
            || val.get("role").and_then(|v| v.as_str()) == Some("user")
            || val
                .get("message")
                .and_then(|m| m.get("role"))
                .and_then(|r| r.as_str())
                == Some("user")
        {
            current_turn += 1;
        }

        let synthetic_res_id = format!("agy_res_{line_idx}");
        let mut modified = false;

        // Check if this step's turn is folded
        for (start_t, end_t, summary) in &folded_turns {
            if current_turn >= *start_t
                && current_turn <= *end_t
                && step_type != "USER_INPUT"
                && let Some(obj) = val.as_object_mut()
            {
                if obj.contains_key("thinking") {
                    obj.insert(
                        "thinking".to_string(),
                        Value::String(format!(
                            "[cw:folded_turns {}..{}] {}",
                            start_t, end_t, summary
                        )),
                    );
                    modified = true;
                }
                if step_type != "PLANNER_RESPONSE" && obj.contains_key("content") {
                    obj.insert(
                        "content".to_string(),
                        Value::String(format!(
                            "[cw:folded_turns {}..{}] {}",
                            start_t, end_t, summary
                        )),
                    );
                    modified = true;
                }
            }
        }

        // Check if this step matches a compacted tool result or oversized output
        if !modified {
            if let Some(stub) = compacted_by_call_id.get(&synthetic_res_id)
                && let Some(obj) = val.as_object_mut()
            {
                obj.insert("content".to_string(), Value::String(stub.clone()));
                modified = true;
            } else if step_type != "USER_INPUT"
                && step_type != "PLANNER_RESPONSE"
                && !committed_rules.is_empty()
            {
                let content_len = val
                    .get("content")
                    .and_then(|c| c.as_str())
                    .filter(|s| s.len() > 1600 && !s.starts_with("[cw:"))
                    .map(|s| s.len());
                if let Some(len) = content_len
                    && let Some(obj) = val.as_object_mut()
                {
                    let stub = format!(
                        "[cw:synced_compaction original_bytes={len} session=\"{session_id}\"]"
                    );
                    obj.insert("content".to_string(), Value::String(stub));
                    modified = true;
                }
            }
        }

        if modified {
            steps_compacted += 1;
        }
        new_lines.push(serde_json::to_string(&val).unwrap_or_else(|_| line.to_string()));
    }

    let out_content = format!("{}\n", new_lines.join("\n"));
    fs::write(&transcript_path, &out_content)?;

    // Also sync `transcript_full.jsonl` if this is an Antigravity session
    let full_path = transcript_path.with_file_name("transcript_full.jsonl");
    if full_path.exists() {
        let full_bak = PathBuf::from(format!("{}.bak", full_path.to_string_lossy()));
        if !full_bak.exists() {
            let _ = fs::copy(&full_path, &full_bak);
        }
        let _ = fs::write(&full_path, &out_content);
    }

    let bytes_after = fs::metadata(&transcript_path).map(|m| m.len()).unwrap_or(0);
    store.set_session_source_path(session_id, &transcript_path_str, bytes_after)?;

    Ok(TranscriptSyncReport {
        session_id: session_id.to_string(),
        transcript_path: transcript_path_str,
        backup_path: backup_path.to_string_lossy().to_string(),
        steps_compacted,
        bytes_before,
        bytes_after,
    })
}

/// Spawns an incremental background watcher task that scans Antigravity (`~/.gemini/antigravity-cli/brain`),
/// Claude Code (`~/.claude/projects`), Codex (`~/.codex/sessions`), OpenCode (`~/.local/share/opencode`),
/// and Pi (`~/.pi/sessions`) transcript directories, tracking `(mtime, byte_len)` per file.
pub fn spawn_transcript_watcher(store: CwStore, _default_workspace: Option<String>) {
    tokio::spawn(async move {
        let mut file_state: HashMap<PathBuf, (SystemTime, u64)> = HashMap::new();
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        let agy_brain_dir = PathBuf::from(&home).join(".gemini/antigravity-cli/brain");
        let claude_dir = PathBuf::from(&home).join(".claude/projects");
        let codex_dir = PathBuf::from(&home).join(".codex/sessions");
        let hermes_db = PathBuf::from(&home).join(".hermes/state.db");
        let hermes_wal = PathBuf::from(&home).join(".hermes/state.db-wal");
        let opencode_dir = PathBuf::from(&home).join(".local/share/opencode");
        let pi_dir = PathBuf::from(&home).join(".pi/sessions");

        loop {
            // 1. Antigravity transcripts
            if agy_brain_dir.exists()
                && let Ok(entries) = fs::read_dir(&agy_brain_dir)
            {
                for entry in entries.flatten() {
                    let transcript = entry
                        .path()
                        .join(".system_generated/logs/transcript.jsonl");
                    if transcript.exists()
                        && let Ok(meta) = fs::metadata(&transcript)
                        && let Ok(mtime) = meta.modified()
                    {
                        let sig = (mtime, meta.len());
                        if file_state.get(&transcript).copied() != Some(sig) {
                            file_state.insert(transcript.clone(), sig);
                            let _ = import_agy_transcript(&transcript, &store, None);
                        }
                    }
                }
            }

            // 2. Hermes Agent SQLite state database (`~/.hermes/state.db` + `-wal`)
            let hermes_watch_target = if hermes_wal.exists() {
                &hermes_wal
            } else {
                &hermes_db
            };
            if hermes_db.exists()
                && let Ok(meta) = fs::metadata(hermes_watch_target)
                && let Ok(mtime) = meta.modified()
            {
                let sig = (mtime, meta.len());
                if file_state.get(&hermes_db).copied() != Some(sig) {
                    file_state.insert(hermes_db.clone(), sig);
                    let _ = import_hermes_state_db(&hermes_db, &store, 18);
                }
            }

            // 3. Claude Code, Codex, OpenCode, and Pi JSONL transcripts
            for (dir, harness) in [
                (&claude_dir, HarnessKind::ClaudeCode),
                (&codex_dir, HarnessKind::Codex),
                (&opencode_dir, HarnessKind::OpenCode),
                (&pi_dir, HarnessKind::Pi),
            ] {
                if dir.exists() {
                    scan_jsonl_dir_recursively(dir, harness, &store, &mut file_state, 0);
                }
            }

            tokio::time::sleep(Duration::from_secs(4)).await;
        }
    });
}

fn scan_jsonl_dir_recursively(
    dir: &Path,
    harness: HarnessKind,
    store: &CwStore,
    file_state: &mut HashMap<PathBuf, (SystemTime, u64)>,
    depth: u8,
) {
    if depth > 6 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            scan_jsonl_dir_recursively(&p, harness, store, file_state, depth + 1);
        } else if p.extension().and_then(|e| e.to_str()) == Some("jsonl")
            && !p.to_string_lossy().ends_with(".bak")
            && !matches!(
                p.file_name().and_then(|n| n.to_str()),
                Some("history.jsonl" | "session_index.jsonl")
            )
            && let Ok(meta) = fs::metadata(&p)
            && let Ok(mtime) = meta.modified()
        {
            let sig = (mtime, meta.len());
            if file_state.get(&p).copied() != Some(sig) {
                file_state.insert(p.clone(), sig);
                let _ = import_generic_jsonl_transcript(&p, harness, store, None);
            }
        }
    }
}

/// Imports recent active conversations from Hermes Agent's `~/.hermes/state.db` SQLite database.
pub fn import_hermes_state_db(
    db_path: &Path,
    store: &CwStore,
    max_sessions: usize,
) -> Result<Vec<String>, HarnessError> {
    let conn = rusqlite::Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;

    let mut stmt = conn.prepare(
        "SELECT
            id,
            COALESCE(source, 'cli'),
            COALESCE(model, 'hermes-model'),
            COALESCE(model_config, ''),
            COALESCE(system_prompt, ''),
            COALESCE(started_at, 0.0),
            COALESCE(last_activity_at, started_at, 0.0),
            COALESCE(input_tokens, 0),
            COALESCE(output_tokens, 0),
            COALESCE(cache_read_tokens, 0),
            COALESCE(title, display_name, ''),
            COALESCE(git_repo_root, cwd, '')
         FROM sessions
         WHERE COALESCE(message_count, 0) > 0
         ORDER BY COALESCE(last_activity_at, started_at, 0.0) DESC
         LIMIT ?1",
    )?;

    let rows = stmt.query_map(rusqlite::params![max_sessions as i64], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, f64>(5)?,
            row.get::<_, f64>(6)?,
            row.get::<_, i64>(7)?,
            row.get::<_, i64>(8)?,
            row.get::<_, i64>(9)?,
            row.get::<_, String>(10)?,
            row.get::<_, String>(11)?,
        ))
    })?;

    let mut imported_ids = Vec::new();
    let mut msg_stmt = conn.prepare(
        "SELECT
            role,
            COALESCE(content, ''),
            COALESCE(tool_call_id, ''),
            COALESCE(tool_calls, ''),
            COALESCE(tool_name, ''),
            COALESCE(timestamp, 0.0),
            COALESCE(reasoning, reasoning_content, '')
         FROM messages
         WHERE session_id = ?1 AND COALESCE(active, 1) = 1
         ORDER BY id ASC
         LIMIT 350",
    )?;

    for r in rows.flatten() {
        let (
            raw_id,
            source,
            model,
            model_cfg_str,
            sys_prompt,
            started_f,
            updated_f,
            cum_inp,
            cum_out,
            cum_cache,
            raw_title,
            raw_cwd,
        ) = r;

        let session_id = format!("hermes_{raw_id}");
        let created_at = DateTime::from_timestamp(started_f as i64, 0).unwrap_or_else(Utc::now);
        let updated_at = DateTime::from_timestamp(updated_f as i64, 0).unwrap_or(created_at);

        let anchor_prompt_tokens: Option<u32> = serde_json::from_str::<Value>(&model_cfg_str)
            .ok()
            .and_then(|v| {
                v.get("_usage_anchor")
                    .and_then(|a| a.get("prompt_tokens"))
                    .and_then(|p| p.as_u64())
                    .map(|p| p as u32)
            });

        let msg_rows = msg_stmt.query_map(rusqlite::params![&raw_id], |m| {
            Ok((
                m.get::<_, String>(0)?,
                m.get::<_, String>(1)?,
                m.get::<_, String>(2)?,
                m.get::<_, String>(3)?,
                m.get::<_, String>(4)?,
                m.get::<_, f64>(5)?,
                m.get::<_, String>(6)?,
            ))
        })?;

        let mut messages = Vec::new();
        let mut turn_counter = 0u32;
        let mut first_user_prompt: Option<String> = None;
        let mut inferred_ws: Option<String> = if !raw_cwd.trim().is_empty() {
            Some(raw_cwd)
        } else {
            None
        };

        for (m_idx, m_res) in msg_rows.enumerate() {
            let Ok((role, content, tool_call_id, tool_calls_str, tool_name, ts_f, reasoning)) =
                m_res
            else {
                continue;
            };
            let ts_iso = if ts_f > 0.0 {
                DateTime::from_timestamp(ts_f as i64, 0).map(|d| d.to_rfc3339())
            } else {
                None
            };

            if inferred_ws.is_none() {
                inferred_ws = infer_workspace_from_text(&content);
            }

            match role.as_str() {
                "user" => {
                    turn_counter += 1;
                    if first_user_prompt.is_none() && !content.trim().is_empty() {
                        let one_line = content.split_whitespace().collect::<Vec<_>>().join(" ");
                        first_user_prompt = Some(one_line.chars().take(78).collect());
                    }
                    messages.push(json!({
                        "role": "user",
                        "content": content,
                        "cw_timestamp": ts_iso,
                        "cw_turn_seq": turn_counter
                    }));
                }
                "assistant" => {
                    let mut blocks = Vec::new();
                    if !reasoning.trim().is_empty() {
                        blocks.push(json!({
                            "type": "thinking",
                            "thinking": reasoning
                        }));
                    }
                    if !content.trim().is_empty() {
                        blocks.push(json!({
                            "type": "text",
                            "text": content
                        }));
                    }
                    if !tool_calls_str.trim().is_empty()
                        && let Ok(Value::Array(tcs)) = serde_json::from_str::<Value>(&tool_calls_str)
                    {
                        for (tc_i, tc) in tcs.iter().enumerate() {
                            let cid = tc
                                .get("call_id")
                                .or_else(|| tc.get("id"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string())
                                .unwrap_or_else(|| format!("hermes_call_{m_idx}_{tc_i}"));
                            let name = tc
                                .get("function")
                                .and_then(|f| f.get("name"))
                                .or_else(|| tc.get("name"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("tool");
                            let raw_args = tc
                                .get("function")
                                .and_then(|f| f.get("arguments"))
                                .or_else(|| tc.get("arguments"))
                                .cloned()
                                .unwrap_or(Value::Null);
                            let input_val = match raw_args {
                                Value::String(ref s) => serde_json::from_str::<Value>(s)
                                    .unwrap_or_else(|_| json!({ "CommandLine": s })),
                                other => other,
                            };
                            if inferred_ws.is_none() {
                                inferred_ws = infer_workspace_from_tool_args(&input_val);
                            }
                            blocks.push(json!({
                                "type": "tool_use",
                                "id": cid,
                                "name": name,
                                "input": input_val
                            }));
                        }
                    }
                    if !blocks.is_empty() {
                        messages.push(json!({
                            "role": "assistant",
                            "content": blocks,
                            "cw_timestamp": ts_iso,
                            "cw_turn_seq": turn_counter.max(1)
                        }));
                    }
                }
                "tool" => {
                    let cid = if !tool_call_id.is_empty() {
                        tool_call_id
                    } else {
                        format!("hermes_res_{m_idx}")
                    };
                    let prefixed = if !tool_name.is_empty() {
                        format!("[tool:{tool_name}] {content}")
                    } else {
                        content
                    };
                    messages.push(json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": cid,
                            "content": prefixed
                        }],
                        "cw_timestamp": ts_iso,
                        "cw_turn_seq": turn_counter.max(1)
                    }));
                }
                _ => {}
            }
        }

        if messages.is_empty() {
            continue;
        }

        // If the Hermes session came from WhatsApp / Telegram / Signal / Cron without a local repo path,
        // label its workspace as `/hermes/<source>` so project grouping shows `📁 hermes-<source>` cleanly!
        let final_workspace = inferred_ws.unwrap_or_else(|| format!("/hermes/hermes-{source}"));
        let final_title = if !raw_title.trim().is_empty() {
            Some(format!("[{source}] {}", raw_title.trim()))
        } else {
            first_user_prompt.map(|p| format!("[{source}] {p}"))
        };

        let system_text = if !sys_prompt.trim().is_empty() {
            sys_prompt
        } else {
            format!("Hermes Agent ({source} channel)\n<RULE[SOUL.md]>\nHermes autonomous multi-channel agent.\n</RULE[SOUL.md]>")
        };

        let payload = json!({
            "model": model,
            "max_tokens": 8192,
            "system": system_text,
            "messages": messages
        });

        let session = Session {
            id: session_id.clone(),
            parent_session_id: None,
            harness: HarnessKind::Hermes,
            workspace_root: Some(final_workspace.clone()),
            model: model.clone(),
            provider: ProviderKind::OpenAICompatible,
            title: final_title,
            created_at,
            updated_at,
        };
        store.upsert_session(&session)?;

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
            None,
            Some(&final_workspace),
        )?;
        turn.created_at = updated_at;
        turn.token_usage.output_tokens = (cum_out as u32).min(32_000);
        turn.token_usage.saved_by_mutations = saved_by_mutations;
        if let Some(anchor_inp) = anchor_prompt_tokens
            && anchor_inp > turn.token_usage.input_tokens
        {
            turn.token_usage.input_tokens = anchor_inp.saturating_sub(saved_by_mutations);
        }
        if cum_cache > 0 {
            let total_cum = (cum_inp + cum_cache).max(1) as f64;
            let hit_ratio = (cum_cache as f64 / total_cum).clamp(0.0, 0.96) as f32;
            let hit_tok = ((turn.token_usage.input_tokens as f32) * hit_ratio).round() as u32;
            turn.token_usage.cached_tokens = hit_tok;
            turn.kv_cache_report.hit_tokens = hit_tok;
            turn.kv_cache_report.newly_cached_tokens =
                turn.token_usage.input_tokens.saturating_sub(hit_tok);
            turn.kv_cache_report.hit_ratio = hit_ratio;
        }

        store.save_turn(&turn, Some(&payload), mutated_payload_opt.as_ref(), None)?;
        store.save_file_states(&session_id, &file_states)?;
        imported_ids.push(session_id);
    }

    Ok(imported_ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_import_agy_transcript_fixture() {
        let store = CwStore::open_in_memory().unwrap();
        let fixture = Path::new("../cw-core/tests/fixtures/agy/transcript.jsonl");
        let sess_id = import_agy_transcript(fixture, &store, None).unwrap();
        let sess = store.get_session(&sess_id).unwrap().unwrap();
        assert_eq!(sess.title.as_deref(), Some("Check Cargo.toml"));
        let turn = store.get_latest_turn(&sess_id).unwrap().unwrap();
        assert!(!turn.nodes.is_empty());
        let files = store.get_file_states(&sess_id).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "Cargo.toml");
    }

    #[test]
    fn test_sync_mutations_to_transcript_on_disk() {
        let store = CwStore::open_in_memory().unwrap();
        let fixture = Path::new("../cw-core/tests/fixtures/agy/transcript.jsonl");
        let tmp_dir = std::env::temp_dir().join(format!("cw_test_sync_{}", std::process::id()));
        let _ = fs::create_dir_all(&tmp_dir);
        let tmp_transcript = tmp_dir.join("transcript.jsonl");
        fs::copy(fixture, &tmp_transcript).unwrap();

        let sess_id = import_agy_transcript(&tmp_transcript, &store, None).unwrap();
        store
            .save_mutation(&cw_core::ir::MutationRule {
                id: "mut_fold_test".to_string(),
                session_id: sess_id.clone(),
                op: MutationOp::FoldSubtree {
                    start_turn: 1,
                    end_turn: 1,
                    summary_text: "Folded Turn 1".to_string(),
                },
                status: MutationStatus::Committed,
                created_at: Utc::now(),
                estimated_saved_tokens: 120,
                estimated_kv_break_tokens: 0,
            })
            .unwrap();

        let report = sync_mutations_to_transcript_on_disk(&store, &sess_id).unwrap();
        assert!(Path::new(&report.backup_path).exists());
        assert!(report.steps_compacted >= 1);
        let updated = fs::read_to_string(&tmp_transcript).unwrap();
        assert!(updated.contains("[cw:folded_turns 1..1] Folded Turn 1"));
        let _ = fs::remove_dir_all(&tmp_dir);
    }
}
