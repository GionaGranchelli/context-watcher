# CW-119C Live Treatment Proof

**Status:** `BLOCKED_SPEND_AUTHORIZATION`  
**Offline runner:** implemented; deterministic local gates passed; GitHub CI is tracked on PR #28  
**Provider calls:** none  
**Product value:** `NOT_EVALUATED`

## Scope

Compare Codex CLI M1 passthrough with M2 committed `PinContext` on the same F1 task, model, and initial workspace state. M1 and M2 each start from a fresh identical fixture. The runner requires successful Codex turns, complete usage telemetry, independent task acceptance, SpendGuard request accounting, and exact M2 egress plus ledger verification. It does not execute M0.

This is a single treatment proof, not a product value study. A passing result establishes that the intended treatment reached the provider and that the task remained acceptable. It does not establish savings, improved reliability, or acceptable repeated-run quality. Those questions belong to CW-120 matched repetitions with success criteria set in advance.

## Offline readiness

The ignored live test is gated by `CW_QUALIFY_LIVE_PILOT=1`, explicit `CW_LIVE_AUTH_USD`, and `OPENAI_API_KEY`. No authorization is present or inferred. This host has Codex CLI `0.166.0`; `cw readiness` reports Codex as `CONFIGURED`, not functionally verified. The proxy route is available from isolated config, but route and credential isolation remain unverified without an observed proxied request. Do not invoke a provider as part of offline checks.

## Spend exposure and enforcement

The runner configures a $0.15 per-session ceiling, at most 25 requests per session, and a $0.01 reservation per provider request. The campaign includes two sessions. The configured authorization must be explicit and positive, and must cover the planned exposure; `$0.30` covers the two nominal session ceilings. This is not a guaranteed maximum charge: a request admitted with a $0.01 reservation can incur a larger provider charge before usage is reported and the kill switch trips. Provider-side retries or charges also cannot be rolled back. The user must choose a fresh authorization after reviewing this limitation.

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
