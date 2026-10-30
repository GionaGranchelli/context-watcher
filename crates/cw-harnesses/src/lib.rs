pub mod adapters;
pub mod auto_heal;
pub mod error;
pub mod qualification;
pub mod sync;
pub mod watcher;

pub use adapters::{
    import_agy_transcript, import_generic_jsonl_transcript, import_hermes_state_db,
    import_opencode_state_db,
};
pub use auto_heal::check_and_apply_auto_heal;
pub use error::HarnessError;
pub use sync::{TranscriptSyncReport, sync_mutations_to_transcript_on_disk};
pub use watcher::{scan_and_import_all_transcripts_once, spawn_transcript_watcher};

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use cw_core::ir::{HarnessKind, MutationOp, MutationStatus, NodeKind};
    use cw_store::CwStore;
    use std::{fs, path::Path};

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
        assert!(Path::new(&report.export_path).exists());
        assert_eq!(report.steps_compacted, 0);
        assert_eq!(
            fs::read(&tmp_transcript).unwrap(),
            fs::read(fixture).unwrap()
        );
        assert_eq!(
            fs::read(&report.export_path).unwrap(),
            fs::read(&tmp_transcript).unwrap()
        );
        let _ = fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn test_import_omp_transcript() {
        let store = CwStore::open_in_memory().unwrap();
        let tmp_dir = std::env::temp_dir().join(format!("cw_test_omp_{}", std::process::id()));
        let _ = fs::create_dir_all(&tmp_dir);
        let transcript =
            tmp_dir.join("2026-10-22T19-57-33-217Z_01a16530-b4a1-71cb-b159-96200fb07f14.jsonl");
        let content = r#"{"type":"title","v":1,"title":"Determine Chrome Install Method","source":"auto"}
{"type":"session","version":3,"id":"01a16530-b4a1-71cb-b159-96200fb07f14","cwd":"/home/gionag/Development/contextWatcher"}
{"type":"message","message":{"role":"user","content":[{"type":"text","text":"How did I install chrome?"}]}}
{"type":"message","message":{"role":"assistant","content":[{"type":"toolCall","id":"call_123","name":"bash","arguments":{"command":"which google-chrome"}}],"model":"flint","provider":"machinist-flint","usage":{"input":84,"output":120,"cacheRead":9000},"contextSnapshot":{"promptTokens":9084}}}
{"type":"message","message":{"role":"toolResult","toolCallId":"call_123","toolName":"bash","content":[{"type":"text","text":"/usr/bin/google-chrome"}]}}
"#;
        fs::write(&transcript, content).unwrap();

        let sess_id =
            import_generic_jsonl_transcript(&transcript, HarnessKind::Omp, &store, None).unwrap();
        assert!(sess_id.starts_with("omp_01a16530-b4a"));
        let sess = store.get_session(&sess_id).unwrap().unwrap();
        assert_eq!(sess.harness, HarnessKind::Omp);
        assert_eq!(
            sess.title.as_deref(),
            Some("Determine Chrome Install Method")
        );
        assert_eq!(
            sess.workspace_root.as_deref(),
            Some("/home/gionag/Development/contextWatcher")
        );

        let turn = store.get_latest_turn(&sess_id).unwrap().unwrap();
        assert_eq!(turn.token_usage.input_tokens, 9084);
        assert_eq!(turn.token_usage.cached_tokens, 9000);

        // Verify tool call and tool result nodes are properly extracted
        let has_tool_call = turn.nodes.iter().any(
            |n| matches!(&n.kind, NodeKind::ToolCall { call_id, .. } if call_id == "call_123"),
        );
        let has_tool_res = turn.nodes.iter().any(
            |n| matches!(&n.kind, NodeKind::ToolResult { call_id, .. } if call_id == "call_123"),
        );
        assert!(has_tool_call, "ToolCall node should exist in IR");
        assert!(has_tool_res, "ToolResult node should exist in IR");

        let _ = fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn test_auto_heal_low_health_transcript() {
        let store = CwStore::open_in_memory().unwrap();
        // Configure AutoPilot with auto_heal_low_health = true, min_health_score = 95
        let mut ap_cfg = store.get_autopilot_config().unwrap();
        ap_cfg.enabled = true;
        ap_cfg.auto_heal_low_health = true;
        ap_cfg.min_health_score = 95;
        ap_cfg.heal_strategy = "min".to_string();
        ap_cfg.auto_sync_to_disk = true;
        store.set_autopilot_config(&ap_cfg).unwrap();

        let tmp_dir = std::env::temp_dir().join(format!("cw_test_heal_{}", std::process::id()));
        let _ = fs::create_dir_all(&tmp_dir);
        let transcript = tmp_dir.join("2026-10-23T10-00-00-000Z_heal_session_test.jsonl");

        // Create a transcript with duplicate file read of the same file
        let content = format!(
            r#"{{"type":"session","version":3,"id":"heal_session_test","cwd":"{}"}}
{{"type":"message","message":{{"role":"user","content":[{{"type":"text","text":"Read Cargo.toml"}}]}}}}
{{"type":"message","message":{{"role":"assistant","content":[{{"type":"toolCall","id":"c1","name":"read_file","arguments":{{"path":"Cargo.toml"}}}}]}}}}
{{"type":"message","message":{{"role":"toolResult","toolCallId":"c1","toolName":"read_file","content":[{{"type":"text","text":"[workspace]\nmembers = [\"crates/*\"]\nresolver = \"2\"\n"}}]}}}}
{{"type":"message","message":{{"role":"user","content":[{{"type":"text","text":"Read Cargo.toml again"}}]}}}}
{{"type":"message","message":{{"role":"assistant","content":[{{"type":"toolCall","id":"c2","name":"read_file","arguments":{{"path":"Cargo.toml"}}}}]}}}}
{{"type":"message","message":{{"role":"toolResult","toolCallId":"c2","toolName":"read_file","content":[{{"type":"text","text":"[workspace]\nmembers = [\"crates/*\"]\nresolver = \"2\"\n"}}]}}}}
"#,
            tmp_dir.display()
        );
        fs::write(&transcript, &content).unwrap();

        let sess_id =
            import_generic_jsonl_transcript(&transcript, HarnessKind::Omp, &store, None).unwrap();
        let turn = store.get_latest_turn(&sess_id).unwrap().unwrap();
        assert!(turn.health_score > 0);
        let mutations = store.list_mutations(&sess_id).unwrap();
        assert!(
            !mutations.is_empty(),
            "Auto-heal should automatically generate committed mutations for duplicate file reads"
        );
        assert!(mutations.iter().any(|m| m.id.starts_with("ap_heal_")));

        assert_eq!(
            fs::read_to_string(&transcript).unwrap(),
            content,
            "Automatic analysis must not rewrite the source"
        );

        let _ = fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn test_import_opencode_sqlite_db() {
        let tmp_dir = std::env::temp_dir().join(format!("cw_test_opencode_{}", std::process::id()));
        let _ = fs::create_dir_all(&tmp_dir);
        let db_path = tmp_dir.join("opencode.db");

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE session_v2 (
                id text PRIMARY KEY,
                directory text NOT NULL,
                title text NOT NULL,
                model text,
                tokens_input integer DEFAULT 0,
                tokens_output integer DEFAULT 0,
                tokens_cache_read integer DEFAULT 0,
                tokens_cache_write integer DEFAULT 0,
                time_created integer NOT NULL,
                time_updated integer NOT NULL
            );
            CREATE TABLE message (
                id text PRIMARY KEY,
                session_id text NOT NULL,
                time_created integer NOT NULL,
                data text NOT NULL
            );
            CREATE TABLE part (
                id text PRIMARY KEY,
                message_id text NOT NULL,
                session_id text NOT NULL,
                time_created integer NOT NULL,
                data text NOT NULL
            );",
        )
        .unwrap();

        // Insert session
        conn.execute(
            "INSERT INTO session_v2 (id, directory, title, model, tokens_input, tokens_output, tokens_cache_read, tokens_cache_write, time_created, time_updated)
             VALUES ('ses_test_123', '/test/project', 'Explore project', '{\"id\":\"claude-3-7-sonnet\"}', 10000, 500, 7500, 0, 1781210000000, 1781210010000);",
            [],
        )
        .unwrap();

        // Insert user message and part
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, data)
             VALUES ('msg_u1', 'ses_test_123', 1781210000000, '{\"role\":\"user\"}');",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO part (id, message_id, session_id, time_created, data)
             VALUES ('prt_u1', 'msg_u1', 'ses_test_123', 1781210000000, '{\"type\":\"text\",\"text\":\"Read Cargo.toml\"}');",
            [],
        )
        .unwrap();

        // Insert assistant message with reasoning and tool call
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, data)
             VALUES ('msg_a1', 'ses_test_123', 1781210005000, '{\"role\":\"assistant\"}');",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO part (id, message_id, session_id, time_created, data)
             VALUES ('prt_a1_reason', 'msg_a1', 'ses_test_123', 1781210005000, '{\"type\":\"reasoning\",\"text\":\"Let me inspect Cargo.toml\"}');",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO part (id, message_id, session_id, time_created, data)
             VALUES ('prt_a1_tool', 'msg_a1', 'ses_test_123', 1781210006000, '{\"type\":\"tool\",\"tool\":\"read\",\"callID\":\"call_c1\",\"state\":{\"status\":\"completed\",\"input\":{\"filePath\":\"Cargo.toml\"},\"output\":\"[package]\\nname=\\\"test\\\"\"}}');",
            [],
        )
        .unwrap();

        let store = CwStore::open_in_memory().unwrap();
        let imported = import_opencode_state_db(&db_path, &store, 10).unwrap();
        assert_eq!(imported.len(), 1);
        assert_eq!(imported[0], "opencode_ses_test_123");

        let sess = store.get_session("opencode_ses_test_123").unwrap().unwrap();
        assert_eq!(sess.title.as_deref(), Some("Explore project"));
        assert_eq!(sess.model, "claude-3-7-sonnet");
        assert_eq!(sess.harness, HarnessKind::OpenCode);

        let turn = store
            .get_latest_turn("opencode_ses_test_123")
            .unwrap()
            .unwrap();
        assert!(turn.token_usage.cached_tokens > 0);
        assert_eq!(turn.token_usage.output_tokens, 500);
        assert!(turn.kv_cache_report.hit_ratio > 0.0);

        let file_states = store.get_file_states("opencode_ses_test_123").unwrap();
        assert_eq!(file_states.len(), 1);
        assert_eq!(file_states[0].path, "Cargo.toml");

        let _ = fs::remove_dir_all(&tmp_dir);
    }
}
