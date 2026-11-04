# CW-122B1-F1 — Runtime fault-injection qualification

**Status: `BLOCKED_WITH_UNVERIFIED_SCENARIOS`.** All 10 fault scenarios pass at the deterministic and required real-Codex mock layers, including F1-09 external interruption during an active Codex request. Five exact partial-evidence interruption boundaries remain unverified. The successful-path regression also passes against a versioned CW-122A source baseline. Live execution remains **NO-GO**.

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

The runner requires an empty evidence directory outside the checkout, mode `0700`. It uses the CW-122A minimal child environment, runs deterministic Rust proxy/SpendGuard tests, the partial-evidence guard test, CW-122A analyzer tests, Python process-runner tests, and gated Codex loopback-mock tests if Codex is available. It has no live mode. The final matrix at `aa6d716db5ade7ccae63278761492f716fd7ef48` took 6.2 seconds with `codex-cli 0.166.0`. It records 10 scenario passes, zero failures, five unverified partial-evidence boundaries, and a passing successful-path gate.

Additional gates run for this change:

```sh
rtk cargo test --workspace --offline --locked
rtk cargo clippy --workspace --all-targets --offline --locked -- -D warnings
rtk cargo fmt --all -- --check
python3 -B benchmark/cw122b1/test_mock_runner.py
python3 -B -m unittest discover -s benchmark/cw122a -p test_campaign.py
```

