# Spec 00: System Architecture, Protocol Normalization & Daemon API

**Status:** Approved  
**Related Tickets:** `CW-001` – `CW-007`, `CW-011`

---

## 1. Architectural Overview

`contextWatcher` (`cw`) runs as a single local Rust daemon (`cw serve`) that exposes two ports (configurable, or multiplexed):
* **Proxy Port (`127.0.0.1:4401`):** Intercepts HTTP/SSE traffic between any LLM harness and upstream model providers (Anthropic, OpenAI, Google Gemini, OpenRouter, local vLLM/SGLang/Ollama).
* **Control & UI Port (`127.0.0.1:4400`):** Serves the embedded Svelte 5 Web UI, REST API (`/api/v1/*`), and real-time WebSocket stream (`/api/v1/ws`) consumed by the Web UI, Ratatui TUI (`cw tui`), CLI (`cw *`), MCP server (`cw mcp`), and IntelliJ Plugin (`cw-intellij`).

In parallel, the daemon runs background **Transcript Watchers (`cw-harnesses`)** that monitor local harness session files (such as Antigravity's `transcript.jsonl`, Codex rollouts, Pi/OpenCode history, and Claude Code `.jsonl` files) so `contextWatcher` works even when a harness is not routed through the HTTP proxy—or combines both sources when both are active.

---

## 2. Canonical Context IR (`cw-core::ir`)

Every provider uses a slightly different wire format (Anthropic `system` + `messages` + `tools`, OpenAI `messages` or `input` items, Gemini `systemInstruction` + `contents` + `tools`).

To analyze, visualize, and mutate context uniformly without losing provider-specific fields, `cw-core` parses every request/transcript into a **Canonical Context Turn (`ContextTurn`)** paired with a **Lossless JSON Pointer Map**.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: SessionId,
    pub harness: HarnessKind, // Antigravity, Codex, Pi, OpenCode, Aider, ClaudeCode, GenericProxy
    pub workspace_root: Option<PathBuf>,
    pub model: String,
    pub provider: ProviderKind, // Anthropic, OpenAI, Gemini, OpenAICompatible
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextTurn {
    pub id: TurnId,
    pub session_id: SessionId,
    pub parent_turn_id: Option<TurnId>, // Supports branching/forking
    pub sequence_number: u32,
    pub nodes: Vec<ContextNode>,        // Ordered sequence of context blocks in the window
    pub response_nodes: Vec<ContextNode>, // Model output for this turn
    pub token_usage: TokenUsageSummary,
    pub kv_cache_report: KvCacheReport,
    pub lint_findings: Vec<LintFinding>,
}
```

### 2.1 `ContextNode` & `NodeKind`

Every distinct semantic unit in the prompt is represented as a `ContextNode`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextNode {
    pub id: NodeId,
    pub fingerprint: NodeFingerprint, // BLAKE3 structural + content hash
    pub raw_json_pointer: String,     // RFC 6901 JSON Pointer into raw request payload (e.g., "/messages/4/content/1")
    pub role: Role,                   // System, Developer, User, Assistant, Tool
    pub kind: NodeKind,
    pub token_span: TokenSpan,        // Start token index, end token index, count
    pub kv_status: KvNodeStatus,      // CachedHit, NewlyCached, CacheInvalidated, Uncached
    pub cache_control: Option<CacheControlBreakpoint>,
    pub active_mutation: Option<AppliedMutation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NodeKind {
    SystemPrompt { text: String },
    InjectedRule { source_file: Option<String>, content: String }, // CLAUDE.md, AGENTS.md, user_rules
    SkillCatalog { skills: Vec<SkillEntry> },
    McpToolSchema { server_name: Option<String>, tool_name: String, schema_json: Value },
    UserMessage { text: String, attachments: Vec<MediaRef> },
    AssistantThinking { thought: String, signature: Option<String> },
    AssistantText { text: String },
    ToolCall { call_id: String, tool_name: String, arguments: Value },
    ToolResult {
        call_id: String,
        tool_name: String,
        is_error: bool,
        content: String,
        semantic_meta: Option<ToolSemanticMeta>, // e.g., FileRead { path, line_range, hash } or BashExec { cmd, exit_code }
    },
    PinnedReminder { source_node_id: NodeId, summary: String },
    FoldedSubtree { original_turn_range: (u32, u32), summary: String, saved_tokens: u32 },
}
```

### 2.2 Semantic Extraction (`SystemPrompt` Deconstruction)
Harnesses typically pack multiple distinct sections into the system prompt (e.g., `<identity>`, `<user_rules>`, `<skills>`, `<mcp_servers>`, `<user_information>`).
`cw-core` includes a **System Block Deconstructor** that splits XML-tagged or Markdown-headed sections inside `system` into child `ContextNode` items (`InjectedRule`, `SkillCatalog`, etc.) so the user can see and toggle each rule/skill individually!

---

## 3. Intercepting Proxy Pipeline (`cw-proxy`)

### 3.1 Routing & Upstream Forwarding
The proxy exposes route prefixes that map directly to upstream providers (or configurable custom upstreams):
* `POST /anthropic/v1/messages` $\to$ `https://api.anthropic.com/v1/messages`
* `POST /openai/v1/chat/completions` & `/openai/v1/responses` $\to$ `https://api.openai.com/v1/*`
* `POST /gemini/v1beta/*` $\to$ `https://generativelanguage.googleapis.com/v1beta/*`

### 3.2 Ingress & Egress Lifecycle
1. **Ingress Capture:** Receive raw JSON body + headers.
2. **Session Resolution:** Match request to an existing `Session` via either:
   * Explicit `X-CW-Session-Id` header injected by `cw wrap`, or
   * Prefix-matching the first $k$ message fingerprints against active sessions.
3. **IR Parsing & Linting:** Parse into `ContextTurn`, compute `NodeFingerprint`s, run KV Cache Prefix Diff against $T_{n-1}$, and run the Context Linter.
4. **Mutation Overlay Application:** Look up active mutations for this session in the **Mutation Ledger** and transform the targeted JSON pointers in a cloned payload.
5. **Context Firewall Check:** Evaluate active breakpoint rules. If triggered, emit `FirewallPaused` over WebSocket and await user resolution (`Approve`, `EditAndApprove`, `Abort`).
6. **Upstream Streaming & Tap:** Forward mutated payload to upstream provider; stream SSE chunks directly back to the harness while assembling the assistant response and `usage` telemetry in the background.
7. **Persistence & Broadcast:** Commit turn and actual provider `cache_read_input_tokens` / `cached_tokens` to SQLite and broadcast `TurnCompleted` over WebSocket.

---

## 4. SQLite Storage Schema (`cw-store`)

Stored at `~/.local/share/context-watcher/cw.db` (or workspace-local `.cw/cw.db`):

* `sessions (id, harness, workspace_root, model, provider, created_at, updated_at)`
* `turns (id, session_id, parent_turn_id, seq_num, raw_request_blob_id, mutated_request_blob_id, raw_response_blob_id, input_tokens, output_tokens, cached_tokens, cache_write_tokens, latency_ms, created_at)`
* `blobs (hash PRIMARY KEY, zstd_compressed_json BLOB)` — Content-addressed storage so repeated history across turns is deduplicated on disk!
* `nodes (id, turn_id, seq_idx, fingerprint, raw_pointer, role, kind_tag, kind_json, start_token, end_token, kv_status)`
* `mutations (id, session_id, target_selector_json, op_kind, op_payload_json, enabled, created_at)`
* `firewall_rules (id, session_id, name, condition_expr, action, enabled)`

---

## 5. Daemon REST & WebSocket API

### REST Endpoints (`http://127.0.0.1:4400/api/v1`)
* `GET /sessions` — List active and historical sessions.
* `GET /sessions/:id/graph` — Full Radix Tree / Context DAG for a session.
* `GET /sessions/:id/turns/:turn_id` — Detailed nodes, KV cache report, and lint findings for a turn.
* `POST /sessions/:id/mutations` — Create/toggle a mutation (`Prune`, `Fold`, `ShadowDedupe`, `GateTool`, `Pin`).
* `DELETE /sessions/:id/mutations/:mutation_id` — Revert a mutation.
* `POST /sessions/:id/replay` — Fork from `turn_id` and execute an A/B replay.
* `POST /firewall/:intercept_id/resolve` — Resume or abort a paused proxy request.

### WebSocket Events (`ws://127.0.0.1:4400/api/v1/ws`)
* `SessionUpdated { session }`
* `TurnStarted { session_id, turn_id, preflight_graph, kv_prediction, lint_findings }`
* `TurnStreamDelta { session_id, turn_id, delta }`
* `TurnCompleted { session_id, turn_id, actual_usage, kv_reconciliation }`
* `FirewallIntercepted { intercept_id, session_id, turn_id, triggering_rule, held_payload }`
* `FileContextStatusChanged { workspace_root, file_states: Vec<FileContextState> }` *(Used by IntelliJ plugin)*
