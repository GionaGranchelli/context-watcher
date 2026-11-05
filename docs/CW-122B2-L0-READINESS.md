# CW-122B2-L0 — Real-provider smoke readiness

**Status: MOCK_QUALIFIED; LIVE_EXECUTION_NO_GO.** No provider request, paid inference, or production server change was made. PR #45 is open, stacked on PR #44 at `f629f22`; it is not yet based on approved `main`.

## Implemented

- GPT-6 Luna standard pricing uses the dated snapshot in `benchmark/cw122b2p/pricing-2026-11-05.json`: short and long context, cached reads, 30-minute writes, 12-hour writes, and output. Cache-write categories are disjoint and use total write rates. Settlement rejects absent or contradictory telemetry and charges through the same `RequestPermit::settle()` path used for provider responses. A known-value settlement test expects `$0.000641` for the fixture usage.
- The runner defaults to `NO_GO`. `--check` validates but does not claim or launch. `--run` requires a new, expiring authorization bound to the provider, model, endpoint, tier, runner/pricing/CW/Codex/bubblewrap hashes, exact credential-file SHA-256 and path, evidence/claim paths, one-request cap, deadline, ceiling, and explicit overrun-risk acceptance. A replacement key at the same path is rejected. Claims are exclusive, synced, and kept in persistent storage outside the evidence directory.
- The live route uses the existing `cw wrap` Codex adapter. The CW server receives a key-file path only; Codex receives a dummy local bearer and isolated home. Bubblewrap hides the host home and provider-key directory, makes the workspace read-only, and gives the process group a hard deadline. The proxy blocks unexpected provider/model requests, rejects before dispatch when admission or required evidence persistence fails, and does not follow redirects.
- Private evidence includes full request bodies, full egress bodies, capped response bodies with completeness markers, turns/usage, SpendGuard ledger, database, Codex event output, and Codex home. Missing artifacts remain explicit and make the result inconclusive. Evidence is never written into the checkout.
- The six-pair campaign has no execution path in this runner and still requires separate authorization.

## Offline verification

- Python runner suite: 17 passed. It covers disabled-contract consistency, invalid/expired/reused authorization, credential replacement, persistent claim behavior, private paths and symlinks, redirects, a local-only end-to-end mock request, partial evidence, missing usage, timeout handling, and resource failure.
- `test_gpt6_luna_loopback_smoke_settles_and_captures_exact_wire_evidence` exercises the real CW proxy/router and SpendGuard against the loopback provider. It verifies model binding, all usage categories, exact ingress/egress/response capture, and `$0.000641` settlement. This test sends the request directly; it does not exercise a Codex process.
- `cargo test --workspace`: 311 passed, 23 ignored. Ignored tests were not run or counted as passing.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- Formatting and diff checks: passed.
- A real bubblewrap process ran the current checkout's `target/debug/cw wrap` with a fake Codex executable under the host home and a deliberately supplied `OPENAI_API_KEY`. The child confirmed that variable was absent and only the dummy bearer was present. No model request was made.
- `codex` is unavailable on this execution host. The Python runner's successful-path test uses fake CW and Codex executables, so actual Codex-to-CW-to-Luna mock qualification remains unverified here.
- Official pricing and cache-write semantics were checked against [OpenAI pricing](https://developers.openai.com/api/docs/pricing) and [prompt-caching documentation](https://developers.openai.com/api/docs/guides/prompt-caching) on 2026-11-05. Account access and billing remain unverified.

## Remaining gates

1. Finish PR #44 and obtain its independent approval/merge. Retarget PR #45 to the resulting `main`.
2. Run CI and obtain independent review of PR #45. GitHub currently reports only GitGuardian green; `.github/workflows/ci.yml` restricts pull-request CI to base `main`, so no CI run is attached while this PR is stacked. Build the reviewed CW binary from the reviewed checkout and bind its exact path and SHA-256 in the separate smoke authorization.
3. Before any `--run`, obtain fresh human authorization for the exact single request, spend ceiling, endpoint/model, deadline, executables, credential, persistent claim directory, and private evidence directory. The existing authorization example remains disabled and has risk acceptance set false.
4. Only after the smoke result is independently reviewed may the separate six-pair campaign be considered for its own approval.

The spend ceiling is enforced by pre-request reservation and post-response settlement. It is not a provider-side hard charge limit; a single admitted response can exceed its reservation before the guard detects the overrun. A smoke `PASS` proves path and telemetry compatibility only, not pruning value, cache savings, or task-quality improvement.
