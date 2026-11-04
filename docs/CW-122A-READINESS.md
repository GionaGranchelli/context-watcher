# CW-122A: offline readiness report

Verdict: **offline treatment and analysis controls pass; live execution is NO-GO**. This is neither a product-value result nor authorization to run CW-122B. The [preregistration](CW-122A-PREREGISTRATION.md) freezes the six-pair design and decision rules. The [rehearsal instructions](../benchmark/cw122a/README.md) reproduce the checks without provider credentials, harness binaries or model servers.

Validation: 282 Rust tests passed, 19 ignored; 20 CW-122A Python tests passed; workspace Clippy and formatting passed. The 15 legacy admission tests, existing offline lifecycle fixture and 12 Hermes plugin tests passed. Web code is unchanged, so local web checks were not repeated. Ignored harness-binary/live suites were not invoked.

[Compact rehearsal results and raw-artifact hashes](../.audit/cw122a/rehearsal.json), [baseline validation record](../.audit/cw122a/validation.json), [review-hardening evidence](../.audit/cw122a/review-hardening.json), [offline host versions](../.audit/cw122a/environment.json), and [append-only decision trail](../.audit/cw122a/decisions.tsv) retain the evidence. Standalone CLI reanalysis is byte-identical. A separate gpt-6-astra review reproduced the initial oracle/early-abort/path defects, verified their corrections and the final hashes, and found no remaining offline code/readiness flags. That review is not independent human approval and did not rerun the full suites.

## Evidence and gates

| Gate | Evidence | Status |
|---|---|---|
| Two frozen coding-task families; six counterbalanced pairs | `campaign.json`, fixtures, separate oracles and source freeze | PASS offline |
| M1 passthrough; M2 sole eligible historical pruning | Real proxy to mock upstream, complete expected-payload comparison on every arm | PASS for frozen Responses fixtures |
| Applied ledger and persisted egress match actual wire | `cw122_pruning.rs`; exactly `historical-prune` in M2 and zero IDs in M1 | PASS offline |
| Unsafe/unread/protected requests abstain | Signed, unknown, stateful, current-unread and file-read cases; unchanged wire bytes | PASS for tested shapes |
| PinContext and other transformations absent | Consent off; automatic policies off; complete wire equality; stored-rule count | PASS offline |
| Source/transcript and historical evidence unchanged | Source freeze before/after rehearsal; historical report/data hashes; no history rewrites | PASS offline |
| No session/control-credential leakage | Foreign-session rule excluded; internal session/control headers stripped; token absent from body | PASS offline |
| Independent functional and constraint oracles | Broken fixtures fail, predefined repairs pass, prohibited policy/signature edits and early exit-zero fail; required oracle completion record; Python -O check | PASS offline |
| Matched analysis and failure accounting | Missing/partial usage and absent pre-dispatch wire remain unavailable; inversion, invalid treatment, failed attempts and unmatched pairs covered | PASS offline |
| Live admission defaults closed | `--live` refuses before execution; missing/stale/mismatched/reused authorization rejected; production guard blocks before mock upstream | PASS offline |
| Live provider/harness adapter, global run/request/deadline enforcement and credential isolation | Must be bound and exercised against mocks for the selected CW-122B harness/model | NOT_VERIFIED; execution blocker |
| Actual provider usage, pricing snapshot, runtime/model identity and initial cache conditions | Not selected or observed in CW-122A | NOT_VERIFIED; execution blocker |
| Native M0 route instrumentation and direct admission | Optional separate qualification, not part of the 12-run scope | NOT_VERIFIED; excluded |
| Product-value efficacy, real agent correctness and live latency | No agent/model executed | NOT_EVALUATED |

The master mutation switch must be on to apply M2's committed rule, even with automatic policies disabled. The first offline test left it off; its exact-payload assertion rejected that no-op treatment. The fixture now sets the master switch only for M2, disables every automatic policy explicitly, and still verifies the entire upstream payload rather than trusting those settings.

Review hardening uses a valid 32–128 character URL-safe fixture control token, a shared subprocess environment allowlist, independently validated strict runtime/wire schema definitions, and exception-safe restoration of the legacy import path. Environment inheritance retains only toolchain/home/temp paths and fixed locale/bytecode settings; it is not an operating-system sandbox. Schema and failed-import regressions failed before their corrections.

## Rehearsal interpretation

The rehearsal uses a real proxy and mocked provider responses, frozen sanitized tool traces, predefined repairs and external acceptance subprocesses. Each arm has one canonical treatment-boundary request. It does not execute a coding agent, establish live multi-request treatment, or qualify bootstrap/task segmentation in a selected harness.

All twelve repairs are predetermined and must pass their independent oracles. Mock input/cache/output usage is deliberately identical across M1 and M2. The aggregate therefore exercises zero session-level economic benefit while separately recording positive local pruning estimates. Product value remains `NOT_EVALUATED`; equality of acceptance does not establish improved reliability. Synthetic prices and durations are not provider prices or latency measurements.

Regression scenarios prove that a cheaper incorrect solution fails the correctness decision; absent/tampered treatment invalidates a pair; lost cache telemetry does not become zero; decreased input with increased calculated cost produces a cache-cost inversion warning; failures and their observed resource records remain visible. Both execution orders aggregate by exact preregistered pair identity, and repeated analysis is deterministic.

The normalized dataset and analyzer support nullable provider measurements and usage-derived costs. Authenticity still requires retained raw egress/response evidence and the selected adapter's qualification; an analyzer cannot authenticate a manually supplied JSON record. No provider cache reuse is inferred from shape, and no general performance claim follows from a small pilot.

## CW-122B execution gates

Keep execution closed until a separate operator approves the exact provider, endpoint, model and bounded scope with fresh identity/expiry, explicit spend and overrun-risk acceptance, isolated credentials/evidence and current hashes. The present template is invalid. No historical $5 balance, PinContext experiment approval or previous campaign authorization applies.

Before inference, qualify the selected adapter's actual request-level wire/usage capture, model/version checks, initial cache fairness, per-run and campaign-wide request/run/deadline counters, process cancellation, private credential handling and no-fallback routing using mocked responses. Attach the existing production SpendGuard to every model-bound request and retain settlement failures. This task deliberately supplies no callable live executor. Do not route around these blockers through legacy live benchmark entry points.

Missing cached-token telemetry invalidates economic qualification even though production SpendGuard can conservatively charge missing cached details as fully uncached. Reservations cannot guarantee maximum in-flight charges. If a strict financial ceiling is required, independently enforce it provider-side before admission. M0 needs separately qualified direct admission and timeout protections.

CW-122B is the next separately authorized task. Do not expand PinContext, implement cache scheduling, alter historical results, modify production servers or merge this PR automatically.
