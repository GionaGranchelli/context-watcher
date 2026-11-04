//! Human-only consent UI. Policy state always comes from the selected daemon, never the TUI store.
use anyhow::{Context, Result, bail};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::{
    Terminal,
    backend::Backend,
    widgets::{Block, Borders, Paragraph},
};
use serde_json::{Value, json};
use std::time::Duration;

pub struct PolicyClient {
    client: reqwest::Client,
    base: reqwest::Url,
}
impl PolicyClient {
    pub fn new(base: &str, token: &str) -> Result<Self> {
        let url = reqwest::Url::parse(base)?;
        let loopback = url.host_str().is_some_and(|h| {
            h == "localhost"
                || h.trim_matches(['[', ']'])
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        });
        if !loopback
            || !matches!(url.scheme(), "http" | "https")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
        {
            bail!("Use a bare loopback HTTP(S) daemon URL");
        }
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-cw-control-token", token.parse()?);
        Ok(Self {
            client: reqwest::Client::builder()
                .default_headers(headers)
                .timeout(Duration::from_secs(5))
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            base: url,
        })
    }
    async fn sessions(&self) -> Result<Vec<String>> {
        let sessions: Vec<cw_core::ir::Session> = self
            .client
            .get(self.base.join("api/v1/sessions")?)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let ids: Vec<_> = sessions.into_iter().map(|s| s.id).collect();
        if ids
            .iter()
            .any(|s| !cw_core::control::is_valid_session_id(s))
        {
            bail!("Invalid daemon session ID");
        }
        Ok(ids)
    }
    async fn policy(&self, id: &str, update: Option<bool>) -> Result<Value> {
        if !cw_core::control::is_valid_session_id(id) {
            bail!("Invalid session ID");
        }
        let url = self
            .base
            .join(&format!("api/v1/sessions/{id}/pin-context-policy"))?;
        let request = match update {
            Some(enabled) => self
                .client
                .post(url)
                .json(&json!({"enabled":enabled,"acknowledge_risk":enabled})),
            None => self.client.get(url),
        };
        let response = request.send().await?.error_for_status()?;
        let body = response.json().await.context("Malformed policy JSON")?;
        Ok(cw_core::control::validate_pin_policy_preview(body, id)?)
    }
}

#[derive(Default)]
struct PolicyView {
    id: String,
    preview: Option<Value>,
    confirmation: Option<String>,
    scroll: usize,
    reviewed: bool,
    message: String,
}
impl PolicyView {
    async fn load(&mut self, client: &PolicyClient, id: &str) {
        *self = Self {
            id: id.into(),
            ..Default::default()
        };
        match client.policy(id, None).await {
            Ok(body) => self.preview = Some(body),
            Err(_) => self.message = "Consent state unconfirmed. Check daemon/credential and [r] refresh; [d] can request revocation.".into(),
        }
    }
    fn begin_confirmation(&mut self) {
        if self.reviewed
            && self.preview.as_ref().is_some_and(|p| {
                p["enabled"] == false
                    && p["session_terminated"] == false
                    && p["reinforcement_supported"] == true
            })
        {
            self.confirmation = Some(String::new());
        }
    }
    async fn update(&mut self, client: &PolicyClient, enabled: bool) {
        let original = self.preview.clone();
        let expected = format!("ENABLE {}", self.id);
        if enabled
            && (!self.reviewed
                || self.confirmation.as_deref() != Some(expected.as_str())
                || original.is_none())
        {
            return;
        }
        self.confirmation = None;
        self.reviewed = false;
        // A changed snapshot requires a new human review. Never reuse acknowledgement after refresh.
        if enabled {
            match client.policy(&self.id, None).await {
                Ok(current) if Some(&current) == original.as_ref() => {}
                _ => {
                    let id = self.id.clone();
                    self.load(client, &id).await;
                    self.message =
                        "Preview changed or unavailable. Review again before enabling.".into();
                    return;
                }
            }
        }
        let result = client.policy(&self.id, Some(enabled)).await;
        let confirmed = result
            .as_ref()
            .is_ok_and(|p| p["enabled"].as_bool() == Some(enabled));
        let id = self.id.clone();
        self.load(client, &id).await;
        self.message = if confirmed && self.preview.as_ref().is_some_and(|p| p["enabled"].as_bool() == Some(enabled)) {
            if enabled { "Session reinforcement authorized. Actual provider application and measured savings are not established." } else { "Consent revoked. Stored rules were not deleted." }
        } else { "Consent update unconfirmed. Refresh before retrying; disable can request revocation." }.into();
    }
    fn text(&self) -> String {
        let mut text = format!(
            "DAEMON POLICY SESSION: {}\nStored/committed is not consent. Authorization is not preview eligibility or actual provider application. Measured savings unavailable.\n\n",
            self.id
        );
        if let Some(preview) = &self.preview {
            // JSON escaping keeps untrusted pin text/control characters literal in the terminal.
            text.push_str(&serde_json::to_string_pretty(preview).unwrap_or_default());
        } else {
            text.push_str("Policy unavailable; enable is disabled.");
        }
        text
    }
}

