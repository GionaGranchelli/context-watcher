use chrono::Utc;
use cw_core::{analyze_turn_from_payload, ir::*};
use cw_store::CwStore;
use serde_json::json;
fn session(store: &CwStore) {
    store
        .upsert_session(&Session {
            id: "s".into(),
            parent_session_id: None,
            harness: HarnessKind::GenericProxy,
            workspace_root: None,
            model: "test".into(),
            provider: ProviderKind::OpenAI,
            title: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        })
        .unwrap();
}
#[test]
fn derived_updates_preserve_originals_and_explicit_missing_stays_missing() {
    let store = CwStore::open_in_memory().unwrap();
    session(&store);
    let original = json!({"messages":[{"role":"user","content":"original"}]});
    let (turn, _) = analyze_turn_from_payload(
        "t".into(),
        "s".into(),
        None,
        1,
        &original,
        ProviderKind::OpenAI,
        None,
        None,
        None,
    )
    .unwrap();
    store.save_turn(&turn, Some(&original), None, None).unwrap();
    store
        .save_turn(&turn, None, Some(&json!({"messages":[]})), None)
        .unwrap();
    assert_eq!(store.get_turn_raw_payload("t").unwrap(), Some(original));
    let mut legacy = turn;
    legacy.id = "legacy".into();
    store
        .save_turn(&legacy, None, Some(&json!({"modified":true})), None)
        .unwrap();
    assert_eq!(store.get_turn_raw_payload("legacy").unwrap(), None);
}
#[test]
fn draft_does_not_replace_committed_rule_and_sequences_are_unique() {
    let store = CwStore::open_in_memory().unwrap();
    session(&store);
    let mut rule = MutationRule {
        id: "committed".into(),
        session_id: "s".into(),
        op: MutationOp::PinContext {
            label: "rule".into(),
            content: "keep".into(),
            source_node_id: None,
        },
        status: MutationStatus::Committed,
        estimated_saved_tokens: 0,
        estimated_kv_break_tokens: 0,
        created_at: Utc::now(),
    };
    store.save_mutation(&rule).unwrap();
    rule.id = "draft".into();
    rule.status = MutationStatus::Draft;
    store.save_mutation(&rule).unwrap();
    assert_eq!(store.list_mutations("s").unwrap().len(), 2);
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let store = store.clone();
            std::thread::spawn(move || store.reserve_turn_sequence("s").unwrap())
        })
        .collect();
    let mut seq: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    seq.sort();
    assert_eq!(seq, (1..=8).collect::<Vec<_>>());
}

#[test]
fn revision_checked_changes_are_atomic_and_session_scoped() {
    let store = CwStore::open_in_memory().unwrap();
    session(&store);
    let rule = MutationRule {
        id: "draft".into(),
        session_id: "s".into(),
        op: MutationOp::PinContext {
            label: "rule".into(),
            content: "keep".into(),
            source_node_id: None,
        },
        status: MutationStatus::Draft,
        estimated_saved_tokens: 0,
        estimated_kv_break_tokens: 0,
        created_at: Utc::now(),
    };
    store.save_mutation(&rule).unwrap();
    let before = store.session_revision("s").unwrap();
    assert_eq!(
        store
            .change_mutations_at_revision("s", &before, &[], true)
            .unwrap(),
        1
    );
    assert!(matches!(
        store.change_mutations_at_revision("s", &before, &["draft".into()], false),
        Err(cw_store::StoreError::Conflict)
    ));
    assert_eq!(
        store.list_mutations("s").unwrap()[0].status,
        MutationStatus::Committed
    );
    let other = store.session_revision("other").unwrap();
    assert_eq!(
        store
            .change_mutations_at_revision("other", &other, &["draft".into()], false)
            .unwrap(),
        0
    );
    assert_eq!(store.list_mutations("s").unwrap().len(), 1);
}

