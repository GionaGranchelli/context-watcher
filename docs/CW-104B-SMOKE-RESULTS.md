# CW-104B smoke results

**READY_FOR_FULL_LOCAL_CAMPAIGN — experimental readiness only.** The separately authorized patched pair completed and passed all measurement, intervention, functional, and constraint gates. It shows no measured product benefit: both arms passed, while M2 used more time, requests, and input/output tokens. One pair cannot establish a reliable advantage or disadvantage across tasks. No full campaign is authorized or running.

## Patched pair — 2026-11-03 14:09 UTC

| Metric | M1 baseline | M2 PinContext | M2 minus M1 |
| --- | ---: | ---: | ---: |
| Coding wall seconds | 59.23 | 93.24 | +34.01 (+57.43%) |
| Provider requests | 5 | 6 | +1.00 (+20.00%) |
| Input tokens | 144,272 | 174,188 | +29,916.00 (+20.74%) |
| Output tokens | 1,850 | 3,236 | +1,386.00 (+74.92%) |
| Cached/reused tokens | 142,735 | 171,395 | +28,660.00 (+20.08%) |
| Prefill milliseconds | 13,303.60 | 15,111.20 | +1,807.60 (+13.59%) |
| Decode milliseconds | 43,815.20 | 75,460.60 | +31,645.40 (+72.22%) |
| Tool calls | 8 | 7 | -1.00 (-12.50%) |
| Tool errors | 0 | 0 | +0.00 |

Both arms passed the original public tests and independent checks of missing/falsey cache behavior, writes to both stores, public signatures, and protected-file bytes. Both had zero compaction events, zero missing usage records, zero observed transport failures, and zero provider retries. The one observed pair therefore provides no correctness/reliability improvement. The fixtures and genuine historical-review reference are a narrow retention smoke test, not production workload qualification.

The extra 34.01 coding seconds in M2 are largely accounted for by 31.65 seconds of additional decode time; prefill increased by 1.81 seconds. M2 generated 1,386 extra output tokens and made one extra provider request. The additional input is mainly consistent with repeating the existing context for that extra call, not a mechanical change in retained context. PinContext is append-only here and removed no historical tokens. This is an observed decomposition, not an isolated causal estimate of PinContext's runtime cost.

## Validity and cache checks

Both arms used the same initial source bytes, frozen early brief/history, static system prompt, tool definitions, local model, temperature 0/top-p 1/seed 104002, and CW transport. Only initial M2 received the exact committed directive. Full initial payload comparison passed; all proxy wire payloads match relay captures. The strict audit checks string-shaped content as well as text blocks. No pin occurs in any tool output, and original native transcripts remain unmodified by injected directives. M2's first tool-only continuation drops the transient initial user-tail overlay; subsequent tool-only requests preserve prior message content. This narrow implementation does not promise a persistent directive on every tool-only continuation.

Actual initial provider contexts were 27,948 tokens (M1) and 27,992 (M2), with exactly 27,929 reused tokens in each. Neutral-suffix preparation gave equal initial reuse, corroborated by provider usage and server records. Within-arm cached reuse was measured rather than inferred from prefix text. The full phase, including mock checks, shared warm-up, and both arms, lasted 208.42 seconds, below the new 60-minute cap. Cache diagnostics and coding usage remain separate in the raw run. No independent model bootstrap was generated.

The same server configuration was retained: enabled cache, 16,384 MiB budget, eight slots. Request counters advanced once per measured request with idle/empty boundaries. The server was idle after completion. No cloud calls, server configuration changes, cache clears, service restarts, or additional harness integrations occurred. GPU snapshots are point observations, not energy measurement. Full weight-file identity and seed reproducibility remain NOT_VERIFIED. Both arms transmitted the same thinking configuration; thinking was not proven disabled server-side.

## Evidence and next decision

Authoritative run: `.audit/cw104b/smoke-20261103T140929Z.json`; independent audit and raw hashes: `smoke-20261103T140929Z-audit.json`; implementation/model-config context: `patched-smoke-context.json`; prefix progression: `patched-prefix-progression.json`. Raw payloads, SSE, native transcripts, and completed workspaces are retained privately with hashes. Re-run the offline audit with:

```sh
python3 -B benchmark/cw104b/audit_smoke.py .audit/cw104b/smoke-20261103T140929Z.json
```

The audit exits nonzero for an invalid treatment. Default Rust tests, Clippy, formatting, and native follow-up mock checks already passed for the unchanged code fix; GitHub CI passed at `efbe279`. Ignored/live suites are not represented as default CI coverage.

The prerequisite work is complete. The later two-family counterbalanced plan is recorded in `CW-104B-EXPERIMENT-PROTOCOL.md`; executing it remains a separate approval decision. Given equal correctness and worse resource use here, the next campaign should test genuinely discriminating early constraints rather than assume a benefit or expand infrastructure. Do not pool this valid patched pair with historical tool-pinned treatments.

## Earlier unpatched attempt — preserved history

The following records describe the earlier invalid attempt before the separately authorized patched pair. Their measurements and failures remain unchanged; their then-current stop status is superseded by the valid result above.

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
