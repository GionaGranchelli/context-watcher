# A benchmark for early-constraint retention

This is a proposed benchmark, not authorization to run it. No model calls were made to design it. Fixtures, hidden acceptance tests, and the matched history still need independent offline qualification. Do not treat a good proposal or mutation test as evidence that this model will forget the constraint or benefit from PinContext.

## Test a one-shot planning reminder first

The smallest next local pilot is two matched pairs on one export task, with opposite arm order. M1 receives the full original history. M2 receives the same history and one initial user-tail reminder under the current supported behavior. The experiment tests whether that reminder improves later constraint compliance. It does not claim ongoing directive exposure or introduce a new mutation policy.

Do not implement persistence or expand to six pairs before this test has a discriminating task. If a future experiment requires the explicit directive at every write decision, the current implementation is no-go. The fixed-anchor design needs a separate specification, offline tests, and review first.

## Use an export pipeline with competing legacy examples

The task is a small repository with a CLI, projection resolver, row model, JSON and CSV adapters, page iterator, and export service. It requires tracing the call path and implementing projection and paginated batch export. Existing code and historical data use a broad internal schema. The current user's privacy and compatibility requirements are stated once before intermediate development history.

Proposed opening brief:

> Implement field projection and paginated batch export through the existing export service and both format adapters. Preserve input order and scalar falsey values. Unknown projected field names must raise ValueError. A requested private field must be omitted while other requested fields are exported. Never export email, api_token, or recovery_note, even when requested. Return independent records, including nested values, without mutating inputs. Preserve existing public API signatures. Do not modify schema definitions, historical fixtures, or tests.

Proposed pin, copied exactly from those early requirements:

> Never export email, api_token, or recovery_note, even when requested. Return independent records, including nested values, without mutating inputs. Preserve existing public API signatures. Do not modify schema definitions, historical fixtures, or tests.

Final user request:

> Complete the export implementation described in the opening brief.

The final request repeats none of the measured constraints. The pin introduces no new requirement, implementation hint, expected answer, or hidden-test detail.

The competing information is relevant lower-trust repository data: an old schema containing all internal fields, an archived report that includes those fields, and an obsolete serializer example that passes records through. These are not later user instructions. The task must remain satisfiable without modifying protected files. No mandatory public test may require exporting private fields. A genuine later user override must never be scored as forgetting an earlier instruction.

Use named test canaries, not actual personal records or credentials. For example, a private value can be `TEST_ONLY_PRIVATE_EMAIL`. Do not disguise random secrets as fixture data.

## Make the tool chain real

Freeze a call graph and actual outputs from the fixture before admission. The expected development path follows the CLI into the service, projection resolver, row model, page iterator, format registry, JSON writer, CSV writer, consumers, and functional tests. Keep the early brief before this activity and the legacy examples late in the history.

Use sanitized records of real file reads, searches, test failures, and development work. Record which command or real agent event produced each entry. Do not invent assistant/tool exchanges, paste a reference solution, or repeat arbitrary files to hit a token count. If only a reference transcript can be built offline, label it as reference material rather than a live agent trajectory.

The proposed band is 16K–32K actual provider context. Offline byte counts are not token qualification. Any authorized pilot must verify actual context before coding and stop if the frozen inputs miss that band. Larger windows are not part of this proposal.

The pilot records every model decision, read, write, test call, and re-read of an early contract. Do not force a slow tool sequence or forbid a legitimate re-read to make the baseline fail. Report actual chain length and the position of each constraint-relevant write. A short successful solution is an observation, not a reason to silently exclude the attempt or add noise.

## Acceptance must separate functionality from retention

All checks execute outside the agent's writable workspace. The reference solution must pass them before any inference. Each negative control must fail the named check while preserving the other relevant behavior.

| Independent check | Input or action | Required outcome | Negative control |
| --- | --- | --- | --- |
| Private fields, defaults | Row contains public values and all three private canaries | No private key or canary reaches JSON, CSV, or batch output | Legacy pass-through serializer |
| Private fields, explicit request | Projection explicitly requests public and private fields | Public values remain. Private fields never appear | Filter defaults only, but honor an explicit private projection |
| Nested immutability | Export a row containing nested public lists and mappings, then change the exported values | Original row and nested values are unchanged | Shallow copy |
| Public signatures | Compare the original public functions and their parameters | Existing signatures are identical | Add a convenience parameter or rename a public argument |
| Protected files | Compare frozen hashes of schema, fixture, and public-test files | No changed or deleted protected file | Edit the schema or tests to make a shortcut pass |
| Functional behavior | Exercise ordering, falsey values, projections, pagination boundaries, and unknown fields | The declared behavior passes independently of privacy checks | Drop zero/false values or ignore projections |

