# Spec 02: Surgical Mutation Engine, Deterministic Overlays, Firewall & Replay

**Status:** Approved  
**Related Tickets:** `CW-024` – `CW-033`

---

## 1. The Stateless Harness Problem & The Overlay Solution

When `contextWatcher` sits as a proxy between an LLM harness (e.g., Antigravity, Codex, Pi, Claude Code) and the model provider:
* If `contextWatcher` mutates Turn 5 on Request 10, **the harness itself does not know** that Turn 5 was modified in transit (unless synced to disk and reloaded).
* On Request 11, the harness will re-send its own in-memory copy of Turns 1–10 (including the *unmutated* Turn 5) plus Turn 11!

### 1.1 The Deterministic Mutation Ledger (`cw-core::mutation`)
To make mutations completely seamless and persistent across every subsequent turn without requiring harness restarts:

1. **Structural & Content Fingerprinting (`NodeFingerprint`):**
   Every `ContextNode` is assigned a deterministic `BLAKE3` fingerprint computed from:
   $$\text{Fingerprint} = \text{BLAKE3}(\text{Role} \parallel \text{NodeKindTag} \parallel \text{ToolCallId\_or\_Ordinal} \parallel \text{NormalizedContentHash})$$

2. **Overlay Matching & Re-Application:**
   When Request $N$ arrives at `cw-proxy`, the **Overlay Engine** matches active `MutationRule` entries against the incoming `ContextTurn` by `NodeFingerprint` (or `TurnRange` / `ToolName` selector) and deterministically applies the transformation **in $<1\text{ms}$** before forwarding the payload upstream.
   * Because the transformation is 100% deterministic, Request 10 and Request 11 produce the **exact same mutated prefix** for Turns 1–10—meaning **the KV Cache remains 100% valid across turns after a mutation is applied!**

---

## 2. Catalog of Surgical Context Mutations (`MutationOp`)

### 2.1 `PruneOrStub` (Single Node Compaction)
* **Target:** Any `ToolResult`, `AssistantThinking`, or `UserMessage` matched by `NodeFingerprint` or `call_id`.
* **Behavior:**
  * Replaces the heavy text content with a concise stub:
    `[cw:pruned original_tokens=14280 reason="Verbose test log" summary="cargo test failed in auth::tests::login_expired"]`
  * **Protocol Safety Invariant:** Never deletes a `tool_result` block outright if its corresponding `tool_use` block exists (which would cause an HTTP 400 error from Anthropic/OpenAI). Always replaces the inner content string while preserving `tool_use_id` / `call_id`.

