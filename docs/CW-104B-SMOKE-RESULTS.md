# CW-104B reserved-window smoke result

**BLOCKED_WITH_EVIDENCE.** A genuine local M1/M2 coding pair completed with both tasks accepted, but the treatment was invalid: Context Watcher appended the pin to tool-output tails on all five M2 follow-ups. These results do not qualify the intended user-text-only PinContext treatment or establish product value. The defect is fixed and deterministically checked locally; no replacement live pair was launched.

## Observed coding outcomes — invalid treatment

| Metric | M1 | M2 |
| --- | ---: | ---: |
| Independent functional/constraint acceptance | True | True |
| Coding seconds | 79.23 | 94.44 |
| Provider requests | 6 | 6 |
| Input tokens | 173473 | 176316 |
| Output tokens | 2590 | 2434 |
| Cached/reused tokens | 172196 | 167644 |
| Prefill milliseconds | 12079.1 | 34335.9 |
| Decode milliseconds | 64809.5 | 57327.7 |
| Tool calls | 8 | 9 |
| Tool errors | 0 | 1 |
| Missing usage | 0 | 0 |
| Compaction events | 0 | 0 |

M2 was 15.21 seconds slower (+19.19%) and consumed 2,843 more input tokens (+1.64%). Output was 156 tokens lower. These are observations from one invalid-treatment pair, not a causal optimization result. Both independent tests and the original public tests passed. M2 attempted an unavailable `bash` tool; OMP returned `Tool bash not found`, and no shell command ran. That is an observed model/harness error, not established as caused by Context Watcher.

## Cache and execution integrity

The user reserved Strata. Temperature 0/top-p 1/seed 104002, model identity, exact initial fixture/history equality, and first-request full-payload difference (one committed pin only) were verified. The actual first requests reused exactly 27,929 tokens in both arms, corroborated by provider usage and server records. All 12 coding dispatches have usage and server timing; there were no observed transport failures or provider retries. Raw provider/proxy payloads agree. Native transcripts do not contain injected pin blocks. No model-server setting, cache reset, restart, cloud call, or automatic full campaign occurred.

A previous warm-up attempt stopped after M1's first response because the exact question suffix had been warmed, adding 14 cached tokens. Its response, 130 output tokens, runtime cost, and pristine workspace are retained in `reserved-window-diagnostic.json`. Neutral-suffix priming before each arm corrected that initial-state mismatch. The completed pair finished about 488 seconds after the original phase start, within the original 60-minute cap. Bootstrap/cache-diagnostic records are separate from coding metrics in the saved run; the full phase includes setup and the failed preparation. A complete historical eight-hour campaign was not started.

## Defect and correction

`inject_pinned_context_at_tail` appended to any `messages` tail with string/array content without checking its role. Native `toolResult` content was then flattened into provider `tool` strings, defeating an array-block-only verifier. The pin disappeared from the previous tool message and reappeared on the next tail, so M2 request prefixes changed. M2 reused only the common early prefix on later requests while M1's active prefix advanced. This observed prefix instability is consistent with the defect; its exact contribution to runtime is not isolated by this pair.

The local fix permits only known user-text tails in the `messages` path. Non-user and tool-result tails abstain unchanged. Positive user-string/text-array behavior remains covered. A regression failed before the fix on a real-shaped `role: tool` message, then passed after correction. An existing pin-deduplication test was strengthened to assert abstention on its tool tail and exercise deduplication after a real user continuation. No new mutation operator or provider integration was added.

Validation: default Rust workspace suites passed (243 passed, 19 ignored); Clippy and formatting passed. Native OMP/CW mock verification now executes a real read tool and proves the entire M1/M2 follow-up payloads are equal while initial M2 differs by exactly one pin. The verifier and relay now inspect pin strings as well as array blocks. Ignored harness/live suites were not claimed as passed. The patched implementation has not received another live coding pair.

## Durable evidence and limits

Saved run: `.audit/cw104b/smoke-20261103T134708Z.json`. Corrected audit: `smoke-20261103T134708Z-audit.json`. Pin locations: `pin-location-audit.json`. Deterministic native check: `followup-mock-preflight.json`. The preliminary READY result is explicitly superseded because its verifier missed string-shaped tool pins. Private raw artifacts remain in the owner-local evidence vault; their hashes are in the corrected audit. GPU snapshots are point observations, not energy measurements or proof of exclusive device scheduling. Full loaded-weight hash and seed reproducibility remain NOT_VERIFIED. The OMP thinking-off flag did not transmit an explicit server `enable_thinking=false`; both arms used the same recorded settings and returned thinking blocks.

The same location audit found tool-output pins in all six historical M2 arms; recovered metrics remain preserved, but those arms also fail intended-treatment qualification. See `recovered-pin-role-audit.json`.

Next decision: review the narrow correctness fix and authorize one replacement bounded pair with the corrected implementation and strict pin-location guard. Keep the tasks/pin frozen; do not launch the full campaign automatically. The later campaign plan remains in `CW-104B-EXPERIMENT-PROTOCOL.md`.
