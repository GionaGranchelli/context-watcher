use chrono::{TimeZone, Utc};
use cw_core::{analyze_turn_from_payload, ir::*};
use cw_store::CwStore;
use serde_json::json;
use std::{fs, sync::Arc};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn generated_history_pages_survive_concurrent_appends() {
    let path = std::env::temp_dir().join(format!("cw_r11a_smoke_{}.db", uuid::Uuid::new_v4()));
    let store = Arc::new(CwStore::open(&path).unwrap());
    let time = Utc.timestamp_opt(1_700_000_000, 0).single().unwrap();
    store
        .upsert_session(&Session {
            id: "r11a-smoke".into(),
            parent_session_id: None,
            harness: HarnessKind::GenericProxy,
            workspace_root: None,
            model: "fixture".into(),
            provider: ProviderKind::OpenAI,
            title: None,
            created_at: time,
            updated_at: time,
        })
        .unwrap();

    let mut writers = Vec::new();
    for worker in 0..4 {
        let store = store.clone();
        writers.push(tokio::spawn(async move {
            for index in 0..5 {
                let seq = store.reserve_turn_sequence("r11a-smoke").unwrap();
                let payload = json!({"messages":[{"role":"user","content":format!("fixture {worker}-{index}")}]});
                let (turn, _) = analyze_turn_from_payload(
                    format!("r11a-smoke-{seq}"), "r11a-smoke".into(), None, seq,
                    &payload, ProviderKind::OpenAI, None, None, None,
                ).unwrap();
                store.save_turn(&turn, Some(&payload), None, None).unwrap();
            }
        }));
    }
    for writer in writers {
        writer.await.unwrap();
    }

    assert_eq!(
        store.list_turns_page("r11a-smoke", 100, 0).unwrap().len(),
        20
    );
    assert_eq!(store.list_turns_page("r11a-smoke", 7, 3).unwrap().len(), 7);
    assert!(store.get_latest_turn("r11a-smoke").unwrap().is_some());
    drop(store);
    let _ = fs::remove_file(&path);
    let _ = fs::remove_file(path.with_extension("db-wal"));
    let _ = fs::remove_file(path.with_extension("db-shm"));
}
