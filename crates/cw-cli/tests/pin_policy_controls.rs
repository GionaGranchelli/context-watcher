//! Real CLI -> isolated loopback control API. No model endpoints or production configuration.
use axum::{Json, Router, http::StatusCode, routing::get};
use chrono::Utc;
use cw_core::ir::{HarnessKind, MutationOp, MutationRule, MutationStatus, ProviderKind, Session};
use cw_proxy::{DaemonState, build_api_router};
use cw_store::CwStore;
use serde_json::json;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

struct Fixture {
    store: CwStore,
    base: String,
    token: String,
    home: PathBuf,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
        let _ = std::fs::remove_dir_all(&self.home);
    }
}
impl Fixture {
    async fn new() -> Self {
        let store = CwStore::open_in_memory().unwrap();
        for (id, provider) in [
            ("session-a", ProviderKind::OpenAI),
            ("session-b", ProviderKind::OpenAI),
            ("gemini-session", ProviderKind::Gemini),
        ] {
            store
                .upsert_session(&Session {
                    id: id.into(),
                    parent_session_id: None,
                    harness: HarnessKind::GenericProxy,
                    workspace_root: None,
                    model: "offline-fixture".into(),
                    provider,
                    title: None,
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                })
                .unwrap();
        }
        for (id, status) in [
            ("stored-committed", MutationStatus::Committed),
            ("stored-draft", MutationStatus::Draft),
        ] {
            store
                .save_mutation(&MutationRule {
                    id: id.into(),
                    session_id: "session-a".into(),
                    op: MutationOp::PinContext {
                        label: id.into(),
                        content: "Preserve the public API.".into(),
                        source_node_id: None,
                    },
                    status,
                    estimated_saved_tokens: 0,
                    estimated_kv_break_tokens: 0,
                    created_at: Utc::now(),
                })
                .unwrap();
        }
        let config = cw_core::autopilot::AutoPilotConfig {
            enabled: true,
            auto_cache_safe_pin: true,
            ..Default::default()
        };
        store.set_autopilot_config(&config).unwrap();
        let mut state = DaemonState::new(store.clone());
        let token = uuid::Uuid::new_v4().simple().to_string();
        state.control_token = token.clone();
        let (base, server) = serve(build_api_router(state)).await;
        let home = std::env::temp_dir().join(format!("cw121c1-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&home).unwrap();
        Self {
            store,
            base,
            token,
            home,
            server,
        }
    }
    async fn command(
        &self,
        id: &str,
        args: &[&str],
        token: &str,
        base: &str,
    ) -> std::process::Output {
        tokio::process::Command::new(env!("CARGO_BIN_EXE_cw"))
            .args(["pin-policy", "--session", id, "--daemon-url", base])
            .args(args)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("XDG_DATA_HOME", self.home.join("data"))
            .env("CW_CONTROL_TOKEN", token)
            .env_remove("OPENAI_API_KEY")
            .current_dir(&self.home)
            .output()
            .await
            .unwrap()
    }
    async fn run(&self, id: &str, args: &[&str]) -> std::process::Output {
        self.command(id, args, &self.token, &self.base).await
    }
}
async fn serve(app: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (base, server)
}
fn output(result: &std::process::Output) -> String {
    String::from_utf8_lossy(&result.stdout).into_owned()
}

#[tokio::test]
async fn cli_previews_before_explicit_enable_and_revokes_without_deleting_rules() {
    let f = Fixture::new().await;
    let original = serde_json::to_value(f.store.list_mutations("session-a").unwrap()).unwrap();
    let preview = f.run("session-a", &["preview"]).await;
    assert!(preview.status.success());
    let text = output(&preview);
    for expected in [
        "stored-committed",
        "stored-draft",
        "Preserve the public API.",
        "may_generate_after_opt_in",
        "activation_scope",
    ] {
        assert!(text.contains(expected), "missing {expected}");
    }
    let refused = f.run("session-a", &["enable"]).await;
    assert!(!refused.status.success());
    assert!(output(&refused).contains(cw_proxy::api::pin_policy::RISK_WARNING));
    assert!(String::from_utf8_lossy(&refused.stderr).contains("--acknowledge-risk"));
    assert!(
        f.store
            .list_pin_context_policy_events("session-a")
            .unwrap()
            .is_empty()
    );
    let enabled = f.run("session-a", &["enable", "--acknowledge-risk"]).await;
    assert!(
        enabled.status.success(),
        "{}",
        String::from_utf8_lossy(&enabled.stderr)
    );
    let text = output(&enabled);
    assert!(
        text.find(cw_proxy::api::pin_policy::RISK_WARNING).unwrap()
            < text.find("Reinforcement authorized").unwrap()
    );
    assert!(text.contains("does not prove provider-request application"));
    assert!(
        f.store
            .get_pin_context_policy("session-a")
            .unwrap()
            .is_enabled_for("session-a")
    );
    assert!(
        !f.store
            .get_pin_context_policy("session-b")
            .unwrap()
            .is_enabled_for("session-b")
    );
    let disabled = f.run("session-a", &["disable"]).await;
    assert!(disabled.status.success());
    assert!(output(&disabled).contains("Stored rules were not deleted"));
    assert!(
        !f.store
            .get_pin_context_policy("session-a")
            .unwrap()
            .is_enabled_for("session-a")
    );
    assert_eq!(
        serde_json::to_value(f.store.list_mutations("session-a").unwrap()).unwrap(),
        original
    );
    assert_eq!(
        f.store
            .list_pin_context_policy_events("session-a")
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn cli_rejects_invalid_sessions_credentials_termination_and_gemini() {
    let f = Fixture::new().await;
    for id in ["bad/id", "missing-session", "gemini-session"] {
        let result = f.run(id, &["enable", "--acknowledge-risk"]).await;
        assert!(!result.status.success());
    }
    let result = f
        .command(
            "session-a",
            &["enable", "--acknowledge-risk"],
            "wrong-token-123456789012345678901234567890",
            &f.base,
        )
        .await;
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("HTTP 401"));
    f.store
        .mark_session_terminated("session-a", Some("offline-test"))
        .unwrap();
    let result = f.run("session-a", &["enable", "--acknowledge-risk"]).await;
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("terminated session"));
    assert!(
        f.store
            .list_pin_context_policy_events("session-a")
            .unwrap()
            .is_empty()
    );
    assert!(f.run("session-a", &["disable"]).await.status.success());
}

#[tokio::test]
async fn cli_handles_unavailable_failed_and_invalid_api_responses_without_success_claims() {
    let f = Fixture::new().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let closed = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let result = f
        .command(
            "session-a",
            &["enable", "--acknowledge-risk"],
            &f.token,
            &closed,
        )
        .await;
    assert!(!result.status.success());
    for (get_status, body, post_status, expected_posts) in [
        (
            StatusCode::SERVICE_UNAVAILABLE,
            json!(null),
            StatusCode::OK,
            0,
        ),
        (StatusCode::OK, json!(null), StatusCode::OK, 0),
        (
            StatusCode::OK,
            json!({"session_id":"session-a","enabled":false,"session_terminated":false,"reinforcement_supported":true,"risk_warning":"Risk: latency, cache, cost, behavior.","activation_scope":"All session pins","preview_basis":"Latest request only","stored_pins":[],"automatic_generation":{"may_generate_after_opt_in":false,"auto_cache_safe_pin":false,"auto_heal_low_health":false,"heal_strategy":"surgical_log_diet"}}),
            StatusCode::INTERNAL_SERVER_ERROR,
            1,
        ),
        (
            StatusCode::OK,
            json!({"session_id":"session-a","enabled":false,"session_terminated":false,"reinforcement_supported":true,"risk_warning":"Risk: latency, cache, cost, behavior.","activation_scope":"All session pins","preview_basis":"Latest request only","stored_pins":[],"automatic_generation":{"may_generate_after_opt_in":false,"auto_cache_safe_pin":false,"auto_heal_low_health":false,"heal_strategy":"surgical_log_diet"}}),
            StatusCode::OK,
            1,
        ),
    ] {
        let posts = Arc::new(AtomicUsize::new(0));
        let count = posts.clone();
        let reply = body.clone();
        let app = Router::new().route(
            "/api/v1/sessions/session-a/pin-context-policy",
            get(move || {
                let body = body.clone();
                async move { (get_status, Json(body)) }
            })
            .post(move || {
                let count = count.clone();
                let body = reply.clone();
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    (post_status, Json(body))
                }
            }),
        );
        let (base, server) = serve(app).await;
        let result = f
            .command(
                "session-a",
                &["enable", "--acknowledge-risk"],
                &f.token,
                &base,
            )
            .await;
        assert!(!result.status.success());
        assert!(!output(&result).contains("Reinforcement authorized"));
        assert_eq!(posts.load(Ordering::SeqCst), expected_posts);
        server.abort();
    }
    assert!(
        f.store
            .list_pin_context_policy_events("session-a")
            .unwrap()
            .is_empty()
    );
}
