# BOARD.md — `contextWatcher` Project Board

> **Legend:** `P0` = Critical Path | `P1` = Core Feature | `P2` = Enhancement
> **Workflow:** Move items across `Ready` $\to$ `In Progress` $\to$ `Review` $\to$ `Done`. Always reference ticket IDs (`CW-XXX`) in commits and PRs.

---

## 🟢 Done

* [x] **CW-000** `[P0]` `[Arch]` Project vision, architecture, roadmap, specs, and `AGENTS.md` initialization.

### Epic 0: Workspace & Canonical Context IR (`specs/00-architecture.md`)
* [x] **CW-001** `[P0]` `[Workspace]` Initialize Cargo workspace (`cw-core`, `cw-store`, `cw-proxy`, `cw-harnesses`, `cw-mcp`, `cw-tui`, `cw-cli`) with shared dependencies and linting rules.
* [x] **CW-002** `[P0]` `[cw-core]` Define Canonical Context IR types (`Session`, `ContextTurn`, `ContextNode`, `NodeKind`, `Role`, `TokenSpan`, `NodeFingerprint`).
* [x] **CW-003** `[P0]` `[cw-core]` Implement lossless Protocol Normalizers & JSON-Pointer maps for **Anthropic Messages API** (`/v1/messages`), **OpenAI Chat/Responses API**, and **Gemini API**.
* [x] **CW-004** `[P0]` `[cw-core]` Implement fast Tokenizer & Token Accounting module (provider-aware prefix serialization order & positional attention salience curve).
* [x] **CW-005** `[P0]` `[cw-store]` Implement SQLite persistence layer (`sessions`, `turns`, `blobs`, `mutations`, `firewall_rules`, `file_states`) with WAL mode enabled.

### Epic 1: Intercepting Proxy & Transcript Watchers (`specs/00-architecture.md`, `specs/04-harness-and-ide-plugins.md`)
* [x] **CW-006** `[P0]` `[cw-proxy]` Build Axum HTTP/SSE reverse proxy with streaming chunk tap, zero-drop header forwarding, and automatic provider detection.
* [x] **CW-007** `[P0]` `[cw-proxy]` Expose Daemon Control REST API (`/api/v1/sessions`, `/api/v1/turns`, `/api/v1/mutations`, `/api/v1/replay`, `/api/v1/firewall`) and real-time WebSocket hub (`/api/v1/ws`).
* [x] **CW-008** `[P0]` `[cw-harnesses]` Implement **Antigravity (AGY)** adapter (`transcript.jsonl` + `transcript_full.jsonl` watcher & parser, including `truncated_fields` resolution and tool call pairing).
* [x] **CW-009** `[P0]` `[cw-harnesses]` Implement **Codex CLI** and **OpenAI-compatible** session rollout watcher & parser.
* [x] **CW-010** `[P1]` `[cw-harnesses]` Implement **Pi / OpenCode / Aider** and **Claude Code** (`~/.claude/projects/**/*.jsonl`) transcript watchers.
* [x] **CW-011** `[P1]` `[cw-cli]` Implement `cw wrap <harness-cmd>` launcher that injects proxy env vars (`OPENAI_BASE_URL`, `ANTHROPIC_BASE_URL`, `GEMINI_API_BASE_URL`) and attaches session metadata.

### Epic 2: Radix Tree, KV Cache X-Ray & Context Linter (`specs/01-context-graph-and-kv-cache.md`)
* [x] **CW-012** `[P0]` `[cw-core]` Implement Token-Prefix Radix Tree builder (`build_radix_graph`) across sequential turns and conversation branches.
* [x] **CW-013** `[P0]` `[cw-core]` Implement KV Cache Boundary & Prefix Diff Analyzer (`CachedHit` 🟢, `NewlyCached` 🟡, `CacheInvalidated` 🔴) with exact invalidation root-cause detection (`DynamicSystemInjection`, `ToolSchemaMutated`, `HistoryEditedOrCompacted`, `CacheTtlExpired`).
* [x] **CW-014** `[P0]` `[cw-core]` Implement **Stale File Detector (`CW-L001`)**: cross-reference `read_file` nodes in context against subsequent `edit_file`/`write_file` tool calls and on-disk file BLAKE3 hashes.
* [x] **CW-015** `[P0]` `[cw-core]` Implement **Duplicate Read & Output Detector (`CW-L002`, `CW-L003`, `CW-L004`)**: detect identical or overlapping file reads, massive outputs, and consecutive failed debugging loops.
* [x] **CW-016** `[P1]` `[cw-core]` Implement **Tool Bloat & Attention Dilution Linter (`CW-L005`, `CW-L006`, `CW-L007`)**: measure MCP tool schema overhead, detect unused tool servers, and compute "Lost in the Middle" attention risk scores.

