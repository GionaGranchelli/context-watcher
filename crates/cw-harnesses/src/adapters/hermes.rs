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
                        && let Ok(Value::Array(tcs)) =
                            serde_json::from_str::<Value>(&tool_calls_str)
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
            format!(
                "Hermes Agent ({source} channel)\n<RULE[SOUL.md]>\nHermes autonomous multi-channel agent.\n</RULE[SOUL.md]>"
            )
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
