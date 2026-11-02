# CW-119C Live Treatment Proof

**Status:** `TREATMENT_FAILED`
**Offline runner:** implemented; deterministic local gates passed; PR #28 merged in the qualified baseline
**Provider calls:** M1 and M2 executed; M2 treatment was not verified
**Product value:** `NOT_EVALUATED`

## Scope

Compare Codex CLI M1 passthrough with M2 committed `PinContext` on the same F1 task, model, and initial workspace state. M1 and M2 each start from a fresh identical fixture. The runner requires successful Codex turns, complete usage telemetry, independent task acceptance, SpendGuard request accounting, and exact M2 egress plus ledger verification. It does not execute M0.

This is a single treatment proof, not a product value study. A passing result establishes that the intended treatment reached the provider and that the task remained acceptable. It does not establish savings, improved reliability, or acceptable repeated-run quality. Those questions belong to CW-120 matched repetitions with success criteria set in advance.

## Offline readiness

The ignored live test is gated by `CW_QUALIFY_LIVE_PILOT=1`, explicit `CW_LIVE_AUTH_USD`, and `OPENAI_API_KEY`. No authorization is present or inferred. This host has Codex CLI `0.166.0`; `cw readiness` reports Codex as `CONFIGURED`, not functionally verified. The proxy route is available from isolated config, but route and credential isolation remain unverified without an observed proxied request. Do not invoke a provider as part of offline checks.

### 2026-11-02 preflight record

The verified baseline is `c7a5cd0d4665803b1c626fbf4394bd73c195c02b`, the PR #29 merge commit. The local PR #29 head and merged baseline have identical trees. `command -v codex` resolved `/home/gionag/.npm-global/bin/codex`; `codex --version` returned `codex-cli 0.166.0`. `cargo run -p cw-cli -- readiness` returned `UNAUTHORIZED ($0.00 / fail-closed)` and Codex `CONFIGURED`, not functionally verified. The shell had `OPENAI_API_KEY` set, but its value and validity were not inspected; readiness does not verify credentials, route, or credential isolation. `CW_LIVE_AUTH_USD` and `CW_QUALIFY_LIVE_PILOT` were unset.

`cargo test -p cw-proxy --test live_pilot_test` passed 7 deterministic tests; the one real-provider test remained ignored. No provider request was made and spend is $0.00. This includes local proof of the existing SpendGuard admission and failure protections, not a live provider-side spend-limit check.

The runner selects `gpt-4o-mini`. The repository pricing profile matches OpenAI's published text rates ($0.15/M uncached input, $0.075/M cached input, $0.60/M output), but OpenAI lists the model as deprecated and schedules `gpt-4o-mini-2024-07-18` shutdown on January 14, 2027, with `gpt-6-luna` as the replacement. A future run must revalidate the exact model alias, account availability, and pricing; this preflight made no model request. Sources: [model and pricing](https://developers.openai.com/api/docs/models/gpt-4o-mini), [deprecation schedule](https://developers.openai.com/api/docs/deprecations).

The initial preflight was `BLOCKED_SPEND_AUTHORIZATION`. Afterward, the user explicitly authorized use of the OpenAI API in the context of the proposed $0.30 nominal M1/M2 campaign. The run used `CW_LIVE_AUTH_USD=0.30`. The supplied dashboard screenshot showed $5.57 credit and a $5 project budget display, but that display was not verified as a hard provider-side cap.

### 2026-11-02 live attempt result

The opt-in test ran at baseline `c7a5cd0d4665803b1c626fbf4394bd73c195c02b` with Codex CLI `0.166.0` and model alias `gpt-4o-mini`. Both M1 and M2 Codex tasks exited successfully, and both independent F1 acceptance runs passed.

| Mode | Wall time | Codex reported input / cached / output tokens | SpendGuard usage-derived cost | Task acceptance |
| --- | ---: | ---: | ---: | --- |
| M1 passthrough | 28.874 s | 155,387 / 127,360 / 1,485 | $0.00963 | Pass |
| M2 committed PinContext | 20.695 s | 165,517 / 137,600 / 987 | $0.00918 | Pass |

The combined SpendGuard amount is $0.01881, calculated from provider-reported token usage with the repository's local pricing profile. It is not an independently reconciled invoice or dashboard charge. Exact admitted-request counts and final per-session SpendGuard state were not retained: the test panicked before writing its final `CW119_PILOT_OUT` report.

The M2 verifier examined three requests after the rule commit, classified zero as eligible, and reported three ineligible requests with egress preservation failures. It reported zero mutated eligible requests. The test failed its required `PASS` assertion. The verifier's emitted status was `BLOCKED_NO_ELIGIBLE_TREATMENT` even though its failure list showed egress mismatches; a deterministic regression test now ensures failures take precedence over that blocked status. Raw and egress payloads were held in an in-memory store and were not persisted after the panic, so the exact changed fields and whether a directive reached any request cannot be established from this run.

**Disposition:** `TREATMENT_FAILED`. The tasks passed, but the intervention was not verified and the egress safety check failed. Stop live runs until the mismatch is explained and corrected. No cache-effect or product-value claim follows from these results.

## Spend exposure and enforcement

The runner configures a $0.15 per-session ceiling, at most 25 provider requests per session (50 across both sessions), and a $0.01 reservation per request. It launches two Codex subprocesses per session, each with a 300-second timeout. The nominal campaign allocation was $0.30 ($0.15 for M1 and $0.15 for M2). This is not a guaranteed maximum charge: an admitted request can exceed its $0.01 reservation before usage settlement trips the kill switch. Provider-side retries or charges cannot be rolled back. No provider-side hard limit was established or independently verified.

## Authorization-gated command

After Codex readiness and credentials are independently confirmed, run from the repository root with an explicit campaign budget `BUDGET_USD` approved by the user:

```bash
CW_QUALIFY_LIVE_PILOT=1 CW_LIVE_AUTH_USD="$BUDGET_USD" \
  OPENAI_API_KEY="$OPENAI_API_KEY" \
cargo test -p cw-proxy --test live_pilot_test \
  test_cw119c_controlled_live_treatment_codex_m1_m2 -- --ignored --exact --nocapture
```

Expected exposure is up to two nominal $0.15 session ceilings ($0.30 total), with an unbounded per-request overage relative to the $0.01 reservation. The authorization value is not a hard provider-side spending cap. No live run is authorized by this document.

## Result record

Record the exact commit, CI result, Codex version/readiness, model, authorized amount, campaign spend, killswitch state, admitted request counts, per-mode input/cached/output usage, wall time, task acceptance, and M2 treatment status. Keep the raw task workspaces and credential material out of committed evidence. Preserve historical pilot results unchanged. A failure, missing field, timeout, unverified request, or spend guard trip blocks qualification.
