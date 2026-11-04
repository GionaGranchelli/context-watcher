# CW-122B1 Codex mock qualification

Run the installed Codex CLI through Context Watcher to the in-process mock:

```sh
python3 benchmark/cw122b1/run_mock_campaign.py --evidence-dir /absolute/private/path/cw122b1
```

Run from a Git checkout: the runner records `git rev-parse HEAD`, and does not accept a caller-supplied commit identifier. Source archives without Git metadata are unsupported. The evidence directory must be empty, outside the checkout, and mode `0700`. Raw JSON/SSE and Codex transcripts stay there with mode `0600`; do not commit them. The runner is offline-only, runs exactly 12 attempts with a 360-request ceiling, and rejects any extra mode such as `--live`.

The ignored Rust test is intentionally launched through this runner. Default CI runs the deterministic redirect regression and normal tests, but does not require a local Codex installation or run the mock campaign.

Run the CW-122B1-F1 fault matrix without contacting a model provider:

```sh
python3 -B benchmark/cw122b1/run_mock_campaign.py --evidence-dir /absolute/private/path/cw122b1-success
python3 -B benchmark/cw122b1/run_fault_matrix.py \
  --evidence-dir /absolute/private/path/cw122b1-f1 \
  --success-path-evidence-dir /absolute/private/path/cw122b1-success
```

It runs deterministic proxy and runner tests, then the Codex loopback fault cases when `codex` is available on the minimal allowlisted `PATH`. It writes raw evidence outside the checkout with mode `0700` directories and `0600` files, and updates the sanitized `fault-matrix.json`. `BLOCKED_WITH_UNVERIFIED_SCENARIOS` means every executed test passed but at least one required execution layer or interruption boundary remains unverified. It never enables live provider traffic.

See [the qualification report](../../docs/CW-122B1-CODEX-MOCK-QUALIFICATION.md) for scope, results, limitations and the live gate.

The accepted host run's [sanitized aggregate](mock-aggregate.json) is safe to commit. Artifact fields named `*_sha256` are computed with SHA-256; the aggregate identifies the algorithm. It contains hashes and synthetic mock telemetry, not raw conversations.
