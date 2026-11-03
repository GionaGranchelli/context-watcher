# CW-121–127: Core product value and PinContext policy

**Status:** Roadmap decision / planned implementation and qualification, 2026-11-03. This document does not assert that the requested behavior is already enforced. No model runs, cloud spending, server changes, production mutations, or merges are authorized by this plan.

## Product decision

Context-Watcher's primary product is **cache-aware, deterministic context reduction for coding agents**, with independent task correctness and provider-/local-engine efficiency as the success measures. PinContext is *not* a proven performance optimization and must not be treated as a prerequisite for core value.

**Demote PinContext to a user-explicit optional capability.** The desired shipping policy is: automatic pin injection **off by default** in every local and cloud mode; automatic pin policies (including any legacy `auto_cache_safe_pin` and strategy tier that may invoke pinning) must respect this setting. Do not claim that today's implementation already satisfies the target policy. Keep supported manual pinning available, but disclose risk before enabling reinforcement.

Distinguish:
- **Protect from pruning:** Retain selected message(s) through CW's own pruning/deduplication decisions. This does not inject a duplicate instruction, but retained tokens still consume context and can affect latency/cost.
- **Reinforce in prompt (PinContext):** Append/copy an instruction into eligible model-bound requests. This adds tokens and may alter model behavior, lengthen local inference, raise cloud input charges, disrupt prompt caching, or create new tool turns. A pin is not a guarantee of instruction adherence. Unsupported/tool-only/signed request shapes must abstain without modifying the original request.

The separate protect/reinforce UX and enforcement across CLI/TUI/web/API/MCP are *planned*, not implemented by this documentation PR. Protecting message(s) must not be marketed as free or as guaranteed to survive the harness's own compaction.

### Mandatory user-facing risk disclosure

> **Experimental — potentially harmful to speed and cost.** PinContext can increase local inference latency and cloud token charges, change agent behavior, and disturb prompt-cache reuse. Benefits to correctness or task efficiency are not established. Enable only when you explicitly want to reinforce selected information. Review actual overhead; disable or remove the pin if it is not useful.

Target controls: explicit per-session/configuration opt-in, scope/expiry, visible pin contents and destinations, removable/reversible activation, optional per-request and per-session token budgets, application/abstention audit, and cost/cache/latency visibility where trustworthy measurements exist. Report absent cost/latency data as **unavailable**, not zero. Never silently convert an inspection or “protect” action into model-prompt reinforcement.

### Why this is a policy change

- **CW-104B patched local pair, PR #32 (open at decision time):** Both arms passed task constraints. M2 PinContext took 93.24 s vs 59.23 s, used 174,188 vs 144,272 input tokens, and had 6 vs 5 requests; the one-pair observation cannot establish causal or general harm. The pin appeared only on the first M2 request, so persistence and product value remain unproven. [Review PR #32](https://github.com/GionaGranchelli/context-watcher/pull/32).
- **CW-115 cloud historical-prune-only (no PinContext):** One exploratory run reported 24.15% fewer input tokens and 25.49% lower usage-derived cost than passthrough with passing task acceptance. This is a promising observation, not repeatable product proof. See [CW-115 four-mode report](CW-115-FOUR-MODE-ISOLATION-REPORT.md).
- **CW-115 earlier mixed optimization:** Across two repetitions, calculated cost rose 46.2% in aggregate when cache invalidation/behavioral divergence dominated the nominal token pruning. See [CW-115 performance qualification](CW-115-PERFORMANCE-SAVINGS-QUALIFICATION.md).

## Roadmap work and exit criteria

