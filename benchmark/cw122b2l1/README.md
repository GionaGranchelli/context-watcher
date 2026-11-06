# CW-122B2-L1 smoke runner

The runner is default NO-GO. Running it without `--run` is offline-only. The ignored Rust integration test is the only path that calls the provider; normal `cargo test` does not run it.

## Smoke scope

One frozen F1 pair (`f1-r1`), serial M1 then M2, GPT-6 Luna on the OpenAI Responses endpoint, standard tier. The shared SpendGuard caps the pair at 60 requests and $5, with 30 requests and 600 seconds per arm and 1,200 seconds overall. SpendGuard reserves before dispatch and settles after usage arrives, so actual charges can exceed the ceiling before the guard stops later requests. The separate authorization must explicitly accept this overrun risk.

This pair verifies task acceptance and an actual provider-bound prune. One pair cannot establish product value or cache economics. Cache state remains unverified; the six-pair campaign stays disabled and requires its own authorization.

## Authorization

A fresh authorization JSON must contain exactly the fields in `SMOKE_FIELDS` in `benchmark/cw122b2p/authorization.py`. Bind the current runner bundle hash, campaign, model, endpoint, pricing/profile files, installed Codex binary, credential-file hash, request/deadline limits, a spend ceiling no greater than $5, and private evidence/claim paths. Record a human approval reference and expiry no more than 24 hours after issue. Store the authorization as mode 0600, the credential as a regular mode-0600 file, and empty evidence plus persistent single-use claim directories as mode 0700, all outside the checkout. Never put the key in the authorization JSON or command line.

The runner hash covers the imported process controls and their environment/rehearsal/analyzer dependencies, full `cw-core`, `cw-store`, `cw-proxy`, and `cw-harnesses` source trees, the F1 fixture and independent oracle, campaign/pricing inputs, and the test itself. Symlinks in the hash bundle are rejected. It recomputes source, pricing, campaign, and Codex hashes; checks the credential hash and paths; validates the exact scope; atomically consumes the authorization ID; and then launches the ignored integration test with a minimal environment. It keeps raw test output and evidence in the private evidence directory. A consumed authorization cannot be retried; issue a new, separately approved authorization for a new run.

## Checks

```sh
python3 -m unittest discover -s benchmark/cw122b2l1 -v
python3 -O -m unittest discover -s benchmark/cw122b2l1 -v
python3 benchmark/cw122b2l1/run_smoke.py
```

The final command must print `NO_GO` and exit 2 without an authorization. A live run requires a reviewed, separately approved file:

```sh
python3 benchmark/cw122b2l1/run_smoke.py --run /absolute/private/path/smoke-authorization.json
```

Do not use the CW-122B2 six-pair campaign authorization for this smoke.
