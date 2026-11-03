# Run the CW-104B offline preflight

The default command is offline. It uses the committed sanitized fixture and the real in-process transform API tests. It needs Python's standard library and the repository's Rust build toolchain and dependencies. It does not need OMP, PyYAML, a provider key, a LAN endpoint, or private history.

```sh
python3 -B benchmark/cw104b/smoke.py
python3 -B -m unittest discover -s benchmark/cw104b -p test_smoke.py -v
```

`--mock-only` is an explicit alias for the same default. `--manifest-dir PATH` changes where the sanitized run manifest is written. CI runs both checks and writes the manifest outside the checkout. Rust dependencies may need the normal initial build download. No inference endpoint is used.

This fixture verifies request lifecycle and integrity. It does not replay the historical coding task, qualify native OMP, measure KV reuse, or establish product value. The historical raw payloads and transcripts remain private. Reviewers without that archive cannot independently replay the original results; the committed hashes and sanitized summaries do not remove that limitation.

## Owner-only private-history preflight

`--owner-replay` explicitly selects the native OMP mock using owner-supplied frozen history. It performs no inference. It requires a built `cw` binary, an installed OMP binary, the private history, and an explicit minimal model profile. Supply a preinstalled native-module cache with `--omp-natives-dir` when OMP needs it in the isolated HOME.

```sh
python3 -B benchmark/cw104b/smoke.py --owner-replay \
  --model-profile /private/models.json --provider LOCAL_PROVIDER \
  --model FROZEN_MODEL_ID --history-file /private/frozen-history.txt \
  --evidence-dir /private/raw-evidence --manifest-dir /private/manifests \
  --omp-natives-dir /private/omp-natives
```

Replace the placeholders with the actual owner inputs. `--cw-bin` and `--omp-bin` override binary locations. The historical reference must match `protocol.json`'s recorded hash. This mode checks native first-request preparation, not the complete historical agent trajectory.

Profiles can be JSON or YAML. Only YAML needs PyYAML. The qualified schema is the existing minimal OpenAI-completions provider with an exact model ID:

```json
{
  "providers": {
    "LOCAL_PROVIDER": {
      "api": "openai-completions",
      "apiKey": "SET_ONLY_IN_THE_PRIVATE_FILE",
      "models": [{"id": "FROZEN_MODEL_ID"}]
    }
  }
}
```

Additional provider/model overrides are rejected instead of silently changing the qualified configuration. Any profile base URL is replaced with the isolated loopback relay. The actual provider key stays in the Python relay and is never passed to OMP. No real key is needed for the owner-only mock.

## Live mode requires a separate, fresh authorization

No live run is authorized by this document, the previous smoke result, or a readiness status. `--live` alone is insufficient. Before another bounded pair, obtain approval and provide a separate authorization JSON file. The example file is deliberately invalid and cannot authorize execution unchanged.

The approval must name a unique authorization ID, approver, approval reference, issue time, expiry, exact private endpoint/model/provider, private evidence directory, and SHA-256 of the current `runner-freeze.json`. It must permit exactly one pair, no cloud spending, at most 3,600 seconds, and at most 30 provider requests including warm-up. It expires within 24 hours. The runner consumes the ID atomically in the bound private evidence directory before endpoint access. Failed setup still consumes approval; retries require a new approval ID.

Only literal private or loopback IP HTTP(S) origins are accepted. Public hosts, DNS names, URL credentials, paths, queries, and fragments are rejected. Use the server origin, not a URL ending in `/v1`. Provider redirects are disabled and environment HTTP proxies are bypassed. There is no cloud fallback.

The syntax, only after that separate approval, is:

```sh
python3 -B benchmark/cw104b/smoke.py --live \
  --authorization-file /private/fresh-approval.json \
  --endpoint APPROVED_PRIVATE_IP_ORIGIN \
  --model-profile /private/models.json --provider LOCAL_PROVIDER \
  --model FROZEN_MODEL_ID --history-file /private/frozen-history.txt \
  --evidence-dir /private/raw-evidence --manifest-dir /private/manifests \
  --omp-natives-dir /private/omp-natives
```

The run manifest records the approval fields, source-document hash, consumption state, runner freeze, and provider admissions. It omits the private evidence-directory path and provider key. This is an operator attestation and an accidental-execution guard, not cryptographic proof of human identity. No test in this change executes the live path against an inference server.

`freeze.json` preserves the historical CW-104B-1 inputs and source hashes. `runner-freeze.json` is the separate current admission/preflight snapshot; it does not retrospectively requalify those observations. The old direct probe scripts are retired before any configuration read. Their source and result records remain for audit; current inference must use this gate.