### Epic 3: Svelte 5 Web UI & Ratatui TUI (`specs/03-web-ui-and-tui.md`)
* [x] **CW-017** `[P0]` `[web]` Initialize Svelte 5 + Vite + TypeScript project in `web/` and embed static build into `cw-cli` via `rust-embed`.
* [x] **CW-018** `[P0]` `[web]` Build real-time WebSocket state store (`$state` / `$derived` / `$effect` runes) synced with the `cw` daemon.
* [x] **CW-019** `[P0]` `[web]` Build **Interactive Context Graph / Radix Tree Canvas** with KV cache color coding and lint badges.
* [x] **CW-020** `[P0]` `[web]` Build **Token Treemap & KV Cache Flamegraph** (`d3-hierarchy`) showing exact proportional usage across System, Rules, Skills, MCP Schemas, File Reads, Bash Outputs, and Thinking.
* [x] **CW-021** `[P1]` `[web]` Build **Node Inspector & Surgical Action Drawer** (JSON Pointer viewer, token span details, and 1-click mutation controls).
* [x] **CW-022** `[P1]` `[cw-tui]` Build **Ratatui Terminal UI (`cw tui`)** with split-pane Session/Turn Tree, KV Cache gauge, Lint list, and quick mutation hotkeys (`p` prune, `f` fold, `d` dedupe, `Space` pin, `o` optimize).
* [x] **CW-023** `[P1]` `[cw-cli]` Implement CLI inspection & manipulation commands: `cw status`, `cw tree`, `cw lint`, `cw fold`, `cw prune`, `cw dedupe`, `cw gate`, `cw pin`, `cw import`.

### Epic 4: Surgical Mutation & Deterministic Overlay Engine (`specs/02-mutation-engine.md`)
* [x] **CW-024** `[P0]` `[cw-core]` Implement `NodeFingerprint` content/structural hashing and the **Deterministic Mutation Ledger (Overlay Engine)** so mutations persist across stateless HTTP proxy turns.
* [x] **CW-025** `[P0]` `[cw-core]` Implement **Prune / Stub Operator (`PruneOrStub`)**: replace heavy tool outputs or thinking blocks with compact stubs while maintaining valid tool-use/tool-result pairing.
* [x] **CW-026** `[P0]` `[cw-core]` Implement **Subtree Folding Operator (`FoldSubtree`)**: collapse a multi-turn trajectory into a single outcome summary node.
* [x] **CW-027** `[P0]` `[cw-core]` Implement **Shadow Deduplication Operator (`ShadowDedupe`)**: automatically replace superseded earlier `read_file` outputs with `[cw:shadow_deduped ...]`.
* [x] **CW-028** `[P0]` `[cw-core]` Implement **Dynamic Tool & Skill Gating Operator (`GateToolsAndSkills`)**: filter disabled MCP tools or servers out of the outgoing request payload on the fly.
* [x] **CW-029** `[P1]` `[cw-core]` Implement **Context Pinning (`PinContext`)**: inject `<cw_pinned_context>` blocks at the high-attention tail of the prompt without invalidating the cached token prefix.
* [x] **CW-030** `[P1]` `[cw-core]` Implement **Cache-Safe Draft Queue**: stage prefix-altering mutations and estimate KV cache rebuild cost (`estimate_mutation_impact`) before committing.

### Epic 5: Context Firewall (Breakpoints) & Time-Travel Replay (`specs/02-mutation-engine.md`, `specs/03-web-ui-and-tui.md`)
* [x] **CW-031** `[P1]` `[cw-proxy]` Implement **Context Firewall (`evaluate_firewall`)**: pause in-flight proxy requests or auto-truncate/dedupe when rules trigger.
* [x] **CW-032** `[P1]` `[cw-proxy]` Implement **Time-Travel Forking & A/B Replay Engine (`/api/v1/sessions/:id/replay`)**: branch from any historical turn, apply mutation overlays, and record side-by-side comparison metrics.
* [x] **CW-033** `[P1]` `[web]` Build **Firewall Interceptor Alert Banner** and **A/B Replay Comparison Studio** in the Svelte 5 Web UI.

