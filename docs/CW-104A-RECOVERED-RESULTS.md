# CW-104A recovered results

**Updated treatment verdict:** all six historical M2 arms injected the directive into tool-output tails. Their measurements are retained, but intended user-text-only treatment is invalid. Evidence: `.audit/cw104b/recovered-pin-role-audit.json`.

Recovery status: all 12 corrected attempts and all six matched pairs recovered from per-run artifacts. The historical aggregate was stale. Experimental-validity and archive-safety findings are recorded below; remaining unverified claims stay explicit. No inference was initiated during recovery.

| Family | Rep | Both acceptance pass | Clean transport/usage | M2 minus M1 coding seconds | M2 minus M1 coding input |
| --- | --- | --- | --- | ---: | ---: |
| F1 | 1 | True | True | +208.92 | 34748 |
| F1 | 2 | True | True | -221.73 | -33759 |
| F1 | 3 | True | True | -246.03 | -37755 |
| F3 | 1 | True | False | +156.46 | NOT_AVAILABLE |
| F3 | 2 | True | True | -0.70 | 101 |
| F3 | 3 | True | True | -195.42 | -31660 |

The five clean pairs total M1 2,840.34 coding seconds and 426,553 coding input tokens; M2 2,385.39 seconds and 358,228 tokens. These reconcile the independent observations. The descriptive aggregate differences are -454.95 seconds (about -16.0%) and -68,325 input tokens (about -16.0%). They do not establish causality or a statistically reliable advantage.

F3-R1 remains in reliability outcomes. Its M1 arm had a stream failure and four upstream errors, with five missing usage records. Its known token counts are lower bounds. No total-token difference is calculated for that pair. Missing values are NOT_AVAILABLE, never zero.

The new machine-readable aggregate is `.audit/cw104b/recovered-results.json`; it separates bootstrap, coding-task, and complete-session dispatch/usage scopes. Historical observations are unchanged. All 12 saved final workspaces passed the CW-104B independent functional and constraint checks; this does not resolve the experimental confounds below.

## Experimental-validity audit

The raw request audit covers all 12 corrected attempts. All six matched fixture-hash pairs agree, and all 12 independently pass functional/constraint checks. Captured coding requests use the same model and fixed temperature/top-p settings; an explicit seed was not recorded. Initial non-message request fields match within every pair. M1 captures contain no intended pin; M2 captures contain one. Original saved transcripts do not contain the injected directive. No compaction events were found in the saved corrected transcripts. Observed server samples report version 0.1.44.1 and caching disabled throughout the sampled campaign. Full loaded-weight identity and exclusive hardware allocation are NOT_VERIFIED.

**The experiment did not isolate a pin-only request change.** Independent bootstrap conversations produced different intermediate assistant/tool messages. Initial coding message counts (M1/M2) were F1-R1 11/17, F1-R2 13/14, F1-R3 16/14, F3-R1 14/18, F3-R2 15/15, F3-R3 14/15. Evaluation/workspace identifiers also differ. Differences are explainable, but they remain confounds rather than proof of a controlled causal effect. Coding-task execution order was counterbalanced in the corrected campaign; the earlier exploratory bootstrap-order claim must not be used as task-order evidence.

The latest task messages also repeated constraints, and all arms pass the small functional suites. These fixtures have a correctness ceiling and weakly discriminate constraint retention. Lower aggregate input reflects a different number/length of model calls; PinContext did not mechanically remove historical context. The cache-disabled campaign cannot qualify cache preservation or enabled-cache economics.

## Root causes and limits

- **Test-design defects:** independently generated bootstrap histories; late reminders of the constraints; absent explicit seed; small samples/easy acceptance; shared hardware/exclusivity not established.
- **Evidence/reporting defects:** stale aggregate marked finished attempts incomplete; one broad reverse-transform regex removed literal source markup and produced a false replay mismatch. The known coding replay case passes after preserving source text. The separate earlier 128K context-matrix case now passes an isolated corrected replay of the full captured payload, applying the expected rule while preserving eight literal source-tag examples. The original broad verifier source was not retained, so its exact failure mechanism remains NOT_VERIFIED. This reconstructed replay does not supply missing original pre-hook wire bytes.
- **Observed runtime failures:** F3-R1-M1 stream/upstream failures and five missing usage records. Exact origin is NOT_VERIFIED and cannot be assigned to Context Watcher, the network, or Strata from current evidence.
- **Genuine Context Watcher implementation defect established by the reserved-window test and retrospective audit:** message-tail pin injection did not restrict the role/content shape, allowing tool-output mutation. All six historical M2 arms exhibit this defect. A narrow local guard and deterministic regressions now pass; no replacement live pair has run. This is not an exhaustive code audit. Full original pre-hook wire bytes and complete historical ledger/skip agreement are NOT_VERIFIED where not retained.

Detailed supporting evidence: `.audit/cw104b/experimental-validity-audit.json`, `independent-constraint-checks.json`, and `recovered-results.json`.

## Complete diagnostic comparison

`.audit/cw104b/reconciled-comparison.json` supplies all six absolute and relative M2–M1 differences for coding time, dispatches, input, and output; incomplete usage remains NOT_AVAILABLE. It also records per-attempt first completed provider context, bootstrap duration, phase-scoped tools/errors, and compaction. A first successful response is not substituted for a missing first-dispatch usage record. Complete-session wall time is NOT_AVAILABLE where only phase durations were retained; their sum is explicitly a phase subtotal.

Reproduce the recovered aggregate and verify all 327 original file hashes without extracting private payloads:

```sh
python3 benchmark/cw104b/recover.py /secure/path/pr32-original-evidence.tar.gz
```

The verifier reproduced all 12 attempts and six pairs exactly. The historical archive and its original file manifest remain the source of record.

The 128K case is reproducible with `python3 benchmark/cw104b/replay_matrix_case.py` when the original local CW database is available. Evidence: `.audit/cw104b/matrix-mismatch-replay.json`. The script backs up the database into a temporary isolated directory, calls only its local transform endpoint, and stops its own daemon; no model inference occurs.
