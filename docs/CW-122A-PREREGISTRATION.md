# CW-122A: historical-pruning-only preregistration

Status: frozen offline design for review. No live campaign is authorized. [Machine configuration](../benchmark/cw122a/campaign.json), [source freeze](../benchmark/cw122a/freeze.json), and [normalized telemetry schema](../benchmark/cw122a/telemetry.schema.json) define this revision. Changes after observing live outcomes require a new revision and separate reporting.

## Question and modes

Does historical pruning reduce complete coding-task input and calculated cost while preserving functional correctness and constraints? Historical CW-115 results motivate this question, but do not answer it. The earlier four-mode report's Mode 0 meant proxy passthrough; its numbering is not this campaign's M0. Historical data is not pooled with new observations.

| Mode | Route and intervention |
|---|---|
| M0 | Optional native agent direct to provider, without CW. Qualify routing, telemetry, request admission and timeout separately before including it. Not in the six-pair authorization scope |
| M1 | Same agent, task, initial tree, instructions and provider through CW. Master mutation switch off; no session rules or automatic transformations |
| M2 | Same setup through the same proxy. Only one committed `PruneOrStub` on an eligible, consumed historical command result. Master mutation switch on solely to apply that rule |

In M2, automatic pinning, tail hygiene, diet, window guarding, low-health healing and transcript sync are explicitly off. Pin consent remains off; no PinContext, gating, normalization, dedupe, folding, summarization or behavioral instruction injection is allowed. A foreign-session rule in the offline negative control must never reach the request. The intended stub is fixed in the task manifest and reports the failed historical test run without supplying a solution.

The source tool result must be completed and followed by an assistant response before treatment. Current unread results and all file-read contents remain protected by the existing engine. Runtime normalization, mutation eligibility, selectors and protocol validators are reused unchanged. No new pruning strategy or cache scheduling is introduced.

At the upstream boundary require full payload equality to the original in M1, and full equality to the expected single-path replacement in M2. Verify exact applied IDs and persisted egress, preserved call/result IDs and every unrelated field. A committed rule or smaller local estimate alone is insufficient. M2 with no actual change is invalid, never a successful treatment. Signed, unknown, stateful, current-unread and protected-file-read cases must abstain byte-for-byte unchanged. Invalid outcomes and their time/usage remain recorded.

## Matched matrix and coding tasks

| Pair | Task | Order |
|---|---|---|
| f1-r1 | Retry cap and validation bug | M1 then M2 |
| f1-r2 | Same frozen F1 fixture | M2 then M1 |
| f1-r3 | Same frozen F1 fixture | M1 then M2 |
| f2-r1 | Namespace and exact-TTL cache repair | M2 then M1 |
| f2-r2 | Same frozen F2 fixture | M1 then M2 |
| f2-r3 | Same frozen F2 fixture | M2 then M1 |

F1 fixes exponential retry delays, capped growth and invalid argument handling without changing the public signature. F2 repairs namespace isolation and exact expiry across storage/service files, preserves falsy values and public signatures, rejects negative TTL and retains the injected clock architecture. Oracles are outside the agent-editable tree and test functionality, signatures, file allowlists, forbidden imports, and the task’s no-runtime-manipulation/no-filesystem-access constraint. The parent requires a completed four-test oracle record, not exit code zero alone. These limited fixture checks are not a general Python sandbox. Initial broken fixtures must fail; predefined offline repairs must pass; altered policy/signatures must fail independently of usage.

These are bounded source-tree fixtures, not proof of production workload representativeness. The frozen traces combine genuine initial acceptance-failure logs with explicitly scripted assistant/tool history. They are sanitized fixture traces, not captured live agent sessions. Their assistant-consumption boundary is known by construction. The live campaign must capture and qualify actual settled history before selecting a candidate; no live result may inherit the synthetic eligibility assertion.

Every arm uses a fresh isolated workspace, home/config/data, session ID, ledger and transcript source. Hash initial file contents and exact instructions before any agent execution. Restore from the frozen tree, never from the preceding arm. Run arms serially without concurrent benchmark interference. Predeclare the model/revision, provider endpoint, agent/runtime version, operating environment and pricing snapshot in the fresh authorization manifest. Record temperature 0 and top-p 1 where supported; record unsupported settings explicitly. An explicit seed is not selected. Residual backend, scheduling and sampling nondeterminism remains.

Equal initial cache conditions must be demonstrated rather than assumed: use qualified independent cache/session state or record the comparison as cache-confounded. Do not reset caches or production servers automatically. Warm-up is either identical and separately accounted for in both arms, or omitted in both; bootstrap, task-only and complete-session scopes must be distinct in a future multi-turn runner. Do not infer cache preservation from prefix shape. Historical pruning changes a prefix and may increase uncached work. CW-123 scheduling is outside this revision.

## Telemetry and estimands

The run schema includes task/pair/order/repetition, canonical unique session and attempt ID, initial repository/configuration hashes, agent/model/runtime version, sampling, status/failure reason, independent acceptance, per-request raw usage and its provenance, first-byte timing, duration provenance, full request transformation and applied/skipped ledger records. Input/output/cache counts are nullable. Never replace a missing count with zero.

