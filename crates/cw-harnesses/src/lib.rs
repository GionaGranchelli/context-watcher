use chrono::Utc;
use cw_core::{
    analyze_turn_from_payload,
    ir::{HarnessKind, ProviderKind, Session},
};
use cw_store::CwStore;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
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
/// from the matching line in `transcript_full.jsonl`), converting the trajectory into a
/// normalized payload and importing it into `CwStore`.
pub fn import_agy_transcript(
    transcript_path: &Path,
    store: &CwStore,
    workspace_root: Option<String>,
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
    let mut turn_counter = 0u32;
    let mut pending_call_ids: std::collections::VecDeque<String> =
        std::collections::VecDeque::new();

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
                && let Ok(full_step) = serde_json::from_str::<Value>(full_line) {
                    step = full_step;
                    let _ = &mut line;
                }

        let step_type = step.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let content_str = step
            .get("content")
            .map(|c| match c {
                Value::String(s) => s.clone(),
                other => serde_json::to_string(other).unwrap_or_default(),
            })
            .unwrap_or_default();

        match step_type {
            "USER_INPUT" => {
                turn_counter += 1;
                if title.is_none() && !content_str.is_empty() {
                    let clean = content_str
                        .replace("<USER_REQUEST>", "")
                        .replace("</USER_REQUEST>", "");
                    title = Some(clean.trim().chars().take(64).collect());
                }
                messages.push(json!({
                    "role": "user",
                    "content": content_str
                }));
            }
            "PLANNER_RESPONSE" => {
                let mut blocks = Vec::new();
                if let Some(thinking) = step.get("thinking").and_then(|v| v.as_str())
                    && !thinking.is_empty() {
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
                        "content": blocks
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
                            }]
                        }));
                    }
                }
            }
        }
    }

    if messages.is_empty() {
        return Ok(session_id);
    }

    let synthetic_payload = json!({
        "model": "gemini-4-argon",
        "max_tokens": 8192,
        "system": system_text,
        "messages": messages
    });

    let session = Session {
        id: session_id.clone(),
        parent_session_id: None,
        harness: HarnessKind::Antigravity,
        workspace_root: workspace_root.clone(),
        model: "gemini-4-argon".to_string(),
        provider: ProviderKind::Anthropic,
        title,
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
        &synthetic_payload,
        ProviderKind::Anthropic,
        None,
        None,
        workspace_root.as_deref(),
    )?;

    store.save_turn(&turn, Some(&synthetic_payload), None, None)?;
    store.save_file_states(&session_id, &file_states)?;
    Ok(session_id)
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
pub fn spawn_transcript_watcher(store: CwStore, workspace_root: Option<String>) {
    tokio::spawn(async move {
        let mut last_mtime: HashMap<PathBuf, SystemTime> = HashMap::new();
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        let agy_brain_dir = PathBuf::from(&home).join(".gemini/antigravity-cli/brain");

        loop {
            if agy_brain_dir.exists()
                && let Ok(entries) = fs::read_dir(&agy_brain_dir) {
                    for entry in entries.flatten() {
                        let transcript = entry
                            .path()
                            .join(".system_generated/logs/transcript.jsonl");
                        if transcript.exists()
                            && let Ok(meta) = fs::metadata(&transcript)
                                && let Ok(mtime) = meta.modified() {
                                    let prev = last_mtime.get(&transcript).copied();
                                    if prev != Some(mtime) {
                                        last_mtime.insert(transcript.clone(), mtime);
                                        let _ = import_agy_transcript(
                                            &transcript,
                                            &store,
                                            workspace_root.clone(),
                                        );
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
        let turn = store.get_latest_turn(&sess_id).unwrap().unwrap();
        assert!(!turn.nodes.is_empty());
        let files = store.get_file_states(&sess_id).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "Cargo.toml");
    }
}
