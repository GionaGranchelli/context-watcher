use std::{collections::HashMap, fs, path::Path};

use cw_core::{ir::NodeKind, mutation::apply_mutations_to_payload};
use cw_store::CwStore;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::HarnessError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptSyncReport {
    pub session_id: String,
    pub transcript_path: String,
    pub export_path: String,
    pub steps_compacted: usize,
    pub bytes_before: u64,
    pub bytes_after: u64,
}

/// Exports selected compactions to a private copy. Never replaces a harness-owned transcript.
pub fn sync_mutations_to_transcript_on_disk(
    store: &CwStore,
    session_id: &str,
) -> Result<TranscriptSyncReport, HarnessError> {
    use std::io::Write;
    let Some((source, _)) = store.get_session_source_path(session_id)? else {
        return Err(HarnessError::NoTranscriptSource(session_id.to_string()));
    };
    let path = Path::new(&source);
    if !fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(std::io::Error::other("Transcript source must be a regular file").into());
    }
    let original = fs::read_to_string(path)?;
    let mut replacements = HashMap::new();
    if let Some(turn) = store.get_latest_turn(session_id)? {
        let session = store.get_session(session_id)?;
        if let (Some(mut raw), Some(session)) = (store.get_turn_raw_payload(&turn.id)?, session) {
            let rules = store.list_mutations(session_id)?;
            let provider = turn.normalization_provider.unwrap_or(session.provider);
            let before =
                cw_core::normalizer::parse_request_to_nodes(&raw, provider, turn.sequence_number)?;
            apply_mutations_to_payload(&mut raw, provider, &rules, turn.sequence_number);
            let after =
                cw_core::normalizer::parse_request_to_nodes(&raw, provider, turn.sequence_number)?;
            for (old, new) in before.iter().zip(&after) {
                if let (
                    NodeKind::ToolResult {
                        call_id,
                        content: old_text,
                        ..
                    },
                    NodeKind::ToolResult { content, .. },
                ) = (&old.kind, &new.kind)
                    && !call_id.is_empty()
                    && old_text != content
                {
                    replacements.insert(call_id.clone(), (old_text.clone(), content.clone()));
                }
            }
        }
    }
    if original.lines().any(|line| {
        serde_json::from_str::<Value>(line)
            .is_ok_and(|value| cw_core::mutation::has_signed_content(&value))
    }) {
        replacements.clear();
    }
    let mut output = String::new();
    let mut steps_compacted = 0;
    for line in original.split_inclusive('\n') {
        let mut changed = false;
        if let Ok(mut value) = serde_json::from_str::<Value>(line) {
            let record = if value.get("message").is_some() {
                &mut value["message"]
            } else if value.get("item").is_some() {
                &mut value["item"]
            } else {
                &mut value
            };
            let role = record
                .get("role")
                .or_else(|| record.get("type"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if matches!(role, "tool" | "toolResult" | "function_call_output") {
                let cid = record
                    .get("tool_call_id")
                    .or_else(|| record.get("toolCallId"))
                    .or_else(|| record.get("call_id"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if let Some((old, new)) = replacements.get(cid) {
                    let field = if record.get("output").is_some() {
                        "output"
                    } else {
                        "content"
                    };
                    match record.get(field) {
                        Some(Value::String(text)) if text == old => {
                            record[field] = Value::String(new.clone());
                            changed = true;
                        }
                        Some(Value::Array(items))
                            if items.iter().all(|item| {
                                item.get("type").and_then(Value::as_str) == Some("text")
                            }) && items
                                .iter()
                                .filter_map(|item| item.get("text").and_then(Value::as_str))
                                .collect::<Vec<_>>()
                                .join("\n")
                                == *old =>
                        {
                            if let Some(items) = record[field].as_array_mut() {
                                for (index, item) in items.iter_mut().enumerate() {
                                    item["text"] = Value::String(if index == 0 {
                                        new.clone()
                                    } else {
                                        String::new()
                                    });
                                }
                                changed = true;
                            }
                        }
                        _ => {}
                    }
                }
            }
            if changed {
                output.push_str(
                    &serde_json::to_string(&value)
                        .map_err(cw_core::normalizer::NormalizeError::from)?,
                );
                if line.ends_with('\n') {
                    output.push('\n');
                }
                steps_compacted += 1;
            }
        }
        if !changed {
            output.push_str(line);
        }
    }
    let export_path = format!("{source}.cw-export-{}.jsonl", uuid::Uuid::new_v4());
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&export_path)?;
    if let Err(error) = file
        .write_all(output.as_bytes())
        .and_then(|()| file.sync_all())
    {
        let _ = fs::remove_file(&export_path);
        return Err(error.into());
    }
    if let Err(error) = store.record_export(session_id, &export_path, output.as_bytes()) {
        let _ = fs::remove_file(&export_path);
        return Err(error.into());
    }
    Ok(TranscriptSyncReport {
        session_id: session_id.to_string(),
        transcript_path: source,
        export_path,
        steps_compacted,
        bytes_before: original.len() as u64,
        bytes_after: output.len() as u64,
    })
}
