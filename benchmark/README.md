# Benchmark evidence

Two different checks are provided. Neither automatically establishes a general speedup, lossless quality, or physical VRAM saving.

## Offline fixture analysis

```sh
CARGO_TARGET_DIR=/tmp/cw-benchmark-target python3 benchmark/run_benchmark.py
```

The runner builds current sources, imports recorded OMP JSONL copies, applies each strategy and reads the separate `cw preview` result. Token counts are heuristic text estimates. The original transcript stays unchanged; compacted exports are separate files. Existing Python artifact tests verify those existing files, not a new model answer. Recorded hardware telemetry belongs to the original recorded run and must not be attributed to the current strategy.

Local import/HUD/optimization durations are single samples and include setup and SQLite work. They are not proxy overhead or model latency measurements. Synthetic imported envelopes are not sent to a provider. The legacy `--upstream` option is refused; use the continuation runner instead.

Historical files under `results/` and `data*/` were produced by older implementations. Their formulas, cache assumptions and artifact-only tests do not qualify the current code. Keep them as provenance, not headline claims.

## Live OMP task continuation

Build the binary first, then supply a credential through an environment variable:

```sh
python3 benchmark/continuation_benchmark.py \
  --cw /absolute/path/to/current/cw \
  --base-url http://127.0.0.1:8080/v1 \
  --model your-model-id \
  --task bucket \
  --key-env CW_BENCHMARK_API_KEY \
  --output /tmp/cw-continuation-new-run
```

`--task` selects a deterministic task and matching checker: `bucket` tests boundary and type rules, `normalize-tags` tests normalization and stable deduplication, and `transition` tests a state transition table.

The output directory must not exist. The runner creates a private agent configuration and separate workspace, disables ambient extensions/skills/rules, fixes the selected model and reasoning-off request dialect, and never chooses another endpoint automatically. It does not read or change your active agent session.

A real OMP session receives the selected task contract and generates tool diagnostics. The same native session is copied into baseline, min, mid, max and native-compaction arms. The cw arms import and export their own copies. Native compaction waits for the harness RPC `compact` result. Each resumed harness produces a new Python answer; twelve independent assertions check the selected task's behavior. Failed requests and unavailable compaction are failures, not substituted estimates.

Artifacts include native sessions, export hashes, model/harness version, logs, preview usage and new generated answers. No API key is written to those artifacts. Each arm uses one trial, so this is a narrow task-retention smoke test. It cannot establish broad agent quality, long-session reliability, cold/warm-cache effects, or performance distributions. The runner does not claim TTFT or memory measurements.

## Sanitized evidence

The [2026-10-29 OMP continuation report](evidence/2026-10-29-omp-continuation.md) summarizes bucket, tag-normalization, and state-transition tasks. The accompanying JSON is safe to consume in automated review. Raw OMP sessions, logs, local endpoint addresses, and credentials are intentionally excluded.

For a performance study, alternate equivalent arms repeatedly, preserve real wire settings and provider usage, record cache conditions and server/hardware versions, and measure first meaningful streamed content separately from whole-response time. Do not compare a warm cached strategy with a cold unmanaged baseline without labeling that condition.
