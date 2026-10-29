# How `contextWatcher` Works: Context Garbage Collection & Architecture

`contextWatcher` (`cw`) is built around a simple premise: **an LLM context window needs a deterministic garbage collector, not just an emergency summarizer.**

In a typical coding session, an agent reads the same files repeatedly, runs verbose test suites (`cargo test`, `npm test`, `git diff`), and generates thousands of tokens of intermediate `<thinking>` monologues. Over turns, this disposable transcript noise crowds out durable state (system instructions, accepted decisions, and fresh code) and inflates prefill latency and token cost.

---

## 1. Why `/compact` Is Not a Garbage Collector

Most coding harnesses include a `/compact` command that invokes an LLM to summarize earlier turns into prose. That is useful as a last-resort emergency parachute when a context window is 95% full, but it has four structural drawbacks for active coding:

| Dimension | Standard `/compact` (LLM Summarization) | `contextWatcher` (`cw` Garbage Collection) |
| :--- | :--- | :--- |
| **Mechanism** | Calls an LLM to rewrite history into an English summary. | Deterministic AST/IR pass in native Rust (`<2ms`, zero LLM calls). |
| **Code Fidelity** | **Lossy:** Summarizes away exact line numbers, type signatures, and diffs. | **Targeted:** Keeps the newest covered file reads, user prompts, and final assistant answers verbatim; replaces only redundant copies and old logs. |
| **KV-Cache Impact** | **Rewrites Prefix:** Summarizing turns `1..N` changes the token prefix from turn 1 onward. | **KV-Cache Aware:** Calculates `kv_break_tokens` per candidate; `min` restricts edits to `kv_break_tokens == 0` so cached prefixes stay intact. |
| **Cost & Latency** | Burns model input/output tokens and takes seconds to generate. | Runs locally on CPU with `$0` token cost. |

---

## 2. Separating Disposable Transcript from Durable State

`contextWatcher` parses every supported request or imported session transcript into a typed **Canonical Context Graph (`ContextTurn` / `ContextNode`)** while retaining a lossless JSON pointer map to the raw wire payload.

On each turn, the linter and optimizer classify nodes into **Durable State** (protected) vs. **Disposable Bloat** (garbage-collectible):

### Protected (Never Auto-Pruned)
- **User Prompts & Final Assistant Responses**
- **Newest / Unique File Reads:** An earlier file read is only eligible for deduplication if a *later* successful read covers the exact same file and line range with identical contents. Disjoint line ranges, modified files without a covering re-read, and failed reads are kept intact.
- **Current & Unread Tail Tool Results:** Any tool result after the last assistant turn, at the end of the prompt, or in the latest turn is protected so the model never goes blind to a compiler error or test failure it is actively debugging.
- **Signed Reasoning Blocks:** Any payload containing cryptographic thinking signatures (Anthropic `signature` / `signature_delta`, Gemini `thoughtSignature`, or OpenAI encrypted reasoning items) is treated as opaque and never mutated.
- **`contextWatcher` MCP Outputs & `<SKILL>` Payloads:** Self-inspection results and skill instructions are never stubbed.

### Garbage-Collectible Candidates
1. **Covered Duplicate File Reads (`ShadowDedupe`):**
   - Replaces earlier covered `read_file` tool results with a compact 1-line pointer (`[cw:shadow_deduped path="..." ...]`) while keeping `tool_use_id` / `call_id` pairing valid and leaving the newest read untouched.
2. **Completed Past-Turn Tool Logs (`PruneOrStub`):**
   - Replaces verbose command outputs (`>100` tokens) from completed earlier turns with a 1-line stub (`[cw:pruned tool="..." turn=... original_tokens=...]`).
3. **Completed Unsigned Thinking (`PruneOrStub`):**
   - Replaces unsigned `<thinking>` blocks from completed earlier turns with `[cw:pruned_thinking turn=...]`.
4. **Low-Salience Rule Reminders (`PinContext`):**
   - When contexts exceed 30,000 tokens and early `InjectedRule` nodes drop below 42% positional attention salience ("Lost in the Middle"), `cw` can append a `<cw_pinned_context>` reminder at the prompt tail without modifying the early prefix.
5. **Explicit Sub-Task Folding (`FoldSubtree`):**
   - When invoked explicitly via `cw fold <start> <end> --summary "..."` or `cw_fold_turns`, collapses a range of completed debugging turns into a caller-supplied outcome summary.

---

## 3. The 5-Stage Pipeline

