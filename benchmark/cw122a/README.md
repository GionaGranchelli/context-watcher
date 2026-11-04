# CW-122A offline campaign

This directory qualifies controls and analysis, **not product value**. No provider, agent binary or model server is invoked. The only network traffic is from the Rust proxy fixture to a mock upstream bound on loopback.

```sh
python3 -B -m unittest discover -s benchmark/cw122a -p test_campaign.py
python3 -B benchmark/cw122a/rehearse.py --output /tmp/cw122a-new-evidence
python3 -B benchmark/cw122a/analyze.py /tmp/cw122a-new-evidence/runs.json
```

Use a new output directory for every rehearsal. Initial task trees, instructions, oracles, traces and configuration are hash-bound by `freeze.json`. `synthetic-solutions.json` supplies predefined repairs only to the offline simulator. It is never a model prompt or evidence of agent capability. The Rust fixture captures complete upstream payloads, checks the production ledger, and emits `wire.json`. Each run has one mock request; complete multi-request agent-session execution remains CW-122B work.

The analyzer labels synthetic usage and frozen synthetic durations explicitly. Its mock prices are not provider prices. Missing input/output/cache usage stays null and disqualifies complete economic comparisons. Failure and abort costs, times and outcomes stay in `all_attempts`; unmatched or invalid pairs are listed rather than pooled. Local pruning/KV estimates are separate from normalized usage-derived metrics. First-byte latency is not TTFT.

`--live` always fails before executing a subprocess or network request. `admission.py` validates and can single-use-claim an operator authorization **without dispatching inference**. The example authorization is deliberately invalid. The future runner must attach the existing production SpendGuard, enforce global request/run/time limits before dispatch, and qualify its provider/harness adapter before live execution. Reservations are not guaranteed maximum charges.

See [preregistration](../../docs/CW-122A-PREREGISTRATION.md) and [readiness gates](../../docs/CW-122A-READINESS.md). After review, CW-122B requires a separate fresh authorization; no previous campaign approval applies.
