# CW-104B local cache qualification

Status: tiny and long-history initial cache reuse VERIFIED after neutral-suffix alignment; product qualification BLOCKED by tool-output pin mutation. Read-only runtime inspection and bounded diagnostic probes were performed. No cache reset or server change was performed.

The recovered CW-104A snapshot reported conversation caching disabled and zero reuse. The current runtime differs: version 0.1.44.1 reports `conversation_cache.enabled=true`, an 8,192 MiB budget, four slots, four parked entries, four restores, and 125,052 reused tokens across seven of eleven requests. The live engine was idle with no queued requests at the observed snapshot. This is current server telemetry, not controlled CW-104B experimental evidence. Who changed the configuration, when it changed, and whether other users own the cached entries are NOT_VERIFIED.

Evidence: `.audit/cw104b/cache-runtime-observation.json`. Preserve its server timestamp; do not attribute these counters to the original CW-104A campaign.

At the initial inspection, the runtime reported caching capability and active reuse. Actual reuse semantics, consecutive OMP request reuse, routing/session/slot behavior, cache-state isolation between matched arms, and restart/configuration requirements remain NOT_VERIFIED pending source/configuration review and a tiny bounded probe after prerequisite validity gates pass. Existing counters alone do not qualify cache preservation or prove equivalent warm-up across arms.

Do not apply BLOCKED_SERVER_CONFIGURATION on the assumption that caching is still disabled. Do not clear shared cache or restart the server automatically. Interpretable cache comparisons require a verified isolated slot/session mechanism or equivalent approved reset. No new smoke campaign is authorized by this document; the user-authorized single pair remains gated on the complete CW-104B validity and measurement checks.

## Bounded consecutive-request probe

Two genuine local requests completed with the advertised model, temperature 0, top-p 1, seed 104002, and at most 16 output tokens per request. Both stopped normally and the engine was idle without a queued request at each checked boundary. The first response reported 123 prompt tokens / 0 cached; the consecutive request reported 148 prompt tokens / 124 cached. Elapsed request-plus-metrics time was 2.10 and 0.52 seconds. These are diagnostic observations, not benchmark speedup estimates. Server-side request telemetry is preserved in `.audit/cw104b/cache-probe-result.json` for corroboration.

Runtime documentation says parked conversations are matched by token/image prefix and steering mode, without client session IDs. Therefore different CW/OMP session IDs alone do not establish isolation: running M1 first can warm M2. A documented request-level shared-prefix mechanism is a candidate for equivalent warm-state control, but it has not been qualified for this harness or shared-server impact. Coding smoke remains gated on this check; no configuration change or larger campaign was performed.

## Declared shared-prefix A/B/A probe

Three further tiny local requests used `strata_prefix: {"messages": 1}`, one unique shared system-document prefix, and question order A/B/A. Prompt length was 106 tokens for each. Reported cached tokens were 0/85/85, corroborated by server reused-token counters. Request counters advanced by one at each boundary, all checked boundaries were idle with no queued work, and model identity matched. Returning to A did not reuse the complete previous A question. This supports equivalent declared-prefix warm state for this direct-API diagnostic. It does not qualify cache isolation for OMP or a long coding history. No server configuration change, cache clear, or reset was performed.

The coding smoke gate now requires demonstrating OMP transmission of the same declared shared-prefix boundary, with identical unmutated task history, plus matched per-arm cache telemetry. If this cannot be verified, stop as BLOCKED_WITH_EVIDENCE rather than running an uninterpretable pair.

## Long-history diagnostic: stopped on concurrent traffic

The frozen-history mock passed exact full-request comparison, including the seed, whole-message boundary, and one committed M2 pin. A bounded real diagnostic then dispatched exactly one warm-up request, capped at 16 output tokens. Server telemetry confirms a 27,926-token prompt, inside the frozen 16K–32K band. No coding agent or tool execution started.

Immediately after that response the runtime was busy. A follow-up read-only snapshot showed unrelated 60- and 164-token requests, with four requests queued and an active 1,024-token generation limit. Those do not match the diagnostic request. The diagnostic stopped without dispatching its M2 or returning M1 request. Equivalent long-prefix warm state remains NOT_VERIFIED. Blocker at that attempt: **concurrent inference on the shared runtime**; this is not evidence of a Context Watcher mutation defect or lack of cache support.

The current cache snapshot reports 16,384 MiB and eight slots, differing from the earlier 8,192 MiB/four-slot observation. No settings were changed by this experiment. Snapshots must not be treated as one fixed server configuration. A quiet execution window and a fresh effective-runtime snapshot are needed before resuming the gate. Do not stop other workloads, clear cache, or restart the server automatically.

Evidence: `.audit/cw104b/long-prefix-probe.json` and `long-prefix-probe-postfailure.json`. The runner initially checked runtime health before persisting its response; therefore the provider usage object was lost. It is explicitly NOT_AVAILABLE, while the corresponding server record is retained separately. The runner now persists responses before post-request health checks and refuses to overwrite an existing attempt. No replacement inference was launched to fill the missing field.

## Reserved-window outcome

After the user reserved Strata, equal first-coding-request reuse was measured at 27,929 tokens in each arm. The initial cache-alignment gate is therefore verified for this frozen history/runtime. The original exact-question warm-up attempt is retained as failed preparation. Product qualification remains blocked because five M2 follow-ups pin tool-output strings, destabilizing later request prefixes. The implementation is fixed locally and verified offline; no replacement pair ran. See `CW-104B-SMOKE-RESULTS.md` for the authoritative current verdict and complete measurements.
