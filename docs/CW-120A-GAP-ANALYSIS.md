# CW-120A implementation gap analysis

**Baseline:** `b1279912e8f2628ccc08090e1da6fec4061dbf3a` (PR #30 merge)
**Purpose:** Record reusable CW-118/CW-119C capability and actual gaps before implementation.

## Already reusable

- `benchmark/cw118_product_value.py` defines F1 and F3 fixtures, independent acceptance commands, seeded task-block ordering, mode-order positions, provider usage cost calculation, attempt/failure accounting, paired M2-versus-M1 analysis, and bootstrap summaries.
- `crates/cw-proxy/tests/product_value_qualification_test.rs` exercises production proxy forwarding, M1 payload transparency, M2 committed mutations, eligibility, and cache-safe PinContext with mock upstreams. This is deterministic local evidence, not an autonomous coding-agent product-value trial.
- `crates/cw-proxy/tests/live_pilot_test.rs` provides the Codex M1/M2 runner, isolated fixture setup, SpendGuard, saved request/egress evidence, and independent acceptance. It is a one-pair F1 treatment proof, not a multi-family matched study.
- CW-119C saved evidence establishes one exact stateless PinContext treatment and seven unchanged tool follow-ups. It does not establish stateful delivery, provider cache effects, or product value. The recorded rerun cost was M1 `$0.011075`, M2 `$0.017005`; these are usage-derived observations, not reconciled charges.

## Gaps to close for CW-120A

1. The CW-118 schedule/runner is fixed around 5 families, 3 modes, and 45 synthetic sessions. Its CLI cannot freeze the requested F1/F3, two-mode, 12-session schedule as a distinct campaign; its offline simulations are not live-agent results.
2. There is no frozen artifact naming the exact F1/F3 instructions, three independent repetitions, paired workspace reset, randomized/counterbalanced order, or an immutable directive shared by M2 attempts.
3. The CW-119C live test cannot execute the proposed 12-session study or F3. Its existing evidence verifier and cost data can be reused, but experimental schedule and per-attempt reporting need a narrow offline readiness layer.
4. CW-118 synthetic telemetry and costs must stay visibly separate from the CW-119C observed usage data. Missing usage must remain unpriced/invalid, and unsuccessful attempts must remain in all-attempt costs and denominators.
5. Financial exposure must be recalculated for 12 sessions with token-variance, retry, and longer-task allowances. The old `$0.30` CW-119C authorization is spent and inapplicable; SpendGuard reservations are not a provider-side hard cap.
6. `BOARD.md` and the opening EVIDENCE status still describe CW-119C as awaiting authorization/treatment proof after PR #30. Those current-status statements conflict with the merged stateless treatment evidence and must be corrected without rewriting historical pilot records.

## Scope decision

Reuse existing fixture, schedule, pricing, acceptance, reporting, and verification code. Add only a frozen CW-120A manifest/report and small tests or runner adjustments where the offline rehearsal demonstrates a real gap. Do not add live-provider execution, a new mutation operator, or generic benchmark infrastructure.
