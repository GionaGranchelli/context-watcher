# CW-104B preregistered smoke protocol

Version CW-104B-1. Task inputs, pins, fixtures, and acceptance code are frozen in `benchmark/cw104b/protocol.json` and `freeze.json`. Execution remains gated; this document does not claim readiness.

## Task and treatment

Two families are defined: F1 numeric edge cases with preserved signatures/types and protected files; F3 backing-store caching with preserved signatures and protected store/test/policy files. The one authorized smoke pair uses F3. The opening briefs specify the requested implementation and all constraints. They appear before the same anonymized, genuine historical repository-review record. The final request is exactly: **Implement the change requested in the opening brief.** It does not repeat constraints. M2's frozen pin repeats only the designated early constraints and provides no implementation answer. M1 has no committed rule.

Both arms receive identical user history and initial fixture bytes in fresh workspaces. No independently generated bootstrap conversation is permitted. Native session initialization must be validated without fabricating assistant/tool history. Any unexplained difference in the unmutated model-bound messages or non-message fields stops execution. Use a static system prompt and identical tool definitions; keep workspace identity out of model-bound history. Freeze hashes must match before dispatch.

## Acceptance

`python3 benchmark/cw104b/acceptance.py FAMILY WORKSPACE` runs outside the agent workspace. Passing requires every functional and constraint check: protected-file byte equality, preserved existing signatures, F1 fallback/empty/Decimal behavior, or F3 missing/falsey cache behavior and writes to both stores. Additional private helpers are permitted. Public tests alone are insufficient. `selfcheck_acceptance.py` confirms passing references and failing buggy, protected-file, signature, and behavior cases under Python optimization. Those fixture self-checks are not agent performance results.

## Runtime and cache gates

Use only the named local model and the same CW proxy transport. Temperature 0, top-p 1, seed 104002; five file tools only. No cloud fallback, shell tools, production server configuration change, or cache clear. Initial provider context must be 16,384–32,768 tokens. The recorded history is supplied by its frozen hash from the secure evidence vault; it is not arbitrary filler or a synthetic agent trajectory.

Direct-API A/B/A tests qualify a candidate declared shared-prefix warm-state control only at tiny scale. Before coding, a mock-endpoint OMP check must prove field/boundary transmission and exact request equivalence. Actual runtime counters must then confirm equivalent declared-prefix cache state. If OMP cannot preserve the boundary or cache state differs, stop with BLOCKED_WITH_EVIDENCE. Do not infer preservation from matching text alone.

## Bound and measurements

Run exactly M1 then M2, serialized, under one 60-minute phase deadline including warm-up and setup. Verify idle runtime/no competing inference at checked boundaries. Record each arm before starting the next. Stop on model/usage/treatment/ledger/cache inconsistency, runtime health failure, unexplained mutation, or deadline. Count failures and retries; missing measurements are NOT_AVAILABLE. Preserve bootstrap, coding-only, and complete-session scopes separately.

Primary outcome is independent success with every constraint satisfied. Secondary outcomes are wall time, actual requests/tokens, prefill/decode/cache telemetry, tools/retries, compaction, and available hardware observations. Report absolute and relative pair differences without statistical or causal claims from one pair. A successful smoke permits preparing a later counterbalanced two-family campaign plan; actual full-campaign execution requires separate approval. Never launch it automatically.

## Current gates

READY_FOR_FULL_LOCAL_CAMPAIGN means the prerequisites and one separately authorized patched smoke pair passed. It does not establish product value or authorize the later campaign. Recovery preserves all six historical pairs and their invalid tool-pinning treatment. Tiny/cache-prefix probes, frozen-input equality, strict pin location, full wire agreement, complete usage, independent acceptance, and the 60-minute bound passed for the patched pair at 14:09 UTC. M2 was slower and used more tokens in that pair; both arms passed all task constraints. See `CW-104B-SMOKE-RESULTS.md`.

## Native initialization and request-shape preflight

