---
name: cw
description: Inspects and compacts the active LLM context window, KV cache prefix, stale file reads, and noisy tool outputs via contextWatcher (`cw`). Use when context exceeds 80k tokens, health score drops, or the user asks to optimize/compact context.
---

# contextWatcher (`cw`) — Context & KV-Cache Optimization Skill

Supports both `/context-watcher` and `/cw` commands:
- `/cw optimize min` (or `/context-watcher min`) — Safe 0-KV-break diet (uncached duplicate reads/tail bloat + rule reminders; use `clean_code_state` for full covered read deduplication)
- `/cw optimize mid` (or `/context-watcher mid`) — Balanced diet (compacts old tool & command outputs and unsigned thinking blocks)
- `/cw optimize max` (or `/context-watcher max`) — Maximum compaction (covered read deduplication + old tool outputs + unsigned thinking + rule reminders; use `cw_fold_turns` with an explicit summary to fold turns)
- `/cw gain` (or `/context-watcher gain`) — Displays token and dollar ($) savings scoreboard across all sessions
- `/cw status` (or `/context-watcher status`) — Checks active context fullness, KV cache hit ratio, and health score

To run optimization:
1. Primary: call MCP tool `cw_optimize_strategy` with `strategy_id`: `"min"` | `"mid"` | `"max"` | `"clean_code_state"` and pass `"export_compacted": true` (`"sync_to_disk": true` is a legacy alias) to apply session overlays and export a separate compacted `.jsonl` copy (leaving the source transcript unchanged).
2. CLI Fallback: if the MCP tool is not available in the current session, run `cw optimize -s min|mid|max --sync-back` via shell (`~/.local/bin/cw optimize -s <level> --sync-back`).
