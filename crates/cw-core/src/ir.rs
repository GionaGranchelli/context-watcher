use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub type SessionId = String;
pub type TurnId = String;
pub type NodeId = String;
pub type MutationId = String;
pub type RuleId = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessKind {
    Antigravity,
    Codex,
    Hermes,
    Pi,
    OpenCode,
    Aider,
    ClaudeCode,
    GenericProxy,
}

impl HarnessKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Antigravity => "antigravity",
            Self::Codex => "codex",
            Self::Hermes => "hermes",
            Self::Pi => "pi",
            Self::OpenCode => "opencode",
            Self::Aider => "aider",
            Self::ClaudeCode => "claude_code",
            Self::GenericProxy => "generic_proxy",
        }
    }

    pub fn from_str_lossy(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "antigravity" | "agy" => Self::Antigravity,
            "codex" => Self::Codex,
            "hermes" | "hermes_agent" | "hermes-agent" => Self::Hermes,
            "pi" => Self::Pi,
            "opencode" => Self::OpenCode,
            "aider" => Self::Aider,
            "claude_code" | "claude" => Self::ClaudeCode,
            _ => Self::GenericProxy,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Anthropic,
    OpenAI,
    Gemini,
    OpenAICompatible,
}

impl ProviderKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAI => "openai",
            Self::Gemini => "gemini",
            Self::OpenAICompatible => "openai_compatible",
        }
    }

    pub fn from_str_lossy(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "anthropic" => Self::Anthropic,
            "openai" => Self::OpenAI,
            "gemini" => Self::Gemini,
            _ => Self::OpenAICompatible,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: SessionId,
    pub parent_session_id: Option<SessionId>,
    pub harness: HarnessKind,
    pub workspace_root: Option<String>,
    pub model: String,
    pub provider: ProviderKind,
    pub title: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    Developer,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeFingerprint(pub String);

impl NodeFingerprint {
    pub fn compute(role: Role, kind_tag: &str, key_or_ordinal: &str, content: &str) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(format!("{role:?}|{kind_tag}|{key_or_ordinal}|").as_bytes());
        hasher.update(content.trim().as_bytes());
        Self(hasher.finalize().to_hex()[..24].to_string())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenSpan {
    pub start: u32,
    pub end: u32,
    pub count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KvNodeStatus {
    CachedHit,
    NewlyCached,
    CacheInvalidated,
    Uncached,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheControlBreakpoint {
    pub cache_type: String,
    pub ttl: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillEntry {
    pub name: String,
    pub path: Option<String>,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "semantic_type", rename_all = "snake_case")]
pub enum ToolSemanticMeta {
    FileRead {
        path: String,
        start_line: Option<u32>,
        end_line: Option<u32>,
        content_hash: String,
    },
    FileWrite {
        path: String,
        content_hash: String,
    },
    CommandExec {
        command: String,
        exit_code: Option<i32>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NodeKind {
    SystemPrompt {
        text: String,
    },
    InjectedRule {
        source_file: Option<String>,
        content: String,
    },
    SkillCatalog {
        skills: Vec<SkillEntry>,
        raw_text: String,
    },
    McpToolSchema {
        server_name: Option<String>,
        tool_name: String,
        description: String,
        schema_json: Value,
    },
    UserMessage {
        text: String,
    },
    AssistantThinking {
        thought: String,
        signature: Option<String>,
    },
    AssistantText {
        text: String,
    },
    ToolCall {
        call_id: String,
        tool_name: String,
        arguments: Value,
        semantic_meta: Option<ToolSemanticMeta>,
    },
    ToolResult {
        call_id: String,
        tool_name: String,
        is_error: bool,
        content: String,
        semantic_meta: Option<ToolSemanticMeta>,
    },
    PinnedReminder {
        source_node_id: Option<NodeId>,
        label: String,
        summary: String,
    },
    FoldedSubtree {
        original_turn_range: (u32, u32),
        summary: String,
        saved_tokens: u32,
    },
}

impl NodeKind {
    pub fn kind_tag(&self) -> &'static str {
        match self {
            Self::SystemPrompt { .. } => "system_prompt",
            Self::InjectedRule { .. } => "injected_rule",
            Self::SkillCatalog { .. } => "skill_catalog",
            Self::McpToolSchema { .. } => "mcp_tool_schema",
            Self::UserMessage { .. } => "user_message",
            Self::AssistantThinking { .. } => "assistant_thinking",
            Self::AssistantText { .. } => "assistant_text",
            Self::ToolCall { .. } => "tool_call",
            Self::ToolResult { .. } => "tool_result",
            Self::PinnedReminder { .. } => "pinned_reminder",
            Self::FoldedSubtree { .. } => "folded_subtree",
        }
    }

    pub fn text_for_tokenization(&self) -> String {
        match self {
            Self::SystemPrompt { text } => text.clone(),
            Self::InjectedRule { content, .. } => content.clone(),
            Self::SkillCatalog { raw_text, .. } => raw_text.clone(),
            Self::McpToolSchema {
                tool_name,
                description,
                schema_json,
                ..
            } => format!("{tool_name}: {description} {schema_json}"),
            Self::UserMessage { text } => text.clone(),
            Self::AssistantThinking { thought, .. } => thought.clone(),
            Self::AssistantText { text } => text.clone(),
            Self::ToolCall {
                tool_name,
                arguments,
                ..
            } => format!("{tool_name}({arguments})"),
            Self::ToolResult { content, .. } => content.clone(),
            Self::PinnedReminder { label, summary, .. } => format!("[{label}] {summary}"),
            Self::FoldedSubtree { summary, .. } => summary.clone(),
        }
    }

    pub fn short_label(&self) -> String {
        match self {
            Self::SystemPrompt { .. } => "System Prompt".to_string(),
            Self::InjectedRule { source_file, .. } => {
                format!("Rule: {}", source_file.as_deref().unwrap_or("inline"))
            }
            Self::SkillCatalog { skills, .. } => format!("Skills Catalog ({})", skills.len()),
            Self::McpToolSchema {
                server_name,
                tool_name,
                ..
            } => match server_name {
                Some(srv) => format!("Tool: {srv}/{tool_name}"),
                None => format!("Tool: {tool_name}"),
            },
            Self::UserMessage { text } => {
                let preview: String = text.chars().take(48).collect();
                format!("User: {}", preview.replace('\n', " "))
            }
            Self::AssistantThinking { .. } => "Assistant Thinking".to_string(),
            Self::AssistantText { text } => {
                let preview: String = text.chars().take(48).collect();
                format!("Assistant: {}", preview.replace('\n', " "))
            }
            Self::ToolCall { tool_name, .. } => format!("Call: {tool_name}"),
            Self::ToolResult {
                tool_name,
                semantic_meta,
                ..
            } => match semantic_meta {
                Some(ToolSemanticMeta::FileRead { path, .. }) => format!("Read: {path}"),
                Some(ToolSemanticMeta::FileWrite { path, .. }) => format!("Write: {path}"),
                Some(ToolSemanticMeta::CommandExec { command, .. }) => {
                    let cmd: String = command.chars().take(36).collect();
                    format!("Exec: {cmd}")
                }
                None => format!("Result: {tool_name}"),
            },
            Self::PinnedReminder { label, .. } => format!("Pinned: {label}"),
            Self::FoldedSubtree {
                original_turn_range,
                ..
            } => format!(
                "Folded Turns {}..{}",
                original_turn_range.0, original_turn_range.1
            ),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppliedMutation {
    pub mutation_id: MutationId,
    pub op_name: String,
    pub saved_tokens: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextNode {
    pub id: NodeId,
    pub turn_seq: u32,
    pub fingerprint: NodeFingerprint,
    pub raw_json_pointer: String,
    pub role: Role,
    pub kind: NodeKind,
    pub token_span: TokenSpan,
    pub kv_status: KvNodeStatus,
    pub attention_salience: f32,
    pub cache_control: Option<CacheControlBreakpoint>,
    pub active_mutation: Option<AppliedMutation>,
    pub timestamp: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsageSummary {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cached_tokens: u32,
    pub cache_write_tokens: u32,
    pub context_limit: u32,
    pub saved_by_mutations: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cause_type", rename_all = "snake_case")]
pub enum CacheInvalidationCause {
    DynamicSystemInjection {
        node_id: NodeId,
        divergence_token: u32,
        detail: String,
    },
    ToolSchemaMutated {
        node_id: NodeId,
        divergence_token: u32,
        detail: String,
    },
    HistoryEditedOrCompacted {
        node_id: NodeId,
        divergence_token: u32,
        detail: String,
    },
    CacheTtlExpired {
        previous_tokens: u32,
    },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KvCacheReport {
    pub hit_tokens: u32,
    pub newly_cached_tokens: u32,
    pub invalidated_tokens: u32,
    pub uncached_tail_tokens: u32,
    pub hit_ratio: f32,
    pub divergence_token_index: Option<u32>,
    pub invalidation_cause: Option<CacheInvalidationCause>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LintRuleId {
    #[serde(rename = "CW-L001")]
    StaleFileInContext,
    #[serde(rename = "CW-L002")]
    DuplicateFileRead,
    #[serde(rename = "CW-L003")]
    MassiveToolOutputBloat,
    #[serde(rename = "CW-L004")]
    FailedDebuggingLoop,
    #[serde(rename = "CW-L005")]
    McpToolSchemaBloat,
    #[serde(rename = "CW-L006")]
    UnintendedKvCacheBreak,
    #[serde(rename = "CW-L007")]
    LostInTheMiddleRisk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LintSeverity {
    Info,
    Warning,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LintFinding {
    pub rule_id: LintRuleId,
    pub severity: LintSeverity,
    pub title: String,
    pub message: String,
    pub affected_node_ids: Vec<NodeId>,
    pub wasted_tokens: u32,
    pub suggested_mutation: Option<MutationOp>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CategoryBreakdown {
    pub system_and_identity: u32,
    pub rules_and_instructions: u32,
    pub skills_catalog: u32,
    pub mcp_and_native_tools: u32,
    pub user_prompts: u32,
    pub file_reads_fresh: u32,
    pub file_reads_stale_or_duplicate: u32,
    pub command_outputs: u32,
    pub assistant_thinking: u32,
    pub assistant_responses_and_calls: u32,
    pub pinned_and_folded: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextTurn {
    pub id: TurnId,
    pub session_id: SessionId,
    pub parent_turn_id: Option<TurnId>,
    pub sequence_number: u32,
    pub nodes: Vec<ContextNode>,
    pub response_nodes: Vec<ContextNode>,
    pub token_usage: TokenUsageSummary,
    pub kv_cache_report: KvCacheReport,
    pub category_breakdown: CategoryBreakdown,
    pub lint_findings: Vec<LintFinding>,
    pub health_score: u8,
    pub latency_ms: Option<u64>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum MutationOp {
    PruneOrStub {
        target_fingerprint: Option<String>,
        target_call_id: Option<String>,
        target_json_pointer: Option<String>,
        stub_text: String,
    },
    FoldSubtree {
        start_turn: u32,
        end_turn: u32,
        summary_text: String,
    },
    ShadowDedupe {
        file_path: Option<String>, // None = deduplicate all stale/duplicate file reads
    },
    GateToolsAndSkills {
        disabled_tools: Vec<String>,
        disabled_mcp_servers: Vec<String>,
        disabled_skills: Vec<String>,
    },
    PinContext {
        label: String,
        content: String,
        source_node_id: Option<NodeId>,
    },
    NormalizePrefix {
        sort_tools_deterministically: bool,
        relocate_dynamic_system_metadata: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationStatus {
    Draft,
    Committed,
    Disabled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MutationRule {
    pub id: MutationId,
    pub session_id: SessionId,
    pub op: MutationOp,
    pub status: MutationStatus,
    pub estimated_saved_tokens: i32,
    pub estimated_kv_break_tokens: u32,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "trigger", rename_all = "snake_case")]
pub enum FirewallTrigger {
    SingleToolResultTokensExceed { max_tokens: u32 },
    TotalTurnDeltaTokensExceed { max_tokens: u32 },
    UnexpectedKvCacheBreak { min_lost_tokens: u32 },
    ToolResultMatchesRegex { pattern: String },
    ManualStepMode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum FirewallAction {
    PauseForHumanApproval { timeout_secs: u32 },
    AutoTruncateHeadTail { head_chars: usize, tail_chars: usize },
    AutoShadowDedupe,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FirewallRule {
    pub id: RuleId,
    pub session_id: Option<SessionId>,
    pub name: String,
    pub trigger: FirewallTrigger,
    pub action: FirewallAction,
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileContextFreshness {
    Fresh,
    Stale,
    Duplicate,
    Pinned,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileContextState {
    pub path: String,
    pub freshness: FileContextFreshness,
    pub last_read_turn: u32,
    pub read_count: u32,
    pub total_tokens_in_context: u32,
    pub line_ranges: Vec<(u32, u32)>,
    pub node_ids: Vec<NodeId>,
}
