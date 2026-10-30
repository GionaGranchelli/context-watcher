use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use cw_core::ir::HarnessKind;
use cw_store::CwStore;

use crate::adapters::{
    import_agy_transcript, import_generic_jsonl_transcript, import_hermes_state_db,
    import_opencode_state_db,
};

/// One-shot synchronous import of all local transcripts across all harnesses.
/// Discovers and imports existing sessions on disk into the CwStore immediately (<25ms)
/// without requiring a long-running background daemon to be active.
pub fn scan_and_import_all_transcripts_once(store: &CwStore) -> usize {
    let mut file_state: HashMap<PathBuf, (SystemTime, u64)> = HashMap::new();
    let home_path = cw_core::home_dir();
    let agy_brain_dir = home_path.join(".gemini/antigravity-cli/brain");
    let claude_dir = home_path.join(".claude/projects");
    let codex_dir = home_path.join(".codex/sessions");
    let hermes_db = home_path.join(".hermes/state.db");
    let opencode_dir = home_path.join(".local/share/opencode");
    let opencode_db = home_path.join(".local/share/opencode/opencode.db");
    let pi_dir = home_path.join(".pi/sessions");
    let pi_agent_dir = home_path.join(".pi/agent/sessions");
    let omp_dir = home_path.join(".omp/agent/sessions");

    let count_before = store.list_sessions().map(|s| s.len()).unwrap_or(0);

    // 1. Antigravity transcripts
    if agy_brain_dir.exists()
        && let Ok(entries) = fs::read_dir(&agy_brain_dir)
    {
        for entry in entries.flatten() {
            let transcript = entry.path().join(".system_generated/logs/transcript.jsonl");
            if transcript.exists() {
                let _ = import_agy_transcript(&transcript, store, None);
            }
        }
    }

    // 2. Hermes Agent
    if hermes_db.exists() {
        let _ = import_hermes_state_db(&hermes_db, store, 18);
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

/// Watches only explicitly imported JSONL sources in the selected workspace.
/// shortcut: changed files are reparsed as bounded snapshots; add format-specific cursors before large-history live qualification.
pub fn spawn_transcript_watcher(store: CwStore, workspace: Option<String>) {
    tokio::spawn(async move {
        let mut seen: HashMap<PathBuf, (SystemTime, u64)> = HashMap::new();
        loop {
            let snapshot = store.clone();
            let filter = workspace.clone();
            let previous = seen.clone();
            if let Ok(updated) = tokio::task::spawn_blocking(move || {
                let mut seen = previous;
                if let Ok(sources) = snapshot.list_session_sources() {
                    for (id, source) in sources {
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
