# CW-122B2-P offline readiness

This directory selects an API-level provider profile and adapts saved CW-122B1 mock telemetry. It does not enable provider inference. CW-122A's preregistration, analyzer, schema, campaign and frozen artifacts remain unchanged.

- `profile.json`: provider/model capability and verification classifications.
- `pricing-2026-11-05.json`: dated official standard short- and long-context prices and disjoint cache billing categories.
- `telemetry.py`: maps every request from a B1 attempt, applies the saved short/long-context tier per request, and refuses complete-session cost when any usage category is absent.
- `rehearse.py`: maps the sanitized 12-attempt aggregate into CW-122A's unchanged schema and analyzer; it keeps rows invalid where raw wire evidence is absent.
- `authorization-contract.json` and `authorization.py`: exact future limits, credential path and SHA-256 binding, durable single-use claim, and explicit overrun-risk acceptance; no authorization exists and no live runner is callable.

Run the deterministic gates with:

```sh
python3 -B -m unittest discover -s benchmark/cw122b2p -p 'test_*.py'
python3 -B benchmark/cw122b2p/rehearse.py
```

The saved B1 aggregate contains four requests per attempt, mock usage, request/response hashes, and mutation ledger summaries. It does not contain complete raw payloads or cache-write usage. Thus B1 remains `MOCK_VERIFIED` for Codex request routing, resumed multi-request flow, and recorded treatment accounting; it is not provider usage evidence, complete cache telemetry, independently re-playable wire evidence from this sanitized file, or product-value evidence.

The candidate is GPT-6 Luna using Responses. Official API docs list Responses, streaming, function calling, tools, 1,050,000 context and 128,000 maximum output. The public deprecation list contains no current GPT-6 Luna shutdown notice; this is a dated observation, not a guarantee. A local `gpt-6-luna` mock-label run failed closed at settlement because the current SpendGuard has no verified GPT-6 Luna pricing profile. This was not model execution. Account availability, actual telemetry semantics, cache reuse, and pricing billed to the account remain `LIVE_NOT_VERIFIED`.

## Cache fairness protocol

For each matched pair, cache economics are eligible only when both arms have complete input, cached-read, cache-write (including 30-minute and 12-hour breakdown), and output usage for every request; same model/runtime/configuration; and independently verified equal initial cache state or isolated cache state. Unknown state, missing cache telemetry, or unverified isolation marks the pair cache-confounded. Missing cached/write fields never become zero. Cache writes and reads are disjoint billing categories; uncached input is total input minus cached reads minus cache writes. A token reduction with higher calculated cost emits a cost-inversion warning. All bootstrap, task, retry, failure, and follow-up requests contribute to full-session cost; the request-level pruned-token estimate stays separate.

OpenAI documents prompt caching as automatic for supported models but does not guarantee reuse: cached state is machine-local, routing can vary, TTL and exact rendered prefixes matter, and cache entries cannot be manually cleared. For future comparison use separate verified cache namespaces/isolation if available; absent that, serialize M1/M2 and mark cache economics confounded. Do not claim matched cache conditions from prompt hashes alone.

## Bounded guard integration

The B1 mock runner creates one `SpendGuard::new_with_campaign_request_limit(..., 360)` and shares clones across all twelve attempt proxies. Each session has a 30-request limit and each attempt/campaign retains 600/7200-second deadlines. This remains a caller-selected protection: `SpendGuard::new()` is campaign-unbounded. The `test_cw122b2p_campaign_cap_360_rejects_request_361_before_dispatch` integration check sends 360 requests across twelve sessions then verifies request 361 from a new session is rejected before upstream dispatch. The future live runner must retain this exact shared-guard wiring and count every retry as an admission. The authorization validator now requires this configuration in the signed-off record; it cannot verify a runner that does not yet exist.
