# Original CW-104A objective completion audit

Status: incomplete. The context-size matrix is supplementary evidence and does not satisfy the coding-agent product-value mission by itself.

## Recovered coding evidence

The preserved `coding/repeat-results.json` contains eight real OMP coding runs (F1/F3, repetitions 2 and 3, both modes), with acceptance-test output, tool-call counts, usage, transcript/workspace references, and M2 committed-rule/egress metadata. These recorded outcomes require independent validation against workspaces and transcripts before making a product-value decision.

| Family | Repetition | M1 task seconds | M2 task seconds | M1 recorded input | M2 recorded input | Tool calls M1/M2 | Recorded acceptance M1/M2 |
| --- | --- | ---: | ---: | ---: | ---: | --- | --- |
| F1 | 2 | 243.6 | 237.5 | 49,430 | 60,079 | 9 / 10 | True / True |
| F1 | 3 | 118.4 | 214.4 | 41,980 | 52,305 | 8 / 11 | True / True |
| F3 | 2 | 111.6 | 99.3 | 49,576 | 46,663 | 9 / 8 | True / True |
| F3 | 3 | 129.2 | 111.0 | 39,309 | 49,255 | 9 / 9 | True / True |

## Remaining completion checks

- Gate 1: consolidate existing compatibility evidence, exact model/server configuration, tool round trips, timings, streaming, hardware and cache observations.
- Gate 2: verify native integration identity, full transformation and ledger agreement, transcript preservation, and control-token isolation for the coding runs.
- Gate 3: locate and validate initial paired coding smoke results at manageable and approximately 32K context.
- Gate 4: recover repetition 1; verify all three matched repetitions per family, pristine reset, order counterbalancing, frozen pins and sampling settings, and legitimate history constraints. Do not treat repetitions 2/3 alone as the complete experiment.
- Gate 5: independently rerun coding acceptance on the saved workspaces; inspect requirement violations, retries, request counts, timings, tokens, overflow and hardware observations. Record unavailable measurements explicitly.
- Gate 6: calculate paired effects and variance only after those checks, include failed attempts and regressions, then issue a supported/unsupported/inconclusive product-value decision.

No new inference requests were made during this recovery audit. Original scope remains active.

## Independent acceptance recheck

All 12 saved coding workspaces (three repetitions for F1 and F3, both modes) passed their existing Python unittest suites on re-execution. The test-file SHA-256 is identical across all six workspaces within each family. Results and command output are preserved in `coding/independent-acceptance.json`. This validates the saved final workspaces against those tests; it does not prove pristine resets, experimental matching, or broader requirement adherence. Coding transcripts and final Python workspaces are now preserved in `coding/coding-transcripts-and-workspaces.tar.gz`.