### Epic 6: MCP Server, Harness Plugins & IntelliJ Plugin (`specs/04-harness-and-ide-plugins.md`)
* [x] **CW-034** `[P0]` `[cw-mcp]` Implement built-in MCP Server (`cw mcp`) exposing self-inspection and self-compaction tools (`cw_context_status`, `cw_evict_stale_reads`, `cw_fold_turns`, `cw_prune_tool_output`, `cw_pin_context`, `cw_gate_tools`).
* [x] **CW-035** `[P0]` `[plugins/agy]` Create **Antigravity (AGY)** integration package: `context-watcher` Skill (`SKILL.md`), MCP configuration, and `transcript.jsonl` + `transcript_full.jsonl` sync.
* [x] **CW-036** `[P1]` `[plugins/codex]` Create **Codex CLI** plugin wrapper, config profile, and MCP integration.
* [x] **CW-037** `[P1]` `[plugins/pi-opencode]` Create **Pi / OpenCode / Aider** and **Claude Code** integration configs and wrapper scripts.
* [x] **CW-038** `[P1]` `[ide/cw-intellij]` Create Kotlin **IntelliJ Platform Plugin** (`cw-intellij`) with WebSocket & HTTP service connecting to local `cw` daemon.
* [x] **CW-039** `[P1]` `[ide/cw-intellij]` Implement IntelliJ **Editor Notification Banner & Gutter Line Markers** (`CwEditorNotificationProvider`, `CwLineMarkerProvider`).
* [x] **CW-040** `[P1]` `[ide/cw-intellij]` Implement IntelliJ **ContextWatcher Tool Window** & Editor Actions (`PinSelectionToContextAction`, `EvictStaleFileAction`).

### Epic 7: Production Hardening, Multi-Harness Sync & Local Model Engine Optimization
* [x] **CW-041** `[P0]` `[cw-harnesses/cw-mcp/cw-cli]` Implement **Safe Transcript Write-Back (`cw sync-back`)** with `.bak` backups for Antigravity, Claude Code, Codex, OpenCode, Pi & Aider + `cw install-integrations` + `cw_optimize_strategy` MCP tool.
* [x] **CW-042** `[P0]` `[cw-proxy]` **Proxy Production Hardening**: Strip internal `cw_*` fields before upstream forwarding, normalize `Accept-Encoding`, and add async **SSE Stream Parser Tap** for Anthropic, OpenAI, Gemini, Ollama & llama.cpp usage/KV metrics.
* [x] **CW-043** `[P0]` `[cw-core]` **Real Disk `mtime` & Line-Slice Hash Verification (`CW-L001`)**: Detect external IDE/git edits on both full and partial (`StartLine..EndLine`) file reads and support 1-click Fresh Disk Diff tail injection.
* [x] **CW-044** `[P0]` `[cw-core/cw-proxy]` **Auto-Pilot Background Policies**: Hands-free policies (`zero_break_tail_hygiene` with `/tmp/cw_logs/` spillover, `auto_cache_safe_pin`, `auto_diet_at_breakpoint`, `local_model_window_guard`) with REST API, CLI & Web UI controls.
* [x] **CW-045** `[P0]` `[cw-core/web]` **Model Pricing, Cost ($) Ledger & Prefill Time Calculator**: Compute live turn cost (`$`), KV cache `$` savings, `cw` overlay `$` savings, and local prefill milliseconds saved across cloud and local models.
* [x] **CW-046** `[P0]` `[cw-harnesses/cw-proxy]` **Incremental Watcher & Lightweight Graph API**: Track file `(mtime, byte_len)` state across all harness directories and serve lightweight historical turn summaries on `/api/v1/sessions/:id/graph` for instant rendering on 350k+ token sessions.
* [x] **CW-047** `[P0]` `[cw-core/cw-proxy/web/cw-cli]` **Local Model Engine & KV-Cache Slot Controller (`Ollama`, `llama.cpp`, `vLLM`, `SGLang`, `LM Studio`)**: Automatic `n_keep` / `num_keep` system-prompt pinning, `cache_prompt: true` + `id_slot` session affinity, PagedAttention block alignment, KV VRAM calculator (`FP16`/`Q8_0`/`Q4_0`), sliding-window overflow guard, and Web UI **`🖥️ Local LLM & Auto-Pilot`** tab + `cw local` CLI.

---

## 🚧 In Progress
*(None)*

---

## 📋 Ready
*(None)*

---

## 🗂️ Backlog
*(None)*
