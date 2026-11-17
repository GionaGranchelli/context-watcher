//! End-to-end MCP → store → proxy lifecycle (Milestone C).
//!
//! The virtual memory tools are driven through the REAL JSON-RPC dispatch handler
//! (`handle_mcp_request`), and their effects must be observable on later
//! `handle_proxy_request` turns purely via the shared [`CwStore`] — no hand-built
//! register state anywhere in this test.

use axum::{Json, body::Bytes, extract::State, http::HeaderMap, response::Response, routing::post};
use cw_core::symbol_index::SymbolIndex;
use cw_core::tokenizer::estimate_tokens;
use cw_mcp::handle_mcp_request;
use cw_mcp::virtual_memory::VirtualMemoryContext;
use cw_proxy::DaemonState;
use cw_proxy::proxy::handle_proxy_request;
use serde_json::{Value, json};
use std::sync::Arc;

const SESSION_ID: &str = "cw-mcp-e2e";

/// On-disk file for the mounted target. `target_fn` occupies lines 1-4.
const TARGET_V1: &str = "pub fn target_fn() -> u32 {\n    let answer = 42;\n    answer\n}\n";

/// Deterministic Zone 1 block built from in-memory sources (no real workspace).
fn test_zone1_block() -> String {
    SymbolIndex::build_from_sources(&[("src/app.rs", "pub fn run() {}\n")])
        .render_zone1_prefix(usize::MAX / 2)
}

/// One JSON-RPC `tools/call` through the real MCP dispatch handler. Returns
/// `(is_error, text)`. The daemon URL is unused for virtual memory tools — they
/// operate directly against the shared store and never hit HTTP.
async fn mcp_call(vm: &VirtualMemoryContext, name: &str, args: Value) -> (bool, String) {
    let client = reqwest::Client::new();
    let resp = handle_mcp_request(
        &client,
        "http://127.0.0.1:1",
        vm,
        Some(json!(1)),
        "tools/call",
        &json!({"name": name, "arguments": args}),
    )
    .await
    .expect("JSON-RPC response");
    let is_error = resp["result"]["isError"].as_bool().unwrap_or(true);
    let text = resp["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("")
        .to_string();
    (is_error, text)
}

async fn post_turn(state: DaemonState, payload: &Value) -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-cw-control-token",
        state.control_token.as_str().parse().unwrap(),
    );
    headers.insert("x-cw-session-id", SESSION_ID.parse().unwrap());
    handle_proxy_request(
        State(state),
        axum::http::Method::POST,
        axum::http::Uri::try_from("/openai/v1/chat/completions").unwrap(),
        headers,
        Bytes::from(serde_json::to_vec(payload).unwrap()),
    )
    .await
}

/// The deterministic Zone 1 region of a system prompt: protocol header through
/// the closing `</workspace_symbol_map>` tag.
fn zone1_region(system: &str) -> String {
    let start = system
        .find("<cw_memory_protocol>")
        .expect("protocol header missing");
    let close = "</workspace_symbol_map>";
    let end = system.find(close).expect("symbol map frame missing") + close.len();
    system[start..end].to_string()
}

