# Spec 01: Context Graph, Radix Prefix Tree, KV Cache X-Ray & Health Linter

**Status:** Approved  
**Related Tickets:** `CW-012` – `CW-016`

---

## 1. Context Radix Prefix Tree (`cw-core::radix`)

LLM inference engines (Anthropic, OpenAI, Gemini, vLLM, SGLang) store key-value (KV) attention tensors in a **Radix Tree (Prefix Tree)** keyed by exact token sequences (typically in blocks of 16–128 tokens).

Even though cloud APIs only return aggregate numbers (`cache_read_input_tokens`, `cache_creation_input_tokens`, `cached_tokens`), `contextWatcher` has the **entire sequence of requests** $T_1, T_2, \dots, T_n$. By combining **Exact Token-Prefix Diffing** across turns with the provider's returned cache token count, we reconstruct the exact node-by-node and token-by-token KV Cache state!

### 1.1 Serialization Order & Token Span Calculation
Before tokenizing a turn $T_n$, `cw-core` orders nodes in the **exact canonical prefix order** used by the target model provider:
* **Anthropic:** `tools` $\to$ `system` $\to$ `messages[0..k]`
* **OpenAI:** `system/developer` $\to$ `tools` $\to$ `messages/input[0..k]`
* **Gemini:** `systemInstruction` $\to$ `tools` $\to$ `contents[0..k]`

*(Note: Knowing that Anthropic places `tools` before `system` and `messages` is critical—toggling an MCP tool invalidates the KV cache for `system` and `messages`, whereas mutating turn 10 leaves `tools`, `system`, and turns 1–9 cached!)*

---

## 2. KV Cache Boundary Reconstruction Algorithm

Given Turn $T_{n-1}$ with ordered node token sequence $S_{n-1}$ and Turn $T_n$ with sequence $S_n$:

1. **Longest Common Prefix (LCP):**
   Compute the exact divergence index $d$:
   $$d = \max \{ i \mid S_{n-1}[0..i] == S_n[0..i] \}$$

2. **Reconcile with Provider Telemetry (`actual_cached_tokens`):**
   * Once the response arrives, compare $d$ with `usage.cached_tokens` (or `cache_read_input_tokens`).
   * Define effective cache boundary $B = \min(d, \text{actual\_cached\_tokens})$ (accounting for provider block-size rounding, e.g., 128-token blocks or TTL expiration).

3. **Classify Every `ContextNode` in $T_n$:**
   * 🟢 **`CachedHit`:** `node.token_span.end <= B`. This node was served directly from KV cache.
   * 🔴 **`CacheInvalidated`:** `node.token_span.start >= d` AND `node.token_span.end <= S_{n-1}.len()` when $d < S_{n-1}.\text{len}()$.
     * This means this node *was* in the context on $T_{n-1}$, **should have been cached**, but was evicted because an earlier node at index $d$ mutated!
   * 🟡 **`NewlyCached`:** New nodes appended at the tail of $T_n$ (`node.token_span.start >= S_{n-1}.len()`) that were written into the KV cache this turn.
   * ⚪ **`Uncached`:** Tail tokens after the last `cache_control` breakpoint (Anthropic) or below the minimum cacheable threshold (1024 / 2048 tokens).

4. **Root-Cause Attribution for Cache Breaks (`CacheInvalidationCause`):**
   Whenever $d < S_{n-1}.\text{len}()$, `cw-core` inspects the node at token $d$ and emits a structured diagnosis:
   * `DynamicSystemInjection` — e.g., a timestamp or git status injected into the system prompt changed.
   * `ToolSchemaMutated` — MCP tools were added, removed, or reordered without deterministic sorting.
   * `HistoryEditedOrCompacted` — A past message in turns $1 \dots n-1$ was modified or deleted.
   * `CacheTtlExpired` — $d == S_{n-1}.\text{len}()$, but `actual_cached_tokens == 0` because $>5\text{ min}$ elapsed between turns.

---

## 3. Context Health & Linting Engine (`cw-core::linter`)

On every turn, `cw-core` runs a fast pass over `ContextTurn` and produces a **Context Health Score (`0–100`)** along with actionable `LintFinding` items that link directly to one-click fixes in the Mutation Engine.

### 3.1 Lint Rules Catalog

| Rule ID | Name | Detection Logic | One-Click Fix (`MutationOp`) |
| :--- | :--- | :--- | :--- |
| **`CW-L001`** | **Stale File in Context** | A `ToolResult(FileRead { path, line_range, hash })` at Turn $k$ is followed by a later `edit_file`/`write_file` on `path`, OR the file on disk (`workspace_root/path`) now has a different BLAKE3 hash. | `ShadowDedupe` |
| **`CW-L002`** | **Duplicate File Read** | The same file (and overlapping line range) is read in multiple turns ($k_1 < k_2$). | `ShadowDedupe` (keep $k_2$, stub $k_1$ with pointer) |
| **`CW-L003`** | **Massive Tool Output Bloat** | A single `ToolResult` exceeds `3,000` tokens (or $>10\%$ of total context). | `PruneOrStub` / `HeadTailTruncate` |
| **`CW-L004`** | **Failed Debugging Loop (Subtree Noise)** | $\ge 3$ consecutive turns contain tool errors (`is_error == true` or non-zero bash exit codes) followed by a successful fix. | `FoldSubtree` into a single outcome summary |
| **`CW-L005`** | **MCP Tool Schema Bloat** | Total `McpToolSchema` tokens $> 8,000$ or $>25$ unused tools across the last 10 turns. | `GateTools` (disable unused MCP servers/tools) |
| **`CW-L006`** | **Unintended KV Cache Break** | Prefix diverged at token $d < S_{n-1}.\text{len}()$ without an explicit user mutation (e.g., harness injected non-deterministic JSON key order or timestamp). | `NormalizePrefix` (strip/freeze dynamic header or sort tool schemas deterministically) |
| **`CW-L007`** | **Lost-in-the-Middle Attention Risk** | High-priority rules or user constraints sit in the middle $30\%–70\%$ of a $>40\text{k}$ token context window without being referenced recently. | `PinContext` (boost to tail reminder) |

---

## 4. Attention & Token Distribution Model

To help users understand *how* the model perceives the context window, `cw-core` computes two distributions per turn:

1. **Category Token Breakdown (`CategoryBreakdown`):**
   * `System & Identity`
   * `Rules & Instructions` (`AGENTS.md`, `CLAUDE.md`, user rules)
   * `Skills Catalog`
   * `MCP & Native Tool Schemas`
   * `User Prompts`
   * `File Reads (Fresh)` vs. `File Reads (Stale/Duplicate)`
   * `Bash / Command Outputs`
   * `Assistant Thinking / Reasoning`
   * `Assistant Responses & Tool Calls`

2. **Positional Attention Curve Estimate:**
   * Uses a U-shaped positional salience curve (primacy + recency bias model) weighted by token distance from the end of the prompt, highlighting which instructions or file reads are in the **Attention Dead Zone** (`salience < 0.35`).
