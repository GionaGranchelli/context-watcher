use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use chrono::{DateTime, Utc};
use cw_core::ir::HarnessKind;
use cw_store::CwStore;

use crate::adapters::{
    import_agy_transcript, import_copilot_session_store, import_generic_jsonl_transcript,
    import_hermes_state_db, import_opencode_state_db,
};

/// Imports newly discovered or changed local harness sessions into the CwStore.
pub fn scan_and_import_all_transcripts_once(store: &CwStore) -> usize {
    let mut file_state: HashMap<PathBuf, (SystemTime, u64)> = HashMap::new();
    let home_path = cw_core::home_dir();
    let agy_brain_dir = home_path.join(".gemini/antigravity-cli/brain");
    let claude_dir = home_path.join(".claude/projects");
    let codex_dir = home_path.join(".codex/sessions");
    let copilot_db = home_path.join(".copilot/session-store.db");
    let hermes_db = home_path.join(".hermes/state.db");
    let opencode_dir = home_path.join(".local/share/opencode");
    let opencode_db = home_path.join(".local/share/opencode/opencode.db");
    let pi_dir = home_path.join(".pi/sessions");
    let pi_agent_dir = home_path.join(".pi/agent/sessions");
    let omp_dir = home_path.join(".omp/agent/sessions");

    let count_before = store.list_sessions().map(|s| s.len()).unwrap_or(0);
    if let Ok(sources) = store.list_session_sources() {
        for (_, (source, byte_offset, imported_at)) in sources {
            let Ok(imported_at) = DateTime::parse_from_rfc3339(&imported_at) else {
                continue;
            };
            let path = PathBuf::from(source);
            if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                continue;
            }
            let Ok(metadata) = fs::metadata(&path) else {
                continue;
            };
            let Ok(modified) = metadata.modified() else {
                continue;
            };
            if metadata.len() == byte_offset
                && modified <= SystemTime::from(imported_at.with_timezone(&Utc))
            {
                file_state.insert(path, (modified, metadata.len()));
            }
        }
    }

    // 1. Antigravity transcripts
    if agy_brain_dir.exists()
        && let Ok(entries) = fs::read_dir(&agy_brain_dir)
    {
        for entry in entries.flatten() {
            let transcript = entry.path().join(".system_generated/logs/transcript.jsonl");
            let Ok(metadata) = fs::metadata(&transcript) else {
                continue;
            };
            let Ok(modified) = metadata.modified() else {
                continue;
            };
            let signature = (modified, metadata.len());
            if file_state.get(&transcript) != Some(&signature)
                && import_agy_transcript(&transcript, store, None).is_ok()
            {
                file_state.insert(transcript, signature);
            }
        }
    }

    // 2. Hermes Agent
    if hermes_db.exists() {
        let _ = import_hermes_state_db(&hermes_db, store, 18);
    }

    // 2a. GitHub Copilot CLI session store
    if copilot_db.exists() {
        let _ = import_copilot_session_store(&copilot_db, store, 10_000);
    }

    // 3. OpenCode V2 SQLite database
    if opencode_db.exists() {
        let _ = import_opencode_state_db(&opencode_db, store, 18);
    }

    // 4. Claude Code, Codex, OpenCode, Pi, OMP
    for (dir, harness) in [
        (&claude_dir, HarnessKind::ClaudeCode),
        (&codex_dir, HarnessKind::Codex),
        (&opencode_dir, HarnessKind::OpenCode),
        (&pi_dir, HarnessKind::Pi),
        (&pi_agent_dir, HarnessKind::Pi),
        (&omp_dir, HarnessKind::Omp),
    ] {
        if dir.exists() {
            scan_jsonl_dir_recursively(dir, harness, store, &mut file_state, 0);
        }
    }

    let count_after = store.list_sessions().map(|s| s.len()).unwrap_or(0);
    count_after.saturating_sub(count_before)
}

fn copilot_store_signature(path: &Path) -> Option<(SystemTime, u64)> {
    let db = fs::metadata(path).ok()?;
    let mut modified = db.modified().ok()?;
    let mut size = db.len();
    let wal_path = PathBuf::from(format!("{}-wal", path.display()));
    if let Ok(wal) = fs::metadata(wal_path) {
        if let Ok(wal_modified) = wal.modified()
            && wal_modified > modified
        {
            modified = wal_modified;
        }
        size = size.saturating_add(wal.len());
    }
    Some((modified, size))
}

/// Watches imported JSONL sources in scope and the Copilot store for global sessions.
/// shortcut: changed files are reparsed as bounded snapshots; add format-specific cursors before large-history live qualification.
pub fn spawn_transcript_watcher(store: CwStore, workspace: Option<String>) {
    tokio::spawn(async move {
        let mut seen: HashMap<PathBuf, (SystemTime, u64)> = HashMap::new();
        let copilot_db = cw_core::home_dir().join(".copilot/session-store.db");
        if workspace.is_none()
            && let Some(signature) = copilot_store_signature(&copilot_db)
        {
            seen.insert(copilot_db.clone(), signature);
        }
        loop {
            let snapshot = store.clone();
            let filter = workspace.clone();
            let previous = seen.clone();
            let copilot_path = copilot_db.clone();
            if let Ok(updated) = tokio::task::spawn_blocking(move || {
                let mut seen = previous;
                if filter.is_none()
                    && let Some(signature) = copilot_store_signature(&copilot_path)
                    && seen.get(&copilot_path) != Some(&signature)
                {
                    if import_copilot_session_store(&copilot_path, &snapshot, 10_000).is_ok() {
                        seen.insert(copilot_path.clone(), signature);
                    }
                }
                if let Ok(sources) = snapshot.list_session_sources() {
                    for (id, (source, byte_offset, imported_at)) in sources {
                        let Ok(Some(session)) = snapshot.get_session(&id) else {
                            continue;
                        };
                        if filter
                            .as_ref()
                            .is_some_and(|root| session.workspace_root.as_ref() != Some(root))
                        {
                            continue;
                        }
                        let path = PathBuf::from(source);
                        if path.extension().and_then(|x| x.to_str()) != Some("jsonl") {
                            continue;
                        }
                        let Ok(meta) = fs::metadata(&path) else {
                            continue;
                        };
                        let Ok(time) = meta.modified() else {
                            continue;
                        };
                        let signature = (time, meta.len());
                        if seen.get(&path) == Some(&signature) {
                            continue;
                        }
                        if !seen.contains_key(&path)
                            && byte_offset == meta.len()
                            && DateTime::parse_from_rfc3339(&imported_at).ok().is_some_and(
                                |imported_at| {
                                    time <= SystemTime::from(imported_at.with_timezone(&Utc))
                                },
                            )
                        {
                            seen.insert(path, signature);
                            continue;
                        }
                        let result = if session.harness == HarnessKind::Antigravity {
                            import_agy_transcript(&path, &snapshot, session.workspace_root.clone())
                        } else {
                            import_generic_jsonl_transcript(
                                &path,
                                session.harness,
                                &snapshot,
                                session.workspace_root.clone(),
                            )
                        };
                        if result.is_ok() {
                            seen.insert(path, signature);
                        }
                    }
                }
                seen
            })
            .await
            {
                seen = updated;
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
            && !p.to_string_lossy().contains(".cw-export-")
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
