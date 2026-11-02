# CW-120A Product Value Experiment Readiness

**Status:** Offline readiness complete; live execution blocked on fresh spend authorization (`BLOCKED_SPEND_AUTHORIZATION`).  
**Baseline:** `b1279912e8f2628ccc08090e1da6fec4061dbf3a` (PR #30 merged)  
**Product value:** `NOT_EVALUATED`; the offline report is synthetic and is not product evidence.

## Baseline reconciliation

The pre-implementation review is recorded in [CW-120A gap analysis](CW-120A-GAP-ANALYSIS.md). CW-118 already supplies the F1/F3 fixtures, acceptance commands, cost calculator, failure-inclusive accounting, paired M2/M1 analysis, bootstrap summaries, mock proxy tests, saved-report behavior, and resume tests. CW-119C supplies the bounded Codex M1/M2 runner, SpendGuard, durable diagnostics, and saved exact-egress proof for one stateless PinContext plus seven unchanged tool follow-ups.

The available live runner remains a single F1 pair. It cannot itself launch the requested 12 live sessions or execute F3. CW-120A reuses its evidence-verification and spend-boundary tests and adds a narrow offline schedule/report rehearsal. The rehearsal does not claim that synthetic Codex behavior, PinContext delivery, cache effects, or savings were measured.

## Frozen hypothesis and decision rules

> A committed, cache-safe PinContext intervention reduces provider cost per successful autonomous coding task relative to Context Watcher passthrough, without reducing correctness.

PinContext is behavioral steering; it does not remove historical context. The frozen CW-118 thresholds remain unchanged: no correctness regression; mean relative cost change below zero; median relative cost change at or below -10%; evidence of benefit in both task families; no unexpected cache invalidation or proxy-induced reliability failure; and every applied intervention independently verified. Failed attempts remain in all-attempt economics. The 12-session result is directional feasibility evidence only. It cannot establish `SUPPORTED_FOR_PILOT`; expansion requires review of these results and a separately authorized full CW-118 Gate B campaign.

## Frozen study configuration

- **Families:** F1 bug fixing (`math_ops.py`) and F3 cache feature (`cache.py`), using existing CW-118 fixture files and independent unittest suites.
- **Repetitions:** Three independent paired repetitions per family; recreate pristine files for each mode, with matching initial fixture hashes within each pair.
- **Modes:** M1 proxy passthrough with zero committed rules; M2 proxy with exactly one explicitly committed PinContext rule. Both use the same Codex version, model, reasoning/configuration, tool setup, and task prompt within each pair. No M0 arm.
- **Directive, frozen for every M2 attempt:** `Keep the requested change focused. Inspect relevant code and tests before editing. Run the relevant tests after editing and report any failures honestly.` Do not tune it after observing outcomes.
- **Randomization:** Seed `1202026`; shuffle the six `(family, repetition)` blocks and separately shuffle three M1-first and three M2-first blocks. The resulting exact order is in the rehearsal report. Reuse the same order for later live execution; no rerandomization after looking at results.

| Pair | Order |
|---|---|
| F1-R2 | M1, M2 |
| F1-R3 | M2, M1 |
| F3-R3 | M1, M2 |
| F1-R1 | M2, M1 |
| F3-R2 | M1, M2 |
| F3-R1 | M2, M1 |

## Measurement and analysis

For every attempt, retain family, pair, mode, order, commit, Codex version, model, acceptance result/failures, usage-derived cost, input/cached/uncached/output tokens, request and tool iteration counts, wall time, pin applications and abstentions, ledger/egress result, provider cache telemetry, SpendGuard admitted/reserved/settled values, and termination status. Persist diagnostics before assertions. Missing usage is unpriced and invalid for paired cost calculations; do not impute zero. Keep failed runs and their incurred spend in all-attempt cost. Report cost per attempt, cost per successful task, paired dollar/relative changes, acceptance-rate differences, requests, latency, and sample-size uncertainty. Separate mechanical context reduction (expected zero), behavioral request/iteration changes, provider cache behavior, and proxy overhead.

## Offline qualification and evidence

Command: `python3 benchmark/cw120a_offline_readiness.py --output target/cw120a-offline-readiness.json`.

The script imports CW-118 fixture, acceptance, synthetic session, pricing, and analysis functions. It creates each attempt in a fresh temporary workspace, runs the independent fixture tests, verifies matched fixture hashes, checks six paired blocks and balanced first position, and writes a 12-session report. The committed report is [cw120a-offline-readiness.json](cw120a-offline-readiness.json). Its `$0.0999` modeled cost and zero relative change are synthetic pipeline checks only; actual provider calls and spend are zero.

Existing and focused deterministic coverage verifies M1 wire passthrough, committed M2 proxy mutation, one exact stateless PinContext and unchanged tool-output follow-ups, changed-egress/ledger failure classification, diagnostic persistence before assertions, cached versus uncached accounting, unknown/missing pricing behavior, failed-attempt accounting, and resume idempotency. Run `cargo test -p cw-proxy --test product_value_qualification_test` and `cargo test -p cw-proxy --test live_pilot_test` for these offline gates. The ignored real-provider case remains ignored and was not run.

This is an offline readiness rehearsal, not the missing multi-family live executor. Before Gate B, extend the existing bounded CW-119C Codex runner only enough to execute this frozen F1/F3 schedule, using its isolated per-session workspaces and SpendGuard/SQLite evidence path; verify the implementation offline and preserve the manifest/order. Do not add provider integrations or general-purpose benchmark layers.

## Financial proposal and authorization gate

The known CW-119C diagnostic pair used 9 M1 and 13 M2 requests. Its usage-derived costs were `$0.011075` and `$0.017005` at the then-used GPT-4o mini rates; they are one F1 observation, not a forecast or reconciled bill.

OpenAI now lists `gpt-4o-mini-2024-07-18` for shutdown on January 14, 2027, with `gpt-6-luna` as the replacement. The official model page lists GPT-6 Luna at `$0.10/M` input and `$0.50/M` output; current standard short-context pricing lists cached input at `$0.01/M`, and 30-minute cache writes at `$0.125/M` (12-hour writes cost more). The offline pricing profile uses these standard rates. Cache-write token counts are absent from historical CW-119C telemetry, so the following estimate excludes them and must be reconciled against complete provider usage before any execution. Revalidate exact model availability, Codex CLI compatibility, account access, pricing tier, and cache-write treatment immediately before a future campaign. Sources: [GPT-6 Luna model page](https://developers.openai.com/api/docs/models/gpt-6-luna), [OpenAI pricing](https://developers.openai.com/api/docs/pricing), [GPT-4o mini deprecation](https://developers.openai.com/api/docs/deprecations).

Applying current GPT-6 Luna standard rates to the known token totals gives an approximate historical-shape pair of `$0.01278`; six pairs are `$0.07665`. A planning stress factor of 3x per-attempt token/request volume plus 25% retry allowance gives about `$0.29` expected stress spend. This is only a planning estimate: F3 and longer runs may differ, cache writes may add charges, and retries are retained as attempts.

Proposed controls for later approval: `$1.80` aggregate nominal local SpendGuard ceiling (`$0.15` per each of 12 attempts), `$0.01` per-request admission reservation, and at most 25 provider requests per attempt. The per-attempt ceiling normally binds before all 25 reservations can be admitted. Settlement occurs after dispatch, so one admitted request can cost more than its reservation before the guard reacts. No provider-side hard spending cap has been independently verified. These local limits are not guaranteed maximum charges. The prior CW-119C authorization is spent and is not reusable. **No live provider request is authorized by CW-120A.**

## Outcomes for a future live decision

- **PROMISING:** Directional savings, preserved correctness, and no unexpected safety failures; review and propose the complete CW-118 Gate B study.
- **INCONCLUSIVE:** Small/inconsistent effects, insufficient treatment exposure, or wide uncertainty; report limits and decide whether a larger campaign is justified.
- **REGRESSION:** Material adverse cost, correctness, or reliability results; stop or revisit the PinContext strategy before further trials.

Only a separately reviewed full Gate B qualification may establish `SUPPORTED_FOR_PILOT`.
