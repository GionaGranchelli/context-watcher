# CW-122B2-L1 — Live pruning value campaign

Status: runner preparation. No campaign authorization is issued and no campaign calls are enabled.

## Outcome being tested

Does one committed historical `PruneOrStub` reduce complete-session input and provider-calculated cost for real coding tasks while preserving independent task acceptance and constraints?

The direct live probe showed that one prune reached GPT-6 Luna and settled. It did not include M1, a coding-task oracle, or a complete agent session. Its estimated 1,768 pruned tokens are not measured savings.

## Frozen design and decision rule

Use the six matched pairs and task order in [CW-122A preregistration](CW-122A-PREREGISTRATION.md): three F1 retry-policy tasks and three F2 storage/TTL tasks, with M1/M2 order counterbalanced. Each arm gets a fresh workspace, session, home, data directory, and transcript source. Only M2 gets one committed prune of an eligible consumed command result. The source tree, prompts, model, pricing snapshot, and sampling settings stay frozen.

The primary outcome is successful completion with every independent functional and constraint oracle passing. Secondary outcomes are complete-session cost per successful task, total/input/output/cached/cache-write tokens, request and retry counts, wall time, and provider timing where available. Bootstrap, coding-only, and whole-session totals stay separate. Estimated pruned tokens stay separate from provider usage.

The preregistered positive pilot signal requires all six pairs valid, at least three per family, all twelve tasks accepted, no abort or new M2 acceptance failure, median input and cost each at least 5% lower, neither family's median regressing, and no pair with a cost inversion. That is only a small pilot signal, not a general or significance claim. Negative and inconclusive outcomes follow the preregistration without retuning after observing results.

Cache telemetry must preserve actual read/write categories. Cache-state equality or isolation needs independent evidence; otherwise label economic pairs cache-confounded. OpenAI caching depends on exact rendered-prefix matches and cannot be manually cleared, so hashes alone are not proof of equal cache state.

## Execution phases

1. **Baseline and feasibility.** Keep PR #45 work isolated. Verify the frozen files and current Codex version. Use loopback mocks to prove each fresh arm can independently build the same frozen fixture and instructions; do not require generated bootstrap history to match, because that history is an outcome of the treatment.
2. **Live adapter.** Reuse the CW-122B1 Codex process controls, CW-122B1-F1 fault controls, CW-122B2-P pricing and telemetry, CW proxy, and independent CW-122A oracles. Capture a real tool result, wait for an assistant response that consumes it, then commit exactly one M2 prune before the coding continuation. M1 remains passthrough.
3. **Offline gates.** Default to NO-GO. Bind runner, test, Codex, model, endpoint, pricing, campaign, credential-file hash, deadlines, request caps, and evidence paths. Test missing/expired/reused/mismatched authorization, model/tier mismatch, redirects/fallback, missing telemetry, request exhaustion, timeout, interruption, and evidence preservation against mocks. Require one shared 360-request SpendGuard across all twelve sessions and attempts.
4. **Review and authorization.** Produce a sanitized report and exact spend envelope. The envelope must disclose reservation-overrun risk. No live campaign starts until independent code review and a separate signed-off 12-run authorization pass.
5. **Campaign and analysis.** Run serially, preserve every failure, stop on safety/model/treatment/telemetry mismatch, and analyze all attempts plus clean-transport diagnostics. Do not replace failed runs or fill missing telemetry with zero.

## Completion predicates

**Runner ready:** the offline adapter and fault gates pass, the exact authorization validates only the frozen selection, all provider access is closed by default, and local analysis reproduces the frozen decision rules.

**Pilot evidence complete:** all twelve authorized attempts have authoritative raw wire, provider usage, settlement, Codex transcript, and independent acceptance evidence, or the run stops with a preserved negative/inconclusive result. The product-value conclusion must use the frozen decision rule.

## Current blockers

- `benchmark/cw122b2p/authorization-contract.json` currently sets `live_runner_callable` and `authorization_issued` to false.
- The frozen traces are scripted; live M2 must select a genuinely generated and consumed tool result. This is a runner feasibility gate, not something to assume from mock success.
- A provider-side hard spend ceiling is unavailable. SpendGuard reserves before a call and settles after it; the operator must explicitly accept the possible overrun.
- Provider cache state cannot be inferred from matching payload hashes. Unknown state makes cache economics confounded.

## Existing real-provider evidence

