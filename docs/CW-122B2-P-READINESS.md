# CW-122B2-P — Provider, telemetry and cache-fairness readiness

**Status: offline readiness work; live execution NO-GO.** No provider request, spend authorization, credential, or callable live mode is part of this change.

## Provider profile

Candidate: OpenAI API `gpt-6-luna`, Responses API, Standard processing tier. The official API model page lists Responses, streaming, function calling and built-in tools; 1,050,000 context and 128,000 max output. The official Codex model page explicitly lists `codex -m gpt-6-luna`, and Codex config docs specify the Responses wire protocol (`wire_api = "responses"`; Chat Completions-only providers are unsupported). No current Luna retirement notice appeared in the API or Codex lists on 2026-11-05. Model/protocol compatibility is `DOC_VERIFIED`; the prior successful B1 mock campaign used the synthetic `gpt-4o-mini` label. A separate local mock attempt with `gpt-6-luna` failed at SpendGuard settlement because that model has no verified pricing profile, so GPT-6 Luna's full mock route remains `NOT_VERIFIED`. This API project's entitlement, actual API-key invocation, observed usage, cache reuse and billed account pricing remain `LIVE_NOT_VERIFIED`. API-key model availability is organization/project-specific. No live call was made.

The installed Codex mock campaign used a synthetic `gpt-4o-mini` model label. Its 48 request interactions establish local request/resume routing only. The committed aggregate preserves input/output/cached usage fields and SHA-256 hashes plus treatment ledger records. It omits raw request/response payloads and cache-write telemetry. Historical B1 egress therefore cannot be independently reconstructed from the public aggregate. Private archive reinspection was not part of this offline step.

## Usage, cache, and economics

The dated [pricing snapshot](../benchmark/cw122b2p/pricing-2026-11-05.json) records GPT-6 Luna standard short-context rates up to 272,000 input tokens and the higher long-context rates above that threshold. The planned CW-122A fixtures are far below the threshold. These are published model rates, not a verified billing integration, invoice estimate or spend ceiling. The local GPT-6 mock attempt exposed that the current SpendGuard has no verified GPT-6 Luna pricing profile; that must be corrected and reviewed before any runner can settle that model's usage. The CW-122A frozen source hashes include `SpendGuard`, so this PR preserves that baseline and records the gap instead of modifying its protected source.

Responses usage must include input, cached input, cache-write total and TTL breakdown, and output for every request to calculate cost. Reads, writes, uncached input, and output are disjoint categories; cache writes are not an additive surcharge. If any field is absent or contradictory, cost is unavailable. Reasoning tokens are part of output usage; retain them as a secondary breakdown if returned. Compare both the request-level local pruning estimate and complete-session provider usage. A lower task-token total may result from fewer requests; it does not prove mechanical token reduction. Cost increase despite lower input tokens is an explicit inversion warning.

Cache fairness requires complete usage and matched or isolated initial cache state. OpenAI documents cache states as machine-local and routing-dependent, with exact rendered-prefix and lifetime requirements; a session or identical prompt prefix does not guarantee a hit. There is no documented manual cache-clear control. Until an authorized provider probe demonstrates effective isolation or matching, economics comparisons are `CACHE_CONFOUNDED`; present cache reuse as not evaluated.

## Bounded execution protection

Limits for any future separately authorized campaign are frozen at 12 attempts, 360 campaign requests, 30 per attempt, 600 seconds per attempt, and 7,200 seconds total. One shared `SpendGuard::new_with_campaign_request_limit(..., 360)` must be cloned into every session's proxy routes and attempts. Retries count as requests. The new integration regression proves that 360 calls over twelve sessions are admitted and the 361st, on a new session, is rejected before dispatch. The ordinary `SpendGuard::new()` stays campaign-unbounded for backward compatibility; its presence alone does not meet the campaign contract. This is caller-selected protection: the campaign test and the future authorization validator require the explicit bounded constructor and shared instance. They do not change or globally harden ordinary production `SpendGuard::new()` callers.

The future authorization contract binds exact approver and reference, validity window, provider/model/endpoint, campaign/runner/Codex executable/profile/pricing hashes, exact attempt and request/time limits (12, 360, 30, 7,200 seconds, 600 seconds), an exact `campaign_request_guard` requirement (`new_with_campaign_request_limit`, one instance, shared across attempts, limit 360), explicit spend ceiling, credential/evidence paths and modes, and affirmative acceptance of reservation-overrun risk. The validator rejects an absent or altered guard requirement. It binds the requirement to a future runner hash but does not prove a runner's implementation before that runner exists and is reviewed. No authorization has been issued. This PR adds no live runner and no `--live` switch.

## Rehearsal/readiness result

