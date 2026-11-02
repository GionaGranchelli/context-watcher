# CW-R11A Large-History Storage and IO Qualification

**Baseline:** merged PR #28, `d5e45a08f1ffb29b753f9b1094859823aed96fb4`  
**Scope:** generated, file-backed SQLite data only; no provider calls or user data  
**Result:** one redundant blob reachability scan was removed. The generated write fixture fell from 122.28 s to 2.29 s (about 53×) with the same fixture generator and row counts.

## Architecture and performance map

- `CwStore` owns one `Arc<Mutex<rusqlite::Connection>>`. SQLite is configured for WAL with `synchronous=NORMAL`; transactions and connection access serialize through that mutex.
- `api_list_sessions`, `api_get_session`, `api_list_turns`, and `api_get_session_graph` call synchronous store methods directly inside async handlers. The graph path then builds a radix graph and optimization preview on that executor thread. In contrast, proxy turn recording uses `spawn_blocking` for store writes.
- `save_turn` serializes and hashes raw/mutated/response payloads while holding the mutex and transaction. Before this change, each new raw request hash also caused a `SELECT EXISTS` over the turns table. With no indexes on the three hash columns, this work grew with history size. This was the measured bottleneck fixed here.
- `list_sessions_page` orders by `updated_at DESC`, scans/sorts without an index, caps pages at 1,000, and has no tie-breaker for identical timestamps. `list_turns_page` orders by `seq_num DESC, created_at DESC`, uses `idx_turns_session_seq`, and sorts the final term; it caps at 1,000 and reverses results into chronological order. These existing ordering and pagination semantics are unchanged.
- `session_history_snapshot` reads the latest 100 turns and all mutations for a session in a transaction. The graph endpoint therefore has a deliberate 100-turn window; callers needing deeper history must use paginated turns. `list_turns()` separately defaults to a 100-turn page. The swarm endpoints load all sessions, then fetch latest turns one session at a time (N+1 synchronous queries); this path was identified but not changed or benchmarked in CW-R11A.
- The page benchmark measured SQLite execution separately from store/API latency. At 10,000 rows, session listing scans and sorts; turn listing uses the session/sequence index plus a temporary B-tree for the final order term. Mutation and evidence queries also scan and sort their session rows. Their measured costs and plans are in the JSON artifacts.

## Method

The ignored release benchmark seeds 10, 100, 1,000, and 10,000 generated sessions and turns, plus 100 turns containing at least 350,000 estimated tokens, 100 mutation records, and 1,000 shadow evidence records. It measures store/API read latency, SQL-only query latency and plans, response bytes, a concurrent mix of readers and writes, mutex wait samples, process peak RSS, and database/WAL size. Each read cell has 20 samples. API timings use Axum in-process, so they exclude TCP, TLS, and network latency. SQL-only timings exclude the store mutex and JSON deserialization. The 500-operation concurrent workload includes 200 read API calls, 50 ingest writes, 50 proxy dry-run writes, 50 evidence reads, 50 latest-turn reads, 50 raw-payload API reads, and 50 mutation-ledger writes. All operations succeeded; the final fixture contained 10,100 primary-session turns, 150 mutation records, and 1,000 evidence records.

The matched before run used the base commit's `save_turn` implementation with the same linear fixture generator and lock probe. The after run used the current change. Both used release builds, SQLite 3.46.0, a temporary database, and this host (AMD Ryzen Threadripper PRO 3945WX, Linux x86_64, 24 logical CPUs, 123 GiB RAM). The database ended at about 54.08 MB plus a 4.17 MB WAL; peak process RSS was about 35 MiB.

## Findings

The first columns below are median/p95 milliseconds. Both runs use the same fixtures, 20 samples per read cell, and a separate build directory per revision.

| Scale | Session API base → after | Turn API base → after | Graph API base → after | Session SQL base → after | Turn SQL base → after |
| --- | ---: | ---: | ---: | ---: | ---: |
| 10 sessions / turns | 0.079/0.088 → 0.079/0.143 | 0.261/0.273 → 0.268/0.314 | 0.240/0.334 → 0.245/0.333 | 0.013/0.016 → 0.014/0.021 | 0.018/0.020 → 0.019/0.028 |
| 100 sessions / turns | 0.470/0.646 → 0.459/0.507 | 2.796/3.046 → 2.717/2.913 | 0.909/1.177 → 0.909/0.985 | 0.066/0.076 → 0.065/0.075 | 0.102/0.122 → 0.101/0.116 |
| 1,000 sessions / turns | 4.332/4.801 → 4.294/4.986 | 32.243/33.501 → 31.843/34.344 | 1.053/1.166 → 1.059/1.207 | 0.627/0.694 → 0.622/0.647 | 0.942/0.977 → 0.936/0.965 |
| 10,000 sessions / turns | 9.074/9.749 → 9.026/9.767 | 41.287/42.805 → 42.526/45.243 | 1.083/1.165 → 1.102/1.150 | 5.134/5.455 → 5.422/5.831 | 11.881/12.550 → 12.500/13.296 |

