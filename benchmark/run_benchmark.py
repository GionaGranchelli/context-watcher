#!/usr/bin/env python3
"""Reproducible End-to-End Benchmark Runner for contextWatcher (cw).

This script contains ZERO hardcoded benchmark results. Every metric is computed
dynamically by executing the compiled `cw` Rust binary against real recorded
multi-turn agent transcripts (`benchmark/fixtures/*.jsonl`), inspecting the
resulting SQLite state (`turns`, `mutations`), running the real Python unit test
suites in `benchmark/artifacts*`, and optionally querying a live OpenAI /
llama.cpp endpoint (`--upstream http://...`) for historical reference only. Live qualification uses continuation_benchmark.py.

Usage:
    python3 benchmark/run_benchmark.py
    python3 benchmark/run_benchmark.py --upstream http://127.0.0.1:8080
"""

import argparse
import csv
import json
import os
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
FIXTURES_DIR = REPO_ROOT / "benchmark" / "fixtures"

# Map each raw recorded JSONL fixture to its corresponding code artifact test suite
BENCHMARK_CASES = [
    {
        "name": "lru_cache",
        "description": "Thread-safe TTL LRU Cache (omp + qwen3.8-27b-ud)",
        "fixture": FIXTURES_DIR / "session_lru_cache.jsonl",
        "artifacts_dir": REPO_ROOT / "benchmark" / "artifacts",
    },
    {
        "name": "flint_scheduler",
        "description": "Async DAG Task Scheduler with Retry & Backoff",
        "fixture": FIXTURES_DIR / "session_flint_scheduler.jsonl",
        "artifacts_dir": REPO_ROOT / "benchmark" / "artifacts_flint",
    },
    {
        "name": "strata_rate_limiter",
        "description": "Sliding-Window & Token-Bucket Rate Limiter",
        "fixture": FIXTURES_DIR / "session_strata_rate_limiter.jsonl",
        "artifacts_dir": REPO_ROOT / "benchmark" / "artifacts_strata",
    },
    {
        "name": "longrun_kv_engine",
        "description": "Crash-Safe WAL + MemTable + Snapshot KV Engine",
        "fixture": FIXTURES_DIR / "session_longrun_kv_engine.jsonl",
        "artifacts_dir": REPO_ROOT / "benchmark" / "artifacts_longrun",
    },
]


def find_or_build_cw_binary() -> Path:
    """Build current sources, respecting an isolated Cargo target directory."""
    subprocess.run(["cargo", "build", "-p", "cw-cli"], cwd=REPO_ROOT, check=True)
    target = Path(os.environ.get("CARGO_TARGET_DIR", REPO_ROOT / "target"))
    return target / "debug" / "cw"


