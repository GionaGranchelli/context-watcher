# CW-122B2-L0 smoke runner

The default invocation exits `NO_GO`, and `--check` validates an authorization without claiming it or starting a process. `--mock-only` is loopback/offline only. `--run` consumes a separate, exact, single-use smoke authorization and may make one paid request. The authorization remains disabled until a human issues a fresh approval; neither PR #44 nor earlier spend approval authorizes a call. The six-pair campaign remains disabled and needs its own approval.

Pricing is pinned to the dated standard-tier snapshot under `benchmark/cw122b2p/`. Current official rates distinguish short and long context, cached reads, 30-minute and 12-hour writes, and output. Cache-write rates are total rates, not surcharges; see [OpenAI pricing](https://developers.openai.com/api/docs/pricing) and [prompt caching](https://developers.openai.com/api/docs/guides/prompt-caching). Verify these again before issuing an authorization.

Run only the offline suite:

```sh
python3 -B -m unittest discover -s benchmark/cw122b2l0 -p 'test_*.py'
```

For a future operator, first complete independent review, confirm account access and the current pricing snapshot, create an external key file with mode `0600`, a new empty evidence directory with mode `0700`, and a persistent claim directory outside the checkout with mode `0700`. Keep the claim directory after the run, even if evidence is archived or removed. Issue a separate approval that names the exact model, endpoint, executable and runner hashes, one-request cap, deadline, spend ceiling, evidence/credential/claim paths, and the SHA-256 of the exact credential-file bytes; explicitly accept reservation-overrun risk. Fill `authorization.example.json` from that approval; never commit the completed record or credential.

The authorized `runner_sha256` is a bundle digest: it covers `smoke.py` and the local fault-control helper sources imported from CW-122B1/CW-122A. A change to those dependencies invalidates the approval.

Validate without execution:

```sh
python3 -B benchmark/cw122b2l0/smoke.py --check \
  --authorization /absolute/private/authorization.json \
  --cw-executable /absolute/path/to/cw \
  --codex-executable /absolute/path/to/codex \
  --sandbox-executable /usr/bin/bwrap
```

After a human separately authorizes this exact smoke scope, the same arguments with `--run` consume the approval before launch. The run uses the existing `cw wrap` Codex adapter, a private temporary database and home, a read-only temporary workspace, bubblewrap isolation, a shared one-request SpendGuard, and private raw wire/response/transcript/ledger evidence. Codex receives only a dummy local-proxy bearer; host provider credentials and host Codex configuration are unavailable. Redirects are disabled. A run is `PASS` only with one completed response, complete Luna usage, exact model/provider, matching ingress and egress, settled spend within the authorized ceiling, the Codex sentinel, and verified cleanup. Missing evidence is preserved where possible and reported `INCONCLUSIVE`; a failed or interrupted authorization cannot be reused.

Before issuing an authorization, build the exact reviewed CW source with `cargo build --release --locked -p cw-cli` and bind the checkout's `target/release/cw` path plus its SHA-256. Do not bind an older globally installed `cw` binary. No live provider call has been made by the current work. Reservation-overrun risk remains: settlement occurs after a provider response, so the authorized spend ceiling is not a provider-enforced hard charge limit. Keep private evidence outside the checkout and do not publish raw payloads or transcripts.
