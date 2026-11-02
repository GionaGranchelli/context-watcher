# CW-R11A Large-History Storage and IO Qualification

**Baseline:** merged PR #28, `d5e45a08f1ffb29b753f9b1094859823aed96fb4`  
**Scope:** generated, file-backed SQLite data only; no provider calls or user data  
**Result:** one redundant blob reachability scan was removed. The generated write fixture fell from 123.20 s to 2.32 s (about 53×) with the same fixture generator and row counts.

## Method

The ignored release benchmark seeds 10, 100, 1,000, and 10,000 generated sessions and turns, plus 100 turns containing at least 350,000 estimated tokens, 100 mutation records, and 1,000 shadow evidence records. It measures store/API read latency, SQL-only query latency and plans, response bytes, a concurrent mix of readers and writes, mutex wait samples, process peak RSS, and database/WAL size. Each read cell has 20 samples. API timings use Axum in-process, so they exclude TCP, TLS, and network latency. SQL-only timings exclude the store mutex and JSON deserialization. The concurrent workload includes 200 read API calls, 50 ingest writes, 50 proxy dry-run requests, and 50 evidence reads.

The matched before run used the base commit's `save_turn` implementation with the same linear fixture generator and lock probe. The after run used the current change. Both used release builds, SQLite 3.46.0, a temporary database, and this host (Linux x86_64, 24 logical CPUs, 123 GiB RAM). The database ended at about 53.96 MB plus a 4.17 MB WAL; peak process RSS was about 22 MiB.

## Findings

| Measurement | Base | After | Notes |
| --- | ---: | ---: | --- |
| Seed 10,000 sessions + 10,000 turns | 123.20 s | 2.32 s | Matched synthetic workload; about 53× faster |
| Concurrent API ingest median / p95 | 20.15 / 43.32 ms | 6.68 / 17.32 ms | 50 writes in a mixed workload |
| Concurrent proxy dry-run median / p95 | 29.73 / 56.23 ms | 13.98 / 15.94 ms | Records locally and returns before upstream dispatch |
| Mutex wait median / p95 | 7.95 / 18.43 ms | 0.10 / 13.40 ms | Separate 1 ms sampling thread; probe affects the workload |
| Session page API at 10,000 rows | 7.00 ms median | 7.03 ms median | 1,000-row page, about 268 KB response |
| Turn page API at 10,000 turns | 18.89 ms median | 18.48 ms median | 1,000-row page, about 1.44 MB response |
| 100-turn graph over ≥350K estimated tokens | — | 2.70 ms | About 141 KB response |

In `save_turn`, every newly inserted raw request hash was added to the candidate list for orphan reclamation. The code then queried whether any turn referenced that hash, even though the just-written turn necessarily did. With no index on the three blob-reference columns, repeated inserts performed a growing table scan. The fix checks only hashes from the prior row version; those are the only candidates that can have become unreachable. A store regression test verifies that a shared old blob survives the first replacement and is reclaimed when the last reference moves.

At 10,000 rows the session query plan still scans and sorts sessions. The turn query uses `idx_turns_session_seq` and a temporary B-tree for the final ordering term. The measured API page latencies are recorded above; this qualification does not add indexes or change pagination behavior. The opt-in benchmark reports full query plans and latency distributions for all four scales in the JSON artifacts.

## Limits and status

This is one synthetic workload on one host, not a production database profile. API calls are in-process; filesystem cache state, database fragmentation, larger payload diversity, long-running growth, TCP behavior, and cross-process contention were not qualified. Linux `perf` sampling could not run because `perf_event_paranoid=4` denies performance counter access. The bottleneck diagnosis instead rests on the SQL/code path and the matched base/after workload. No broad product optimization claim follows from this result.

The remaining 10,000-row page cost is visible but is not changed here. Decide on indexes or pagination changes only against a concrete latency target and a workload that includes write amplification and migration cost. CW-R11 remains partial pending broader IO qualification.

## Reproduction

```bash
# Deterministic CI-sized concurrent append and pagination check
rtk cargo test -p cw-proxy --test r11a_smoke

# Full workspace deterministic test suite
rtk cargo test --workspace

# Opt-in generated release benchmark (no provider access)
CW_R11A_REVISION="$(git rev-parse HEAD)" rtk cargo test -p cw-proxy --release \
  --features r11a-qualification --test r11a_qualification \
  cw_r11a_large_history_baseline -- --ignored --exact --nocapture

# Static analysis and formatting gates
rtk cargo clippy --workspace --all-targets -- -D warnings
rtk cargo fmt --all -- --check
```

The opt-in test writes `target/cw-r11a-baseline.json`. Its source is `crates/cw-proxy/tests/r11a_qualification.rs`; it is ignored and feature-gated so ordinary CI does not run the stress workload.

## Artifacts

- [Matched base workload](cw-r11a-before.json)
- [After-change workload](cw-r11a-benchmark-data.json)
- [Deterministic CI smoke test](../crates/cw-proxy/tests/r11a_smoke.rs)
- [Opt-in stress benchmark](../crates/cw-proxy/tests/r11a_qualification.rs)
