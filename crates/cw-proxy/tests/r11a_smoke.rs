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

    let barrier = Arc::new(tokio::sync::Barrier::new(5));
    let mut writers = Vec::new();
    for worker in 0..4 {
        let store = store.clone();
        let barrier = barrier.clone();
        writers.push(tokio::spawn(async move {
            barrier.wait().await;
            for index in 0..5 {
                let seq = store.reserve_turn_sequence("r11a-smoke").unwrap();
                let payload = json!({"messages":[{"role":"user","content":format!("fixture {worker}-{index}")}]});
                let (turn, _) = analyze_turn_from_payload(
                    format!("r11a-smoke-{seq}"), "r11a-smoke".into(), None, seq,
                    &payload, ProviderKind::OpenAI, None, None, None,
                ).unwrap();
                store.save_turn(&turn, Some(&payload), None, None).unwrap();
            }
            store.save_mutation(&MutationRule {
                id: format!("r11a-smoke-mutation-{worker}"),
                session_id: "r11a-smoke".into(),
                op: MutationOp::PinContext {
                    label: "smoke fixture".into(),
                    content: format!("generated pin {worker}"),
                    source_node_id: None,
                },
                status: MutationStatus::Committed,
                estimated_saved_tokens: 0,
                estimated_kv_break_tokens: 0,
                created_at: time,
            }).unwrap();
        }));
    }
    let reader_store = store.clone();
    let reader_barrier = barrier.clone();
    let reader = tokio::spawn(async move {
        reader_barrier.wait().await;
        for _ in 0..100 {
            let turns = reader_store.list_turns_page("r11a-smoke", 100, 0).unwrap();
            assert!(turns.len() <= 20);
            if let Some(latest) = reader_store.get_latest_turn("r11a-smoke").unwrap() {
                assert!(latest.sequence_number <= 20);
                assert!(
                    reader_store
                        .get_turn_raw_payload(&latest.id)
                        .unwrap()
                        .is_some()
                );
            }
            assert!(reader_store.list_mutations("r11a-smoke").unwrap().len() <= 4);
        }
    });
    for writer in writers {
        writer.await.unwrap();
    }
    reader.await.unwrap();

    let turns = store.list_turns_page("r11a-smoke", 100, 0).unwrap();
    assert_eq!(turns.len(), 20);
    assert_eq!(
        turns
            .iter()
            .map(|turn| turn.sequence_number)
            .collect::<Vec<_>>(),
        (1..=20).collect::<Vec<_>>()
    );
    assert!(
        turns
            .iter()
            .all(|turn| store.get_turn_raw_payload(&turn.id).unwrap().is_some())
    );
    assert_eq!(store.list_turns_page("r11a-smoke", 7, 3).unwrap().len(), 7);
    assert!(store.get_latest_turn("r11a-smoke").unwrap().is_some());
    assert_eq!(store.list_mutations("r11a-smoke").unwrap().len(), 4);
    for (id, second) in [
        ("r11a-order-a", 1),
        ("r11a-order-b", 2),
        ("r11a-order-c", 3),
    ] {
        let future = Utc
            .timestamp_opt(4_000_000_000 + second, 0)
            .single()
            .unwrap();
        store
            .upsert_session(&Session {
                id: id.into(),
                parent_session_id: None,
                harness: HarnessKind::GenericProxy,
                workspace_root: None,
                model: "fixture".into(),
                provider: ProviderKind::OpenAI,
                title: None,
                created_at: future,
                updated_at: future,
            })
            .unwrap();
    }
    assert_eq!(
        store
            .list_sessions_page(2, 0)
            .unwrap()
            .iter()
            .map(|session| session.id.as_str())
            .collect::<Vec<_>>(),
        vec!["r11a-order-c", "r11a-order-b"]
    );
    assert_eq!(
        store.list_sessions_page(2, 2).unwrap()[0].id,
        "r11a-order-a"
    );
    drop(store);
    let _ = fs::remove_file(&path);
    let _ = fs::remove_file(path.with_extension("db-wal"));
    let _ = fs::remove_file(path.with_extension("db-shm"));
}
