# BOARD.md — `contextWatcher` Project Board

> **Legend:** `P0` = Critical Path | `P1` = Core Feature | `P2` = Enhancement
> **Workflow:** Move items across `Ready` $\to$ `In Progress` $\to$ `Review` $\to$ `Done`. Always reference ticket IDs (`CW-XXX`) in commits and PRs.

> **Current delivery priority after CW-114 qualification:** CW-115 (Empirical performance and savings qualification). Maintainability gate (CW-116), Codex qualification (CW-112), and Long-Session Reliability campaign (CW-114) complete.

---

## Historical implementation milestones

These checkboxes record earlier implementation work, not current qualification. The remediation section and [support contract](docs/COMPATIBILITY.md) govern current behavior; unsupported claims below remain historical design descriptions.

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

### Epic 8: Pre-Forward Validation Gate, Mutation Reliability & Taxonomy
* [x] **CW-048** `[P0]` `[cw-proxy/cw-core]` **Post-Mutation Pre-Forward Validation Gate & Fail-Open Fallback**:
  - Re-parse mutated outgoing JSON through `cw-core::ir` protocol normalizer before upstream dispatch.
  - Assert structural invariants: strict role alternation, `tool_use` <-> `tool_result` ID 1:1 pairing, thinking signature integrity.
  - Fail-Open Guarantee: On any validation error, log diagnostic warning, revert payload to raw unmutated clone, and forward safely so upstream never returns HTTP 400.
* [x] **CW-049** `[P1]` `[cw-store/cw-proxy]` **Closed-Loop Mutation Effectiveness & Telemetry Tracker**:
  - Correlate applied `MutationLedgerId` with subsequent turn metrics (TTFT ms, provider cache hit/miss tokens, follow-up read frequency).
  - Surface per-mutation effectiveness scores in `cw gain --history` and Web UI.
* [x] **CW-050** `[P2]` `[cw-core/cw-mcp]` **Taxonomy Reconciliation (`ShadowDedupe`)**:
  - Align `CW-L001` linter fix name, MCP tool description, and `MutationOp` onto unified `ShadowDedupe` taxonomy.
* [x] **CW-051** `[P0]` `[cw-core/cw-harnesses/cw-proxy/cw-cli]` **Proactive Low-Health Self-Healer (`AutoPilot Rule 5`) & OMP Benchmark**:
  - Autonomous health threshold evaluation (`min_health_score`, default 75) across in-flight proxy requests and background transcript ingestion.
  - Zero-prompt auto-healing: applies non-breaking shadow deduplication and surgical log compaction, commits mutations, and atomically syncs back to `.jsonl` transcript with `.bak` safety.
  - CLI control via `cw autopilot --min-health <u8> --strategy <str> --sync-to-disk <bool>`.
  - Deep empirical benchmark conducted with `omp` against local LLM `flint` (`192.168.1.156:8090`, Q8_0, 40B): health restored from 48 to 100/100, 5,074 tokens saved, 99.1% KV cache hit ratio preserved, TTFT accelerated 4.3x (428ms), 100% test pass rate (8/8).