### 2.2 `FoldSubtree` (Multi-Turn Trajectory Compression)
* **Target:** A contiguous span of turns $[T_{\text{start}} \dots T_{\text{end}}]$ (e.g., a 6-turn failed debugging rabbit hole).
* **Behavior:**
  * Collapses all intermediate `Assistant` tool calls and `ToolResult` messages in $[T_{\text{start}} \dots T_{\text{end}}]$ into a single valid synthetic exchange (or summary block inside the first turn's result):
    ```text
    [cw:folded_subtree turns=6..11 saved_tokens=22400]
    Summary of folded trajectory:
    - Attempted X and Y; failed due to missing DB migration.
    - Applied migration 004_users.sql and verified schema.
    ```
  * **Protocol Safety Invariant:** Ensures strict `user` $\leftrightarrow$ `assistant` role alternation and valid `tool_use` / `tool_result` pairing required by Anthropic and OpenAI APIs.
  * **Summarization Modes:**
    1. *Deterministic Heuristic Extract:* Extracts tool names, file paths touched, exit codes, and final assistant text (0 latency, 0 LLM cost).
    2. *Fast-Model Assisted:* Summarizes via a local/fast model or user-edited text.

### 2.3 `ShadowDedupe` (Stale & Duplicate File Read Eviction)
* **Target:** Session-wide automatic policy or targeted file path (`path: "src/engine.rs"`).
* **Behavior:**
  * Scans all `ToolResult(FileRead { path, line_range, .. })` nodes in the turn history.
  * If `path` is read at Turn $k_1$ and read again (or overwritten) at Turn $k_2 > k_1$, replaces the content at $k_1$ with:
    `[cw:shadow_deduped path="src/engine.rs" superseded_by_turn=14 saved_tokens=3120]`
  * Eliminates conflicting/stale code versions from the model's attention window.

### 2.4 `GateToolsAndSkills` (Dynamic Schema & Rule Filtering)
* **Target:** List of disabled MCP server prefixes, tool names, or `<skills>` / `<user_rules>` blocks.
* **Behavior:**
  * Strips disabled tool definitions from the top-level `tools` array in the JSON payload.
  * Strips or stubs corresponding `<mcp_servers>` or `<skills>` catalog entries inside the system prompt.
  * **Prefix Order Optimization:** Sorts remaining `tools` deterministically so tool ordering never causes accidental KV cache invalidation.

### 2.5 `PinContext` (Cache-Safe Attention Boosting)
* **Target:** Any `ContextNode`, code snippet, or user-authored architectural constraint.
* **Behavior:**
  * Instead of injecting pinned items into the `system` prompt (which would **invalidate the entire KV cache prefix**!), `PinContext` appends a lightweight `<pinned_context>` reminder block to the **final message in the tail of the prompt** (the highest-attention recency zone):
    ```xml
    <cw_pinned_context>
    - [Pinned from Turn 3]: Always use parameterized SQL queries in cw-store; never interpolate strings.
    - [Pinned File]: crates/cw-core/src/ir.rs#L12-L48
    </cw_pinned_context>
    ```
  * **Result:** Maximum model attention + **0% KV Cache invalidation**!

### 2.6 `NormalizePrefix` (Auto-Cache Stabilization)
* **Target:** System prompts or headers that inject per-turn volatile strings (e.g., `<ADDITIONAL_METADATA>The current local time is: 2026-10-21T20:32:57...</ADDITIONAL_METADATA>` when placed early in the prompt).
* **Behavior:**
  * Freezes or relocates volatile timestamps/metadata to the uncached tail so the main prefix stays 100% cache-hit across turns.

---

## 3. Cache-Safe Staging ("Draft vs. Commit" Mode)

Because mutating an early turn (e.g., folding Turns 3–6 when you are on Turn 20) changes the token prefix at Turn 3 and forces the LLM provider to recompute the KV cache for Turns 7–20 on the very next request, `contextWatcher` supports **two execution modes** for prefix-mutating operations:

1. **Immediate Commit:** Applies right away on the next request (shows exact KV rebuild cost in tokens/dollars before confirming).
2. **Staged Draft (Cache-Safe Queue):**
   * Queues prefix mutations (`FoldSubtree`, `PruneOrStub`, `ShadowDedupe`, `GateTools`) as **Staged Drafts**.
   * Automatically flushes all staged drafts together when:
     * The user clicks **"Commit Batch (Rebuild KV Once)"**, OR
     * The natural KV cache TTL (e.g., 5 minutes idle) expires anyway, OR
     * Context fullness crosses a configurable threshold (e.g., $75\%$ of context window).

---

## 4. Context Firewall (Interactive Breakpoints)

The **Context Firewall** lets users define declarative rules evaluated on every request *before* it leaves `cw-proxy`:

```rust
pub struct FirewallRule {
    pub id: RuleId,
    pub name: String,
    pub trigger: FirewallTrigger,
    pub action: FirewallAction,
}

pub enum FirewallTrigger {
    SingleToolResultTokensExceed(u32), // e.g., > 4,000 tokens
    TotalTurnDeltaTokensExceed(u32),   // e.g., > 12,000 new tokens in one turn
    UnexpectedKvCacheBreak { min_lost_tokens: u32 },
    ToolResultMatchesRegex(String),    // e.g., secret leak or massive stack trace
    ManualStepMode,                    // Pause every turn like a debugger breakpoint
}

pub enum FirewallAction {
    PauseForHumanApproval { timeout_secs: u32 },
    AutoTruncateHeadTail { head_tokens: u32, tail_tokens: u32 },
    AutoShadowDedupe,
}
```

---

## 5. Time-Travel Forking & A/B Replay Engine

From any historical turn $T_k$ in SQLite:
1. User clicks **"Fork & Replay"** in Web UI or runs `cw replay --turn 12`.
2. User modifies the context graph on the fork (e.g., folds Turns 4–9, disables 20 MCP tools, or edits the system prompt).
3. `cw-proxy` dispatches the forked request directly to the upstream provider API.
4. Both the original Turn $T_k$ response and the Forked Turn $T'_k$ response are stored and displayed side-by-side (comparing tool calls chosen, reasoning quality, TTFT latency, and token/cache cost).
