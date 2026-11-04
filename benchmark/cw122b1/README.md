# CW-122B1 Codex mock qualification

Run the installed Codex CLI through Context Watcher to the in-process mock:

```sh
python3 benchmark/cw122b1/run_mock_campaign.py --evidence-dir /absolute/private/path/cw122b1
```

The evidence directory must be empty, outside the checkout, and mode `0700`. Raw JSON/SSE and Codex transcripts stay there with mode `0600`; do not commit them. The runner is offline-only, runs exactly 12 attempts with a 360-request ceiling, and rejects any extra mode such as `--live`.

The ignored Rust test is intentionally launched through this runner. Default CI runs the deterministic redirect regression and normal tests, but does not require a local Codex installation or run the mock campaign.

See [the qualification report](../../docs/CW-122B1-CODEX-MOCK-QUALIFICATION.md) for scope, results, limitations and the live gate.

The accepted host run's [sanitized aggregate](mock-aggregate.json) is safe to commit. It contains hashes and synthetic mock telemetry, not raw conversations.
