---
name: context-watcher
description: >
  Inspect, evaluate, and surgically optimize the active LLM context window and KV cache
  using contextWatcher (cw). Use when the context window grows large, when stale or
  duplicate file reads accumulate, after a multi-turn debugging rabbit hole, or when the
  user asks to check context health, KV cache hits, fold turns, pin constraints, or gate
  MCP tools.
---

# contextWatcher (`cw`) Skill for Antigravity (AGY)

You have access to `contextWatcher` (`cw`) via both the **`contextWatcher` MCP Server (`cw mcp`)** and the **`cw` CLI**.

## 1. When to Self-Check & Clean Your Context

1. **After a Multi-Turn Debugging Loop (`cw_fold_turns`):**
   * Once you finish fixing a bug that took 3+ failed commands or stack traces, call `cw_fold_turns` (or `cw fold <start> <end> --summary "..."`) so the failed intermediate turns are collapsed into a single outcome summary on subsequent turns.
2. **After Editing Files You Read Earlier (`cw_evict_stale_reads`):**
   * Whenever you modify a file with `edit_file` or `write_file` that was read earlier in the conversation, call `cw_evict_stale_reads` (or `cw dedupe`) so older stale `read_file` blocks are replaced with lightweight pointers.
3. **Preserving Critical Invariants (`cw_pin_context`):**
   * Use `cw_pin_context` to pin critical architectural rules or file/line references to the high-attention tail of the prompt without invalidating the KV cache prefix!
4. **Reducing Tool Schema Overhead (`cw_gate_tools`):**
   * If `cw_context_status` reports `CW-L005` (MCP Tool Schema Bloat), disable unused MCP servers for the current session via `cw_gate_tools`.

## 2. Quick CLI Commands

```bash
cw status                # Check token fullness, KV cache hit %, and health score
cw tree                  # View ASCII Radix Prefix Tree of current context
cw lint --fix            # Diagnose and auto-fix stale reads, duplicates, and cache breaks
cw dedupe                # Shadow-deduplicate all stale/duplicate file reads
cw fold 4 9 --summary "Resolved SQLite lock contention in cw-store"
cw pin "Invariant" "Never break tool_use / tool_result pairing"
```
