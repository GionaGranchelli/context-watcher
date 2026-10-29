# OMP continuation qualification (2026-10-29)

## Result

Across five sessions and three small Python task families, 24 of 25 newly generated answers passed the task checker. Every baseline answer and every `min`, `mid`, and `max` answer passed (20/20). Four of five native-compaction answers passed. The one failure was a real behavior defect in the transition task: for a valid state with an unsupported event, the answer leaked `KeyError`; the contract requires `ValueError`.

The native `compact` RPC itself succeeded in all five sessions. This is separate from answer quality: the transition answer still failed its contract after successful compaction. The saved normalize-tags native answer passed a final recheck with the published checker; the first runner record marked it failed under an earlier checker/deadline. The 24 passing answers each passed 12 assertions (288 total); the failing transition answer stopped on its first contract violation.

| Task | Sessions | Passing answers | Task contract |
| --- | ---: | ---: | --- |
| `bucket` | 3 | 15/15 | Reject bool/non-int with `TypeError`, negatives with `ValueError`; map 0–6 to copper, 7–22 to silver, and 23+ to jade. |
| `normalize-tags` | 1 | 5/5 | Require a list of strings; trim and lowercase, omit empty values, and deduplicate while preserving order. |
| `transition` | 1 | 4/5 | Enforce four specified state transitions, reject non-strings with `TypeError`, and all other pairs with `ValueError`. |

Across the 15 `cw` strategy runs, source transcripts stayed unchanged. `min` left every export unchanged (5/5); `mid` and `max` changed every export (5/5 each), while all corresponding generated answers passed. The analyzer's estimated 1,421-token reduction for each `mid`/`max` export is a heuristic, not provider-measured usage. No latency, cost, cache-hit, or speedup result was measured.

The model was a local Qwen-family model; the exact served model ID and endpoint address are omitted. These results give a small signal that the strategy arms retained simple task requirements after context changes. They do not establish broad agent quality, long-session reliability, general coding performance, or real-provider compatibility.

## Method and reproduction

The runner is [`../continuation_benchmark.py`](../continuation_benchmark.py). It creates a fresh OMP agent directory, workspace, and cw database for each invocation. Run it once for each `--task` choice, using a new output directory for every run, as shown in [`../README.md`](../README.md#live-omp-task-continuation). It rechecks each newly generated answer against a task-specific restricted Python AST and executes the allowed code in an isolated process with resource limits.

Each session starts with one native transcript, then exercises baseline, `min`, `mid`, `max`, and native-compaction arms. cw arms import and export their own copies. The native arm waits for the actual harness compact RPC response. Each arm has one trial; the bucket task has three independent sessions, and the other tasks have one each.

The evidence records the cw binary SHA-256 and the checked-in runner SHA-256. The binary was built from an uncommitted worktree; the exact source revision is not recorded. The runner requires an explicit `--model`; no endpoint or credential is embedded in it.

## Privacy and limits

The checked-in files contain no credentials, exact model ID, endpoint address, raw sessions, execution logs, or local file paths. Raw captures remain outside the repository. All tasks use synthetic contracts and one model/harness setup; sample counts are too small for general claims. The token estimate is analyzer output, not observed provider usage.
