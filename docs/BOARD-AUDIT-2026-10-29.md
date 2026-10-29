# Board versus current implementation

Audited against the current working tree and evidence in `docs/REMEDIATION.md`, not the board's prior claims or historical benchmark reports. The board’s checked items remain useful as a record that code was once added. Several descriptions overstate current behavior. None should be read as a release certification.

## Implemented and currently exercised

- Rust workspace, CLI, SQLite store, proxy, transcript parsers, MCP, TUI, Svelte app, and IntelliJ plugin exist.
- Local control authentication, strict session ID validation, request sequence allocation, immutable original request storage, export copies, revision checks, and default-off active policies exist.
- Core mutation operators exist. They now require stronger target identity, preserve signed payloads and unread/file content, and only deduplicate a successfully covered read with matching contents.
- Proxy tests exercise original byte passthrough, session isolation, approval timeout, malformed preview rejection, concurrent request recording, split SSE usage and stale revisions.
- Store tests cover immutable originals, concurrent sequence allocation, revision-checked writes, migration snapshots, purge behavior, complete enumeration above 200 sessions, and same-sequence snapshot revisions.
- Core tests cover signed payload opacity, target identity, range-aware read coverage, text arrays and metadata, Responses parsing and cache uncertainty.
- Current local verification: `cargo test --workspace`, strict Clippy, `cargo fmt --all -- --check`, web build/check and `git diff --check` passed after the last code changes.

## Board claims that need narrowing

| Ticket(s) | Board wording | Current state |
|---|---|---|
| CW-003, CW-006, CW-013 | Lossless normalizers, zero-drop transparency and exact cache diff | Original wire bytes pass through observation; analysis and token spans are partial estimates. Unknown, multimodal, stateful Responses and signed shapes are observation-only. Unknown cache state is not a cache hit. |
| CW-008–011, CW-035–037 | Broad harness watcher/wrapper support | Parsers/importers exist for several formats. OMP v18.12.6 passed a local mock-upstream check for authenticated session association and overlay reapplication. Codex does not transmit the injected Anthropic headers and remains observation-only. Aider Markdown import and in-place sync are unsupported. OpenCode/Hermes DBs are explicit imports, not live watchers. |
| CW-020, CW-029, CW-030 | Exact proportional token graphics and cache-safe prompt-tail pinning | Node spans, attention and cache-break values are estimates. Pins can change the prefix. `min` currently selects no mutation when no exact boundary exists. |
| CW-026, CW-031–033 | Generic folds, automatic truncation and A/B replay | Folds need user-supplied summaries; the generated generic marker is not a factual summary. Firewall approval works on tested mock paths. Replay is a local preview and does not call a model or record a provider comparison. |
| CW-041 | Safe in-place write-back with backups for six harnesses | `sync-back` writes a separate private export. The source transcript remains unchanged. Only exact eligible tool-result edits map to supported JSONL exports; no live writer/reload boundary is proven. |
| CW-044, CW-051, CW-054–055 | Zero-touch self-healing, spill files and transcript sync | Safety defaults disable automatic mutation, `/tmp/cw_logs` spills and disk sync. Legacy config fields remain for compatibility. Limited auto proposal/config plumbing exists; no zero-touch claim is supported. |
| CW-045, CW-049, CW-053 | Live cost, TTFT and per-mutation effectiveness | Provider usage and observed first-body-byte time are stored separately. Cost, prefill, cache and node-span math remain estimates; overlapping per-rule savings are unattributed. No measured speedup is claimed. |
| CW-046 | Incremental watchers across all harness directories and large-history graph | Registered JSONL sources are reparsed as bounded snapshots on blocking workers. There is no validated incremental cursor or global autodiscovery. API session and turn lists are paginated. |
| CW-047, CW-070 | Automatic engine settings, slot affinity and physical page unmapping | Slot discovery may query the engine. Calculations and splice actions are previews; execution returns unsupported. Slot exclusivity and physical VRAM reclamation are not proven. |
| CW-048 | Provider schema validation and universal fail-open guarantee | Local checks cover a subset. Unsupported mutations fall back to original payload; approval failures block. This is not a full schema or signature validator. |
| CW-052 | Zero-panic, JSON-RPC 2.0 and atomic replacement | MCP is stdio tool protocol, not JSON-RPC 2.0. There are still non-fatal fallback reads in observer paths. Export uses unique create-new files, not source replacement. |
| CW-060–061, CW-075, CW-085 | Exact swarm VRAM savings and hardware benchmark results | Grouping and old reports exist, but reported VRAM and speed figures are historical estimates/results from older code. They are not current qualification evidence. |
| CW-080 | Empirical attention curve and sub-millisecond guarantee | Attention values are a model heuristic; no empirical adherence or latency guarantee is established. |

## Remaining work

- Qualify real-provider behavior and verify each additional harness transport before enabling overlays there. The OMP Anthropic path passed a local mock-upstream wire check; Codex remains observation-only until a safe documented transport is verified.
- Qualify browser interaction and IDE runtime behavior; plugin build alone is not runtime validation.
- Add incremental, rotation-aware transcript cursors and measure large-history storage/UI costs.
- Expand real provider fixtures for signatures, errors, multimodal and streaming edge cases.
- Run independent task types and repeated latency samples before broad quality or speed claims. The three current continuations repeat one task template.
- Decide whether disabled legacy features should be deleted or exposed as explicitly unsupported, then revise remaining board ticket text accordingly.
