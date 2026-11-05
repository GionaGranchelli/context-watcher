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

const DEFAULT_UPSTREAM_TIMEOUT_SECS: u64 = 300;
const MAX_UPSTREAM_TIMEOUT_SECS: u64 = 3600;

fn upstream_timeout(raw: Option<&str>) -> Duration {
    let seconds = raw
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|seconds| (1..=MAX_UPSTREAM_TIMEOUT_SECS).contains(seconds))
        .unwrap_or(DEFAULT_UPSTREAM_TIMEOUT_SECS);
    Duration::from_secs(seconds)
}

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
    /// When set, every provider request must be admitted by the SpendGuard before
    /// dispatch and settled from provider-reported usage (live qualification runs).
    pub spend_guard: Option<cw_core::spend_guard::SpendGuard>,
    #[cfg(feature = "qualification-fault-hooks")]
    #[doc(hidden)]
    pub qualification_fault_hook: Option<QualificationFaultHook>,
    pub(crate) intercepts: Arc<Mutex<HashMap<String, ActiveIntercept>>>,
}

/// File-backed pause point used only by the explicit qualification-fault-hooks build.
#[cfg(feature = "qualification-fault-hooks")]
#[doc(hidden)]
#[derive(Clone)]
pub struct QualificationFaultHook {
    boundary_id: String,
    root: std::path::PathBuf,
}

#[cfg(feature = "qualification-fault-hooks")]
impl QualificationFaultHook {
    pub fn from_env() -> Option<Self> {
        let boundary_id = std::env::var("CW122B1_F1_BOUNDARY_ID").ok()?;
        let root = std::env::var_os("CW122B1_F1_BOUNDARY_DIR")?.into();
        Some(Self { boundary_id, root })
    }

    pub async fn pause(
        &self,
        boundary_id: &str,
        mut details: serde_json::Value,
        artifacts: &[(&str, &[u8])],
    ) {
        if self.boundary_id != boundary_id {
            return;
        }
        use sha2::{Digest, Sha256};
        use std::io::Write;
        let mut files = Vec::new();
        for (name, bytes) in artifacts {
            let path = self.root.join(name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("create boundary evidence directory");
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
                        .expect("restrict boundary evidence directory");
                }
            }
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options
                .open(&path)
                .expect("create boundary evidence artifact");
            file.write_all(bytes)
                .expect("write boundary evidence artifact");
            let digest = Sha256::digest(bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            files.push(serde_json::json!({
                "path": name,
                "sha256": digest,
                "kind": name.rsplit('.').next().unwrap_or("artifact"),
            }));
        }
        if let Some(object) = details.as_object_mut() {
            object.insert("boundary_id".into(), serde_json::json!(boundary_id));
            object.insert("result".into(), serde_json::json!("BOUNDARY_REACHED"));
            object.insert(
                "evidence_preservation".into(),
                serde_json::json!({"status":"PRESERVED","directory":self.root,"files":files}),
            );
        }
        let marker = self.root.join("ready.json");
        let temporary = self.root.join("ready.json.tmp");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let bytes = serde_json::to_vec_pretty(&details).expect("serialize boundary marker");
        options
            .open(&temporary)
            .expect("create ready boundary marker")
            .write_all(&bytes)
            .expect("write ready boundary marker");
        std::fs::rename(temporary, marker).expect("publish ready boundary marker");
        loop {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

impl DaemonState {
    pub fn new(store: CwStore) -> Self {
        let (ws_tx, _) = broadcast::channel(256);
        // Large local-model prompts can need longer than the default five-minute bound.
        let http_client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(upstream_timeout(
                std::env::var("CW_UPSTREAM_TIMEOUT_SECS").ok().as_deref(),
            ))
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
            spend_guard: None,
            #[cfg(feature = "qualification-fault-hooks")]
            qualification_fault_hook: None,
            intercepts: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn broadcast(&self, ev: WsEvent) {
        let _ = self.ws_tx.send(ev);
    }
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_UPSTREAM_TIMEOUT_SECS, MAX_UPSTREAM_TIMEOUT_SECS, upstream_timeout};
    use std::time::Duration;

    #[test]
    fn upstream_timeout_keeps_default_and_accepts_bounded_override() {
        assert_eq!(
            upstream_timeout(None),
            Duration::from_secs(DEFAULT_UPSTREAM_TIMEOUT_SECS)
        );
        assert_eq!(upstream_timeout(Some("1800")), Duration::from_secs(1800));
        assert_eq!(
            upstream_timeout(Some("0")),
            Duration::from_secs(DEFAULT_UPSTREAM_TIMEOUT_SECS)
        );
        assert_eq!(
            upstream_timeout(Some("3601")),
            Duration::from_secs(DEFAULT_UPSTREAM_TIMEOUT_SECS)
        );
        assert_eq!(
            upstream_timeout(Some("invalid")),
            Duration::from_secs(DEFAULT_UPSTREAM_TIMEOUT_SECS)
        );
        assert_eq!(
            upstream_timeout(Some(&MAX_UPSTREAM_TIMEOUT_SECS.to_string())),
            Duration::from_secs(MAX_UPSTREAM_TIMEOUT_SECS)
        );
    }
}
