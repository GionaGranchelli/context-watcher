use chrono::{DateTime, Utc};
use cw_core::{
    analyze_turn_from_payload,
    ir::{HarnessKind, ProviderKind, Session},
    mutation::apply_mutations_to_payload,
};
use cw_store::CwStore;
use serde_json::{Value, json};
use std::path::Path;

use super::common::{infer_workspace_from_text, infer_workspace_from_tool_args};
use crate::error::HarnessError;

/// Discovers and imports OpenCode V2 SQLite sessions (`~/.local/share/opencode/opencode.db`).
///
/// OpenCode V2 stores all sessions in SQLite tables (`session_v2` / `session`, `message`, `part`)
/// rather than writing individual JSONL files. This importer parses conversations losslessly into
/// contextWatcher's canonical ContextTurn/ContextNode IR with precise token usage and cache hit stats.
pub fn import_opencode_state_db(
    db_path: &Path,
    store: &CwStore,
    max_sessions: usize,
) -> Result<Vec<String>, HarnessError> {
    let conn = rusqlite::Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;

    // Detect if `session_v2` (OpenCode 2.x) or legacy `session` table exists
    let table_name: String = conn
        .query_row(
            "SELECT name FROM sqlite_master WHERE type='table' AND name='session_v2'",
            [],
            |row| row.get(0),
        )
        .unwrap_or_else(|_| "session".to_string());

    let session_sql = format!(
        "SELECT
            id,
            COALESCE(directory, ''),
            COALESCE(title, ''),
            COALESCE(model, ''),
            COALESCE(tokens_input, 0),
            COALESCE(tokens_output, 0),
            COALESCE(tokens_cache_read, 0),
            COALESCE(tokens_cache_write, 0),
            COALESCE(time_created, 0),
            COALESCE(time_updated, time_created, 0)
         FROM {table_name}
         ORDER BY COALESCE(time_updated, time_created, 0) DESC
         LIMIT ?1"
    );

    let mut stmt = conn.prepare(&session_sql)?;
    let rows = stmt.query_map(rusqlite::params![max_sessions as i64], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, i64>(4)?,
            row.get::<_, i64>(5)?,
            row.get::<_, i64>(6)?,
            row.get::<_, i64>(7)?,
            row.get::<_, i64>(8)?,
            row.get::<_, i64>(9)?,
        ))
    })?;

    let mut imported_ids = Vec::new();

    let mut msg_stmt = conn.prepare(
        "SELECT
            m.id,
            COALESCE(json_extract(m.data, '$.role'), 'user'),
            COALESCE(m.time_created, 0),
            COALESCE(p.id, ''),
            COALESCE(json_extract(p.data, '$.type'), ''),
            COALESCE(p.data, ''),
            COALESCE(p.time_created, m.time_created, 0)
         FROM message m
         LEFT JOIN part p ON p.message_id = m.id
         WHERE m.session_id = ?1
         ORDER BY m.time_created ASC, p.time_created ASC
         LIMIT 2000",
    )?;

    for r in rows.flatten() {
        let (
            raw_id,
            raw_cwd,
            raw_title,
            raw_model,
            cum_inp,
            cum_out,
            cum_cache_read,
            _cum_cache_write,
            created_ms,
            updated_ms,
        ) = r;

        let session_id = format!("opencode_{raw_id}");
        let created_at = DateTime::from_timestamp_millis(created_ms).unwrap_or_else(Utc::now);
        let updated_at = DateTime::from_timestamp_millis(updated_ms).unwrap_or(created_at);

        let model = if let Ok(v) = serde_json::from_str::<Value>(&raw_model) {
            v.get("id")
                .or_else(|| v.get("modelID"))
                .and_then(|s| s.as_str())
                .unwrap_or("opencode-model")
                .to_string()
        } else if !raw_model.trim().is_empty() {
            raw_model.clone()
        } else {
            "opencode-model".to_string()
        };

        let msg_rows = msg_stmt.query_map(rusqlite::params![&raw_id], |m| {
            Ok((
                m.get::<_, String>(0)?,
                m.get::<_, String>(1)?,
                m.get::<_, i64>(2)?,
                m.get::<_, String>(3)?,
                m.get::<_, String>(4)?,
                m.get::<_, String>(5)?,
                m.get::<_, i64>(6)?,
            ))
        })?;

        struct OpenCodeMsg {
            id: String,
            role: String,
            time_created: i64,
            parts: Vec<(String, String, Value)>,
        }

        // Group rows by message ID
        let mut grouped_msgs: Vec<OpenCodeMsg> = Vec::new();
        for m_res in msg_rows.flatten() {
            let (m_id, role, m_time, p_id, p_type, p_data_str, _p_time) = m_res;
            let p_val: Value = serde_json::from_str(&p_data_str).unwrap_or(Value::Null);

            if let Some(last) = grouped_msgs.last_mut().filter(|l| l.id == m_id) {
                if !p_id.is_empty() {
                    last.parts.push((p_id, p_type, p_val));
                }
                continue;
            }
            let mut parts = Vec::new();
            if !p_id.is_empty() {
                parts.push((p_id, p_type, p_val));
            }
            grouped_msgs.push(OpenCodeMsg {
                id: m_id,
                role,
                time_created: m_time,
                parts,
            });
        }

        let mut messages = Vec::new();
        let mut turn_counter = 0u32;
        let mut first_user_prompt: Option<String> = None;
        let mut inferred_ws: Option<String> = if !raw_cwd.trim().is_empty() {
            Some(raw_cwd.clone())
        } else {
            None
        };

        for msg in grouped_msgs {
            let ts_iso = DateTime::from_timestamp_millis(msg.time_created).map(|d| d.to_rfc3339());

            if msg.role == "user" {
                turn_counter += 1;
                let mut text_parts = Vec::new();
                for (_pid, ptype, pval) in &msg.parts {
                    if ptype == "text" {
                        if let Some(t) = pval.get("text").and_then(|v| v.as_str()) {
                            text_parts.push(t.to_string());
                        }
                    } else if ptype == "file"
                        && let Some(fname) = pval.get("filename").and_then(|v| v.as_str())
                    {
                        text_parts.push(format!("[attached file: {fname}]"));
                    }
                }
                let full_text = text_parts.join("\n");
                if first_user_prompt.is_none() && !full_text.trim().is_empty() {
                    let one_line = full_text.split_whitespace().collect::<Vec<_>>().join(" ");
                    first_user_prompt = Some(one_line.chars().take(78).collect());
                }
                if inferred_ws.is_none() {
                    inferred_ws = infer_workspace_from_text(&full_text);
                }
                messages.push(json!({
                    "role": "user",
                    "content": full_text,
                    "cw_timestamp": ts_iso,
                    "cw_turn_seq": turn_counter
                }));
            } else if msg.role == "assistant" {
                let mut blocks = Vec::new();
                let mut tool_results = Vec::new();

                for (pid, ptype, pval) in msg.parts {
                    if ptype == "reasoning" {
                        if let Some(t) = pval.get("text").and_then(|v| v.as_str()) {
                            blocks.push(json!({
                                "type": "thinking",
                                "thinking": t
                            }));
                        }
                    } else if ptype == "text" {
                        if let Some(t) = pval.get("text").and_then(|v| v.as_str()) {
                            blocks.push(json!({
                                "type": "text",
                                "text": t
                            }));
                        }
                    } else if ptype == "tool" {
                        let tool_name = pval.get("tool").and_then(|v| v.as_str()).unwrap_or("tool");
                        let call_id = pval.get("callID").and_then(|v| v.as_str()).unwrap_or(&pid);
                        let state = pval.get("state").cloned().unwrap_or(Value::Null);
                        let input = state.get("input").cloned().unwrap_or_else(|| json!({}));
                        let output = state.get("output").and_then(|v| v.as_str()).unwrap_or("");

                        if inferred_ws.is_none() {
                            inferred_ws = infer_workspace_from_tool_args(&input);
                        }

                        blocks.push(json!({
                            "type": "tool_use",
                            "id": call_id,
                            "name": tool_name,
                            "input": input
                        }));

                        tool_results.push((call_id.to_string(), output.to_string()));
                    }
                }

                if !blocks.is_empty() {
                    messages.push(json!({
                        "role": "assistant",
                        "content": blocks,
                        "cw_timestamp": ts_iso.clone(),
                        "cw_turn_seq": turn_counter.max(1)
                    }));
                }

                if !tool_results.is_empty() {
                    let res_content: Vec<Value> = tool_results
                        .into_iter()
                        .map(|(cid, out)| {
                            json!({
                                "type": "tool_result",
                                "tool_use_id": cid,
                                "content": out
                            })
                        })
                        .collect();
                    messages.push(json!({
                        "role": "user",
                        "content": res_content,
                        "cw_timestamp": ts_iso,
                        "cw_turn_seq": turn_counter.max(1)
                    }));
                }
            }
        }

        if messages.is_empty() {
            continue;
        }

        let final_workspace = inferred_ws.unwrap_or_else(|| "/opencode".to_string());
        let final_title = if !raw_title.trim().is_empty() {
            Some(raw_title.trim().to_string())
        } else {
            first_user_prompt
        };

        let payload = json!({
            "model": model,
            "max_tokens": 8192,
            "system": "OpenCode V2 Agent",
            "messages": messages
        });

        let session = Session {
            id: session_id.clone(),
            parent_session_id: None,
            harness: HarnessKind::OpenCode,
            workspace_root: Some(final_workspace.clone()),
            model: model.clone(),
            provider: ProviderKind::Anthropic,
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
        turn.token_usage.output_tokens = (cum_out as u32).min(128_000);
        turn.token_usage.saved_by_mutations = saved_by_mutations;
        if cum_inp > 0 {
            turn.token_usage.input_tokens = (cum_inp as u32).saturating_sub(saved_by_mutations);
        }
        if cum_cache_read > 0 {
            let total_cum = (cum_inp + cum_cache_read).max(1) as f64;
            let hit_ratio = (cum_cache_read as f64 / total_cum).clamp(0.0, 0.99) as f32;
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
