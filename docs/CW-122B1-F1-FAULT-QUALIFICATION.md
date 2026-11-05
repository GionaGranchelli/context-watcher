# CW-122B1-F1 — Runtime fault-injection qualification

**Status: `READY_FOR_REVIEW`.** All 10 fault scenarios and all five critical interruption boundaries passed on commit `bdd3ba79714d5f1e34d94cdea5ed0df8840645cb`. The 12-attempt Codex mock success path also passed on that revision. Live execution remains **NO-GO**.

Implementation and evidence are in [PR #43](https://github.com/GionaGranchelli/context-watcher/pull/43). Independent human review is still required; no reviewer is currently assigned.

No real provider, cloud API, local model, actual provider credentials, or production model server was used. Codex CLI `0.166.0` sent requests only to the in-process loopback mock. Provider calls and provider spend were both zero. Product value remains `NOT_EVALUATED`.

## Reproduce

From a Git checkout with Rust, Python, and Codex CLI available, use separate empty private evidence directories outside the checkout:

```sh
python3 -B benchmark/cw122b1/run_mock_campaign.py \
  --evidence-dir /absolute/private/path/cw122b1-success
python3 -B benchmark/cw122b1/run_fault_matrix.py \
  --evidence-dir /absolute/private/path/cw122b1-f1 \
  --success-path-evidence-dir /absolute/private/path/cw122b1-success
```

The runner requires Git provenance, uses the CW-122A minimal child environment, and has no live mode. It runs deterministic Rust proxy/SpendGuard tests, CW-122A analyzer tests, process-runner tests, Codex loopback fault cases, and five supervised boundary interruptions. The latter compile only with the explicit `qualification-fault-hooks` feature. Each dedicated Cargo process group is terminated after the boundary marker and evidence are written; cleanup and artifact hashes are then checked. The final run took 8.2 seconds after compilation and produced `READY_FOR_REVIEW`.

The final run used the versioned [`CW-122A-1-F1-BOUNDARY-HOOKS-BASELINE`](../benchmark/cw122a/freeze.json). The original [`CW-122A-1` freeze](../benchmark/cw122a/freeze-cw122a-1.json) remains unchanged, and the prior active baseline is preserved as `benchmark/cw122a/freeze-cw122a-f1-pre-boundary-hooks.json`. The only additional frozen source change for F1 is the feature-gated proxy lifecycle hook in `crates/cw-proxy/src/proxy/mod.rs`.

## Required checks

Results on the F1 code revision:

| Check | Result |
|---|---:|
| `cargo test --workspace --offline --locked` | 296 passed, 23 ignored |
| `cargo clippy --workspace --all-targets --offline --locked --features cw-proxy/qualification-fault-hooks -- -D warnings` | Passed |
| `cargo fmt --all -- --check` | Passed |
| `cargo test -p cw-proxy --features qualification-fault-hooks --test qualification f1_` | 8 passed, 3 ignored |
| `python3 -B -m unittest discover -s benchmark/cw122b1 -p test_mock_runner.py` | 15 passed |
| `python3 -B -m unittest discover -s benchmark/cw122a -p test_campaign.py` | 20 passed |
| 12-attempt Codex loopback success campaign | 12/12 accepted; 48 mock requests |
| F1 fault matrix | 10/10 scenarios and 5/5 boundaries passed |

The three ignored targeted Rust tests are host-gated Codex workers and are exercised by the private supervisor. The matrix run exercised those Codex cases and the actual campaign task boundary. No result below is a provider inference or product-value measurement.

## F1-01 through F1-10

Layer B is not required for F1-06 and F1-10; their deterministic Layer A tests exercise the relevant settlement and cancellation contracts.

| Scenario | Layer A | Layer B | Observed dispatches | Result |
|---|---:|---:|---:|---:|
| F1-01 per-session request exhaustion | PASS | PASS | 4 | PASS |
| F1-02 campaign-wide request exhaustion | PASS | PASS | 4 | PASS |
| F1-03 provider retry loop | PASS | PASS | 6 | PASS |
| F1-04 HTTP failures | PASS | PASS | 8 | PASS |
| F1-05 malformed or incomplete SSE | PASS | PASS | 5 | PASS |
| F1-06 missing or invalid usage settlement | PASS | Not required | 3 | PASS |
| F1-07 per-run deadline and process cleanup | PASS | PASS | 1 | PASS |
| F1-08 campaign deadline | PASS | PASS | 1 | PASS |
| F1-09 external interruption | PASS | PASS | 1 | PASS |
| F1-10 failed cancellation | PASS | Not required | 0 | PASS |

F1-01 rejected the fourth session request before upstream dispatch. F1-02 used the shared atomic campaign cap and rejected a new session after the cap. F1-03 observed retryable 429 dispatches; every retry consumed admission and the next request was blocked. F1-04 covered 429, 500, 503, 401, and 403 without fallback. F1-05 covered invalid JSON, truncated and prematurely closed SSE, and a stalled response. F1-06 preserved missing or contradictory usage as unavailable, engaged the SpendGuard failure policy, and blocked later admission. Its conservative reservation accounting remains identified as an estimate, not provider usage.

F1-07 reaped detached nested process groups, including a real Codex process stalled on the mock. F1-08 bounded an active attempt by the remaining simulated campaign deadline and rejected any attempt after expiry. F1-09 interrupted the dedicated campaign runner while Codex awaited one stalled mock request; the runner recorded `INTERRUPTED`, exited 130, verified process cleanup, and retained the active-attempt evidence. The reservation remained `NOT_SETTLED_AT_INTERRUPTION`. F1-10 recorded injected cancellation failure, returned non-success, and did not start the next command. Its disposable test child was reaped for test safety; an unkillable production process was not simulated.

## Partial evidence at each boundary

Every boundary was deliberately reached. The supervisor killed the dedicated campaign/test worker with SIGKILL and verified the process group was gone. `after_task_before_final_manifest` uses the real Codex mock campaign: the independent task oracle passed, raw request/transcript evidence was written, and the campaign worker was interrupted before writing the attempt or campaign manifest.

| Boundary | Observed dispatches | Admissions | Settlement at interruption | Final attempt classification | Evidence | Result |
|---|---:|---:|---|---|---:|---:|
| Before first provider dispatch | 0 | 0 | Not applicable | `NOT_STARTED_NO_PROVIDER_REQUEST` | 3 artifacts; cleanup verified | PASS |
| After ingress, before completed egress | 0 | 0 | Not available | `INTERRUPTED_BEFORE_DISPATCH` | 2 artifacts; cleanup verified | PASS |
| After admission, before dispatch | 0 | 1 accepted, 0 rejected | `NOT_SETTLED_AT_INTERRUPTION` | `ADMITTED_NOT_DISPATCHED_UNSETTLED` | 2 artifacts; cleanup verified | PASS |
| After response, before usage settlement | 1 | 1 accepted, 0 rejected | `NOT_SETTLED_AT_INTERRUPTION` | `RESPONSE_RECEIVED_USAGE_UNSETTLED` | 4 artifacts; cleanup verified | PASS |
| After task, before final manifest | 4 | 4 accepted, 0 rejected | Mock usage settled | `TASK_ACCEPTED_ATTEMPT_MANIFEST_MISSING` | 19 artifacts; cleanup verified | PASS |

The first boundary runs inside the actual Codex campaign test immediately before the first Codex process is launched. The last boundary confirms the Codex task returned and its independent fixture oracle passed; no successful attempt or campaign manifest was produced. The boundary-by-boundary exit signal, process groups, admission and settlement details, classification, and recomputable artifact hashes are in the [machine-readable matrix](../benchmark/cw122b1/fault-matrix.json).

## Successful-path regression

The new exact-revision campaign passed all 12 attempts across six matched pairs and two task families, with 48 loopback requests and 12/12 independent acceptance checks. M1 remained passthrough; each M2 attempt verified two applications of only the intended historical-pruning rule. Original Codex transcripts remained unchanged. Codex used a dummy key only to authenticate to the loopback proxy. No provider traffic or spend occurred. Scripted mock responses and telemetry mean task quality and product value remain `NOT_EVALUATED`.

The sanitized [successful-path aggregate](../benchmark/cw122b1/fault-success-path-aggregate.json) has SHA-256 `da6eea8645bc6fb9b7b95f572bbb0bf946f3a2dc886c25131c47d2d2352ca5ec`. Its private manifest SHA-256 is `fea04a8c2a08e721165fb52496c2cc9680ad288d3ffe3df63fa36217ad8eaef6`.

## Implementation corrections

- Added an atomic pre-dispatch campaign request cap shared across sessions.
- Removed stale response `Content-Length` so stream EOF is recorded and incomplete responses are classified correctly.
- Made process cleanup verify descendant groups even after the root process exits; unverified cleanup blocks subsequent commands.
- Enforced private evidence modes and verified path containment, symlink absence, and SHA-256 for retained artifacts.
- Added lifecycle pauses only behind the non-default `qualification-fault-hooks` feature; normal proxy builds do not compile them.
- Reclassified the earlier synthetic dropped-attempt fixture as a regression test only. It is no longer counted as the after-task boundary; the new pass comes from the interrupted, oracle-accepted Codex campaign task.

## Evidence, review, and remaining gate

The latest sanitized matrix SHA-256 is `a5ad0b91e85259f9053f6e9db3356338f185c14d461141052fa00b2bd3d174d1`. The private matrix is at `/tmp/cw122b1-f1-matrix-bdd3ba7`; its SHA-256 is `390fcc56405d60004652a2cf5fbe918b88447f4727fd0775ead00b76be6e1da0`, and the private runner output SHA-256 is `6454a56d4a0c560ffb0df4532b711b60bd405b23c939d3f383c4cb3e8b636621`. It contains 183 files in 51 directories. Successful-path raw evidence is at `/tmp/cw122b1-f1-success-bdd3ba7`: 173 files in 15 directories. Both roots are mode `0700`, files are `0600`, and the audits found no symlinks or permission violations. Raw requests, responses, transcripts, and Codex data remain private.

The matrix is bound to code commit `bdd3ba79714d5f1e34d94cdea5ed0df8840645cb`; provider calls and spend are zero. This qualifies offline runtime fault handling only. CW-122B2 remains **NO-GO** until separate provider/model compatibility, real usage and cache telemetry, cache fairness, financial-risk acceptance, and fresh written spend authorization are completed. No synthetic result here demonstrates product efficiency gains.

## Campaign-wide SpendGuard integration

The campaign ceiling is opt-in by design: `SpendGuard::new()` delegates to `new_with_campaign_request_limit(..., usize::MAX)` for backward compatibility. F1-02 verifies that the bounded constructor's atomic counter is shared across sessions, but it does not make every caller bounded. The opt-in CW-119C live Codex pilot now constructs one bounded guard with a 50-request ceiling (25 per each of its two M1/M2 sessions) and shares clones with both proxies. Any future CW-122B2 runner must likewise create and share one campaign-bounded guard across all sessions and retries; a per-session limit alone is not the campaign ceiling. No live execution was run for this change.
