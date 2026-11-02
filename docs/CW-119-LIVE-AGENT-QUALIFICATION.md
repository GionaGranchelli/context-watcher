# CW-119 — Controlled Live-Agent Product Value Qualification Report

**Milestone:** CW-119 (Controlled Live-Agent Product Value Qualification)
**Campaign Status:** 🟡 **IN PROGRESS: harness and safeguards repaired; product value NOT EVALUATED**
**Pilot 1 (diagnostic, preserved):** 2026-11-01T20:54:39Z, Codex CLI 0.166.0, OpenAI `gpt-4o-mini`, F1 bug-fix workload
**Pilot 1 spend:** $0.05332 USD (reconciled from Codex-reported token usage)

---

## 1. Conclusion so far

### CW-119B stateful Responses compatibility (offline qualification)

Stateful OpenAI Responses requests are operation-specific. A request with `previous_response_id` may receive only an explicitly committed `PinContext` appended to a recognized new user-text tail. The exact `previous_response_id`, roles, IDs, item order, and all original user-authored text are preserved as an exact prefix. Tool-only continuations, unknown input forms, signed/encrypted content, non-null `conversation` references, and historical Prune/Fold/Dedupe/Normalize operations abstain. These rules do not establish provider-side cache reuse; cache effect remains unverified until observed from provider telemetry. This offline capability does not alter the historical conclusions below: Pilot 1 treatment was not verified; Pilot 2 treatment was blocked by stateful Responses eligibility; neither pilot establishes economic effect, and the product hypothesis remains **NOT EVALUATED**.

> Three Codex runs completed successfully. The intended M2 intervention was not verified, and no reliable economic benefit has been demonstrated.

What Pilot 1 **does** show:
- Codex completed both turns in all three modes. The independent acceptance suite (`python3 -m unittest test_math_ops.py`) passed in all three workspaces.
- Codex ran through the M1 proxy across a multi-turn Responses session with resume and no protocol failure.
- The reported costs reconcile with the captured token counts under the `gpt-4o-mini` pricing profile.

What Pilot 1 **does not** show:
- **Whether M2 had any treatment effect.** The `PinContext` rule was committed, but AutoPilot was left disabled. The proxy only applies mutations when `cfg.enabled` is true, so the directive most likely never reached the provider. No wire-egress assertion was made. **M2 is labeled `treatment_unverified`**, and its numbers are effectively a second passthrough sample.
- **Any economic effect, positive or negative.** With n=1 per mode and an unverified treatment, the M2-vs-M1 cost difference (+32%) can't be attributed to ContextWatcher. One cheaper M2 run wouldn't have proven a benefit either.
- **Proxy overhead.** The M1-vs-M0 wall-time difference (−7.9 s) comes from provider latency and agent stochasticity (different numbers of tool iterations). It is not a measurement of proxy overhead and is no longer reported as one.
- **In-flight spend enforcement.** In Pilot 1, SpendGuard reconciled costs only after each agent finished. A session reservation can't stop an autonomous agent from issuing extra billable requests before that reconciliation happens.

## 2. Pilot 1 raw data (diagnostic only)

Source: [`cw119_pilot_benchmark_data.json`](cw119_pilot_benchmark_data.json) (unchanged).

| Metric | M0 Native | M1 Passthrough | M2 (treatment unverified) |
|:---|:---:|:---:|:---:|
| Input tokens | 194,493 | 163,875 | 211,489 |
| Cached input tokens | 165,760 | 136,320 | 182,272 |
| Output tokens | 2,377 | 1,318 | 3,248 |
| Cost (USD, reconciled) | $0.01817 | $0.01515 | $0.02000 |
| Session wall time (incl. provider latency) | 24.2 s | 16.2 s | 31.8 s |
| Acceptance suite | pass | pass | pass |

## 3. Corrections implemented after review 5730603003