```text
Raw Request / Session Transcript (.jsonl / SQLite)
                   │
                   ▼
┌─────────────────────────────────────────────────────────────┐
│ 1. Canonical IR Normalization (cw-core::normalizer)         │
│    Maps Anthropic / OpenAI / Gemini payloads into typed     │
│    ContextNodes while preserving raw JSON pointers.         │
└──────────────────────────────┬──────────────────────────────┘
                               │
                               ▼
┌─────────────────────────────────────────────────────────────┐
│ 2. BLAKE3 Fingerprinting & Salience Scoring (cw-core::ir)   │
│    Computes content/structural hashes (NodeFingerprint)     │
│    and U-curve positional attention salience.               │
└──────────────────────────────┬──────────────────────────────┘
                               │
                               ▼
┌─────────────────────────────────────────────────────────────┐
│ 3. KV-Cache Prefix Diff & Linter (cw-core::radix, linter)   │
│    Diffs token prefixes across turns and flags covered      │
│    duplicate reads, large tool logs, and low-salience rules.│
└──────────────────────────────┬──────────────────────────────┘
                               │
                               ▼
┌─────────────────────────────────────────────────────────────┐
│ 4. Deterministic Overlay Engine (cw-core::mutation)         │
│    Applies session MutationRules (min / clean / mid / max)  │
│    with pre-forward structural validation & fail-open guard.│
└──────────────────────────────┬──────────────────────────────┘
                               │
                               ▼
┌─────────────────────────────────────────────────────────────┐
│ 5. Persistence & Non-Destructive Export (cw-store, harness) │
│    Stores immutable raw blobs + revision-checked overlays   │
│    in SQLite; optionally exports .cw-compacted.jsonl copies.│
└─────────────────────────────────────────────────────────────┘
```

---

## 4. Workspace Layout

| Directory | Description |
| :--- | :--- |
| `crates/cw-core` | Canonical Context IR, protocol normalizers, Radix prefix analyzer, linter (`CW-L001`..`CW-L007`), mutation engine, and strategy optimizer. |
| `crates/cw-store` | SQLite persistence (`~/.local/share/context-watcher/cw.db`), content-addressed blob store with automatic superseded-blob GC, and revision-checked mutation ledger. |
| `crates/cw-proxy` | Loopback Axum HTTP/SSE proxy (`127.0.0.1:4401`) and authenticated control REST/WebSocket API (`127.0.0.1:4400`). |
| `crates/cw-harnesses` | Transcript importers (AGY, Codex, Claude Code, OMP/Pi, OpenCode SQLite, Hermes SQLite) and non-destructive `.cw-compacted.jsonl` exporter. |
| `crates/cw-mcp` | Built-in JSON-RPC 2.0 MCP server (`cw mcp`) over stdio with automatic workspace session resolution. |
| `crates/cw-tui` | Ratatui terminal dashboard (`cw tui`). |
| `crates/cw-cli` | Main `cw` binary (`serve`, `status`, `lint`, `optimize`, `preview`, `sync-back`, `purge`, `update`, `init`, `wrap`, `mcp`). |
| `web/` | Svelte 5 + XYFlow + D3 interactive Web UI embedded into `cw-cli`. |
| `ide/cw-intellij/` | Kotlin IntelliJ Platform plugin connecting to the local `cw` daemon. |

---

## 5. Data, Security & Storage Retention

- **Plaintext Local Store:** SQLite (`~/.local/share/context-watcher/cw.db`) and exported `.jsonl` files store plaintext conversation data protected by OS user permissions (`0600` / `0700`). Upstream `Authorization` headers are forwarded to the provider and stripped from stored request payloads.
- **Loopback Control Token:** All mutating control API endpoints require `Authorization: Bearer <token>` (printed by `cw auth-token`, stored in a private per-user token file) and `If-Match` graph revision headers. Proxy traffic only binds to an active overlay session when carrying both `x-cw-session-id` and `x-cw-control-token`. The token is a same-user credential, not a sandbox boundary against untrusted same-UID processes—see [`docs/COMPATIBILITY.md`](COMPATIBILITY.md).
- **Blob Garbage Collection & `64 MB` Migration Backup Cap:**
  - When a transcript snapshot is re-imported for an existing `(session_id, sequence_number)`, `cw-store` updates the turn in-place and automatically deletes superseded request/response blobs that are no longer referenced by any turn.
  - Before applying the additive safety schema migration (`safety_version = 2`), `cw-store` creates a private `cw.pre-safety-*.db` copy **only if the existing database is $\le 64\text{ MB}$**; databases larger than `64 MB` skip the automatic file copy (rather than creating a truncated backup) to avoid exhausting disk space.
  - Legacy Auto-Pilot configs (`safety_version < 2`) are disabled on upgrade and report a CLI migration notice in `cw status` / `cw autopilot` until re-enabled with `cw autopilot --enable`.
- **Purging History:** Run `cw purge --session <id>` to delete a session's stored history and unchanged exports, or `cw purge --session <id> --keep-turns N` to retain only the newest $N$ turns and garbage-collect unreferenced blobs.