| Additional measurement | Base | After | Notes |
| --- | ---: | ---: | --- |
| Seed 10,000 sessions + 10,000 turns | 122.28 s | 2.29 s | Matched synthetic workload; about 53× faster |
| Concurrent API ingest median / p95 | 39.45 / 48.73 ms | 14.48 / 23.95 ms | 50 writes in 500 mixed operations; 22.8 → 52.1 writes/s |
| Concurrent proxy dry-run median / p95 | 39.44 / 64.41 ms | 15.18 / 16.81 ms | 50 locally recorded dry-run requests in the mixed workload |
| Concurrent mutation writes median / p95 | 0.019 / 0.030 ms | 0.019 / 0.305 ms | 50 writes per run; one scheduling outlier |
| Mutex wait median / p95 | 7.73 / 20.84 ms | 0.00008 / 1.94 ms | Separate 1 ms probe; it competes for the mutex |
| Peak RSS | 35,756 KiB | 35,576 KiB | Process high-water mark |
| Database / WAL bytes | 54,075,392 / 4,169,472 | 54,083,584 / 4,169,472 | Same generated workload |
| 100-turn graph over 351K estimated tokens | 3.43 ms | 3.63 ms | About 141 KB response; no material change |
| All API pages at 10K sessions/turns | 10,000 unique / 10,000 unique | 10,000 unique / 10,000 unique | 10 pages each; session API 7.60 → 8.14 ms median, turns API 37.45 → 38.97 ms median |

API and SQL paging results remain close between revisions; no read-path optimization was justified by this change. Small p95 differences are within the limits of a single 20-sample run on a shared host. The measured write-path improvement is large, while the existing 10K-row page costs remain visible.

In `save_turn`, every newly inserted raw request hash was added to the candidate list for orphan reclamation. The code then queried whether any turn referenced that hash, even though the just-written turn necessarily did. With no index on the three blob-reference columns, repeated inserts performed a growing table scan. The fix checks only hashes from the prior row version; those are the only candidates that can have become unreachable. A store regression test verifies that a shared old blob survives the first replacement and is reclaimed when the last reference moves.

At 10,000 rows the session query plan still scans and sorts sessions. The turn query uses `idx_turns_session_seq` and a temporary B-tree for the final ordering term. The measured API page latencies are recorded above; this qualification does not add indexes or change pagination behavior. The opt-in benchmark reports full query plans and latency distributions for all four scales in the JSON artifacts.

## Limits and status

This is one synthetic workload on one host, not a production database profile. API calls are in-process; filesystem cache state, database fragmentation, larger payload diversity, long-running growth, TCP behavior, and cross-process contention were not qualified. CPU isolation was not enforced. Linux `perf` sampling could not run because `perf_event_paranoid=4` denies performance counter access. The bottleneck diagnosis instead rests on the SQL/code path and the matched base/after workload. No broad product optimization claim follows from this result.

The remaining 10,000-row page cost is visible but is not changed here. The run verifies all 10,000 session identities and turn sequence numbers through the paginated APIs with no duplicates or missing rows. The graph API intentionally returns a 100-turn snapshot; `list_turns()` also uses a 100-turn limit, and current CLI tree/calibration and recorder callers inherit that bound. Those bounded callers are pre-existing behavior, not full-history APIs, and deserve an explicit follow-up if full-history views or correlation are required. Decide on indexes or pagination changes only against a concrete latency target and a workload that includes write amplification and migration cost. CW-R11 remains partial pending broader IO qualification.

## Correctness checks

- The store regression replaces blobs shared by multiple turns and verifies the old blob is deleted only after its last reference moves.
- The locked-database test checks the SQLite busy/locked error is returned, the failed transaction leaves neither a turn nor a blob, and a later committed turn and raw payload survive reopening the database.
- Missing records return `None`; malformed persisted turn JSON returns `StoreError::Json`; an interrupted long-running SQLite query returns `OperationInterrupted`.
- The default concurrent smoke test runs four appenders with a simultaneous reader, checks complete sequence order and raw payload presence, verifies four mutation-ledger entries, and checks session identity/order over paginated reads.
- The stress benchmark validates all 10,000 sessions and all 10,000 turn sequences through 10 API pages each. The mixed run validates 10,100 primary-session turns, 150 committed mutations, 1,000 evidence rows, and 500/500 successful operations.
- Existing workspace tests cover proxy safety, provider-transparent forwarding, streaming, and authenticated-session isolation. No proxy authorization, forwarding, transcript, or streaming code changed in this qualification.

## Reproduction

```bash
# Deterministic CI-sized concurrent append and pagination check
cargo test -p cw-proxy --test r11a_smoke

# Full workspace deterministic test suite
cargo check --workspace
cargo test --workspace

# Opt-in generated release benchmark (no provider access)
CW_R11A_REVISION="$(git rev-parse HEAD)" cargo test -p cw-proxy --release \
  --features r11a-qualification --test r11a_qualification \
  cw_r11a_large_history_baseline -- --ignored --exact --nocapture

# Static analysis and formatting gates
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
python3 plugins/hermes/context-watcher/test_plugin.py

# Feature-gated benchmark target lint
cargo clippy -p cw-proxy --all-targets --features r11a-qualification -- -D warnings
```

The opt-in test writes `target/cw-r11a-baseline.json`. Its source is `crates/cw-proxy/tests/r11a_qualification.rs`; it is ignored and feature-gated so ordinary CI does not run the stress workload.

For base/candidate comparisons, build each checkout with its own `CARGO_TARGET_DIR`. Sharing one Cargo target directory between the two worktrees can reuse the wrong package artifacts and invalidate the comparison.

## Artifacts

- [Matched base workload](cw-r11a-before.json)
- [After-change workload](cw-r11a-benchmark-data.json)
- [Deterministic CI smoke test](../crates/cw-proxy/tests/r11a_smoke.rs)
- [Opt-in stress benchmark](../crates/cw-proxy/tests/r11a_qualification.rs)