| Priority | Finding | Correction |
|---|---|---|
| P0 | M2 rule committed while AutoPilot was disabled | The live runner enables AutoPilot for M2 with every auto-generation rule turned off, so only the explicitly committed `PinContext` applies. `verify_m2_treatment` checks every Turn-2 request: (a) the rule is recorded as applied, (b) the captured **wire egress** contains the directive, (c) all original `input` items before the tail item are identical and in the same order as the raw inbound payload, and (d) no pre-commit request carried the directive. The test fails if any check fails. |
| P0 | A missing `CW_LIVE_AUTH_USD` silently defaulted to $0.50 | `spend_guard::parse_authorization` has no default. A missing value returns `SpendNotAuthorized`. Malformed, zero, negative, NaN or infinite values return `InvalidAuthorization`. Both are rejected before any paid request. `SpendGuard::from_env` and `cw readiness` use the same parser. |
| P1 | Spend was only accounted after the agent finished | `SpendGuard::admit_request` runs in the proxy **before each provider request is dispatched**. It enforces the killswitch, the per-session request cap, the per-session cost ceiling, and a per-request reservation against the global budget. A rejected request gets HTTP 429 and never reaches the provider. Each request is settled incrementally from the provider-reported usage. Non-GET requests that can't be metered are refused. Harness subprocesses are killed after 300 s. |
| P1 | Missing telemetry handling | If usage is missing or incomplete (no input or output count, or cached > input), if pricing is unverified, or if a permit is dropped without settlement (e.g. an aborted stream), the full reservation is charged and the killswitch trips. |
| P1 | READY was based on binary detection | The readiness ladder is now `Unavailable < Detected < Installed < Configured < FunctionallyVerified`. Static checks can reach `Configured` at most, and only for harnesses with a generated isolated proxy config (Codex, OMP). `FunctionallyVerified` requires observed evidence via `mark_functionally_verified`; the live runner records it after a successful proxied M1 session. Credential isolation is reported as *declared*, not verified. Hermes is reported as `Installed`. |
| P2 | Missing usage fields defaulted to zero | Codex `turn.completed` parsing requires `input_tokens`, `cached_input_tokens` and `output_tokens`, sums every `turn.completed` event, and errors on missing fields. |
| P2 | Duration labeled as proxy overhead | Removed. Reports note that wall time includes provider latency. |

### Residual limitation (M0)

M0 bypasses the proxy by design, so its requests can't be metered while they run. M0 reserves the full session ceiling up front, is bounded by the subprocess timeout, and is reconciled afterwards (`spend_enforcement: post_hoc_reconciliation_with_upfront_ceiling_reservation_and_timeout`). This is recorded in every run's output.

## 4. Verification status

| Test | Kind | Status |
|---|---|---|
| `cw-core` `spend_guard::tests` (12) | default CI | pass |
| `cw-harnesses` `qualification::tests` (6) | default CI | pass |
| `test_cw119_proxy_spend_guard_blocks_requests_before_dispatch` (real proxy and mock upstream: request 3 → 429, upstream sees 2) | default CI | pass |
| `test_cw119_proxy_spend_guard_fails_closed_without_usage` | default CI | pass |
| `test_cw119_controlled_live_pilot_codex_m0_m1_m2` (repaired runner) | opt-in, paid | **not yet run** |

## 5. Next experiment (requires authorization)

The repaired runner gives one replicate per mode with verified treatment. A product-value claim would also need repeated matched workloads (multiple replicates per mode, randomized order) and a pre-registered decision rule. Until then the product hypothesis stays **NOT EVALUATED**.

```bash
cargo run -p cw-cli -- readiness
cargo test -p cw-core spend_guard
cargo test -p cw-harnesses qualification
cargo test -p cw-proxy --test live_pilot_test
# Paid, opt-in. Output goes to target/cw119_pilot_run.json (override with CW119_PILOT_OUT).
CW_QUALIFY_LIVE_PILOT=1 CW_LIVE_AUTH_USD=<explicit amount> \
  cargo test -p cw-proxy --test live_pilot_test test_cw119_controlled_live_pilot_codex_m0_m1_m2 -- --ignored --nocapture
```
