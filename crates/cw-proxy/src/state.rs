use chrono::Utc;
use cw_core::{
    autopilot::AmbientSavingsPulse,
    ir::{ContextTurn, FileContextState, MutationRule, Session},
};
use cw_store::CwStore;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::{Mutex, broadcast, oneshot};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum WsEvent {
    SessionUpdated {
        session: Session,
    },
    TurnCompleted {
        session_id: String,
        turn: Box<ContextTurn>,
    },
    MutationsUpdated {
        session_id: String,
        mutations: Vec<MutationRule>,
    },
    FileContextStatusChanged {
        session_id: String,
        workspace_root: Option<String>,
        file_states: Vec<FileContextState>,
    },
    FirewallIntercepted {
        intercept: PendingInterceptInfo,
    },
    FirewallResolved {
        intercept_id: String,
    },
    AmbientSavingsPulse {
        session_id: String,
        pulse: Box<AmbientSavingsPulse>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingInterceptInfo {
    pub intercept_id: String,
    pub session_id: String,
    pub turn_seq: u32,
    pub rule_name: String,
    pub payload_preview: Value,
    pub created_at: chrono::DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterceptResolution {
    pub action: ApprovalAction,
    pub edited_payload: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalAction {
    Approve,
    Abort,
    Edit,
}

pub(crate) struct ActiveIntercept {
    pub(crate) info: PendingInterceptInfo,
    pub(crate) tx: oneshot::Sender<InterceptResolution>,
}

#[derive(Clone)]
pub struct DaemonState {
    pub store: CwStore,
    pub control_token: String,
    pub ws_tx: broadcast::Sender<WsEvent>,
    pub http_client: reqwest::Client,
    pub anthropic_upstream: String,
    pub openai_upstream: String,
    pub deepseek_upstream: String,
    pub gemini_upstream: String,
    pub ollama_upstream: String,
    pub llamacpp_upstream: String,
    pub llamacpp_api_key: Option<String>,
    pub vllm_upstream: String,
    pub capture_wire_egress: bool,
    pub(crate) intercepts: Arc<Mutex<HashMap<String, ActiveIntercept>>>,
}

impl DaemonState {
    pub fn new(store: CwStore) -> Self {
        let (ws_tx, _) = broadcast::channel(256);
        let http_client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(300))
            .build()
            .unwrap_or_default();

        Self {
            store,
            control_token: Uuid::new_v4().simple().to_string(),
            ws_tx,
            http_client,
            anthropic_upstream: std::env::var("CW_ANTHROPIC_UPSTREAM")
                .unwrap_or_else(|_| "https://api.anthropic.com".to_string()),
            openai_upstream: std::env::var("CW_OPENAI_UPSTREAM")
                .unwrap_or_else(|_| "https://api.openai.com".to_string()),
            deepseek_upstream: std::env::var("CW_DEEPSEEK_UPSTREAM")
                .unwrap_or_else(|_| "https://api.deepseek.com".to_string()),
            gemini_upstream: std::env::var("CW_GEMINI_UPSTREAM")
                .unwrap_or_else(|_| "https://generativelanguage.googleapis.com".to_string()),
            ollama_upstream: std::env::var("CW_OLLAMA_UPSTREAM")
                .unwrap_or_else(|_| "http://127.0.0.1:11434".to_string()),
            llamacpp_api_key: std::env::var("CW_LLAMACPP_API_KEY").ok(),
            llamacpp_upstream: std::env::var("CW_LLAMACPP_UPSTREAM")
                .unwrap_or_else(|_| "http://127.0.0.1:8080".to_string()),
            vllm_upstream: std::env::var("CW_VLLM_UPSTREAM")
                .unwrap_or_else(|_| "http://127.0.0.1:8000".to_string()),
            capture_wire_egress: std::env::var("CW_CAPTURE_WIRE_EGRESS").ok().as_deref()
                == Some("1"),
            intercepts: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn broadcast(&self, ev: WsEvent) {
        let _ = self.ws_tx.send(ev);
    }
}