A local mock endpoint confirmed identical full OMP requests from fresh workspace contents at one canonical path, with a user-only saved session followed by the final request. No assistant/tool events were invented. Offline CW import preserved the complete native session ID. The benchmark hook transmits an explicit seed and a whole-message `strata_prefix` boundary. The earlier character-offset candidate was rejected because the server requires string content for offsets while OMP supplies text arrays. The full committed-pin mock preflight passed; actual long-prefix cache-equivalence checks remain required. These are harness checks, not product-value results.

## Committed-pin mock gate

The isolated CW/native OMP preflight passed: exactly one M2 rule was committed before inference dispatch; the complete captured M2 request equals the M1 request plus the exact expected standalone pin block; original native transcripts contain no pin. Both requests carry the same explicit seed and whole-message shared-prefix specification. No model endpoint was contacted, and the mock/CW processes were stopped. Evidence: `.audit/cw104b/committed-pin-mock-preflight.json`. Actual long-prefix cache-state equivalence remains a runtime stop gate, not an inference from these hashes.

## Frozen-history mock replay

The committed-pin mock gate was repeated using all 103,418 characters of the frozen historical reference. Both arms exited successfully, seed/prefix fields were transmitted, and the full M2 body exactly matched the expected M1-plus-pin transformation. Historical source examples contain literal pin tags, so transcript contamination is checked against the exact experiment block rather than a broad tag search. Captured request bodies remain private; public evidence records their hashes.

## Runtime gate outcome

BLOCKED_WITH_EVIDENCE at the long-history cache diagnostic: concurrent unrelated requests appeared after the single 27,926-token warm-up. No coding smoke arm ran. Preserve that failed diagnostic and its time cost. Resume only in a quiet runtime window after revalidating effective configuration; no automatic retry or full campaign. Tiny cache reuse and frozen-history mock gates remain supporting evidence, not a substitute for this failed runtime gate.

## Reserved-window execution bounds

The user reserved Strata exclusively before resumption. `smoke.py` enforces one 3,600-second phase deadline, at most 1,500 seconds and 12 provider requests per coding arm, and the same thinking-off configuration in both arms. Three cache diagnostics generate at most 16 output tokens each, then exactly one M1/M2 coding pair is eligible. A credential-holding local relay rejects payload inconsistencies before forwarding to the fixed LAN endpoint. It never uses a cloud fallback. These safety bounds are frozen before any new coding result.

## Cache-alignment correction within the original phase

The first reserved-window attempt stopped after M1’s first response: 27,943 reused tokens exceeded the shared-prefix gate value of 27,929 because the last diagnostic had repeated the exact M1 question. No workspace change or M2 coding request occurred. Preserve this invalid attempt. The corrected cache preparation sends the same neutral suffix (`Reply OK.`) before each matched diagnostic and each coding arm. Requalify equal measured reuse before coding. The original 60-minute phase start remains 20261103T134247Z across recovery attempts; do not reset it. No task, pin, fixture, sampling, or model change is permitted.

## Later campaign plan — not authorized for execution

If the smoke passes its independent audit, retain both task families and preregister six matched pairs (three per family), with M1/M2 order alternating and opposite initial order between families. Match the seed within each pair; freeze any chosen seed schedule before execution. Use identical initial source/history/tool definitions and the qualified neutral-prefix alignment before every arm. Keep the same transport and model configuration. Preserve every failure and report both all-attempt reliability and clean-transport diagnostics.

Primary outcome remains full independent functional and constraint acceptance. Predeclare that any new protected-file/signature violation is a failure regardless of speed. Compare paired requests, actual tokens, prefill/decode time, end-to-end wall time, tool errors, and measured reuse. Local computational efficiency is not a billed-cloud-cost result. A candidate improvement must preserve acceptance and show repeatable paired reductions exceeding run variation; report uncertainty, not a causal claim from aggregate totals. If both arms pass all constraints, report a correctness ceiling rather than improved reliability. Stronger discriminating fixtures may need a separate frozen design before spending a full campaign on ceiling-level tasks.

This plan authorizes no additional inference. Larger 32K/64K/92K/128K bands and more task families are separate experiments, after the initial design proves interpretable. No server configuration change or full campaign may be started automatically.
