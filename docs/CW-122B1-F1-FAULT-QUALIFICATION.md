# CW-122B1-F1 — Runtime fault-injection qualification

**Status: `BLOCKED_WITH_UNVERIFIED_SCENARIOS`.** Nine fault scenarios passed the declared deterministic and real-Codex mock layers. F1-09 still lacks an interrupted real Codex attempt, four partial-evidence interruption boundaries remain untested, and the original 12-attempt success campaign was stopped by its historical source-freeze check. Live execution remains **NO-GO**.

Implementation is under independent review in [PR #43](https://github.com/GionaGranchelli/context-watcher/pull/43).

No real provider, cloud API, local model, or production model server was contacted. The real Codex CLI 0.166.0 sent requests only to the in-process loopback mock. The aggregate reports zero provider calls and zero provider spend. Product value remains `NOT_EVALUATED`.

## Reproduce

From a Git checkout with Rust and Python available, run:

```sh
python3 -B benchmark/cw122b1/run_fault_matrix.py --evidence-dir /absolute/private/path/cw122b1-f1
```

The runner requires an empty evidence directory outside the checkout, mode `0700`. It uses the CW-122A minimal child environment, runs deterministic Rust proxy/SpendGuard tests, the partial-evidence guard test, CW-122A analyzer tests, Python process-runner tests, and gated Codex loopback-mock tests if Codex is available. It has no live mode. In this run, the matrix took 5.5 seconds; Codex version was `codex-cli 0.166.0`.

Additional gates run for this change:

```sh
rtk cargo test --workspace --offline --locked
rtk cargo clippy --workspace --all-targets --offline --locked -- -D warnings
rtk cargo fmt --all -- --check
python3 -B benchmark/cw122b1/test_mock_runner.py
python3 -B -m unittest discover -s benchmark/cw122a -p test_campaign.py
```

Results: 296 Rust tests passed, 22 ignored; 11 runner tests passed; 20 CW-122A analyzer tests passed; Clippy and formatting passed. Ignored tests are not included in those counts. The fault matrix also ran eight focused Rust fault tests, the existing partial-evidence test, and the two gated Codex fault tests.

## Fault matrix

Dispatch counts below sum the Layer A and Layer B executions. A Layer B `NOT_VERIFIED` is intentional for scenarios that do not require Codex; the scenario passes when every required layer passes.

| Scenario | Layer A | Layer B | Total mock dispatches | Result |
|---|---:|---:|---:|---|
| F1-01 per-session exhaustion | PASS | PASS | 4 | PASS |
| F1-02 campaign-wide exhaustion across sessions | PASS | PASS | 4 | PASS |
| F1-03 retry loop | PASS | PASS | 6 | PASS |
| F1-04 HTTP failures | PASS | PASS | 8 | PASS |
| F1-05 malformed/incomplete/stalled SSE | PASS | PASS | 5 | PASS |
| F1-06 missing/invalid/contradictory usage | PASS | Not required | 3 | PASS |
| F1-07 per-run timeout and process reaping | PASS | PASS | 1 | PASS |
| F1-08 campaign deadline | PASS | PASS | 1 | PASS |
| F1-09 external interruption during active Codex attempt | PASS | NOT_VERIFIED | 0 | NOT_VERIFIED |
| F1-10 cancellation failure | PASS | Not required | 0 | PASS |

F1-01 rejected the fourth request before upstream dispatch. F1-02 used the atomic shared SpendGuard counter and rejected an over-cap request from a new session. The campaign fixture uses a 360-request shared cap, in addition to the existing 30-request per-run cap. F1-03 observed three loopback 429 dispatches/retries in each layer before the next request was rejected; the evidence uses observed dispatch count rather than elapsed-time estimates.

F1-04 covered 429, 500, 503, 401, and 403 on the deterministic proxy path; Codex was also exercised against repeated 429 responses. No fallback endpoint was configured. F1-05 covered invalid SSE JSON, a truncated stream, closure without a terminal completion event, and a stalled response. F1-06 confirmed missing, malformed, and contradictory token usage is not converted to measured zero; the SpendGuard trips and refuses subsequent admission. Its conservative reservation accounting is recorded separately from provider-reported usage.

F1-07 killed and verified reaping of detached nested process groups, including an actual Codex process stalled on the mock. F1-08 used a shortened deadline with one second remaining in a simulated two-hour campaign budget; the active Codex attempt was cancelled, and the expired budget admitted no later attempt. F1-10 injected a cancellation helper failure, recorded the failure, returned non-success, and did not start the next command. The test reaped its disposable child for test safety; it did not simulate an unkillable production process.

## Partial evidence boundaries

| Boundary | Result | Evidence state |
|---|---|---|
| Before first provider dispatch | NOT_VERIFIED | Admission rejection was tested; runner interruption was not injected. |
| After request admission, before dispatch | NOT_VERIFIED | No deterministic hook at this boundary. |
| After ingress capture, before completed egress | NOT_VERIFIED | Provider failures were captured; interruption at this exact point was not injected. |
| After response receipt, before usage settlement | NOT_VERIFIED | Settlement failures were tested; process interruption at this point was not injected. |
| After task completion, before final manifest | PASS | The attempt guard preserved ingress, egress, mock response, and partial status. |

F1-09's Layer A test sent SIGTERM only to a dedicated disposable runner and verified its detached child was reaped and partial evidence survived. It did not interrupt a real Codex campaign. That missing Layer B result keeps F1-09 unqualified.

## Successful-path regression gate

The previous 12-attempt Codex mock result remains historical and unchanged. Rerunning it through `benchmark/cw122b1/run_mock_campaign.py` at `d33ac821e90ec51409271c1debe27c381e95e4cd` stopped before launching Cargo or Codex: `verify_freeze()` found that `crates/cw-core/src/spend_guard.rs` no longer matches the CW-122A source freeze. This is the intended fail-closed behavior. The historical freeze and prior aggregate were not rewritten; the successful path after the request-cap change is `NOT_VERIFIED` in this F1 result.

The valid preflight at PR head `d6117f0b0e2d935a27734410fb0977c73f135821` stopped at the CW-122A source-freeze check before launching Codex. Private raw output: `/tmp/cw122b1-f1-final-success-preflight-4/runner-output.log` (SHA-256 `a27f6d771702f9033755920de4d2130e6a9dc756bac7131a0f2f8530ab6d0bb1`). Its structured summary is a diagnostic record, not raw runner evidence (SHA-256 `cd1689d0d44419a877ea76b4aa290e682e1fbadd1c2aa5449c15255e5d831c9a`). The preceding capture attempt is preserved at `/tmp/cw122b1-f1-final-success-preflight-3`; it failed earlier because output was redirected into the directory before the runner's empty-directory check. Both attempts made zero provider calls. A future successful-path rerun needs a separately reviewed, versioned source baseline or an explicitly scoped current-code regression fixture; do not update the historical freeze in place.

## Implementation corrections

- Added an atomic request cap shared by every session using one SpendGuard, configured through the additive `SpendGuard::new_with_campaign_request_limit` constructor. Existing `SpendGuardConfig` struct literals and `SpendGuardError` variants remain source-compatible.
- Removed the stale response `Content-Length` when re-streaming a provider body, allowing the recorder to finalize at stream EOF and classify incomplete responses correctly.
- Made runner cancellation verify descendant process groups and remaining processes. An unverified cleanup now fails the campaign and blocks later commands.
- Made nested evidence creation enforce `0700` directories and `0600` files, and made matrix verification reject unsafe modes and out-of-root/symlinked artifacts.

The first matrix run exposed colliding evidence filenames under concurrent tests; the next exposed non-private intermediate directories. Both failed attempts are retained in `benchmark/cw122b1/fault-matrix-attempt-01.json` and `fault-matrix-attempt-02.json`. A third passing intermediate run was superseded by the public-API compatibility adjustment and is retained as `fault-matrix-attempt-03.json`. Only the latest aggregate is authoritative for the final code revision.

## Evidence and remaining gate

The sanitized [machine-readable matrix](../benchmark/cw122b1/fault-matrix.json) is bound to commit `8fa0baa84a5ee8a2311f5d42be2db125d4d68867`. SHA-256 values:

- Sanitized matrix: `20860e9a7c25f96fc49e388116c53c82b0d4f0726c7a42f8fb63814c866e2607`
- Private matrix: `1928c6c81c4532e49941810c760d02ccda1e5d3a36f2fd946025158fd792015d`
- Private runner output: `d338100160e8ca7f07c103bbd45440f336add882202aa421fde7fb97579e81ec`

Raw evidence is at `/tmp/cw122b1-f1-matrix-final-7`: 146 files with mode `0600` in 36 directories with mode `0700`; the root is `0700`. The private matrix parser recomputed every retained artifact hash and checked containment and file modes. Raw payloads and Codex data are not committed.

Before CW-122B1-F1 can be called fully qualified, run F1-09 through an actual dedicated Codex campaign interruption, inject the four untested evidence boundaries, and rerun the original successful-path qualification using a separately reviewed source baseline without modifying historical results. Keep live provider execution **NO-GO**. CW-122B2 additionally requires separate provider/model compatibility, verified real usage and cache telemetry, cache fairness, financial-risk acceptance, and fresh written spend authorization. No synthetic test here measures product value or token savings.
