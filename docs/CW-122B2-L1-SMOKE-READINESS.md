# CW-122B2-L1 smoke readiness

Status: `PREPARED_OFFLINE`; no provider request was made by this change.

## What is demonstrated already

The private evidence was independently rechecked during this turn: all 20 artifact sizes and SHA-256 hashes match the manifest; M1 ingress is byte-identical to egress; M1 and M2 share the same ingress hash; and a full JSON diff finds only `/input/2/output` changed in M2. Usage-derived repricing from the committed snapshot reproduces the recorded settlements at `$0.000177` and `$0.000153` using the default 30-minute cache-write rate. This is SpendGuard accounting, not invoice reconciliation. The sanitized check is [here](../.audit/cw122b2-l1/exploratory-pair-recheck.json); raw request/response payloads remain outside the checkout.

Private evidence from an earlier exploratory real-provider pair shows that a committed historical prune reached GPT-6 Luna. Both requests had the same ingress hash. M1 egress was unchanged; M2 changed only `/input/2/output`. Both responses completed and reported usage. Input tokens were 1,049 for M1 and 270 for M2; SpendGuard settlement was `$0.000177` and `$0.000153` respectively. That proves this request-level transformation can reduce provider input and calculated request cost in that observation.

It does not prove coding-task value. The prior pair was not a full matched Codex task with independent acceptance, the driver revisions differed, and provider cache state was not independently verified. Therefore product value and cache economics remain `NOT_EVALUATED`.

The private archive also contains an adverse same-ingress M1/M2 attempt: M1 completed at 1,049 input and 91 output tokens for `$0.000177`; M2 applied the same prune and used 270 input tokens, but hit the 512-token output limit, returned `incomplete`, and settled at `$0.000283`. This shows that lower input can be outweighed by extra generation. It is exploratory evidence, not an independent coding-task result; driver equivalence and cache-state comparability remain unverified. The requests omit `prompt_cache_options.ttl`; the response's legacy `prompt_cache_retention: "24h"` field describes a separate retention policy and does not select the 12-hour cache-write rate. Current pricing therefore uses the default 30-minute write rate, as recorded in the recheck artifact. See [OpenAI prompt caching documentation](https://developers.openai.com/api/docs/guides/prompt-caching).

## Code-review prerequisite