At `aa6d716`, the targeted partial-evidence fixture test passed (1 test), the Python runner suite passed (14 tests), and formatting passed. The final matrix reran the deterministic fault tests, CW-122A analyzer tests, runner regressions, two gated Codex fault tests, and the separately supervised F1-09 interruption run. GitHub CI and GitGuardian passed on this code commit in [run 41930737469](https://github.com/GionaGranchelli/context-watcher/actions/runs/41930737469).

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
| After task completion, before final manifest | NOT_VERIFIED | The existing dropped-attempt fixture checks partial-evidence cleanup but does not interrupt a completed Codex task before the final manifest. |

F1-09 now passes both layers. Layer B used the real Codex CLI against the local mock, observed a live Codex process and one admitted request, then sent SIGTERM to the dedicated campaign runner. The runner recorded the signal and interrupted status, terminated its process groups, verified no remaining PIDs, and retained hashes for the active request evidence. All five partial-evidence boundaries remain unverified; the last row is only a synthetic fixture test, not a post-task campaign interruption. These gaps prevent the overall F1 qualification from being marked ready.

## Successful-path regression gate

The prior 12-attempt result remains historical and unchanged. The initial rerun stopped before launching Cargo or Codex because `spend_guard.rs` differed from the original CW-122A freeze. We preserved that freeze byte-for-byte as `benchmark/cw122a/freeze-cw122a-1.json` and added a separately versioned active baseline at `benchmark/cw122a/freeze.json`; its only file-hash difference is `crates/cw-core/src/spend_guard.rs`. The task campaign, fixtures, oracles, and all other frozen files are unchanged.

The first rerun at `a95ef0c` passed its task checks but failed the post-run privacy audit because the nested `raw/` directory was mode `0775`. That attempt is preserved at `/tmp/cw122b1-f1-success-path-final-a95ef0c` and is excluded from the qualification aggregate. The writer now creates `raw/` as `0700` and asserts that mode. The exact-head rerun at commit `aa6d716db5ade7ccae63278761492f716fd7ef48` passed all 12 Codex mock attempts (six pairs, both task families), 48 loopback requests, and all 12 independent acceptance checks. Each M2 attempt recorded two applications of the intended pruning rule; no M1 attempt applied a rule. The runner observed zero provider calls and zero provider spend. This is adapter and regression evidence only; task responses, usage, and cache telemetry came from the local mock and do not evaluate product value. The sanitized [successful-path aggregate](../benchmark/cw122b1/fault-success-path-aggregate.json) is SHA-256 `5e5a23af6c258f25403efcfb9a2873499d0cc214bb4b235d829198cc34e55a57`. Private evidence is at `/tmp/cw122b1-f1-success-path-final-aa6d716`: manifest SHA-256 `4c7450f6f71b3ebd8d6e47a9ee1da42619189e02a77f9e989b54968b9c0c2d91`, raw runner-output SHA-256 `35cda7a86ea4f08c2155ba3ef5c3a4d4c76a39ab733591938fb9ac318fa97821`; 173 files are `0600`, 14 directories are `0700`. The earlier freeze-rejection preflight and privacy-invalid `a95ef0c` attempt remain preserved and unchanged.

## Implementation corrections

- Added an atomic request cap shared by every session using one SpendGuard, configured through the additive `SpendGuard::new_with_campaign_request_limit` constructor. Existing `SpendGuardConfig` struct literals and `SpendGuardError` variants remain source-compatible.
- Removed the stale response `Content-Length` when re-streaming a provider body, allowing the recorder to finalize at stream EOF and classify incomplete responses correctly.
- Made runner cancellation verify descendant process groups and remaining processes. An unverified cleanup now fails the campaign and blocks later commands.
- Made nested evidence creation enforce `0700` directories and `0600` files, and made matrix verification reject unsafe modes and out-of-root/symlinked artifacts. Cancellation also kills and verifies the original process group if its leader has already exited; a Linux regression leaves an orphaned child in that group and verifies it is stopped without reporting the absent leader as a remaining PID.

The first matrix run exposed colliding evidence filenames under concurrent tests; the next exposed non-private intermediate directories. Both failed attempts are retained in `benchmark/cw122b1/fault-matrix-attempt-01.json` and `fault-matrix-attempt-02.json`. A third passing intermediate run was superseded by the public-API compatibility adjustment and is retained as `fault-matrix-attempt-03.json`. Only the latest aggregate is authoritative for the final code revision. Later interrupted reruns are also preserved privately: the `cd7d876` run had a Codex-test selector collision, the `af8b106` run exposed a stale F1-09 placeholder command, and the `45b581b` run exposed a missing `signal` import. The last failure left the dedicated mock campaign active; it was detected and terminated by SIGTERM, and its process tree was confirmed gone. These runs are excluded. The `cd2c5d4` matrix passed all fault scenarios, but its after-task boundary was mistakenly counted from a synthetic fixture drop. Commit `aa6d716` reclassifies that record as a fixture regression, not a completed-task interruption; the final matrix therefore leaves all five partial-evidence boundaries `NOT_VERIFIED`.

## Evidence and remaining gate

The sanitized [machine-readable matrix](../benchmark/cw122b1/fault-matrix.json) is bound to code commit `aa6d716db5ade7ccae63278761492f716fd7ef48`. It records 10 PASS, zero FAIL, and five `NOT_VERIFIED` partial-evidence boundaries; status is `BLOCKED_WITH_UNVERIFIED_SCENARIOS`. F1-09 and the 12-attempt success-path regression pass. Provider calls and spend are zero. SHA-256 values:

- Sanitized matrix: `428b5bd618469e697a73ba6c3d9824315068aeb46bf17ce5fdbbe425e5290b52`
- Private matrix: `bd6f142c7d4a7094465bfe795cd2fe0ac551aa7ed5592ced86ad86ccdec8c53f`
- Private runner output: `d4cf1165459e5ae93837cc61943fe22f0baa05c14cbeb58b4eab639e3b944808`

Private matrix evidence is at `/tmp/cw122b1-f1-matrix-final-aa6d716`: 153 files with mode `0600` in 40 directories with mode `0700`; root mode is `0700`. The private verifier recomputed artifact hashes and checked path containment, symlink absence, and every file/directory mode. Raw payloads and Codex data remain private.

Before the F1 fault qualification is complete, inject and verify interruption at all five boundaries: before the first provider dispatch; after request admission but before dispatch; after ingress capture but before completed egress; after response receipt but before usage settlement; and after actual task completion but before final manifest creation. The existing fixture drop test does not satisfy the last boundary. Live provider execution remains **NO-GO**. CW-122B2 also requires separate provider/model compatibility, verified real usage and cache telemetry, cache fairness, financial-risk acceptance, and fresh written spend authorization. No synthetic test here measures product value or token savings.