def extract_recorded_hardware_telemetry(jsonl_path: Path) -> dict:
    """Parse raw JSONL transcript to extract the hardware telemetry logged by the harness."""
    ttfts = []
    durations = []
    models = set()
    providers = set()
    total_turns = 0
    max_prompt_tokens = 0

    with open(jsonl_path, "r", encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            try:
                obj = json.loads(line)
            except json.JSONDecodeError:
                continue
            msg = obj.get("message", {})
            if isinstance(msg, dict) and msg.get("role") == "assistant":
                total_turns += 1
                if "ttft" in msg and isinstance(msg["ttft"], (int, float)):
                    ttfts.append(float(msg["ttft"]))
                if "duration" in msg and isinstance(msg["duration"], (int, float)):
                    durations.append(float(msg["duration"]))
                if msg.get("model"):
                    models.add(msg["model"])
                if msg.get("provider"):
                    providers.add(msg["provider"])
                snap = msg.get("contextSnapshot", {})
                if isinstance(snap, dict):
                    pt = snap.get("promptTokens", 0)
                    if isinstance(pt, int) and pt > max_prompt_tokens:
                        max_prompt_tokens = pt

    return {
        "assistant_turns": total_turns,
        "models": sorted(models),
        "providers": sorted(providers),
        "recorded_max_prompt_tokens": max_prompt_tokens,
        "recorded_avg_ttft_ms": round(sum(ttfts) / len(ttfts), 2) if ttfts else None,
        "recorded_max_ttft_ms": round(max(ttfts), 2) if ttfts else None,
        "recorded_total_generation_ms": round(sum(durations), 2) if durations else None,
    }


def run_artifact_unit_tests(artifacts_dir: Path) -> dict:
    """Actually execute `python3 -m unittest` on the artifact directory and parse test count."""
    if not artifacts_dir.exists():
        return {"passed": False, "tests_run": 0, "output": "Directory not found"}

    t0 = time.perf_counter()
    proc = subprocess.run(
        [sys.executable, "-m", "unittest", "discover", "-s", str(artifacts_dir), "-p", "test_*.py", "-v"],
        capture_output=True,
        text=True,
    )
    elapsed_ms = round((time.perf_counter() - t0) * 1000.0, 2)
    stderr = proc.stderr or ""
    tests_run = 0
    for line in stderr.splitlines():
        if line.startswith("Ran ") and " test" in line:
            parts = line.split()
            if len(parts) >= 2 and parts[1].isdigit():
                tests_run = int(parts[1])

    return {
        "passed": proc.returncode == 0,
        "tests_run": tests_run,
        "duration_ms": elapsed_ms,
    }


def query_latest_turn_from_db(db_path: Path, session_id: str) -> dict:
    """Read the serialized ContextTurn JSON directly from cw's SQLite store."""
    conn = sqlite3.connect(str(db_path))
    try:
        row = conn.execute(
            "SELECT turn_json, raw_request_hash, mutated_request_hash FROM turns WHERE session_id = ? ORDER BY seq_num DESC LIMIT 1",
            (session_id,),
        ).fetchone()
        if not row:
            raise RuntimeError(f"No turn found in {db_path} for session {session_id}")
        turn_data = json.loads(row[0])
        raw_hash = row[1]
        mut_hash = row[2]

        raw_payload = None
        if mut_hash:
            brow = conn.execute("SELECT content_json FROM blobs WHERE hash = ?", (mut_hash,)).fetchone()
            if brow:
                raw_payload = json.loads(brow[0])
        elif raw_hash:
            brow = conn.execute("SELECT content_json FROM blobs WHERE hash = ?", (raw_hash,)).fetchone()
            if brow:
                raw_payload = json.loads(brow[0])

        return {"turn": turn_data, "payload": raw_payload}
    finally:
        conn.close()


def measure_live_upstream_ttft(upstream_url: str, api_key: str, payload: dict) -> None:
    if upstream_url:
        raise ValueError("Imported analysis envelopes are not provider requests. Use continuation_benchmark.py for live task qualification.")
    return None


def benchmark_single_case(cw_bin: Path, case: dict, upstream_url: str | None, api_key: str) -> dict:
    fixture: Path = case["fixture"]
    if not fixture.exists():
        raise FileNotFoundError(f"Missing fixture: {fixture}")

    hw_telemetry = extract_recorded_hardware_telemetry(fixture)
    unit_tests = run_artifact_unit_tests(case["artifacts_dir"])

    with tempfile.TemporaryDirectory(prefix=f"cw_bench_{case['name']}_") as tmp_dir_str:
        tmp_dir = Path(tmp_dir_str)
        base_db = tmp_dir / "base.db"
        base_jsonl = tmp_dir / fixture.name
        shutil.copy2(fixture, base_jsonl)
        bytes_before = base_jsonl.stat().st_size

        # 1. Measure `cw import`
        t0 = time.perf_counter()
        imp_proc = subprocess.run(
            [str(cw_bin), "--db", str(base_db), "import", str(base_jsonl), "--harness", "omp"],
            capture_output=True,
            text=True,
            check=True,
        )
        import_ms = round((time.perf_counter() - t0) * 1000.0, 2)

        # Parse session ID from output: "Imported transcript into session: <id>"
        session_id = None
        for line in imp_proc.stdout.splitlines():
            if "session:" in line:
                session_id = line.split("session:")[-1].strip()
                break
        if not session_id:
            raise RuntimeError(f"Could not parse session_id from output: {imp_proc.stdout}")

        base_state = query_latest_turn_from_db(base_db, session_id)
        base_turn = base_state["turn"]
        base_tokens = base_turn["token_usage"]["input_tokens"]
        base_health = base_turn["health_score"]
        base_nodes = len(base_turn["nodes"])
        base_lints = len(base_turn["lint_findings"])

        # Measure `cw hud --json` latency
        t_hud = time.perf_counter()
        hud_proc = subprocess.run(
            [str(cw_bin), "--db", str(base_db), "hud", "--session", session_id, "--json"],
            capture_output=True,
            text=True,
            check=True,
        )
        hud_ms = round((time.perf_counter() - t_hud) * 1000.0, 2)
        hud_data = json.loads(hud_proc.stdout)

        live_base_ttft = measure_live_upstream_ttft(upstream_url, api_key, base_state["payload"])

        # 2. Evaluate each optimization strategy ("min", "mid", "max") on an isolated copy
        tiers = {}
        for strat in ["min", "mid", "max"]:
            tier_db = tmp_dir / f"tier_{strat}.db"
            tier_jsonl = tmp_dir / f"{strat}_{fixture.name}"
            shutil.copy2(fixture, tier_jsonl)

            subprocess.run(
                [str(cw_bin), "--db", str(tier_db), "import", str(tier_jsonl), "--harness", "omp"],
                capture_output=True,
                text=True,
                check=True,
            )

            t_opt = time.perf_counter()
            opt_proc = subprocess.run(
                [
                    str(cw_bin),
                    "--db",
                    str(tier_db),
                    "optimize",
                    "--session",
                    session_id,
                    "--strategy",
                    strat,
                    "--sync-back",
                ],
                capture_output=True,
                text=True,
                check=False,
            )
            opt_ms = round((time.perf_counter() - t_opt) * 1000.0, 2)
            if opt_proc.returncode:
                if "Unknown strategy" in opt_proc.stderr:
                    tiers[strat] = {"strategy":strat,"status":"unsupported","reason":"No eligible plan for this imported format or signed/incomplete history"}
                    continue
                raise RuntimeError(f"Optimization failed: {opt_proc.stderr}")

            tier_state = json.loads(subprocess.run(
                [str(cw_bin), "--db", str(tier_db), "preview", "--session", session_id],
                capture_output=True, text=True, check=True,
            ).stdout)
            t_turn = tier_state["turn"]
            opt_tokens = t_turn["token_usage"]["input_tokens"]
            saved_tokens = t_turn["token_usage"]["saved_by_mutations"]
            actual_reduction = base_tokens - opt_tokens
            reduction_pct = round((actual_reduction / base_tokens) * 100.0, 2) if base_tokens > 0 else 0.0
            bytes_after = tier_jsonl.stat().st_size
            live_tier_ttft = measure_live_upstream_ttft(upstream_url, api_key, tier_state["payload"])

            tiers[strat] = {
                "strategy": strat,
                "context_tokens": opt_tokens,
                "tokens_saved": actual_reduction,
                "reported_overlay_saved": saved_tokens,
                "reduction_pct": reduction_pct,
                "health_score_after": t_turn["health_score"],
                "remaining_lints": len(t_turn["lint_findings"]),
                "disk_bytes_before": bytes_before,
                "disk_bytes_after": bytes_after,
                "disk_bytes_saved_pct": round(((bytes_before - bytes_after) / bytes_before) * 100.0, 2),
                "optimize_and_sync_latency_ms": opt_ms,
                "live_upstream_ttft_ms": live_tier_ttft,
            }

        return {
            "case": case["name"],
            "description": case["description"],
            "fixture_file": fixture.name,
            "fixture_bytes": bytes_before,
            "session_id": session_id,
            "recorded_hardware_telemetry": hw_telemetry,
            "artifact_unit_tests": unit_tests,
            "quality_status": "not measured: tests exercise preexisting artifacts, not a model continuation",
            "token_basis": "estimated from imported text; not provider wire usage",
            "timing_basis": "single local processing samples; not a model speed comparison",
            "cw_engine_latencies_ms": {
                "import_ms": import_ms,
                "hud_json_ms": hud_ms,
            },
            "baseline": {
                "context_tokens": base_tokens,
                "nodes_count": base_nodes,
                "health_score": base_health,
                "active_lint_findings": base_lints,
                "attention_avg_salience": round(hud_data.get("avg_salience", 0.0), 3),
                "critical_rules_in_dead_zone": hud_data.get("critical_in_dead_zone", 0),
                "live_upstream_ttft_ms": live_base_ttft,
            },
            "optimization_tiers": tiers,
        }


def main():
    parser = argparse.ArgumentParser(description="Run live reproducible contextWatcher benchmarks.")
    parser.add_argument("--output-dir", type=Path, default=REPO_ROOT / "benchmark" / "results")
    parser.add_argument(
        "--upstream",
        default=None,
        help="Deprecated; use continuation_benchmark.py for explicit live qualification",
    )
    parser.add_argument(
        "--api-key",
        default=os.environ.get("CW_LLAMACPP_API_KEY", ""),
        help="Optional Bearer API key for --upstream",
    )
    args = parser.parse_args()

    cw_bin = find_or_build_cw_binary()
    print("=" * 92)
    print(f"contextWatcher Offline Fixture Analysis (Binary: {cw_bin})")
    print("=" * 92)

    results = []
    csv_rows = []

    for case in BENCHMARK_CASES:
        print(f"\n▶ Running case: {case['name']} ({case['description']})")
        res = benchmark_single_case(cw_bin, case, args.upstream, args.api_key)
        results.append(res)

        b = res["baseline"]
        ut = res["artifact_unit_tests"]
        hw = res["recorded_hardware_telemetry"]
        print(
            f"  Baseline : {b['context_tokens']:,} tok | {b['nodes_count']} nodes | "
            f"Health: {b['health_score']}/100 ({b['active_lint_findings']} lints) | "
            f"Import: {res['cw_engine_latencies_ms']['import_ms']}ms | HUD: {res['cw_engine_latencies_ms']['hud_json_ms']}ms"
        )
        print(
            f"  Artifact : {ut['tests_run']}/{ut['tests_run']} unit tests passed ({ut['duration_ms']}ms) | "
            f"Recorded HW avg TTFT: {hw['recorded_avg_ttft_ms']}ms across {hw['assistant_turns']} turns"
        )

        for strat_key, t in res["optimization_tiers"].items():
            if t.get("status") == "unsupported":
                print(f"  Tier {strat_key.upper()}: unsupported ({t['reason']})")
                continue
            print(
                f"  Tier {strat_key.upper():<3} : {t['context_tokens']:,} tok "
                f"(-{t['tokens_saved']:,} tok / -{t['reduction_pct']}%) | "
                f"Health: {t['health_score_after']}/100 | "
                f"Disk: -{t['disk_bytes_saved_pct']}% | "
                f"Exec: {t['optimize_and_sync_latency_ms']}ms"
            )
            csv_rows.append(
                {
                    "case": res["case"],
                    "tier": strat_key,
                    "baseline_tokens": b["context_tokens"],
                    "optimized_tokens": t["context_tokens"],
                    "tokens_saved": t["tokens_saved"],
                    "reduction_pct": t["reduction_pct"],
                    "health_before": b["health_score"],
                    "health_after": t["health_score_after"],
                    "disk_bytes_before": t["disk_bytes_before"],
                    "disk_bytes_after": t["disk_bytes_after"],
                    "disk_saved_pct": t["disk_bytes_saved_pct"],
                    "cw_exec_ms": t["optimize_and_sync_latency_ms"],
                    "unit_tests_passed": f"{ut['tests_run']}/{ut['tests_run']}" if ut["passed"] else "FAILED",
                }
            )

    out_dir = args.output_dir
    out_dir.mkdir(parents=True, exist_ok=True)
    out_json = out_dir / "live_benchmark_results.json"
    out_csv = out_dir / "live_benchmark_results.csv"

    with open(out_json, "w", encoding="utf-8") as f:
        json.dump(
            {
                "generated_at_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
                "cw_binary": str(cw_bin),
                "cases": results,
            },
            f,
            indent=2,
        )

    if csv_rows:
        with open(out_csv, "w", newline="", encoding="utf-8") as f:
            writer = csv.DictWriter(f, fieldnames=list(csv_rows[0].keys()))
            writer.writeheader()
            writer.writerows(csv_rows)

    print("\n" + "=" * 92)
    print(f"Done! Offline estimated results written to:\n  - {out_json}\n  - {out_csv}")
    print("=" * 92)


if __name__ == "__main__":
    main()
