use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use cw_core::ir::{ContextTurn, KvNodeStatus, MutationOp, MutationRule, Session};
use cw_store::CwStore;
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, List, ListItem, ListState, Paragraph, Wrap},
};
use std::{io, time::Duration};
use uuid::Uuid;

pub struct TuiApp {
    store: CwStore,
    sessions: Vec<Session>,
    session_idx: usize,
    latest_turn: Option<ContextTurn>,
    mutations: Vec<MutationRule>,
    node_state: ListState,
    status_msg: String,
}

impl TuiApp {
    pub fn new(store: CwStore, initial_session: Option<String>) -> Self {
        let mut app = Self {
            store,
            sessions: Vec::new(),
            session_idx: 0,
            latest_turn: None,
            mutations: Vec::new(),
            node_state: ListState::default(),
            status_msg: "Ready. Use ↑/↓ to navigate nodes, [p] Prune, [d] Dedupe, [f] Fold, [Space] Pin, [o] Optimize, [q] Quit".to_string(),
        };
        app.reload(initial_session.as_deref());
        app
    }

    pub fn reload(&mut self, target_session_id: Option<&str>) {
        self.sessions = self.store.list_sessions().unwrap_or_default();
        if let Some(tid) = target_session_id
            && let Some(pos) = self.sessions.iter().position(|s| s.id == tid) {
                self.session_idx = pos;
            }
        if self.session_idx >= self.sessions.len() {
            self.session_idx = 0;
        }
        if let Some(sess) = self.sessions.get(self.session_idx) {
            self.latest_turn = self.store.get_latest_turn(&sess.id).ok().flatten();
            self.mutations = self.store.list_mutations(&sess.id).unwrap_or_default();
            if self.node_state.selected().is_none() {
                self.node_state.select(Some(0));
            }
        } else {
            self.latest_turn = None;
            self.mutations.clear();
        }
    }

    fn apply_op(&mut self, op: MutationOp, desc: &str) {
        let Some(sess) = self.sessions.get(self.session_idx) else {
            return;
        };
        let rule = MutationRule {
            id: format!("mut_{}", &Uuid::new_v4().to_string()[..8]),
            session_id: sess.id.clone(),
            op,
            status: cw_core::ir::MutationStatus::Committed,
            estimated_saved_tokens: 0,
            estimated_kv_break_tokens: 0,
            created_at: chrono::Utc::now(),
        };
        let _ = self.store.save_mutation(&rule);
        self.status_msg = format!("Applied mutation: {desc}");
        self.reload(None);
    }
}