The executable [offline replay](../benchmark/cw122b2p/rehearse.py) maps the saved B1 attempts and all four requests per attempt into the unchanged CW-122A normalized schema, validates it, and runs the existing analyzer. Its sidecar adapter preserves request/response hashes, statuses, raw synthetic usage, mutation IDs/skips, model and sampling fields, retry status, timing scopes, and acceptance summary. SpendGuard admission/settlement details are absent from the sanitized aggregate and explicitly `NOT_AVAILABLE`. It marks the normalized rows `invalid` because the public B1 aggregate omits raw full-wire payloads; it does not invent source hashes or treat unset B1 sampling fields as observed CW-122A values. The replay finds zero valid pairs and leaves product value `NOT_EVALUATED`. Separately, the B1 campaign's acceptance summary reports all scripted fixture oracles passed, but raw oracle output is not copied. Cache-write usage is absent, so all six cache/economic comparisons remain confounded and unavailable.

The committed sanitized source aggregate `benchmark/cw122b1/mock-aggregate.json` has SHA-256 `23f682dadab1d16331cb572ec8099ad7b088f3469f78956c64cf001647dc1a85`; replaying it passed schema validation and again returned zero valid pairs, with product value `NOT_EVALUATED` and provider calls/spend both zero.

The final local mock rehearsal ran `codex-cli 0.166.0` against the existing in-process loopback server: 12 attempts and 48 requests passed, with the manifest reporting `MOCK_CODEX_CAMPAIGN_PASS`, `cache_fairness: NOT_EVALUATED`, and `product_value: NOT_EVALUATED`. No model provider call or spend occurred. Private evidence is at `/tmp/cw122b2p-mock-rehearsal` (root mode `0700`, files mode `0600`); sanitized aggregate SHA-256 is `e9954aa9753cebae036377cf8c7647829b5383ab9907bfa86a31204c7f05de51`, and manifest SHA-256 is `915446103cf1e4d342e4ff39dc8619229f34561ca8597f0ded718fdc9379bb9a`. It uses the already merged B1 Codex loopback harness and CW-122A frozen task/oracle sources. The rehearsal validates the local mock lifecycle only; its provider usage and cache counters remain synthetic.

A later optional `gpt-6-luna` mock-label attempt is preserved privately at `/tmp/cw122b2p-gpt6-mock`. Codex reached the loopback mock for one request; SpendGuard then failed closed because `gpt-6-luna` is not in its verified pricing allowlist, and the campaign stopped. The stored status, request and response artifacts remain private (directory mode `0700`, files mode `0600`); the safe runner-result SHA-256 is `fe46ed5ae293d263b43e31df44164c90aad91c923773038ecb93100e42a37d7c`. This is an adapter/pricing integration failure, not a provider call or model result. The incomplete attempt is retained and excluded from the passing B1 mock-campaign evidence.

Existing CW-122A regression tests prove that a failed correctness oracle cannot be offset by token savings, missing/tampered treatment invalidates its pair, and missing usage stays unavailable. New CW-122B2-P tests verify that an absent or malformed initial cache-state hash, unknown cache state, or absent treatment blocks economics; missing cache details leave cost unavailable; inconsistent raw usage is rejected; and a complete synthetic two-request session costs twice the one-request session. Input-token reductions with higher cost emit `COST_INVERSION_M2_MORE_EXPENSIVE`.

| Claim | Evidence class | Result |
|---|---|---|
| GPT-6 Luna Responses, streaming and tools supported | `DOC_VERIFIED` | Supported by dated official model docs |
| Codex local multi-request Responses routing and mock treatment accounting | `MOCK_VERIFIED` | CW-122B1 aggregate, 12 attempts / 48 mock requests |
| GPT-6 Luna labeled request through local mock | `INCOMPLETE` | One local request reached mock; settlement failed closed on missing verified pricing |
| B1 historical full-wire revalidation from sanitized aggregate | `UNSUPPORTED` | Raw payload omitted; only hashes/ledger remain |
| Actual model invocation and account access | `LIVE_NOT_VERIFIED` | No provider calls |
| Cached reads/writes and cache fairness | `LIVE_NOT_VERIFIED` | Mock cache counters do not establish provider behavior |
| Product-value/economic effect | `LIVE_NOT_VERIFIED` | No real usage or task outcomes |

## Reproducible offline checks

```sh
python3 -B benchmark/cw122b2p/rehearse.py
python3 -B -m unittest discover -s benchmark/cw122b2p -p 'test_*.py'
cargo test -p cw-proxy --test qualification test_cw122b2p_campaign_cap_360_rejects_request_361_before_dispatch -- --exact
```

The mock-only Codex runner remains `benchmark/cw122b1/run_mock_campaign.py`; it contacts only its local loopback mock. CW-122A's `freeze.json` is not amended and its existing campaign/analyzer are unchanged. No new provider run is authorized by this readiness report.

## Official sources checked 2026-11-05

- [GPT-6 Luna model](https://developers.openai.com/api/docs/models/gpt-6-luna)
- [API pricing](https://developers.openai.com/api/docs/pricing)
- [Prompt caching and cache telemetry](https://developers.openai.com/api/docs/guides/prompt-caching)
- [Model deprecations](https://developers.openai.com/api/docs/deprecations)
- [Responses create API reference](https://developers.openai.com/api/reference/resources/responses/methods/create)
- [Codex model selection](https://developers.openai.com/codex/models)
- [Codex config reference](https://developers.openai.com/codex/config-reference)
- [Codex advanced config](https://developers.openai.com/codex/config-advanced)