fn draw_policy(frame: &mut ratatui::Frame, view: &mut PolicyView, base: &str) {
    let area = frame.area();
    if area.width < 40 || area.height < 12 {
        view.reviewed = false;
        view.confirmation = None;
        frame.render_widget(
            Paragraph::new(
                "Resize terminal to at least 40x12 to review policy. Enable is unavailable.",
            ),
            area,
        );
        return;
    }
    let width = usize::from(area.width.saturating_sub(2)).max(1);
    // Conservative wrapping also keeps wide Unicode glyphs inspectable without horizontal clipping.
    let columns = (width / 2).max(1);
    let lines: Vec<String> = view
        .text()
        .lines()
        .flat_map(|line| {
            let chars: Vec<_> = line.chars().collect();
            if chars.is_empty() {
                vec![String::new()]
            } else {
                chars.chunks(columns).map(|c| c.iter().collect()).collect()
            }
        })
        .collect();
    let height = usize::from(area.height.saturating_sub(8)).max(1);
    view.scroll = view.scroll.min(lines.len().saturating_sub(height));
    if view.scroll + height >= lines.len() && view.preview.is_some() {
        view.reviewed = true;
    }
    let footer = if let Some(input) = &view.confirmation {
        format!(
            "Type ENABLE {} then Enter (Esc cancels): {}",
            view.id, input
        )
    } else {
        format!(
            "[j/k/PgDn/PgUp] Read | [e] Confirm enable after reading | [d] Revoke | [r] Refresh | [Tab] Daemon session | [q/Esc] Back\n{}",
            view.message
        )
    };
    let visible = lines
        .iter()
        .skip(view.scroll)
        .take(height)
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    let parts = ratatui::layout::Layout::vertical([
        ratatui::layout::Constraint::Min(1),
        ratatui::layout::Constraint::Length(6),
    ])
    .split(area);
    frame.render_widget(
        Paragraph::new(visible).block(Block::default().borders(Borders::ALL).title(format!(
            " PinContext (daemon {base}; separate from local optimization workspace) "
        ))),
        parts[0],
    );
    frame.render_widget(
        Paragraph::new(footer).wrap(ratatui::widgets::Wrap { trim: false }),
        parts[1],
    );
}