Current GitHub state for [PR #45](https://github.com/GionaGranchelli/context-watcher/pull/45): open, base `main`, mergeable, and full CI run [42261372762](https://github.com/GionaGranchelli/context-watcher/actions/runs/42261372762) passed on head `d80d87354f758168d77251e485ff26f9e00879c5`. All six review threads are resolved, but reviews are only COMMENTED by the author and Copilot; no independent human approval is recorded. This PR covers the L0 preparation, not the L1 changes in this worktree. The superseded snapshot is retained as `pr45-prerequisite-status-before-l0-final.json`; the current sanitized status is [pr45-prerequisite-status.json](../.audit/cw122b2-l1/pr45-prerequisite-status.json).

The L1 work is in draft [PR #46](https://github.com/GionaGranchelli/context-watcher/pull/46), based on PR #45. It is reviewable, but has no GitHub Actions run because the current workflow only triggers for PRs targeting `main`; the full local Rust/Python gates passed on the PR source. Do not merge or execute the smoke before independent review and separate live-envelope approval.

## Live account preflight

An authenticated, direct TLS `GET /v1/models/gpt-6-luna` succeeded at 2026-11-06 16:36:50 UTC (HTTP 200, model ID matched). Redirects were not followed; no inference endpoint was called and no generation tokens were requested. This confirms current key/model metadata access and direct network reachability, but not inference completion, billing telemetry, or task quality. Sanitized evidence: [provider-model-preflight.json](../.audit/cw122b2-l1/provider-model-preflight.json).

## Offline smoke runner prepared

`benchmark/cw122b2l1/run_smoke.py` defaults to NO-GO. A separately authorized run is limited to frozen F1 `f1-r1`, M1 then M2, 60 provider requests total, 30 per arm, 600 seconds per arm, 1,200 seconds overall, and a spend ceiling no higher than `$5`. It uses one shared bounded SpendGuard, records complete private wire, response, usage, ledger, and Codex evidence, and requires both arms to pass the independent F1 oracle. The M2 arm must show the exact historical prune in provider-bound egress.

Review found that the authorization runner hash omitted imported CW-122B1 process controls and task-oracle inputs. The hash now covers that module, the imported offline environment and rehearsal/analyzer helpers, the F1 oracle's `retry.py` dependency, full `cw-core`, `cw-store`, `cw-proxy`, and `cw-harnesses` source trees, the frozen F1 fixture, and campaign/pricing inputs. Dependency-closure regressions assert these files are included; nested source changes alter the hash, and nested symlink directories are rejected. The current bundle hash is `ef6b3ca2203340f206a741c27f9843df575797b2c2e76561ca69aa94d7bef575`. The private draft's runner hash predates these corrections and remains unapprovable because overrun risk acceptance is false; any fresh authorization must bind the current hash.

The ignored Rust integration test compiles. Failed attempts now also copy any Codex rollout transcripts into private failure evidence and report transcript preservation separately; the regression verifies exact transcript bytes alongside raw provider wire. This closes the failure-transcript gap found in review. The test remains ignored and was not run against a real provider. A private authorization draft is prepared at `/home/gionag/.local/state/context-watcher/cw122b2l1-smoke-review/smoke-authorization-review.json`; it has `reservation_overrun_risk_accepted: false` and pending approver fields. Running it through the actual runner returned NO_GO before claiming the ID or writing evidence. The six-pair product-value campaign remains disabled and requires separate authorization. The sanitized rejection record is [here](../.audit/cw122b2-l1/smoke-authorization-draft-check.json).

## Verification run

- L1 authorization tests: 8 passed, including under `python -O`.
- CW-122B2-P Python suite: 13 passed, including under `python -O`.
- Full Rust workspace: 323 passed, 25 ignored, 0 failed across 34 test targets; all ignored provider, harness-binary, and long-run suites remained skipped.
- Workspace formatting and Clippy with `-D warnings`: passed.
- SpendGuard tests: 20 passed, including known-price settlements and bounded request accounting.
- Explicit OpenAI-upstream loopback regression: 1 passed.
- Current qualification integration suite after the response-evidence timing fix: 30 passed, 16 ignored; targeted test, test-target Clippy, formatting and `git diff --check` passed. All ignored live/provider tests remained unexecuted.
- Repeated after the timing fix: full Rust workspace 323 passed/25 ignored; L1 authorization 5, L0 runner 28, and B2-P authorization 13 passed in both normal and `python -O` modes.
- After binding the process controls and task-oracle inputs: L1 authorization tests 8 passed in normal and optimized modes; the runner still rejects the private draft before claim/evidence because overrun-risk acceptance remains false.
- Live pair integration test: compiled and listed as ignored; not executed.
- Clippy for the qualification test target, formatting, and `git diff --check`: passed.
- CW-122B2-L0 runner suite: 28 passed normally and under `python -O`; CW-122B2-P suite: 13 passed normally and under `python -O`.
- Default smoke runner: printed `NO_GO` and exited 2, including when a dummy API key was present in its parent environment.
- No API key was printed, no production server was changed, and no provider request was made during this readiness work.

## Decision

The request-level feature has one real-provider proof point. A defensible product-value result needs the frozen six-pair M1/M2 campaign: complete tasks with independent acceptance, all attempts retained, and total provider input/output/cache-write usage, cost, requests, retries, and wall time reported per pair. Cache economics must remain marked confounded unless cache comparability has evidence beyond matching hashes.

The next gate is review of the runner and an exact, separate smoke authorization that states the `$5` maximum, request and time limits, and acceptance of pre-request reservation overrun risk. The smoke pair is a compatibility/measurement gate, not product-value qualification.