### Epic 9: Architecture, SOLID Refactoring & Robustness Hardening
* [x] **CW-052** `[P0]` `[cw-core/cw-store/cw-proxy/cw-harnesses/cw-mcp]` **Codebase Audit, SOLID Principles Refactoring & Multi-Packet Stream Hardening**:
  - **Zero-Panic Guarantee**: Eliminated `.unwrap()` hazards across proxy dry-run handlers, store mutex locks, and CLI status lookups.
  - **Gemini Tool Compatibility**: Fixed LSP violation by normalizing tool filtering and deterministic sorting across both `functionDeclarations` and `function_declarations` schemas.
  - **Dependency Inversion (DIP)**: Abstracted database coupling by introducing [`ContextRepository`](file:///home/gionag/Development/contextWatcher/crates/cw-core/src/repository.rs) and [`MutationRepository`](file:///home/gionag/Development/contextWatcher/crates/cw-core/src/repository.rs) in `cw-core`.
  - **JSON-RPC 2.0 Compliance**: Standardized MCP error handling with error code `-32601` for unknown methods and diagnostic errors for unknown tools.
  - **Multi-Packet SSE Stream Tap**: Implemented [`SseChunkAccumulator`](file:///home/gionag/Development/contextWatcher/crates/cw-proxy/src/lib.rs) ensuring zero dropped usage or KV cache metrics under fragmented TCP packets.
  - **Crash-Safe Atomic Sync-Back**: Implemented atomic temp file writes (`fs::rename`) and automated `.bak` backups before modifying harness transcripts.
  - **Verification**: 25 automated tests passing across the workspace with 0 clippy warnings.

---

### Epic 10: Horizon 1 — Always-On Zero-Touch Automation & Ambient Pulse
* [x] **CW-053** `[P0]` `[cw-core/cw-store]` **Ambient Savings Metric Accumulator & Pulse Formatter**:
  - Compute cumulative tokens saved, KV cache hit ratio, Time-to-First-Token acceleration factor (e.g. 176s -> 1.6s, 108x faster), and dollar value saved.
  - Generate clean, friendly pulse strings: `⚡ [cw] We saved you 12,966 tokens | KV Cache: 98.2% preserved | TTFT: 176s -> 1.6s (108x faster) | $0.18 saved`.
* [x] **CW-054** `[P0]` `[cw-proxy/cw-harnesses]` **Always-On Autonomous Background Self-Healer**:
  - Automatically evaluate in-flight proxy turns and background transcript watcher updates.
  - Proactively apply non-breaking surgical log and monologue diet whenever context bloat exceeds 15,000 tokens or Health drops below 75, with zero user prompting.
* [x] **CW-055** `[P0]` `[cw-cli]` **Ambient Savings Pulse Delivery**:
  - Output periodic pulse updates to stderr in `cw wrap` and log pulse events to SQLite.
  - Support automatic terminal notifications and non-breaking transcript comments.
* [x] **CW-056** `[P1]` `[cw-cli]` **`cw pulse` CLI & Ambient Notification Integration**:
  - Implemented `cw pulse` command to query recent ambient savings notifications across active sessions (`cw pulse --session <id>`).
  - Added REST API routes `/api/v1/pulses` and `/api/v1/sessions/:id/pulses` + WebSocket event `AmbientSavingsPulse`.

---

### Epic 11: Horizon 2 — Multi-Agent Swarm Radix Router
* [x] **CW-060** `[P1]` `[cw-core]` **Cross-Session Prefix Grouping & Radix Tree Sharing**:
  - Implemented [`SwarmPrefixGroup`](file:///home/gionag/Development/contextWatcher/crates/cw-core/src/swarm.rs) and [`analyze_swarm_prefix_sharing`](file:///home/gionag/Development/contextWatcher/crates/cw-core/src/swarm.rs) in `cw-core`.
  - Automatically clusters active and historical agent sessions by `(workspace, model)` and extracts their Longest Common Prefix (LCP) across ordered root nodes (system prompt, rules, tool definitions).
  - Computes exact local GPU VRAM memory multiplexing reduction: $(M - 1) \times P$ tokens saved, with FP16 and INT8/Q8 megabyte savings estimates.
  - Implemented [`build_swarm_radix_graph`](file:///home/gionag/Development/contextWatcher/crates/cw-core/src/swarm.rs) generating a unified visual Radix DAG with shared trunk and branching subagent leaves.
* [x] **CW-061** `[P1]` `[cw-proxy/cw-cli]` **Swarm Prompt Multiplexer & CLI Inspection (`cw swarm`)**:
  - Implemented `cw swarm [--project <name>] [--model <name>] [--min-members <N>] [--json]` CLI command in `cw-cli`.
  - Added REST API routes `/api/v1/swarms` and `/api/v1/swarms/:group_id/graph` in `cw-proxy`.
  - Tested on live sessions: identified 47 multi-agent clusters across 300 active agents with 7,485 shared tokens saved and 1.87 GB VRAM preserved.

---

### Epic 12: Horizon 3 — Engine-Level PagedAttention Splicing
* [x] **CW-070** `[P2]` `[cw-core/cw-proxy/cw-cli]` **Engine-Level PagedAttention & Slot Bridge**:
  - Implemented [`crates/cw-core/src/slot_bridge.rs`](file:///home/gionag/Development/contextWatcher/crates/cw-core/src/slot_bridge.rs) providing discrete PagedAttention memory block mapping (`block_size = 16` or `64`), deterministic slot affinity hashing, and zero-recompute physical page unmapping calculation (`plan_slot_splicing`).
  - Added REST API routes `/api/v1/engine/slots` and `/api/v1/engine/slots/:slot_id/splice` in [`crates/cw-proxy/src/lib.rs`](file:///home/gionag/Development/contextWatcher/crates/cw-proxy/src/lib.rs).
  - Implemented `cw slot [--session <id>] [--engine <url>] [--slot-id <N>] [--splice-range <start..end>] [--json]` in [`crates/cw-cli/src/main.rs`](file:///home/gionag/Development/contextWatcher/crates/cw-cli/src/main.rs).
  - Validated live against upstream engine at `http://192.168.1.156:8095`: successfully retrieved slot status (131k context, 28k tokens, 1,761 blocks, 7.0 GB VRAM) and simulated physical page unmap (300 blocks / 4,800 tokens / 1.2 GB VRAM / ~7.4s prefill recompute avoided).

### Epic 13: Live Multi-Agent Swarm Benchmark on Hardware (.156)
* [x] **CW-075** `[P1]` `[benchmark]` **Multi-Agent Swarm Concurrency Benchmark on Strata/Qwen3.8**:
  - Implemented [`benchmark/run_swarm_benchmark.py`](file:///home/gionag/Development/contextWatcher/benchmark/run_swarm_benchmark.py) simulating 4 subagents (Lead, Coder, Tester, Auditor) hitting `http://192.168.1.156:8095`.
  - Proven on hardware: unshared execution cold-prefills 12,886 tokens taking 85.6s, while `cw swarm` multiplexing drops team memory to 3,286 tokens (74.5% memory reduction, 2.34 GB VRAM saved) and slashes TTFT to 21.2s (4.04x speedup, 120x+ on subagents).
  - Generated report in [`benchmark/artifacts_swarm/SWARM_BENCHMARK_REPORT.md`](file:///home/gionag/Development/contextWatcher/benchmark/artifacts_swarm/SWARM_BENCHMARK_REPORT.md).

### Epic 14: Horizon 4 — Ambient HUD & Attention Gradient ECG
* [x] **CW-080** `[P2]` `[cw-core/cw-proxy/cw-cli]` **Context HUD & Real-Time Attention Gradient ECG**:
  - Implemented [`crates/cw-core/src/hud.rs`](file:///home/gionag/Development/contextWatcher/crates/cw-core/src/hud.rs) computing empirical U-curve attention salience (`calculate_attention_salience`), Primacy / Trough / Recency zone classification, and `<1ms` HUD statusline formatting.
  - Added REST API routes `/api/v1/hud`, `/api/v1/sessions/:id/hud`, and `/api/v1/sessions/:id/ecg` in [`crates/cw-proxy/src/lib.rs`](file:///home/gionag/Development/contextWatcher/crates/cw-proxy/src/lib.rs).
  - Implemented `cw hud [--session <id>] [--compact] [--ecg] [--json]` in [`crates/cw-cli/src/main.rs`](file:///home/gionag/Development/contextWatcher/crates/cw-cli/src/main.rs). Tested and verified live on active sessions.

---

### Epic 15: Final Comprehensive Hardware Benchmark Suite (.156)
* [x] **CW-085** `[P1]` `[benchmark]` **End-to-End Multi-Horizon Validation on Strata ROCm Dual AMD gfx906**:
  - Implemented [`benchmark/run_comprehensive_horizon_benchmark.py`](file:///home/gionag/Development/contextWatcher/benchmark/run_comprehensive_horizon_benchmark.py) providing empirical validation across all 4 evolutionary horizons on `Qwen3.8-Flash-Next` on remote host `http://192.168.1.156:8095`.
  - Generated comprehensive multi-horizon performance report in [`benchmark/artifacts_comprehensive/COMPREHENSIVE_HORIZON_BENCHMARK_REPORT.md`](file:///home/gionag/Development/contextWatcher/benchmark/artifacts_comprehensive/COMPREHENSIVE_HORIZON_BENCHMARK_REPORT.md).

---

## Current status audit (2026-10-29)

The checked tickets below are historical implementation milestones. They are not current acceptance evidence. See [the detailed board audit](docs/BOARD-AUDIT-2026-10-29.md) and [verified support contract](docs/COMPATIBILITY.md). The review found several tickets whose wording claims guarantees or end-to-end support that the current code does not provide.

| Area | Current code status | Board correction needed |
|---|---|---|
| Core IR, CLI, UI, store, proxy and adapters | Code exists; workspace build/tests pass | Keep as implemented milestones, not feature qualification |
| Observation, auth, approval and raw request capture | Safer defaults and mock coverage implemented | Real-provider and every-client header paths still need verification |
| Mutations and overlays | Core operators exist with stricter identity and content guards | Generic folds/tool gates are not safe automatic compaction; pins may change prefix |
| Transcript sync | Separate private JSONL export, source unchanged | CW-041 in-place `.bak` write-back across six harnesses is not implemented |
| Autopilot | Config/API/UI and limited proposal logic remain | Automatic spill, sync and several self-heal behaviors are disabled |
| Watchers | Registered JSONL snapshots are reparsed on blocking workers | No incremental cursor/all-home scan; SQLite harness sources are import-only |
| Local engine/slots | Heuristics and preview endpoints exist | No physical page unmapping, exclusive slot ownership, or promised engine option injection |
| Cache/cost/attention analytics | Estimates plus provider usage fields exist | No exact tokenizer/cache-boundary, quality or speed guarantees |
| Clients | Web build/check and IntelliJ plugin build passed. Codex CLI 0.165.0 now uses a wrapper-supplied Responses provider with authenticated headers; local mock verified session reuse and pin reapplication. | Browser/IDE runtime and real-provider behavior remain unverified. |
| Qualification | Five isolated sessions across three task families produced 24/25 passing answers. All baseline and cw strategy answers passed; one native transition answer failed its contract. | No broad quality/performance or hardware qualification. |

### Remediation status

| Ticket | Status | Current evidence / remaining work |
|---|---|---|
| **CW-R0** Evidence and finding ledger | Done | Baseline and review findings are recorded in `docs/BOARD-AUDIT-2026-10-29.md` and `docs/REMEDIATION.md`. |
| **CW-R1** Observation defaults and capability labels | Done | Safer defaults and compatibility limits are documented. |
| **CW-R2** Approvals and local control | Done for tested paths | API/proxy authorization tests pass; real-provider paths remain outside qualification. |
| **CW-R3** Session identity and concurrent requests | Partial | Codex session reuse and overlay reapplication remain mock-verified. OMP/Claude request headers worked in mocks, but CW headers are now stripped from non-Codex children after a shell probe demonstrated credential exposure. |
| **CW-R4** Immutable originals and mutation revisions | Done | Migration snapshots and revision checks are covered by store tests. |
| **CW-R5** Selectors and conservative mutations | Done for covered shapes | Responses `input` pin injection now works; unsupported tails are reported unapplied. Other shapes remain fail-closed. |
| **CW-R6** Transcript export and privacy | Done within support contract | Separate private exports and purge are covered; in-place transcript replacement is unsupported. |
| **CW-R7** Provider and harness support contract | In Progress | Codex has authenticated overlay reapplication evidence. Hermes has an enabled `llm_request` plugin. Direct launches now use persistent `model.base_url: http://127.0.0.1:4401/deepseek`; plain launch plus resume recorded two turns under the same CW session on 2026-10-29. `cw wrap hermes chat` remains a per-process alternative. Isolated two-turn testing verified session binding and committed pin reapplication; live signature qualification is not claimed. Other Hermes providers and SQLite write-back remain unverified or unsupported. OpenCode 1.17.3, Pi 1.0.4, and Aider 0.86.2 passed local mock requests with native provider headers and bound the configured session; `cw wrap` does not create those settings, and same-user tool isolation is not established. An automated proxy test checks authenticated session selection, overlay application, and CW-header stripping. OMP/Claude wrapper auth is disabled after a shell probe exposed credentials to Claude Bash. Gemini CLI 0.42.0 and Antigravity CLI 1.4.10 completed local mock calls through `cw wrap`, but explicit wrapper session IDs were not attached; both remain observation-only. Gemini CLI's custom provider-header variable is unsafe for CW credentials: the installed 0.42.0 source preserves all `GEMINI_CLI_*` variables in shell-tool environments, including when environment redaction is enabled. The wrapper now strips inherited `x-cw-*` values from Gemini custom provider headers; focused CLI test passes. Do not bind sessions through that variable. A safe Gemini/Antigravity session transport remains open. Signed Anthropic request and SSE response fixtures pass byte-preservation checks. On 2026-10-29, OpenRouter-backed live checks passed for an Anthropic-routed Claude Haiku 5.5 streaming `signature_delta` and a Gemini tool response with nonempty normalized `reasoning_details`. OpenRouter did not return a native `thoughtSignature` in the Gemini response; direct Anthropic and native Gemini signature checks remain outstanding. |
| **CW-R8** Cache planning and engine capabilities | Done as a conservative preview | Unknown usage stays unknown; physical page unmapping is not implemented. |
| **CW-R9** Request/response lifecycle telemetry | Done for mock paths | Raw bytes and completed-stream paths are covered by mocks; no real-provider latency measurement exists. |
| **CW-R10** Web and IDE client behavior | Deferred | Final acceptance task by user request. Web check/build, IntelliJ package build, and proxy API mutation test pass; interactive browser/IDE behavior remains unverified. |
| **CW-R11** Storage and IO ownership | Partial | Blocking work and pagination are implemented; large-history and full database IO qualification remain open. |
| **CW-R12** CI and product qualification | Partial | CI/docs and sanitized multi-task evidence are current; broad quality, performance, and hardware qualification remain open. |
| **CW-090** Built-in updater | Done (implementation) | Release checksum verification, bounded downloads, safe binary extraction, atomic install, source build, integration refresh, and optional daemon restart are implemented. Six CLI tests and clippy pass; no live update was run against the user's installed binary. |
| **CW-100** Verified context optimization milestone | Done | Milestone addressing logic risk remediation, claims reconciliation, and focused end-to-end qualification foundation. |
| **CW-101** Cache telemetry uncertainty & prefix safety | Done | Centralized `calculate_kv_break_tokens` across optimizer and mutation estimator; redefined `min` (`cache_safe_boost`) as strictly prefix-preserving (zero historical prefix modification to guarantee next-request cache reuse); added regression tests covering 0, partial, and unknown telemetry across consecutive requests. |
| **CW-102** Claims reconciliation in EVIDENCE.md & README.md | Done | Separate static AST bloat opportunity (278 sessions, 14.33M tok) from active live proxy savings (153.8k tok, $2.10 saved). Differentiate theoretical prefill models, local GPU benchmarks, and cloud TTFT nuance. |
| **CW-103** Focused qualification: Codex CLI & Hermes Agent | Paused for CW-116 | Mock-provider and harness-binary qualification exist. Hermes concurrency is completed in CW-113; resume real-provider work through CW-112 after the maintainability gate. |
| **CW-104** Hardware & local model qualification | Backlog | Differentiate PagedAttention memory footprint modeling from cloud provider KV caching, document slot bridge limitations, and benchmark local Ollama/llama.cpp engines. |
| **CW-110** Shared harness qualification runner | Done | Automated qualification test suite in `crates/cw-proxy/tests/qualification.rs` covering all 10 acceptance criteria across mock Responses, Chat Completions, and Messages upstreams; documented baseline support matrix and blockers in `docs/HARNESS_MATRIX.md`. |
| **CW-110A** Qualification evidence hardening | Done | Completely separated qualification suite into Suite A (CI default contract tests), Suite B (harness binary integration requiring installed binaries and failing explicitly if absent), and Suite C (live provider continuation). Hardened AC4 (active mutation tool-pairing), AC5 (byte-for-byte & JSON-tree thinking signature fidelity), AC6 (tool env sandboxing), AC7 (BLAKE3 bit-for-bit transcript preservation), AC8 (Validation Gate fail-open), and AC10 (un-fabricated cache status reconciliation). |
| **CW-111** OMP native runtime integration | Done | Replaced command-only extension with native runtime `context` and `before_provider_request` hooks and `/api/v1/sessions/:id/transform` endpoint; automated test (`test_harness_binary_omp_native_runtime_overlay_reapplication`) proved active overlay injection on Turn 2 wire payload while on-disk `.jsonl` transcript remained 100% immutable. |
| **CW-111B** OMP native runtime hardening | Done | Eliminated control token from `models.json` and deleted unsafe helper; purged `CW_CONTROL_TOKEN` from `process.env` in memory on startup; enforced loopback-only URLs on `CW_DAEMON_URL`; prioritized native session ID to eliminate cross-session binding under switching; replaced time/length heuristics with reference-based WeakSet idempotency; hardened `PruneOrStub` safety to strictly forbid pointer-only user/system prompt modification; verified 3-turn native resume (`omp -r <uuid>`), wire pinning, on-disk transcript preservation, and offline fail-open recovery. |
| **CW-112** Codex CLI multi-turn qualification | Done | 3-turn native session resume (`codex exec resume <thread_id>`), tool execution, overlay reapplication, credential sandboxing, on-disk rollout immutability, and live provider continuation with prompt caching verified with installed Codex CLI 0.165.0 and `gpt-4o-mini`. Documented in docs/CW-112-CODEX-QUALIFICATION.md. |
| **CW-113** Hermes Agent lifecycle hardening | Done | Replaced shared override file with UUID-scoped `hermes-override-<uuid>.json` (mode 0600, atomic `create_new`); middleware validates non-symlink, mode 0600, age <= 120s, and atomically unlinks on consumption; gateway daemon session isolation in `_SESSION_OVERRIDES`; passed 7-gate multi-process qualification and 20-session stress test. |
| **CW-114** Long-session reliability campaign | Done | Unified acceptance suite across OMP, Codex, and Hermes across 5 coding-task families $\times$ 3 repetitions = 45 sessions (100% pass, 45/45 passed, 0 failed, 360 checkpoints). Documented in `docs/CW-114-LONG-SESSION-RELIABILITY.md`. |
| **CW-115** Performance and savings qualification | Done | Empirical performance qualification across 2 phases: (1) 3-mode pilot (Mode A Native vs Mode B Passthrough vs Mode C Optimized across 6 live sessions, headline finding +46.2% cost across pilot due to cache invalidation vs -20.7% cache-aligned savings, PR #21 merged); (2) 4-mode optimization isolation experiment (Mode 0 Passthrough vs Mode 1 Tail Pin vs Mode 2 Hist Prune vs Mode 3 Combined), proving 100% prefix preservation in Mode 1 (-34.06% tokens, +37.71% savings) and compound maximum savings in Mode 3 (-50.02% tokens, +48.31% savings); documented in docs/CW-115-FOUR-MODE-ISOLATION-REPORT.md and docs/cw115_four_mode_benchmark_data.json. |
| **CW-116** Architecture consolidation & maintainability | Done | Three bounded, behavior-preserving PRs: CW-116A characterization/contracts, CW-116B CLI/harness modularization, CW-116C proxy/transformation boundaries. Maintainability gate complete. |
| **CW-116A** Characterization tests & architecture contracts | Done | Characterization baseline documented in docs/CW-116A-BASELINE.md; architecture decomposition plan in docs/CW-116-ARCHITECTURE.md; 12 net new characterization tests added covering CLI lifecycle, transform API, proxy routing, internal header stripping, dry-run preview, and store immutability; 100/100 workspace tests passing (+12 net increase over 88 baseline) with 0 warnings; PR #13 merged into main. |
| **CW-116B** CLI & harness architecture | Done | Decomposed monolithic `main.rs` (3,210 -> 618 lines, -80.7%) and `lib.rs` (2,385 -> 277 lines, -88.4%) into cohesive command handlers (`wrap.rs`, `daemon.rs`, `init.rs`, `reports.rs`, `session.rs`, `slot.rs`, `web.rs`) and harness lifecycle modules (`error.rs`, `qualification.rs`, `sync.rs`, `auto_heal.rs`, `watcher.rs`, `adapters/`). 100/100 workspace tests passing, 12/12 Hermes Python tests passing, 3/3 ignored harness binary qualification tests passing with zero warnings; PR #14 merged into main. |
| **CW-116C** Proxy & transformation boundaries | Done (PR ready) | Decomposed monolithic `lib.rs` (1,974 -> 249 lines, -87.4%) and `forwarding.rs` (585 lines dismantled) into modular `state.rs`, `security.rs`, `sse.rs`, `proxy/` and `api/` subsystems. Preserved dual `/api/v1/sessions/:id/transform` routes, raw wire byte streaming, thinking block signatures, and fail-open validation. 100/100 workspace tests passing, 12/12 Hermes Python tests passing, 3/3 ignored qualification tests passing with zero warnings; report delivered in `docs/CW-116C-REFACTOR-REPORT.md`. |

---

### Epic 16: Architecture consolidation and maintainability (CW-116)

**Why now:** CW-111B and CW-113 strengthened runtime behavior and isolation, but large CLI, harness, proxy, store and normalizer modules increase the effort and risk of later changes. Stabilize responsibilities before CW-112/CW-114/CW-115.

**CW-116A — Characterization and contracts (first PR; branch: feat/cw-116a-characterization-contracts)**
- Inventory actual crate dependencies, call flows, configuration side effects, high-risk modules, responsibility ownership and unwanted dependency directions. Use current code and the verified compatibility/support matrix rather than historical claims.
- Deliver docs/CW-116A-BASELINE.md (or equivalent) with module/change-hotspot inventory, architecture map, test/CI coverage, unverified areas, and proposed seams for CW-116B/C.
- Add focused characterization/regression tests for *existing* CLI behavior; authenticated vs observation-only session binding; provider-transparent and signed/unknown passthrough; mutation eligibility and validation; original transcript safety; and error/fail-open behavior.
- Test with isolated HOME/config directories, ports, stores and subprocesses. Do not modify real Hermes configuration, user transcripts, installed harness settings or running services. Keep ignored harness-binary and live-provider tests explicitly opt-in; a skip is not a pass.
- Record reproducible commands and exact baseline revision, CI/default vs optional test results, and limits. Reuse production code instead of copying cleanup guards, parsers or auth checks into tests.
- **Exit:** no intentional runtime behavior changes; existing tests continue passing, new characterization tests pass, evidence and limitations are documented, and CW-116B/C can proceed in small independent PRs. Do not mark Done based on documentation alone.
- Complete: PR #13 merged into main at `e0c7b5b`.

**CW-116B — CLI and harness modularization (second PR; branch: feat/cw-116b-cli-harness-modularization)**
- Extract command, harness launch, configuration/install and cleanup responsibilities from the large CLI and harness modules into cohesive units. Define only contracts proven necessary for existing Codex, Hermes and OMP flows.
- Preserve public CLI, environment/auth, session identity, installer behavior and all CW-116A contracts. Avoid speculative universal plugin infrastructure.
- Complete: PR #14 merged into main at `b60c7d1`.

**CW-116C — Proxy and transformation boundaries (third PR; branch: feat/cw-116c-proxy-boundaries)**
- Isolate API routing, authentication/session selection, provider forwarding/streaming, mutation orchestration/validation and telemetry responsibilities. Keep mutation policy in cw-core.
- Preserve provider wire bytes, SSE ordering, signed/unknown payloads, mutation decisions and existing API contracts. Require specific safety/regression evidence for each moved boundary.
- Complete: refactoring finished on branch `feat/cw-116c-proxy-boundaries`, report delivered in `docs/CW-116C-REFACTOR-REPORT.md`.

**Non-goals:** new providers or harnesses, new optimizer strategies, performance tuning, cross-crate rewrites, arbitrary line-count targets, schema migrations or broad store/normalizer rework without an evidenced blocker.

**Sequence:** CW-116A -> CW-116B -> CW-116C -> CW-112 -> CW-114 -> CW-115. Each subtask gets its own PR, test evidence and board transition.

---

## 🚧 In Progress

* None (CW-115 qualification complete; PR ready)

## 📋 Ready

* None

---

## 🗂️ Backlog (ordered)

* **CW-103** Broader Codex/Hermes qualification continuation (CW-112 live qualification and CW-114 multi-task completed).
* **CW-104** Hardware and local model qualification.
* **CW-R11** Large-history IO and pagination qualification.
* **CW-R12** Broader task-family and provider qualification.