#[test]
fn migration_backup_is_private_and_purge_preserves_changed_exports() {
    let dir = std::env::temp_dir().join(format!("cw-store-safety-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&dir).unwrap();
    let db = dir.join("cw.db");
    let store = CwStore::open(&db).unwrap();
    session(&store);
    let export = dir.join("export.jsonl");
    std::fs::write(&export, b"original").unwrap();
    store
        .record_export("s", export.to_str().unwrap(), b"original")
        .unwrap();
    std::fs::write(&export, b"changed").unwrap();
    assert!(store.purge_history("s", 0).is_err());
    assert!(store.get_session("s").unwrap().is_some());
    assert_eq!(std::fs::read(&export).unwrap(), b"changed");
    drop(store);
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch("PRAGMA user_version=0").unwrap();
    drop(conn);
    let store = CwStore::open(&db).unwrap();
    assert!(store.get_session("s").unwrap().is_some());
    let backups: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|x| x.unwrap().path())
        .filter(|p| p.to_string_lossy().contains("pre-safety-"))
        .collect();
    assert_eq!(backups.len(), 1);
    let backup = rusqlite::Connection::open(&backups[0]).unwrap();
    assert_eq!(
        backup
            .query_row("SELECT count(*) FROM sessions", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&backups[0]).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    drop(backup);
    drop(store);
    let store = CwStore::open(&db).unwrap();
    assert_eq!(
        std::fs::read_dir(&dir)
            .unwrap()
            .filter(|x| x
                .as_ref()
                .unwrap()
                .path()
                .to_string_lossy()
                .contains("pre-safety-"))
            .count(),
        1
    );
    std::fs::write(&export, b"original").unwrap();
    store.purge_history("s", 0).unwrap();
    assert!(!export.exists());
    assert!(store.get_session("s").unwrap().is_none());
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn list_sessions_keeps_legacy_callers_complete_and_pageable() {
    let store = CwStore::open_in_memory().unwrap();
    for n in 0..205 {
        let id = format!("session-{n:03}");
        store
            .upsert_session(&Session {
                id,
                parent_session_id: None,
                harness: HarnessKind::GenericProxy,
                workspace_root: None,
                model: "test".into(),
                provider: ProviderKind::OpenAI,
                title: None,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            })
            .unwrap();
    }
    assert_eq!(store.list_sessions().unwrap().len(), 205);
    assert_eq!(store.list_sessions_page(200, 0).unwrap().len(), 200);
    assert_eq!(store.list_sessions_page(200, 200).unwrap().len(), 5);
}

#[test]
fn revision_tracks_new_snapshots_with_same_sequence() {
    let store = CwStore::open_in_memory().unwrap();
    session(&store);
    let a = json!({"messages":[{"role":"user","content":"old snapshot"}]});
    let (first, _) = analyze_turn_from_payload(
        "a".into(),
        "s".into(),
        None,
        3,
        &a,
        ProviderKind::OpenAI,
        None,
        None,
        None,
    )
    .unwrap();
    store.save_turn(&first, Some(&a), None, None).unwrap();
    let revision = store.session_revision("s").unwrap();
    let b = json!({"messages":[{"role":"user","content":"new snapshot"}]});
    let (second, _) = analyze_turn_from_payload(
        "z".into(),
        "s".into(),
        None,
        3,
        &b,
        ProviderKind::OpenAI,
        None,
        None,
        None,
    )
    .unwrap();
    store.save_turn(&second, Some(&b), None, None).unwrap();
    assert_ne!(revision, store.session_revision("s").unwrap());
}

#[test]
fn wire_requests_are_immutable_and_blob_storage_is_content_addressed() {
    let store = CwStore::open_in_memory().unwrap();
    session(&store);

    // 1. Content-addressed blob storage: identical payloads share exact hash
    let payload = json!({"messages": [{"role": "user", "content": "hello world"}]});
    let hash1 = store.put_blob(&payload).unwrap();
    let hash2 = store.put_blob(&payload).unwrap();
    assert_eq!(
        hash1, hash2,
        "Identical JSON payloads must yield identical BLAKE3 hashes"
    );

    let retrieved = store.get_blob(&hash1).unwrap();
    assert_eq!(retrieved, Some(payload));

    let missing = store.get_blob("nonexistent_hash_12345").unwrap();
    assert_eq!(missing, None);

    // 2. Wire requests: immutable byte storage per turn
    let wire_bytes = b"{\"model\":\"mock\",\"stream\":false,\"custom_byte_sequence\":[1,2,3]}";
    store.save_wire_request("turn_wire_1", wire_bytes).unwrap();

    // Re-saving with INSERT OR IGNORE does not overwrite original
    let wire_bytes_mutated = b"{\"model\":\"tampered\"}";
    store
        .save_wire_request("turn_wire_1", wire_bytes_mutated)
        .unwrap();

    let saved = store.get_wire_request("turn_wire_1").unwrap();
    assert_eq!(
        saved.as_deref(),
        Some(&wire_bytes[..]),
        "Wire requests must be immutable once written"
    );
}