pub fn run_tui(store: CwStore, session_id: Option<String>) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = TuiApp::new(store, session_id);

    loop {
        terminal.draw(|f| draw_ui(f, &mut app))?;

        if event::poll(Duration::from_millis(250))?
            && let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Char('r') => app.reload(None),
                    KeyCode::Tab => {
                        if !app.sessions.is_empty() {
                            app.session_idx = (app.session_idx + 1) % app.sessions.len();
                            app.node_state.select(Some(0));
                            app.reload(None);
                        }
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        if let Some(turn) = &app.latest_turn
                            && !turn.nodes.is_empty() {
                                let i = app.node_state.selected().unwrap_or(0);
                                app.node_state.select(Some((i + 1) % turn.nodes.len()));
                            }
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        if let Some(turn) = &app.latest_turn
                            && !turn.nodes.is_empty() {
                                let i = app.node_state.selected().unwrap_or(0);
                                let next = if i == 0 { turn.nodes.len() - 1 } else { i - 1 };
                                app.node_state.select(Some(next));
                            }
                    }
                    KeyCode::Char('p') => {
                        if let Some(turn) = &app.latest_turn
                            && let Some(idx) = app.node_state.selected()
                                && let Some(node) = turn.nodes.get(idx) {
                                    let op = MutationOp::PruneOrStub {
                                        target_fingerprint: Some(node.fingerprint.0.clone()),
                                        target_call_id: None,
                                        target_json_pointer: Some(node.raw_json_pointer.clone()),
                                        stub_text: format!(
                                            "[cw:pruned original_tokens={} label=\"{}\"]",
                                            node.token_span.count,
                                            node.kind.short_label()
                                        ),
                                    };
                                    let label = node.kind.short_label();
                                    app.apply_op(op, &format!("Pruned '{label}'"));
                                }
                    }
                    KeyCode::Char('d') => {
                        app.apply_op(
                            MutationOp::ShadowDedupe { file_path: None },
                            "Shadow-Deduplicated all stale & duplicate file reads",
                        );
                    }
                    KeyCode::Char('f') => {
                        if let Some(turn) = &app.latest_turn
                            && let Some(idx) = app.node_state.selected()
                                && let Some(node) = turn.nodes.get(idx) {
                                    let t = node.turn_seq;
                                    app.apply_op(
                                        MutationOp::FoldSubtree {
                                            start_turn: t,
                                            end_turn: t,
                                            summary_text: format!("Folded turn {t} via TUI"),
                                        },
                                        &format!("Folded turn {t}"),
                                    );
                                }
                    }
                    KeyCode::Char(' ') => {
                        if let Some(turn) = &app.latest_turn
                            && let Some(idx) = app.node_state.selected()
                                && let Some(node) = turn.nodes.get(idx) {
                                    let label = node.kind.short_label();
                                    let snippet: String = node
                                        .kind
                                        .text_for_tokenization()
                                        .chars()
                                        .take(160)
                                        .collect();
                                    app.apply_op(
                                        MutationOp::PinContext {
                                            label: label.clone(),
                                            content: snippet,
                                            source_node_id: Some(node.id.clone()),
                                        },
                                        &format!("Pinned '{label}' to tail"),
                                    );
                                }
                    }
                    KeyCode::Char('o') => {
                        if let Some(turn) = app.latest_turn.clone() {
                            for f in &turn.lint_findings {
                                if let Some(op) = &f.suggested_mutation {
                                    app.apply_op(op.clone(), &f.title);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

fn draw_ui(f: &mut Frame, app: &mut TuiApp) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(3),
        ])
        .split(f.area());

    // 1. Top Header & KV Cache Gauge
    let header_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(chunks[0]);

    let sess_title = match app.sessions.get(app.session_idx) {
        Some(s) => format!(
            " Session: {} ({}) • Model: {} [Tab to switch] ",
            s.id,
            s.harness.as_str(),
            s.model
        ),
        None => " No active sessions recorded yet ".to_string(),
    };

    let (input_tok, hit_ratio, health) = match &app.latest_turn {
        Some(t) => (
            t.token_usage.input_tokens,
            t.kv_cache_report.hit_ratio,
            t.health_score,
        ),
        None => (0, 0.0, 100),
    };

    let top_info = Paragraph::new(Line::from(vec![
        Span::styled(
            format!(" Tokens: {input_tok} "),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
        Span::raw("│"),
        Span::styled(
            format!(" Health Score: {health}/100 "),
            Style::default().fg(if health >= 80 {
                Color::Green
            } else if health >= 55 {
                Color::Yellow
            } else {
                Color::Red
            }),
        ),
        Span::raw("│"),
        Span::styled(
            format!(" Active Overlays: {} ", app.mutations.len()),
            Style::default().fg(Color::Magenta),
        ),
    ]))
    .block(Block::default().borders(Borders::ALL).title(sess_title));
    f.render_widget(top_info, header_chunks[0]);

    let gauge = Gauge::default()
        .block(Block::default().borders(Borders::ALL).title(" KV Cache Hit Ratio "))
        .gauge_style(Style::default().fg(Color::Green))
        .percent(((hit_ratio * 100.0) as u16).min(100));
    f.render_widget(gauge, header_chunks[1]);

    // 2. Main Split Pane: Left = Context Nodes Tree, Right = Inspector & Lints
    let body_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(48), Constraint::Percentage(52)])
        .split(chunks[1]);

    let mut items = Vec::new();
    if let Some(turn) = &app.latest_turn {
        for n in &turn.nodes {
            let (badge, color) = match n.kv_status {
                KvNodeStatus::CachedHit => ("🟢 HIT ", Color::Green),
                KvNodeStatus::NewlyCached => ("🟡 NEW ", Color::Yellow),
                KvNodeStatus::CacheInvalidated => ("🔴 BRK ", Color::Red),
                KvNodeStatus::Uncached => ("⚪ TAIL", Color::Gray),
            };
            let line = Line::from(vec![
                Span::styled(badge, Style::default().fg(color)),
                Span::raw(format!(" [T{}] {:<32} ", n.turn_seq, n.kind.short_label())),
                Span::styled(
                    format!("{:>5} tok", n.token_span.count),
                    Style::default().fg(Color::Cyan),
                ),
            ]);
            items.push(ListItem::new(line));
        }
    }

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" [1] Context Radix Tree & Nodes "),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");
    f.render_stateful_widget(list, body_chunks[0], &mut app.node_state);

    // Right Pane: Top = Node Inspector, Bottom = Lints & Category Breakdown
    let right_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(body_chunks[1]);

    let inspector_text = if let Some(turn) = &app.latest_turn {
        if let Some(sel) = app.node_state.selected().and_then(|i| turn.nodes.get(i)) {
            format!(
                "Node ID: {}  |  Fingerprint: {}\nJSON Pointer: {}\nRole: {:?}  |  Tokens: {} ({}..{})  |  Attention Salience: {:.0}%\nKV Status: {:?}\n────────────────────────────────────────────────────────\n{}",
                sel.id,
                sel.fingerprint.0,
                sel.raw_json_pointer,
                sel.role,
                sel.token_span.count,
                sel.token_span.start,
                sel.token_span.end,
                sel.attention_salience * 100.0,
                sel.kv_status,
                sel.kind.text_for_tokenization()
            )
        } else {
            "Select a node on the left to inspect its payload.".to_string()
        }
    } else {
        "No turn loaded.".to_string()
    };

    let inspector = Paragraph::new(inspector_text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" [2] Node Inspector & Payload "),
        )
        .wrap(Wrap { trim: false });
    f.render_widget(inspector, right_chunks[0]);

    let mut lint_lines = Vec::new();
    if let Some(turn) = &app.latest_turn {
        if turn.lint_findings.is_empty() {
            lint_lines.push(Line::from(Span::styled(
                "✓ No context rot or cache-break issues detected.",
                Style::default().fg(Color::Green),
            )));
        } else {
            for fnd in &turn.lint_findings {
                lint_lines.push(Line::from(vec![
                    Span::styled(
                        format!("⚠ {:?}: ", fnd.rule_id),
                        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(format!("{} (wasted: {} tok)", fnd.title, fnd.wasted_tokens)),
                ]));
            }
        }
    }

    let lints_box = Paragraph::new(lint_lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" [3] Context Health Linter Findings "),
        )
        .wrap(Wrap { trim: false });
    f.render_widget(lints_box, right_chunks[1]);

    // 3. Bottom Status Bar
    let footer = Paragraph::new(format!(" {}", app.status_msg)).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Shortcuts & Actions "),
    );
    f.render_widget(footer, chunks[2]);
}
