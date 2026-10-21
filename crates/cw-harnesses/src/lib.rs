use chrono::{DateTime, Utc};
use cw_core::{
    analyze_turn_from_payload,
    ir::{HarnessKind, ProviderKind, Session},
};
use cw_store::CwStore;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

#[derive(Debug, thiserror::Error)]
pub enum HarnessError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Store error: {0}")]
    Store(#[from] cw_store::StoreError),
    #[error("Normalize error: {0}")]
    Normalize(#[from] cw_core::normalizer::NormalizeError),
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
        if let Some(m) = extract_model_from_settings_change(&content_str) {
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

    let seq = turn_counter.max(1);
    let turn_id = format!("turn_{}_{}", session_id, seq);
    let (mut turn, file_states) = analyze_turn_from_payload(
        turn_id,
        session_id.clone(),
        None,
        seq,
        &synthetic_payload,
        ProviderKind::Anthropic,
        None,
        latest_cache_read_tokens,
        final_workspace.as_deref(),
    )?;

    turn.created_at = updated_at;
    turn.token_usage.output_tokens = latest_output_tokens;
    if let Some(real_inp) = latest_input_tokens
        && real_inp > turn.token_usage.input_tokens
    {
        turn.token_usage.input_tokens = real_inp;
        if let Some(cached) = latest_cache_read_tokens {
            turn.token_usage.cached_tokens = cached.min(real_inp);
            turn.kv_cache_report.hit_tokens = cached.min(real_inp);
            turn.kv_cache_report.newly_cached_tokens = real_inp.saturating_sub(cached);
            turn.kv_cache_report.hit_ratio = cached as f32 / real_inp.max(1) as f32;
        }
    }

    store.save_turn(&turn, Some(&synthetic_payload), None, None)?;
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
    if let Some(idx) = raw.find("Model Selection` from") {
        let rest = &raw[idx..];
        if let Some(to_idx) = rest.find(" to ") {
            let after_to = &rest[to_idx + 4..];
            if let Some(dot) = after_to.find('.') {
                let model_name = after_to[..dot].trim();
                if !model_name.is_empty() {
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

/// Imports a Claude Code / Codex / OpenCode JSONL session transcript file into `CwStore`.
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
    let session_id = format!("{}_{}", harness.as_str(), &stem[..stem.len().min(12)]);

    let reader = BufReader::new(fs::File::open(path)?);
    let mut messages = Vec::new();
    let mut turn_counter = 0u32;

    for line_res in reader.lines() {
        let line = line_res?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(msg) = entry.get("message") {
            if msg.get("role").and_then(|r| r.as_str()) == Some("user") {
                turn_counter += 1;
            }
            messages.push(msg.clone());
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

    let payload = json!({
        "model": "claude-sonnet-4",
        "max_tokens": 8192,
        "messages": messages
    });

    let session = Session {
        id: session_id.clone(),
        parent_session_id: None,
        harness,
        workspace_root: workspace_root.clone(),
        model: "claude-sonnet-4".to_string(),
        provider: ProviderKind::Anthropic,
        title: Some(format!("{} ({stem})", harness.as_str())),
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };
    store.upsert_session(&session)?;

    let seq = turn_counter.max(1);
    let turn_id = format!("turn_{}_{}", session_id, seq);
    let (turn, file_states) = analyze_turn_from_payload(
        turn_id,
        session_id.clone(),
        None,
        seq,
        &payload,
        ProviderKind::Anthropic,
        None,
        None,
        workspace_root.as_deref(),
    )?;
    store.save_turn(&turn, Some(&payload), None, None)?;
    store.save_file_states(&session_id, &file_states)?;
    Ok(session_id)
}

/// Spawns a background watcher task that scans Antigravity (`~/.gemini/antigravity-cli/brain`),
/// Codex (`~/.codex/sessions`), and Claude Code (`~/.claude/projects`) transcript directories
/// and syncs modified transcripts into `CwStore`.
pub fn spawn_transcript_watcher(store: CwStore, _default_workspace: Option<String>) {
    tokio::spawn(async move {
        let mut last_mtime: HashMap<PathBuf, SystemTime> = HashMap::new();
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        let agy_brain_dir = PathBuf::from(&home).join(".gemini/antigravity-cli/brain");

        loop {
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
                        let prev = last_mtime.get(&transcript).copied();
                        if prev != Some(mtime) {
                            last_mtime.insert(transcript.clone(), mtime);
                            // Pass None for fallback_workspace so each transcript infers its own real project!
                            let _ = import_agy_transcript(&transcript, &store, None);
                        }
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(4)).await;
        }
    });
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
}
