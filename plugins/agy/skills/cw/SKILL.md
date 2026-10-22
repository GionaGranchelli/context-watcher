---
name: cw
description: Inspects and compacts the active LLM context window, KV cache prefix, stale file reads, and noisy tool outputs via contextWatcher (`cw`). Use when context exceeds 80k tokens, health score drops, or the user asks to optimize/compact context.
---

# contextWatcher (`cw`) — Context & KV-Cache Optimization Skill

Supports the `/context-watcher` and `/cw` command suite:
- `/cw optimize min` — Safe diet (deduplicates duplicate/stale file reads, 0% KV break)
- `/cw optimize mid` — Balanced diet (stale files + truncates large old tool & command outputs)
- `/cw optimize max` — Maximum compaction (stale files + tool outputs + old reasoning + folded turns)
- `/cw gain` — Displays token and dollar ($) savings scoreboard across all sessions
- `/cw status` — Checks active context fullness, KV cache hit ratio, and health score

To run optimization: call MCP tool `cw_optimize_strategy` with `strategy_id`: "min" | "mid" | "max" and pass `"sync_to_disk": true` so resumed sessions inherit the compacted transcript.