The private evidence bundle now includes an exploratory direct-request pair. Both arms used the same 4,978-byte ingress (`763e77bb…af792`); M1 egress was byte-for-byte passthrough, while M2 changed only `/input/2/output` to the configured prune stub. GPT-6 Luna completed both responses and returned the requested marker.

| Measure | M1 | M2 | M2 minus M1 |
|---|---:|---:|---:|
| Provider input tokens | 1,049 | 270 | -779 (-74.3%) |
| Output tokens | 91 | 252 | +161 |
| SpendGuard settled cost | $0.000177 | $0.000153 | -$0.000024 (-13.6%) |

At the saved standard rates, M1's reported input categories cost $0.00013105 (3 uncached tokens and 1,046 cache-write tokens) and its 91 output tokens cost $0.00004550. M2's 270 uncached input tokens cost $0.00002700 and its 252 output tokens cost $0.00012600. Per-request rounding yields the recorded settlements. The input reduction therefore exceeded M2's additional output cost in this pair; this is a single observed request calculation, not an invoice reconciliation.

This is evidence that the tested prune reached the real provider and reduced measured input and request-level settled cost for this one request pair. It is **not** a product-value result: the arms were exploratory, the test-driver source differed, responses were checked only for a marker rather than the independent coding-task oracle, and cache state was not independently established. M1 reported 1,046 cache-write tokens; M2 reported none. The proxy's 1,768-token estimate is not measured savings. The six-attempt record also retains an incomplete M2 response, a later M1 cache-read observation, and an HTTP 400 with no usage telemetry.

The same private evidence also contains a second M2 attempt with the same ingress and the same targeted egress pointer, but an incomplete response at the 512-token output cap. It used 74.26% fewer input tokens yet settled at `$0.000283` versus M1's `$0.000177` because output reached 512 tokens; its task was not accepted. The sanitized hashes and wire-level recheck are recorded in [same-ingress-incomplete-observation.json](../.audit/cw122b2-l1/same-ingress-incomplete-observation.json). Driver equivalence and cache-state comparability are `NOT_VERIFIED`, so retain this as an adverse exploratory observation, not a matched campaign result.

The sanitized repository contains no raw provider payloads. Private raw artifacts and their file hashes are at `$HOME/.local/state/context-watcher/cw122-live-prune/`; current manifest SHA-256 is `39b3041f27e49681a805fdad70df42e0f1a8a290fe12b6c6d4b8d2fb7da913d9`. Its pre-reconciliation version is preserved as `manifest-pre-reconciliation.json` with SHA-256 `b8f19f57e162050f17287664c9b30e691b421c9cf20b28b7ba789605ccacc733`.

## Live-adapter feasibility finding

The existing `test_cw122b1_codex_mock_campaign` cannot be switched to OpenAI by changing its upstream URL. Its `MockProvider` synthesizes the `cw122b1_probe` tool call, the `Inspection complete.` reply, the `cw122b1_patch` call, and fixed usage. The campaign then relies on those call IDs and synthetic command results. `CodexStep` also hard-codes `gpt-4o-mini`. Therefore, relabeling this campaign or only swapping the endpoint would still test a scripted model. A real L1 runner needs a separate provider-backed bootstrap and response capture path, while reusing the fixture/oracle, Codex process isolation, bounded deadlines, and shared SpendGuard. The contract correctly remains `live_runner_callable: false` until that path is implemented and mock-gated.

## Offline gate results on this working tree

- CW-122A, CW-122B1, and CW-122B2-P Python unit suites passed: 20, 15, and 13 tests respectively.
- CW-122B2-P offline replay passed but correctly found zero economically eligible pairs and `product_value=NOT_EVALUATED`.
- On a clean checkout of committed candidate `64c5115`, the CW-122A fixture rehearsal passed its source freeze and proxy suite: six synthetic pairs, `product_value=NOT_EVALUATED`, no inference. Its saved manifest SHA-256 is recorded in `/tmp/cw122a-l1-head-rehearsal/manifest.json` (`853a1ee9…5a5b32`).
- The same rehearsal stopped at the source-freeze check in the working tree. Exactly three frozen files differ only because of uncommitted L0 edits: `spend_guard.rs`, proxy `mod.rs`, and `cw122_pruning.rs`. Do not refresh the historical CW-122A freeze to hide those differences; rerun against the reviewed, committed L1 source bundle.
