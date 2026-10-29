---
description: Inspect and compact active context window, KV cache, and stale files via contextWatcher (`cw`)
---

Execute contextWatcher optimization with arguments: `$ARGUMENTS`.
- If `$ARGUMENTS` starts with `optimize` or `opt` or is `min`, `mid`, or `max`: call tool `cw_optimize_strategy` with `strategy_id` set to `$ARGUMENTS` (defaulting to `"mid"`) and `export_compacted: true`.
- If `$ARGUMENTS` is `gain`: run `cw gain` via Bash and display the token & cost savings scoreboard.
- If `$ARGUMENTS` is `status`: call tool `cw_context_status`.
