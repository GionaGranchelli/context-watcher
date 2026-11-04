# CW-122B1-F1 — Runtime fault-injection qualification

**Status: `BLOCKED_WITH_UNVERIFIED_SCENARIOS`.** Nine fault scenarios passed the declared deterministic and real-Codex mock layers. F1-09 still lacks an interrupted real Codex attempt, and four partial-evidence interruption boundaries remain untested. The separate successful-path regression now passes against a versioned CW-122A source baseline. Live execution remains **NO-GO**.

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

The prior 12-attempt result remains historical and unchanged. The initial rerun stopped before launching Cargo or Codex because `spend_guard.rs` differed from the original CW-122A freeze. We preserved that freeze byte-for-byte as `benchmark/cw122a/freeze-cw122a-1.json` and added a separately versioned active baseline at `benchmark/cw122a/freeze.json`; its only file-hash difference is `crates/cw-core/src/spend_guard.rs`. The task campaign, fixtures, oracles, and all other frozen files are unchanged.

The first rerun at `a95ef0c` passed its task checks but failed the post-run privacy audit because the nested `raw/` directory was mode `0775`. That attempt is preserved at `/tmp/cw122b1-f1-success-path-final-a95ef0c` and is excluded from the qualification aggregate. The writer now creates `raw/` as `0700` and asserts that mode. The corrected rerun at commit `74821b1111e766ad8e05352700d28f83f9bb4d09` passed all 12 Codex mock attempts (six pairs, both task families), 48 loopback requests, and all 12 independent acceptance checks. Each M2 attempt recorded two applications of the intended pruning rule; no M1 attempt applied a rule. The runner observed zero provider calls and zero provider spend. This is adapter and regression evidence only; task responses, usage, and cache telemetry came from the local mock and do not evaluate product value. The sanitized [successful-path aggregate](../benchmark/cw122b1/fault-success-path-aggregate.json) is SHA-256 `54ed4fe760175ec13485f944cf24f77f25e4ae1d4124cdc80e53b4db63a439ee`. Private evidence is at `/tmp/cw122b1-f1-success-path-final-74821b1`: manifest SHA-256 `0cab3c01dfe24ee3e4f97927235c4a0786cac5568a6c21ada5d03d9bffce98eb`, raw runner-output SHA-256 `c9c374be0a4b10fb93777597ea34f26d99c5b84bd1b93a6ee734a4fc7d50ea51`; 173 files are `0600`, 14 directories are `0700`. The earlier freeze-rejection preflight records remain preserved separately and were not overwritten.

## Implementation corrections

- Added an atomic request cap shared by every session using one SpendGuard, configured through the additive `SpendGuard::new_with_campaign_request_limit` constructor. Existing `SpendGuardConfig` struct literals and `SpendGuardError` variants remain source-compatible.
- Removed the stale response `Content-Length` when re-streaming a provider body, allowing the recorder to finalize at stream EOF and classify incomplete responses correctly.
- Made runner cancellation verify descendant process groups and remaining processes. An unverified cleanup now fails the campaign and blocks later commands.
- Made nested evidence creation enforce `0700` directories and `0600` files, and made matrix verification reject unsafe modes and out-of-root/symlinked artifacts.

The first matrix run exposed colliding evidence filenames under concurrent tests; the next exposed non-private intermediate directories. Both failed attempts are retained in `benchmark/cw122b1/fault-matrix-attempt-01.json` and `fault-matrix-attempt-02.json`. A third passing intermediate run was superseded by the public-API compatibility adjustment and is retained as `fault-matrix-attempt-03.json`. Only the latest aggregate is authoritative for the final code revision.

## Evidence and remaining gate

The sanitized [machine-readable matrix](../benchmark/cw122b1/fault-matrix.json) is bound to commit `74821b1111e766ad8e05352700d28f83f9bb4d09`. The rerun took 5.4 seconds and records 9 PASS, 1 NOT_VERIFIED, zero provider calls, and zero provider spend. SHA-256 values:

- Sanitized matrix: `a14ab82f1f1e77576176c984095312f6c00497a31cddf6076b6d74998a5e7f72`
- Private matrix: `11851a9c44900b15a2bb0256d723bc1100e0c7731de7675aab0fe31fd76caf7a`
- Private runner output: `4e52ebf02fc4aa54c31a86a63facbcb02fe33642b7d0a1edd30c417ff19efa4c`

Raw evidence is at `/tmp/cw122b1-f1-matrix-final-9`: 146 files with mode `0600` in 35 directories with mode `0700`; the root is `0700`. The private matrix parser recomputed every retained artifact hash and checked containment and file modes. Raw payloads and Codex data are not committed.

Before CW-122B1-F1 can be called fully qualified, run F1-09 through an actual dedicated Codex campaign interruption and inject the four untested evidence boundaries. Keep live provider execution **NO-GO**. CW-122B2 additionally requires separate provider/model compatibility, verified real usage and cache telemetry, cache fairness, financial-risk acceptance, and fresh written spend authorization. No synthetic test here measures product value or token savings.
