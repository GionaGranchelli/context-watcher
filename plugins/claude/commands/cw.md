---
description: Inspect and optimize active context window, KV cache, and stale files via contextWatcher
---

Run `cw status` and `cw lint` to inspect the current session's context health, KV cache hit ratio, and any stale or duplicate file reads. If lint warnings are present, call the `cw_evict_stale_reads` or `cw_fold_turns` MCP tools (or run `cw lint --fix`) and report the tokens saved.
