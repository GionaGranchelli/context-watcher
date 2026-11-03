# Recovered coding-run comparison

All 12 final workspaces independently pass the existing acceptance tests. Within each family, test file hashes match across all six workspaces. This is stronger evidence than the separate four-string context retrieval checks, but these small suites do not cover all possible requirements.

All six matched pairs have identical visible user prompts. Bootstrap order alternates; coding-task order does not. Repetitions 2 and 3 ran M1 coding before M2 in both families. The task-order counterbalancing requirement is not satisfied. These observations are derived directly from saved OMP transcripts.

| Family | Rep | M1 input tokens | M2 input tokens | M2 minus M1 | Requests M1/M2 | Tools M1/M2 |
| --- | --- | ---: | ---: | ---: | --- | --- |
| F1 | 1 | 36,135 | 44,242 | +8,107 | 7 / 8 | 7 / 8 |
| F1 | 2 | 49,430 | 60,079 | +10,649 | 8 / 10 | 9 / 10 |
| F1 | 3 | 41,980 | 52,305 | +10,325 | 8 / 9 | 8 / 11 |
| F3 | 1 | 36,325 | 37,371 | +1,046 | 7 / 7 | 7 / 7 |
| F3 | 2 | 49,576 | 46,663 | -2,913 | 9 / 9 | 9 / 8 |
| F3 | 3 | 39,309 | 49,255 | +9,946 | 7 / 9 | 9 / 9 |

F1 input-token paired differences average +9,693.7 tokens (sample standard deviation 1,383.6); F3 differences average +2,693.0 tokens (sample standard deviation 6,585.8). These are descriptive results with three pairs, not statistically established effects. Recorded cache-read tokens are zero in all 12 runs.

Decision from currently audited evidence: measurable benefit is not established. Acceptance is equal in both modes, and these runs show no consistent reduction in requests or tool calls. The F1 treatment used more input in all three pairs. A causal economic or latency conclusion requires verifying all configuration and usage scope first.

The later fixture and pin audits verify matching pre-edit source/test contents and frozen pin equality. Remaining gaps are explicit sampling/seed in the original runs, complete original pre-hook wire preservation, host concurrency controls, comprehensive constraint grading, and a completed controlled approximately-32K coding experiment. The corrected campaign now captures fixed sampling and available hardware telemetry, but its paired outcomes remain pending. The larger-context matrix disabled tools and tested source retrieval; it cannot stand in for long-context autonomous coding. No model-server changes or inference requests were made during this audit.

## Pin and runtime audit

All six M2 pins have identical content and were committed before their coding-task prompt. All 12 transcript model-change records identify the same model with no fallback. Sampling settings and seed remain unproven. See `coding-pin-and-order-audit.json`.

## Initial fixture evidence

Saved read-tool results before the first edit contain identical initial source/test contents across all six attempts within each family. All 12 final test files, and all six F3 backing-store files, match their initial read content after ignoring trailing newlines. This supports equivalent starting fixtures and preservation of those files; it does not reconstruct every filesystem attribute. Evidence: `initial-source-read-audit.json` and `fixture-preservation-checks.json`.

## Additional constraint checks

All 12 runs preserve public function signatures and pass additional behavior probes: F1 normal/negative/empty/fallback cases, and F3 absent-value non-caching, falsey-value caching, and writes to backing store. The 12 retained repetition-2/3 M2 wire captures omit temperature, top-p, top-k and seed; the experiment therefore relied on server defaults. Fixed explicit sampling is unproven, and the seed requirement is not met by these captures. See `constraint-behavior-audit.json` and `request-settings-audit.json`.

## Usage scope correction

The earlier table reports whole-session input and request counts, including bootstrap inspection. `task-only-telemetry.json` now separately records coding-task usage, tool errors, and end-to-end duration from the final user prompt through the final assistant completion. OMP TTFT is recorded as OMP telemetry and is not described as independently observed server prefill. Do not use the whole-session paired deltas as task-only cost measurements.