pub async fn run_policy_view<B: Backend>(
    terminal: &mut Terminal<B>,
    base: &str,
    initial: Option<&str>,
) -> Result<()>
where
    B::Error: Send + Sync + 'static,
{
    let token = cw_core::control::control_token(false)?;
    let client = PolicyClient::new(base, &token)?;
    let mut ids = client.sessions().await?;
    if ids.is_empty() {
        bail!("No daemon sessions. Policy uses the daemon, not the local optimization database.");
    }
    let mut index = match initial {
        Some(id) => ids.iter().position(|s| s == id).context(
            "Selected local session is absent from the daemon; check the daemon URL/database",
        )?,
        None => 0,
    };
    let mut view = PolicyView::default();
    view.load(&client, &ids[index]).await;
    loop {
        terminal.draw(|frame| draw_policy(frame, &mut view, base))?;
        if !event::poll(Duration::from_millis(250))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        if let Some(input) = &mut view.confirmation {
            match key.code {
                KeyCode::Esc => view.confirmation = None,
                KeyCode::Enter => view.update(&client, true).await,
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Char(c) if !c.is_control() && input.len() < 160 => input.push(c),
                _ => {}
            }
            continue;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return Ok(()),
            KeyCode::Down | KeyCode::Char('j') => view.scroll = view.scroll.saturating_add(1),
            KeyCode::Up | KeyCode::Char('k') => view.scroll = view.scroll.saturating_sub(1),
            KeyCode::PageDown => view.scroll = view.scroll.saturating_add(10),
            KeyCode::PageUp => view.scroll = view.scroll.saturating_sub(10),
            KeyCode::Char('e') => view.begin_confirmation(),
            KeyCode::Char('d') => view.update(&client, false).await,
            KeyCode::Char('r') | KeyCode::Tab => match client.sessions().await {
                Ok(current) if !current.is_empty() => {
                    let old = view.id.clone();
                    ids = current;
                    index = ids.iter().position(|s| s == &old).unwrap_or(0);
                    if key.code == KeyCode::Tab {
                        index = (index + 1) % ids.len();
                    }
                    view.load(&client, &ids[index]).await;
                }
                _ => {
                    view = PolicyView {
                        id: view.id.clone(),
                        message: "Daemon sessions unavailable; refresh before retrying.".into(),
                        ..Default::default()
                    };
                }
            },
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cw_core::ir::{
        HarnessKind, MutationOp, MutationRule, MutationStatus, ProviderKind, Session,
    };
    use cw_proxy::{DaemonState, build_api_router};
    use cw_store::CwStore;

    async fn fixture() -> (PolicyClient, CwStore, tokio::task::JoinHandle<()>) {
        let store = CwStore::open_in_memory().unwrap();
        for (id, provider) in [
            ("first", ProviderKind::OpenAI),
            ("second", ProviderKind::OpenAI),
            ("gemini", ProviderKind::Gemini),
        ] {
            store
                .upsert_session(&Session {
                    id: id.into(),
                    parent_session_id: None,
                    harness: HarnessKind::GenericProxy,
                    workspace_root: None,
                    model: "offline".into(),
                    provider,
                    title: None,
                    created_at: chrono::Utc::now(),
                    updated_at: chrono::Utc::now(),
                })
                .unwrap();
        }
        store
            .save_mutation(&MutationRule {
                id: "pin".into(),
                session_id: "first".into(),
                op: MutationOp::PinContext {
                    label: "Important".into(),
                    content: "Full preserved contents".into(),
                    source_node_id: None,
                },
                status: MutationStatus::Committed,
                estimated_saved_tokens: 0,
                estimated_kv_break_tokens: 0,
                created_at: chrono::Utc::now(),
            })
            .unwrap();
        let mut state = DaemonState::new(store.clone());
        state.control_token = "offline-token".into();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, build_api_router(state))
                .await
                .unwrap();
        });
        (
            PolicyClient::new(&base, "offline-token").unwrap(),
            store,
            server,
        )
    }
    #[tokio::test]
    async fn human_confirmation_refresh_switch_and_revoke_use_daemon_state() {
        let (client, store, server) = fixture().await;
        let mut view = PolicyView::default();
        view.load(&client, "first").await;
        assert!(view.text().contains("Full preserved contents"));
        assert!(view.text().contains("future PinContext"));
        assert_eq!(view.preview.as_ref().unwrap()["enabled"], false);
        view.begin_confirmation();
        assert!(view.confirmation.is_none());
        view.reviewed = true;
        view.begin_confirmation();
        assert_eq!(view.confirmation, Some(String::new()));
        view.confirmation = Some("yes".into());
        view.update(&client, true).await;
        assert!(
            !store
                .get_pin_context_policy("first")
                .unwrap()
                .is_enabled_for("first")
        );
        view.confirmation = Some("ENABLE first".into());
        view.update(&client, true).await;
        assert!(
            store
                .get_pin_context_policy("first")
                .unwrap()
                .is_enabled_for("first")
        );
        assert!(view.message.contains("authorized"));
        view.update(&client, false).await;
        assert!(
            !store
                .get_pin_context_policy("first")
                .unwrap()
                .is_enabled_for("first")
        );
        assert_eq!(store.list_mutations("first").unwrap().len(), 1);
        view.reviewed = true;
        view.begin_confirmation();
        view.confirmation = Some("ENABLE first".into());
        view.load(&client, "second").await;
        assert!(!view.reviewed);
        assert!(view.confirmation.is_none());
        view.update(&client, true).await;
        assert!(
            !store
                .get_pin_context_policy("second")
                .unwrap()
                .is_enabled_for("second")
        );
        view.load(&client, "first").await;
        view.reviewed = true;
        view.begin_confirmation();
        view.confirmation = Some("ENABLE first".into());
        // A concurrent grant changes events and invalidates the reviewed snapshot.
        store.set_pin_context_policy("first", true, true).unwrap();
        let count = store.list_pin_context_policy_events("first").unwrap().len();
        view.update(&client, true).await;
        assert!(view.message.contains("Preview changed"));
        assert_eq!(
            store.list_pin_context_policy_events("first").unwrap().len(),
            count
        );
        server.abort();
        let _ = server.await;
    }
    #[tokio::test]
    async fn invalid_terminated_unsupported_and_failed_responses_cannot_enable() {
        let (client, store, server) = fixture().await;
        store
            .mark_session_terminated("first", Some("test"))
            .unwrap();
        for id in ["first", "gemini", "missing", "bad/id"] {
            let mut view = PolicyView::default();
            view.load(&client, id).await;
            view.reviewed = true;
            view.begin_confirmation();
            assert!(view.confirmation.is_none());
        }
        server.abort();
        let _ = server.await;
        // Use a closed listener: an aborted Axum listener can leave pooled connections alive.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let unavailable = PolicyClient::new(&base, "offline-token").unwrap();
        let mut view = PolicyView::default();
        view.load(&unavailable, "second").await;
        assert!(view.preview.is_none());
        assert!(view.message.contains("unconfirmed"));
        view.update(&unavailable, false).await;
        assert!(!view.message.contains("Consent revoked"));
        for body in [json!(null), json!({"enabled":true})] {
            assert!(cw_core::control::validate_pin_policy_preview(body, "second").is_err());
        }
    }
    #[tokio::test]
    async fn malformed_and_failed_api_operations_never_report_authorization() {
        for (body, status) in [
            ("not JSON", axum::http::StatusCode::OK),
            ("null", axum::http::StatusCode::OK),
            ("{}", axum::http::StatusCode::SERVICE_UNAVAILABLE),
        ] {
            let app = axum::Router::new().route(
                "/api/v1/sessions/first/pin-context-policy",
                axum::routing::get(move || async move { (status, body) })
                    .post(move || async move { (status, body) }),
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let client = PolicyClient::new(
                &format!("http://{}", listener.local_addr().unwrap()),
                "offline-token",
            )
            .unwrap();
            let server = tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            let mut view = PolicyView::default();
            view.load(&client, "first").await;
            assert!(view.preview.is_none());
            view.reviewed = true;
            view.begin_confirmation();
            assert!(view.confirmation.is_none());
            view.update(&client, false).await;
            assert!(view.message.contains("unconfirmed"));
            server.abort();
            let _ = server.await;
        }
    }

    #[test]
    fn policy_renderer_requires_scrolling_before_confirmation() {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        let mut view = PolicyView {
            id: "first".into(),
            preview: Some(
                json!({"enabled":false,"session_terminated":false,"reinforcement_supported":true,"content":"long content ".repeat(300)}),
            ),
            ..Default::default()
        };
        terminal
            .draw(|f| draw_policy(f, &mut view, "http://127.0.0.1:4400"))
            .unwrap();
        assert!(!view.reviewed);
        view.begin_confirmation();
        assert!(view.confirmation.is_none());
        view.scroll = usize::MAX;
        terminal
            .draw(|f| draw_policy(f, &mut view, "http://127.0.0.1:4400"))
            .unwrap();
        assert!(view.reviewed);
        view.begin_confirmation();
        assert_eq!(view.confirmation, Some(String::new()));
        terminal
            .draw(|f| draw_policy(f, &mut view, "http://127.0.0.1:4400"))
            .unwrap();
        let screen = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(screen.contains("Type ENABLE first"));
        let mut tiny = ratatui::Terminal::new(ratatui::backend::TestBackend::new(20, 8)).unwrap();
        tiny.draw(|f| draw_policy(f, &mut view, "http://127.0.0.1:4400"))
            .unwrap();
        assert!(!view.reviewed);
        assert!(view.confirmation.is_none());
    }

    #[test]
    fn control_client_rejects_remote_and_credential_urls() {
        for base in [
            "https://example.com",
            "http://secret@localhost:4400",
            "http://localhost:4400/path",
        ] {
            assert!(PolicyClient::new(base, "test").is_err());
        }
    }
}
