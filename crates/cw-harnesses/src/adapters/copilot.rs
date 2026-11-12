use chrono::{DateTime, Utc};
use cw_core::{
    analyze_turn_from_payload,
    ir::{HarnessKind, ProviderKind, Session},
};
use cw_store::CwStore;
use rusqlite::{Connection, OpenFlags, params};
use serde_json::json;
use std::path::Path;

/// Imports GitHub Copilot CLI sessions from its local SQLite session store.
pub fn import_copilot_session_store(
    db_path: &Path,
    store: &CwStore,
    max_sessions: usize,
) -> Result<Vec<String>, crate::error::HarnessError> {
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let limit = max_sessions.min(i64::MAX as usize) as i64;
    let mut sessions = conn.prepare(
        "SELECT s.id, COALESCE(s.cwd, ''), COALESCE(s.repository, ''),
                COALESCE(s.summary, ''), COALESCE(s.created_at, ''), COALESCE(s.updated_at, '')
         FROM sessions s
         WHERE EXISTS (SELECT 1 FROM turns t WHERE t.session_id=s.id
                       AND (COALESCE(t.user_message, '') <> '' OR COALESCE(t.assistant_response, '') <> ''))
         ORDER BY s.updated_at DESC LIMIT ?1",
    )?;
    let rows = sessions.query_map(params![limit], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
        ))
    })?;
    let mut turns = conn.prepare(
        "SELECT turn_index, COALESCE(user_message, ''), COALESCE(assistant_response, ''),
                COALESCE(timestamp, '')
         FROM turns WHERE session_id=?1 ORDER BY turn_index ASC",
    )?;
    let mut imported = Vec::new();

    for row in rows {
        let (native_id, cwd, repository, summary, created, updated) = row?;
        let session_id = format!("copilot_{native_id}");
        if !cw_core::control::is_valid_session_id(&session_id) {
            continue;
        }
        let created_at = DateTime::parse_from_rfc3339(&created)
            .map(|date| date.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());
        let updated_at = DateTime::parse_from_rfc3339(&updated)
            .map(|date| date.with_timezone(&Utc))
            .unwrap_or(created_at);
        if store.get_session(&session_id)?.is_some_and(|session| {
            session.harness == HarnessKind::Copilot && session.updated_at >= updated_at
        }) {
            imported.push(session_id);
            continue;
        }
        let mut messages = Vec::new();
        let mut latest_turn = 0u32;
        let mut first_prompt = None;
        for message in turns.query_map(params![native_id], |row| {
            Ok((
                row.get::<_, u32>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })? {
            let (turn_seq, user, assistant, timestamp) = message?;
            latest_turn = latest_turn.max(turn_seq);
            if !user.trim().is_empty() {
                first_prompt.get_or_insert_with(|| {
                    user.split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                        .chars()
                        .take(78)
                        .collect::<String>()
                });
                messages.push(json!({
                    "role": "user",
                    "content": user,
                    "cw_timestamp": timestamp,
                    "cw_turn_seq": turn_seq.max(1)
                }));
            }
            if !assistant.trim().is_empty() {
                messages.push(json!({
                    "role": "assistant",
                    "content": assistant,
                    "cw_timestamp": timestamp,
                    "cw_turn_seq": turn_seq.max(1)
                }));
            }
        }
        if messages.is_empty() {
            continue;
        }

        let workspace = if cwd.trim().is_empty() {
            repository
        } else {
            cwd
        };
        let workspace = (!workspace.trim().is_empty()).then_some(workspace);
        let model = "github-copilot".to_string();
        let title = (!summary.trim().is_empty())
            .then_some(summary)
            .or(first_prompt);
        let payload = json!({
            "model": model,
            "max_tokens": 8192,
            "system": "GitHub Copilot CLI session",
            "messages": messages
        });
        store.upsert_session(&Session {
            id: session_id.clone(),
            parent_session_id: None,
            harness: HarnessKind::Copilot,
            workspace_root: workspace.clone(),
            model: model.clone(),
            provider: ProviderKind::OpenAICompatible,
            title,
            created_at,
            updated_at,
        })?;

        let seq = latest_turn.max(1);
        let turn_id = format!("turn_{session_id}_{seq}");
        let (mut turn, file_states) = analyze_turn_from_payload(
            turn_id,
            session_id.clone(),
            None,
            seq,
            &payload,
            ProviderKind::OpenAICompatible,
            None,
            None,
            workspace.as_deref(),
        )?;
        turn.created_at = updated_at;
        store.save_turn(&turn, Some(&payload), None, None)?;
        store.save_file_states(&session_id, &file_states)?;
        imported.push(session_id);
    }

    Ok(imported)
}
