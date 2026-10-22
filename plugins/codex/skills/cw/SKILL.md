---
name: cw
description: Inspects and compacts the active LLM context window, KV cache prefix, stale file reads, and noisy tool outputs via contextWatcher (`cw`). Use when context exceeds 80k tokens, health score drops, or the user asks to optimize/compact context.
---

# contextWatcher (`cw`) — Context & KV-Cache Optimization Skill

Supports both `/context-watcher` and `/cw` commands:
- `/cw optimize min` (or `/context-watcher min`) — Safe diet (deduplicates duplicate/stale file reads, 0% KV break)
- `/cw optimize mid` (or `/context-watcher mid`) — Balanced diet (stale files + truncates large old tool & command outputs)
- `/cw optimize max` (or `/context-watcher max`) — Maximum compaction (stale files + tool outputs + old reasoning + folded turns)
- `/cw gain` (or `/context-watcher gain`) — Displays token and dollar ($) savings scoreboard across all sessions
- `/cw status` (or `/context-watcher status`) — Checks active context fullness, KV cache hit ratio, and health score

To run optimization:
1. Primary: call MCP tool `cw_optimize_strategy` with `strategy_id`: "min" | "mid" | "max" and pass `"sync_to_disk": true` so resumed sessions inherit the compacted transcript.
2. CLI Fallback: if the MCP tool is not available in the current session, run `cw optimize -s min|mid|max --sync-back` via shell (`~/.local/bin/cw optimize -s <level> --sync-back`).