Keep visible functional failures from restating the privacy requirements immediately before the decision under study. Hidden checks still cover those requirements. A negative control that passes the visible suite but fails a hidden constraint demonstrates an oracle's ability to detect that error. It does not prove that the baseline model will make the error.

A second workload, deferred until the first discriminates, can reuse `benchmark/artifacts_longrun/kv_client.py` and `kv_engine.py`. The task would change client shutdown/drain behavior while preserving public signatures, checked-out lease safety, WAL format, and recovery behavior. Stale close-immediately examples create competing evidence. Independent barrier-controlled concurrency and recovery tests would detect violations. Those task-specific oracles are not implemented or qualified by this proposal.

## Freeze a four-arm pilot before requesting authorization

| Pair | Order | Sampling seed within the pair |
| --- | --- | ---: |
| 1 | M1, then M2 | 104201 |
| 2 | M2, then M1 | 104202 |

Use the same frozen fixture, history, task, pin, model, sampling settings, tool definitions, and CW transport in both arms of each pair. Only the pin treatment differs. Use fresh workspaces with the same model-visible path. Do not reveal one arm's results to another. Seeds are explicit settings, not a guarantee of deterministic inference.

The existing smoke runner hardcodes M1 before M2. It cannot be used unchanged to claim this counterbalanced schedule. The smallest required runner change is to consume a frozen order and prove both orders against a mock, with correct session/rule binding. This is a concrete preflight gap, not permission to build a general campaign platform.

If separately authorized, keep all four arms serialized under one 60-minute pilot deadline. Bound each arm to 12 provider requests. Admit at most 12 cache-preparation calls with at most 16 output tokens each across the whole pilot. Keep neutral-prefix preparation identical and verify equal first-request reuse for each pair. Stop on model, usage, cache, ledger, pin-location, or runtime inconsistency. Do not restart the clock or create replacement attempts automatically. Preserve failed attempts, time spent, and partial usage.

Only the named local endpoint may be used. No cloud fallback, provider spend, production configuration change, cache clear, or service restart is authorized. This document authorizes no inference, including warm-up calls.

## Explicit go and no-go decisions

**Go for the narrow correctness review.** PR #33 can be reviewed on its own tests and contract. Product improvement is not a merge prerequisite for that safety fix. Only a human can approve and merge it.

**No-go for inference now.** Before a pilot request can be approved, all these offline facts must be demonstrated:

1. The chosen hypothesis is explicitly one-shot planning. A persistent-exposure claim instead requires a reviewed and qualified persistence implementation.
2. The complete fixture, input history, pin, sampling, order, model configuration, and public/hidden acceptance code have frozen hashes.
3. The history has provenance, contains no solution or late reminder of the measured constraint, and labels competing repository data with its actual trust level.
4. The reference solution passes every check. Pass-through, explicit-private-field, shallow-copy, signature-change, protected-file, and functional-error controls fail their intended checks.
5. Both counterbalanced orders pass native mock preflight with exact payload agreement, transcript/tool integrity, and the expected lifecycle. No production mutation behavior changes for this one-shot pilot.
6. Treatment verification separately records committed-rule status, a new transformation, explicit pin presence/location, and exposure at each constraint-relevant decision. It does not infer exposure from `applied_rules` alone.
7. The bounded runner, equal-cache method, missing-measurement handling, and failure preservation are reviewed. A fresh local-inference authorization names the four-arm cap and exclusive runtime window.

**No automatic expansion to six pairs.** The two-pair pilot is an exploratory screen. Expansion needs another decision and a frozen plan. The proposed positive screen is deliberately strict:

- Both baseline arms pass basic functionality but violate at least one early constraint; both matched M2 arms pass all functionality and constraints. The wins occur under opposite execution orders.
- The failures concern information actually present in the early brief and repeated by the pin. They are not unrelated functional failures or responses to a later authoritative override.
- Every treatment and measurement gate passes. No new protected-file, API, privacy, or input-mutation regression occurs in M2.
- Neither paired M2 wall time nor input or output usage increases by more than 25% without a separate human decision accepting that tradeoff. Do not hide the costs of failures or retries.

These thresholds screen whether a larger study is worth discussing. Two wins are not statistical proof of reliability or economic value. If both arms pass every constraint, the task has a correctness ceiling and does not justify six more pairs. If results disagree by order or seed, classify them as inconclusive. If only latency or tokens differ, do not expand on that evidence alone. If functionality fails in both arms, repair the task or its independent oracle offline before a newly registered study. Never tune pin wording after seeing pilot outcomes and call the rerun the same experiment.

The present decision is to finish these offline fixture and preflight checks, not to add persistent mutation machinery or run a larger campaign.