| ID | Priority | Work | Required result / exit evidence |
|---|---|---|---|
| **CW-121** | **P0** | Demote PinContext to opt-in, with explicit risk disclosure and separate protect-vs-reinforce semantics | No automatic pin injection in default configurations, including auto-pilot and preset strategies; positive authorization tests for intended modes; unsupported request types abstain unchanged; TUI/web/CLI/API/MCP affordances and docs clearly warn of potentially higher local latency, cloud cost, and cache disruption. Protect-from-pruning does not secretly inject text. Explicitly verify supported routes; mark others unsupported. |
| **CW-122** | **P0** | Test **historical pruning only** against CW passthrough (no PinContext) | Repeatable, usage-backed reductions in input tokens and calculated provider cost across more than one realistic task family; independent correctness acceptance; paired/counterbalanced runs with reported uncertainty. A negative result is a valid stop decision, not a missing success. |
| **CW-123** | **P0** | Test cache-aware **defer/apply** policy on warm, cold and unknown states | No **unexplained or undisclosed** cache-cost inversions; verify pre-dispatch decision against actual cache telemetry where available; unknown states abstain/defer; log and explain any inversion, and establish a safe fallback. Do not claim a policy can guarantee zero stochastic cost regressions. |
| **CW-124** | **P0** | Test real **local inference** with historical pruning only | Measure actual first meaningful token latency (TTFT), total wall time, prefill and decode time, request count, cached tokens and model/runtime configuration against matched passthrough; report warm/cold cache states separately and include failed/invalid attempts. No theoretical speedups presented as observed. |
| **CW-125** | **P0** | Check **complete agent tasks** under pruning-only optimization | Independently check functional results and preserved constraints/files, tool/protocol integrity and full task completion; no unacceptable correctness or reliability regression against passthrough. Include failed attempts and negative cases. |
| **CW-126** | **P1** | Test **longer sessions near context limits** | Demonstrate lower measured context pressure or fewer required compactions without worse task correctness; compare behavior across context-pressure bands and capture cache-rebuild cost. If no such benefit is reproducible, report that result. |
| **CW-127** | **P2** | Improve **optimization/abstention observability** in TUI and dashboard | For each proposal/application show what changed, why it was applied/deferred/rejected, estimated versus observed token and cache impact, uncertainty/provenance, and measured costs/latency when available; user can inspect raw evidence. |

## Qualification contract (CW-122 through CW-126)

1. **Baseline:** Where feasible use M0 native agent, M1 CW passthrough, M2 CW **pruning-only**. M2 excludes PinContext, other behavioral steering, and automatic tool/schema modifications. Record exact effective strategies and version hashes.
2. **Comparable executions:** Preregister tasks, seeds/sampling where supported, order counterbalancing, independent acceptance tests, warm/cold cache controls, and repetition count. Separate behavioral divergence from mechanical token reduction.
3. **Measurements:** Use real provider usage for actual token counts and cached tokens, prices valid for the tested provider when calculating charges, and model-engine telemetry for local prefill/decode/TTFT. Distinguish measured, modeled, and unavailable values; cost calculations are not invoices.
4. **Safety:** Original transcripts unchanged; no unsupported/signed/tool-tail mutation, no cross-session credential leaks, no unapproved production setting changes, and fail-open/abstain when there is insufficient evidence.
5. **Decision:** Progress only on demonstrated repeatable efficiency gains with acceptable correctness. Report negative and inconclusive results transparently; do not make automatic pruning the default merely because a synthetic benchmark or an isolated run passed.

**Execution authorization:** This is a planning PR only. Local inference workloads and any cloud spend require explicit, separate run authorization and bounded resource controls. Existing CW-120A/PR #32 PinContext evidence remains part of the historical record; do not pool invalid previous treatments with valid pairs or reuse a prior spend authorization.

## Sequencing and scope

- **First:** CW-121 behavior/policy implementation and warning coverage in a dedicated implementation PR; preserve existing safe manual functionality. Complete the offline PinContext exposure investigation already underway without presuming automatic benefit.
- **Then:** CW-122 + CW-123 and CW-125 make pruning-only value and safety measurable; CW-124 adds local real-inference timing; CW-126 validates long-session value.
- **Finally:** CW-127 surfaces proven and unproven results clearly to users. UX warnings required for CW-121 should not wait for CW-127.
- **Out of scope for this PR:** runtime toggle implementation, product UX coding, new proxy integrations, token optimizations, inference experiments, and any merge/approval of other PRs.