#[tokio::test]
async fn mcp_mount_unmount_lifecycle_drives_three_zone_proxy() {
    // --- Workspace holding the file to be mounted.
    let root = std::env::temp_dir().join(format!("cw_vm_e2e_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), TARGET_V1).unwrap();

    // --- Mock OpenAI upstream capturing every egress body.
    let (egress_tx, mut egress_rx) = tokio::sync::mpsc::unbounded_channel::<Bytes>();
    let app = axum::Router::new().route(
        "/v1/chat/completions",
        post(move |body: Bytes| {
            let tx = egress_tx.clone();
            async move {
                let _ = tx.send(body);
                (
                    axum::http::StatusCode::OK,
                    Json(json!({
                        "id": "chatcmpl-mcp-e2e",
                        "object": "chat.completion",
                        "model": "gpt-test",
                        "choices": [{
                            "index": 0,
                            "message": {"role": "assistant", "content": "ok"},
                            "finish_reason": "stop"
                        }],
                        "usage": {"prompt_tokens": 12, "completion_tokens": 3, "total_tokens": 15}
                    })),
                )
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_url = format!("http://{}", listener.local_addr().unwrap());
    let _server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    // --- Shared in-memory store with autopilot enabled (active turns), used by
    //     BOTH the MCP tools and the proxy turns: the same handle.
    let base_state = DaemonState::new(cw_store::CwStore::open_in_memory().unwrap());
    base_state
        .store
        .set_autopilot_config(&cw_core::autopilot::AutoPilotConfig {
            enabled: true,
            ..Default::default()
        })
        .unwrap();

    // --- MCP side: mount + pin through the real dispatch handler.
    let vm = VirtualMemoryContext::with_workspace_root(base_state.store.clone(), root.clone());

    let (err, text) = mcp_call(
        &vm,
        "cw_mount",
        json!({
            "session_id": SESSION_ID,
            "uri": "repo://src/lib.rs#target_fn",
            "mode": "full"
        }),
    )
    .await;
    assert!(!err, "{text}");
    let slice_content = TARGET_V1.trim_end(); // full mode: the exact function block
    assert_eq!(
        text,
        format!(
            "Mounted repo://src/lib.rs#target_fn (mode: full, lines: 1-4, ~{} tokens) into Zone 2.",
            estimate_tokens(slice_content)
        )
    );

    let (err, text) = mcp_call(
        &vm,
        "cw_pin",
        json!({
            "session_id": SESSION_ID,
            "key": "arch",
            "rule": "no unwrap in proxy"
        }),
    )
    .await;
    assert!(!err, "{text}");
    assert_eq!(text, "Pinned invariant [arch] in Zone 2.");

    // MCP → store handoff: registers are visible through the store API.
    let ws = base_state.store.get_working_set(SESSION_ID).unwrap();
    assert_eq!(ws.mounted.len(), 1);
    assert_eq!(
        ws.invariants.get("arch"),
        Some(&"no unwrap in proxy".to_string())
    );

    // --- Turn 1: duplicate raw read of the mounted file in the tool history.
    let turn1_payload = json!({
        "model": "gpt-test",
        "messages": [
            {"role": "system", "content": "base system prompt"},
            {"role": "user", "content": "inspect target_fn"},
            {"role": "assistant", "content": null, "tool_calls": [{"id": "c1"}]},
            {"role": "tool", "tool_call_id": "c1", "content": format!("src/lib.rs:\n{TARGET_V1}")}
        ]
    });

    let mut turn1_state = base_state.clone();
    turn1_state.openai_upstream = upstream_url.clone();
    turn1_state.zone1_block = Some(Arc::from(test_zone1_block()));
    assert_eq!(
        post_turn(turn1_state, &turn1_payload).await.status(),
        axum::http::StatusCode::OK
    );

    // --- Unmount through MCP; only the register state changes between turns.
    let (err, text) = mcp_call(
        &vm,
        "cw_unmount",
        json!({"session_id": SESSION_ID, "uri": "repo://src/lib.rs#target_fn"}),
    )
    .await;
    assert!(!err, "{text}");
    assert_eq!(text, "Unmounted repo://src/lib.rs#target_fn from Zone 2.");

    let ws = base_state.store.get_working_set(SESSION_ID).unwrap();
    assert!(
        ws.mounted.is_empty(),
        "mount must be evicted from the store"
    );
    assert_eq!(
        ws.invariants.get("arch"),
        Some(&"no unwrap in proxy".to_string())
    );

    // --- Turn 2: resends the full turn-1 history plus the reply and a new user message.
    let turn2_payload = json!({
        "model": "gpt-test",
        "messages": [
            {"role": "system", "content": "base system prompt"},
            {"role": "user", "content": "inspect target_fn"},
            {"role": "assistant", "content": null, "tool_calls": [{"id": "c1"}]},
            {"role": "tool", "tool_call_id": "c1", "content": format!("src/lib.rs:\n{TARGET_V1}")},
            {"role": "assistant", "content": "the answer is 42"},
            {"role": "user", "content": "verify it"}
        ]
    });

    let mut turn2_state = base_state.clone();
    turn2_state.openai_upstream = upstream_url;
    turn2_state.zone1_block = Some(Arc::from(test_zone1_block()));
    assert_eq!(
        post_turn(turn2_state, &turn2_payload).await.status(),
        axum::http::StatusCode::OK
    );

    let body_1: Value = serde_json::from_slice(&egress_rx.recv().await.unwrap()).unwrap();
    let body_2: Value = serde_json::from_slice(&egress_rx.recv().await.unwrap()).unwrap();

    // --- Turn 1: Zone 1 → Zone 2 (mount + invariant) → stubbed Zone 3 → HUD.
    {
        let system = body_1["messages"][0]["content"]
            .as_str()
            .unwrap_or("")
            .to_owned();
        assert!(system.starts_with("base system prompt"));
        let z1_close = "</workspace_symbol_map>";
        let z1_end = system.find(z1_close).expect("Zone 1 frame missing") + z1_close.len();
        let mounted_pos = system
            .find("<mounted_context>\n")
            .expect("Zone 2 <mounted_context> missing on turn 1");
        let pinned_pos = system
            .find("<pinned_invariants>")
            .expect("invariants frame missing on turn 1");
        assert!(mounted_pos > z1_end, "Zone 2 must follow Zone 1: {system}");
        assert!(
            pinned_pos > mounted_pos,
            "invariants frame follows mounts: {system}"
        );
        assert!(
            system.contains("### [repo://src/lib.rs#target_fn] (mode: full, lines: 1-4)"),
            "mounted symbol header missing on turn 1"
        );
        assert!(system.contains("- [arch]: no unwrap in proxy"));

        // Zone 3: the duplicate raw read is evicted to a one-line stub (the HUD
        // lands on this same final message, hence prefix + line-based checks).
        let tool = body_1["messages"][3]["content"].as_str().unwrap();
        assert!(
            tool.starts_with(
                "[Mounted in Zone 2: repo://src/lib.rs#target_fn — raw duplicate evicted]"
            ),
            "raw dump must be stubbed on turn 1: {tool}"
        );

        // HUD on the final message of turn 1.
        let hud_lines: Vec<&str> = tool.lines().filter(|l| l.starts_with("[cw:")).collect();
        assert_eq!(hud_lines.len(), 1, "exactly one HUD line on turn 1");
        assert!(
            hud_lines[0].starts_with("[cw: "),
            "HUD frame open: {}",
            hud_lines[0]
        );
        assert!(
            hud_lines[0].contains("mounted: 1 | pinned: 1"),
            "{}",
            hud_lines[0]
        );
        assert!(hud_lines[0].ends_with(']'), "{}", hud_lines[0]);
    }

    // --- Zone 1 bytes are identical across turns even though registers changed.
    let sys_1 = body_1["messages"][0]["content"].as_str().unwrap();
    let sys_2 = body_2["messages"][0]["content"].as_str().unwrap();
    assert_eq!(
        zone1_region(sys_1),
        zone1_region(sys_2),
        "Zone 1 bytes must be identical across turns"
    );

    // --- Turn 2: mount evicted, invariant kept, raw dump no longer stubbed.
    {
        assert!(
            !sys_2.contains("target_fn"),
            "evicted mount must not re-render on turn 2: {sys_2}"
        );
        assert!(
            sys_2.contains("- [arch]: no unwrap in proxy"),
            "pinned invariant must survive the unmount"
        );

        // No register left to match: Zone 3 leaves the raw dump untouched (the HUD
        // is appended after it on this same message).
        let tool = body_2["messages"][3]["content"].as_str().unwrap();
        assert!(
            tool.starts_with("src/lib.rs:\n"),
            "dump header must survive: {tool}"
        );
        assert!(
            tool.contains(TARGET_V1.trim_end()),
            "raw dump must remain after the unmount"
        );

        // HUD on the final message of turn 2.
        let hud_lines: Vec<&str> = body_2["messages"][5]["content"]
            .as_str()
            .unwrap()
            .lines()
            .filter(|l| l.starts_with("[cw:"))
            .collect();
        assert_eq!(hud_lines.len(), 1, "exactly one HUD line on turn 2");
        assert!(
            hud_lines[0].contains("mounted: 0 | pinned: 1"),
            "{}",
            hud_lines[0]
        );
    }

    let _ = std::fs::remove_dir_all(&root);
}