| Category | Interpretation |
|---|---|
| Measured in a future live run | Explicit provider usage; captured request count; monotonic complete-task duration; first-byte latency where available |
| Calculated | Uncached input = total minus explicitly reported cached input; cost from complete usage and a versioned pricing snapshot, not an invoice |
| Estimated | Local token reduction and predicted KV-break impact. Provider token reduction for a pruned fragment is not directly observed here |
| Unavailable | Missing cached or partial usage, unobserved timings, unknown runtime versions. Use null plus provenance/reason |
| Synthetic rehearsal | Mock response usage, predefined task repair and frozen 1000 ms duration. None are live economic, latency or agent-quality measurements |

Complete-session effects include all bootstrap/task requests, retries, failures and output tokens under their scopes. Mechanical request-level reduction compares the frozen original and transformed request and remains a local estimate. Fewer requests or behavioral divergence are separate mechanisms; do not attribute total-session savings mechanically to the removed fragment. TTFB is never labeled time-to-first-token. The one-request offline rehearsal does not qualify live bootstrap segmentation or multi-request timing collection.

For CW-122B, a pricing snapshot must state provider/model, currency, effective date, official source and input/cache/output unit rates. Confirm any cache-write token semantics to avoid double counting. Unknown prices or incomplete usage invalidate cost qualification. The rehearsal's `synthetic-test-rates-v1` is deliberately unsuitable for live analysis.

## Analysis and decision rules

Analyze M2 minus M1 within each preregistered pair. Report absolute and percentage differences for total/cached/uncached input, output, calculated cost, requests, duration, and estimated mechanical/KV effects. Report acceptance differences and all failure/abort reasons. List invalid, incomplete and missing pairs. Do not match an unmatched attempt to another repetition or pool different repository/configuration/runtime/sampling conditions.

Report all-attempt outcomes, including failed-run resource costs where observed. The separately labeled complete-valid-pair summaries are diagnostic and do not remove failures from reliability reporting. A partial usage total is unavailable, not a zero-cost failure. Report per-family and overall means, medians, range, sample standard deviation and mean standard error, with the contributing n. Three pairs per family cannot support a population or significance claim. A descriptive pilot signal is not product qualification.

- **Positive pilot signal:** All six pairs valid, at least three per family, all twelve tasks accepted with no aborted run or added M2 acceptance failure, median input and cost changes at most -5%, neither family's median regresses, and no pair has a cost inversion. Even then, claim only a small matched-pilot signal pending larger qualification.
- **Negative:** M2 adds an acceptance failure, or complete matched evidence shows median input/cost regression. Preserve the result and investigate, without retuning this experiment after seeing outcomes.
- **Inconclusive:** Missing/invalid/confounded pairs, unavailable telemetry, or effects failing the thresholds above. Preserve zero benefit and adverse cache observations.

A cost increase receives a visible inversion warning even if tokens decreased. Cached-token loss alongside higher cost is reported as association, not proof of causality. Equal acceptance in both arms does not establish improved reliability. Synthetic reports always mark product value `NOT_EVALUATED` regardless of their rehearsed decision.

## Fresh CW-122B admission requirements

No inference is callable from CW-122A. Its `--live` path refuses before execution. `admission.py` only validates and single-use-claims a record, reusing CW-104B's durable exclusive marker. It cannot dispatch a provider request. The example record is invalid and authorizes nothing.

A fresh operator record must identify CW-122B, the exact provider/model/endpoint, current campaign and runner hashes, approval identity/reference, issue time and expiry within 24 hours, exactly 12 M1/M2 runs, at most 360 total requests, at most 7200 seconds, and an explicit positive spend ceiling. Per-run limits remain 30 requests and 600 seconds. Earlier campaign approvals are rejected. Credentials must be a separate regular 0600 file and evidence a private 0700 directory outside the checkout. Do not put secrets in agent environments, transcripts or committed evidence. This is an operator attestation, not cryptographic identity proof.

Before execution, the CW-122B adapter must bind these approved limits to actual subprocess/HTTP admissions, attach the production `cw_core::spend_guard::SpendGuard` to every proxy route, retain all raw request/response usage and settlement outcomes, stop on missing telemetry or inconsistent treatment/model identity, and enforce a campaign-wide serialized request/run/deadline counter. Per-session guards alone do not enforce the global request or run count. The frozen credential isolation and no-fallback route must be independently exercised against mocks using the selected harness/version. These live adapter, native routing and environment gates are **NOT_VERIFIED** by this offline rehearsal.

SpendGuard uses pre-request reservations and post-response settlement. An admitted request can exceed its reservation before enforcement reacts; a reservation is not a hard financial maximum. The authorization must explicitly accept this residual risk, or execution remains blocked until independently enforced provider-side limits exist. Missing usage trips the guard, but its conservative cost fallback is not an observed cache measurement. Native M0 additionally needs its own direct-route admission and timeout safeguards; do not reuse proxy guards as proof of native enforcement.
