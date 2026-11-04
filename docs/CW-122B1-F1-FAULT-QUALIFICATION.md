# CW-122B1-F1 — Runtime fault-injection qualification

**Status: `BLOCKED_WITH_UNVERIFIED_SCENARIOS`.** All 10 fault scenarios pass at the deterministic and required real-Codex mock layers, including F1-09 external interruption during an active Codex request. Four exact partial-evidence interruption boundaries remain unverified. The successful-path regression also passes against a versioned CW-122A source baseline. Live execution remains **NO-GO**.

Implementation is under independent review in [PR #43](https://github.com/GionaGranchelli/context-watcher/pull/43).

No real provider, cloud API, local model, or production model server was contacted. The real Codex CLI 0.166.0 sent requests only to the in-process loopback mock. The aggregate reports zero provider calls and zero provider spend. Product value remains `NOT_EVALUATED`.

## Reproduce

From a Git checkout with Rust and Python available, run both commands with separate empty evidence directories outside the checkout:

```sh
python3 -B benchmark/cw122b1/run_mock_campaign.py --evidence-dir /absolute/private/path/cw122b1-success
python3 -B benchmark/cw122b1/run_fault_matrix.py \
  --evidence-dir /absolute/private/path/cw122b1-f1 \
  --success-path-evidence-dir /absolute/private/path/cw122b1-success
```

The runner requires an empty evidence directory outside the checkout, mode `0700`. It uses the CW-122A minimal child environment, runs deterministic Rust proxy/SpendGuard tests, the partial-evidence guard test, CW-122A analyzer tests, Python process-runner tests, and gated Codex loopback-mock tests if Codex is available. It has no live mode. The final matrix at `cd2c5d41f1376eab0f9dd76fe1ca84d3aa5a99ab` took 6.2 seconds with `codex-cli 0.166.0`. It records 10 scenario passes, zero failures, four unverified partial-evidence boundaries, and a passing successful-path gate.

Additional gates run for this change:

```sh
rtk cargo test --workspace --offline --locked
rtk cargo clippy --workspace --all-targets --offline --locked -- -D warnings
rtk cargo fmt --all -- --check
python3 -B benchmark/cw122b1/test_mock_runner.py
python3 -B -m unittest discover -s benchmark/cw122a -p test_campaign.py
```

Local verification: 296 Rust tests passed, 22 ignored; 14 runner tests passed; 20 CW-122A analyzer tests passed; Clippy and formatting passed. Ignored tests are not included in those counts. The final fault matrix ran the deterministic Rust fault tests, partial-evidence guard test, CW-122A analyzer tests, runner regressions, two gated Codex fault tests, and the separately supervised F1-09 interruption run.

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
| F1-09 external interruption during active Codex attempt | PASS | PASS | 1 | PASS |
| F1-10 cancellation failure | PASS | Not required | 0 | PASS |

F1-01 rejected the fourth request before upstream dispatch. F1-02 used the atomic shared SpendGuard counter and rejected an over-cap request from a new session. The campaign fixture uses a 360-request shared cap, in addition to the existing 30-request per-run cap. F1-03 observed three loopback 429 dispatches/retries in each layer before the next request was rejected; the evidence uses observed dispatch count rather than elapsed-time estimates.

F1-04 covered 429, 500, 503, 401, and 403 on the deterministic proxy path; Codex was also exercised against repeated 429 responses. No fallback endpoint was configured. F1-05 covered invalid SSE JSON, a truncated stream, closure without a terminal completion event, and a stalled response. F1-06 confirmed missing, malformed, and contradictory token usage is not converted to measured zero; the SpendGuard trips and refuses subsequent admission. Its conservative reservation accounting is recorded separately from provider-reported usage.

F1-07 killed and verified reaping of detached nested process groups, including an actual Codex process stalled on the mock. F1-08 used a shortened deadline with one second remaining in a simulated two-hour campaign budget; the active Codex attempt was cancelled, and the expired budget admitted no later attempt. F1-09 confirmed one admitted Codex request reached the loopback mock, which withheld its response; SIGTERM to the dedicated runner returned 130, recorded `INTERRUPTED`, reaped all observed process groups, and preserved the active-attempt evidence. Its $0.01 reservation remained explicitly `NOT_SETTLED_AT_INTERRUPTION` because no response or usage arrived. F1-10 injected a cancellation helper failure, recorded the failure, returned non-success, and did not start the next command. The test reaped its disposable child for test safety; it did not simulate an unkillable production process.

## Partial evidence boundaries

| Boundary | Result | Evidence state |
|---|---|---|
| Before first provider dispatch | NOT_VERIFIED | Admission rejection was tested; runner interruption was not injected. |
| After request admission, before dispatch | NOT_VERIFIED | No deterministic hook at this boundary. |
| After ingress capture, before completed egress | NOT_VERIFIED | Provider failures were captured; interruption at this exact point was not injected. |
| After response receipt, before usage settlement | NOT_VERIFIED | Settlement failures were tested; process interruption at this point was not injected. |
| After task completion, before final manifest | PASS | The attempt guard preserved ingress, egress, mock response, and partial status. |

F1-09 now passes both layers. Layer B used the real Codex CLI against the local mock, observed a live Codex process and one admitted request, then sent SIGTERM to the dedicated campaign runner. The runner recorded the signal and interrupted status, terminated its process groups, verified no remaining PIDs, and retained hashes for the active request evidence. Four other interruption boundaries remain unverified; those prevent the overall F1 qualification from being marked ready.

## Successful-path regression gate

The prior 12-attempt result remains historical and unchanged. The initial rerun stopped before launching Cargo or Codex because `spend_guard.rs` differed from the original CW-122A freeze. We preserved that freeze byte-for-byte as `benchmark/cw122a/freeze-cw122a-1.json` and added a separately versioned active baseline at `benchmark/cw122a/freeze.json`; its only file-hash difference is `crates/cw-core/src/spend_guard.rs`. The task campaign, fixtures, oracles, and all other frozen files are unchanged.

The first rerun at `a95ef0c` passed its task checks but failed the post-run privacy audit because the nested `raw/` directory was mode `0775`. That attempt is preserved at `/tmp/cw122b1-f1-success-path-final-a95ef0c` and is excluded from the qualification aggregate. The writer now creates `raw/` as `0700` and asserts that mode. The exact-head rerun at commit `cd2c5d41f1376eab0f9dd76fe1ca84d3aa5a99ab` passed all 12 Codex mock attempts (six pairs, both task families), 48 loopback requests, and all 12 independent acceptance checks. Each M2 attempt recorded two applications of the intended pruning rule; no M1 attempt applied a rule. The runner observed zero provider calls and zero provider spend. This is adapter and regression evidence only; task responses, usage, and cache telemetry came from the local mock and do not evaluate product value. The sanitized [successful-path aggregate](../benchmark/cw122b1/fault-success-path-aggregate.json) is SHA-256 `8b83b069e4b16c1c795322cdf7c6415cd83385cfc9bc15266482e5620971e1e1`. Private evidence is at `/tmp/cw122b1-f1-success-path-final-cd2c5d4`: manifest SHA-256 `14d588d4a13ed985e99887e86b4901ebeb408fece37171b616bda67a3b24b205`, raw runner-output SHA-256 `4dd7fff20b2881f02cafd9a1ad049ef2b0d904025c66a1d5066220c1ab2167a5`; 173 files are `0600`, 14 directories are `0700`. The earlier freeze-rejection preflight and privacy-invalid `a95ef0c` attempt remain preserved and unchanged.

## Implementation corrections

- Added an atomic request cap shared by every session using one SpendGuard, configured through the additive `SpendGuard::new_with_campaign_request_limit` constructor. Existing `SpendGuardConfig` struct literals and `SpendGuardError` variants remain source-compatible.
- Removed the stale response `Content-Length` when re-streaming a provider body, allowing the recorder to finalize at stream EOF and classify incomplete responses correctly.
- Made runner cancellation verify descendant process groups and remaining processes. An unverified cleanup now fails the campaign and blocks later commands.
- Made nested evidence creation enforce `0700` directories and `0600` files, and made matrix verification reject unsafe modes and out-of-root/symlinked artifacts. Cancellation also kills and verifies the original process group if its leader has already exited; a Linux regression leaves an orphaned child in that group and verifies it is stopped without reporting the absent leader as a remaining PID.

The first matrix run exposed colliding evidence filenames under concurrent tests; the next exposed non-private intermediate directories. Both failed attempts are retained in `benchmark/cw122b1/fault-matrix-attempt-01.json` and `fault-matrix-attempt-02.json`. A third passing intermediate run was superseded by the public-API compatibility adjustment and is retained as `fault-matrix-attempt-03.json`. Only the latest aggregate is authoritative for the final code revision. Later interrupted reruns are also preserved privately: the `cd7d876` run had a Codex-test selector collision, the `af8b106` run exposed a stale F1-09 placeholder command, and the `45b581b` run exposed a missing `signal` import. The last failure left the dedicated mock campaign active; it was detected and terminated by SIGTERM, and its process tree was confirmed gone. These runs are excluded. The final `cd2c5d4` matrix includes the corrected orchestration and passes F1-09; its four unrelated boundary gaps remain explicit.

## Evidence and remaining gate

The sanitized [machine-readable matrix](../benchmark/cw122b1/fault-matrix.json) is bound to code commit `cd2c5d41f1376eab0f9dd76fe1ca84d3aa5a99ab`. It records 10 PASS, zero FAIL, and four `NOT_VERIFIED` partial-evidence boundaries; status is `BLOCKED_WITH_UNVERIFIED_SCENARIOS`. F1-09 and the 12-attempt success-path regression pass. Provider calls and spend are zero. SHA-256 values:

- Sanitized matrix: `56b69b4c3a1b5314a4f49345d6e70ca9006211ef3d60a3b47fad3fcc1fd8f94d`
- Private matrix: `3c6e6d1c0e9387214fd7ba1872400e9ce338f73d3af9af340b19f817008f8925`
- Private runner output: `35ebfad799e391a814c5b009fbb84c773abf61b5535123d7fae224187825a40a`

Private matrix evidence is at `/tmp/cw122b1-f1-matrix-final-cd2c5d4`: 153 files with mode `0600` in 40 directories with mode `0700`; root mode is `0700`. The private verifier recomputed artifact hashes and checked path containment, symlink absence, and every file/directory mode. Raw payloads and Codex data remain private.

Before the F1 fault qualification is complete, inject and verify interruption at four remaining boundaries: before the first provider dispatch; after request admission but before dispatch; after ingress capture but before completed egress; and after response receipt but before usage settlement. Live provider execution remains **NO-GO**. CW-122B2 also requires separate provider/model compatibility, verified real usage and cache telemetry, cache fairness, financial-risk acceptance, and fresh written spend authorization. No synthetic test here measures product value or token savings.
