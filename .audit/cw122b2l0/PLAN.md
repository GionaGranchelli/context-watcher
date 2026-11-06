# CW-122B2-L0 execution plan

## Done condition

A mock-only Codex smoke run proves that one OpenAI Responses request passes through the CW proxy, uses the selected model, produces complete usage, settles to the exact expected GPT-6 Luna cost, writes private raw wire/ledger/transcript evidence with hashes, and cannot start without a fresh single-use authorization matching the runner and limits. No provider call or spend occurs in this task; the real smoke remains `NOT_AUTHORIZED`.

## Scope and blockers

Expected change: SpendGuard pricing/profile and usage parsing; a small live-only Codex runner; a distinct smoke authorization contract and validator; mock tests for authorization, usage, endpoint behavior, fallback, deadlines and resource limits; operator instructions and sanitized evidence metadata. No pruning/cache policy or server changes. PR #44 is still open, so this branch is stacked on its corrected head and must be retargeted after #44 merges. Independent approval and explicit exact-scope spend authorization are required before real execution.

## Phases

1. Ground SpendGuard, Codex adapter, proxy transport and private evidence paths; preserve baseline checks.
2. Select and record the pricing/authorization data model before implementation.
3. Add exact tiered settlement and fail-closed telemetry validation with literal known-value tests.
4. Build a one-attempt runner that defaults to NO-GO and validates a separate single-use authorization before launching Codex.
5. Exercise the real runner path against local mocks, including absent/expired/mismatched/reused authorization, model mismatch, incomplete usage, redirects/fallback, timeouts and resource limits.
6. Verify private evidence permissions and hashes, run deterministic suites, open a focused PR, and leave it unmerged.
7. Keep live execution disabled pending independent PR review and a separate explicit authorization; keep the six-pair campaign separately disabled.
