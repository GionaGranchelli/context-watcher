# CW-121B: Consent-aware PinContext creation

Status: implementation for independent review. Based on merged PR #37. This revision runs no inference, model-server calls, spending, production changes, or automatic merge.

## Creation contract

Automatic policies and strategy execution require the session's existing reinforcement consent before creating PinContext rules. Direct manual pin creation may still store committed or draft rules without consent. Neither creating nor committing a rule grants consent.

`auto_cache_safe_pin` suppresses candidates before constructing rules when consent is absent. Low-health policy plans filter pin candidates and retain non-pin operations. Suppressed candidates are reported separately from generated rules. Optimizer plans remain proposals: inspecting a plan does not create anything.

All derived persistence uses `save_generated_mutations_at_revision()`. It checks the current session policy and provider inside the same immediate SQLite transaction that writes the rules. A stale proposal or previously read grant cannot bypass a later revocation. Session mismatch and cross-session ID conflicts fail the batch. Other operators retain the existing persistence and application behavior. Direct manual writes use the existing APIs unchanged.

The proxy evaluates proposals on a copy, saves only authorized derived rules, then applies the persisted set through CW-121A's final application guard. The guard, transcript immutability, signed/unknown content abstention, session scope, and Gemini restriction remain in force. Import/sync paths still do not write reinforcement into source transcripts.

Revocation stops subsequent automatic creation and leaves stored rules intact. As in CW-121A, this does not cancel an application already running with its policy snapshot. Gemini automatic pin creation is blocked even with consent. Manual Gemini rules can be inspected but do not become eligible for reinforcement.

## Read-only activation preview

The existing authenticated `GET /api/v1/sessions/:id/pin-context-policy` now also returns:

- `stored_pins`: rule IDs, labels, complete contents, stored status, current session authorization, and nullable `eligible_in_latest_request_preview_after_opt_in`.
- `activation_scope`: consent permits all eligible stored and future pins in this session until revocation or session end. It is not approval of specific content.
- `automatic_generation`: effective global enablement, auto-pin/low-health flags, recovery strategy, whether those configured paths may generate future pins after opt-in, and whether they are currently permitted.
- `strategy_application_may_generate_after_opt_in`: strategy-derived creation remains possible when qualified, independently of background auto-pilot enablement.
- `preview_basis` and `risk_warning`: eligibility refers only to the latest recorded request. Null means there is no request to evaluate. Every future request still needs its own shape/safety checks. Latency, cache, token cost, and behavior may worsen; benefits remain unproven.
- `measured_savings: null`: preview is not a provider measurement.

Preview applies to an in-memory copy only. It creates no rules or grants, and writes no transcript or turn. Stored drafts and disabled rules remain distinguishable from committed candidates. The grant/revoke POST contract is unchanged. Operators can inspect contents and scope before enabling; this bounded change does not add a content-specific approval mechanism or a new activation UI.

## Complete activation-path inventory

| Entry point | Classification and gate | Reporting / remaining work |
|---|---|---|
| Proxy `auto_cache_safe_pin` | Automatic; core consent filter plus transactional derived-write gate | Suppressed candidates are not generated or applied rules |
| Proxy low-health recovery, including strategy fallbacks | Automatic; filter pin items, preserve non-pin items, recheck at write | Only persisted rules reach final application |
| Harness import auto-heal | Automatic; transactional derived-write gate | Existing source-transcript and pin-disabled disk behavior retained |
| Core optimizer/preset construction and lint suggestions | Proposals only, no persistence | Candidates are not authorization or application |
| REST `/optimize`, with preset, selected items, or lint suggestions | Derived creation; gate applies to draft and committed requests | `proposed_count`, `created_count`, `committed_count`, `creation_skipped_rules`, and preview-only `applied_count` |
| CLI optimize, batch optimize, and aliases | Derived creation; shared transactional gate | Actual creation/skips reported separately from plan estimates; no hypothetical value substituted for measured/preview tokens |
| TUI strategy execution | Derived creation; shared transactional gate | Proposed/created/skipped counts; no claimed measured savings |
| Web strategy actions | REST derived creation | Confirmations distinguish proposals, creation/staging, preview eligibility, and unavailable measured savings |
| MCP `cw_optimize_strategy` | REST derived creation | Uses the same gate and response fields |
| REST explicit mutation create, CLI `cw pin`, TUI pin key, web explicit pin button | Deliberate manual creation; can store inactive rules | No implicit grant; application still needs session consent |
| MCP `cw_pin_context` | Deliberate manual REST creation | Same inactive/manual semantics |
| Draft commit and status changes | Change stored status only; never grant consent | Even committed manual pins remain inactive without a grant |
| Native context transform, proxy forwarding, ingest, and replay/graph previews | Application paths, not authorization or automatic creation | CW-121A final boundary remains authoritative |
| Transcript sync/export | No reinforcement persistence | Never treated as a way to authorize or persist a pin into the original transcript |
| Policy grant/revoke API | Explicit authenticated session-level authorization | Read preview first; later per-content activation controls remain CW-121C |
| Gemini | Automatic creation and reinforcement blocked | Issue #35 and separate provider qualification remain required |

Manual means an explicit PinContext command or payload with pin content, not choosing a generic optimization preset. Session consent can still permit later automatically generated content. No claim is made that the operator approved that future content individually.

## Offline evidence

- `configured_auto_pin_cannot_bypass_reinforcement_consent_or_claim_application`: default configuration and a legacy enabled flag create no pins without consent; explicit consent allows generation.
- `low_health_strategy_filters_pins_but_preserves_non_pin_generation`: disabled/enabled/revoked permission snapshots affect pin generation while non-pin operations remain identical.
- `derived_creation_rechecks_consent_preserves_non_pins_and_blocks_gemini`: transactional persistence, stale-grant rejection, unchanged stored rules after revocation, session isolation, cross-session ID collision rejection, Gemini blocking, and explicit manual storage.
- `configured_automatic_creation_requires_consent_and_stops_after_revocation`: actual proxy dry-run path and SQLite verify no default creation, one opted-in generation, no further generation after revocation, original request preservation, and read-only activation preview with labels/content and future-policy disclosure.
- `manual_draft_is_inspectable_inactive_and_never_grants_consent`: authenticated manual staging, preview visibility, and no application/grant.
- Strategy API regressions cover both draft and committed consent-blocked proposals. Five web confirmation tests distinguish proposals and creation from preview application and measured savings.

Run `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all -- --check`, and `cd web && npm test && npm run check && npm run build`. The existing default smoke runner is offline only; refreshing its current source hashes does not qualify a live runner or change historical observations.

Validation: 262 Rust tests passed, 19 ignored. Five web confirmation tests, Svelte check/build, workspace Clippy/formatting, 15 runner-admission tests, and the offline lifecycle preflight passed. No live-provider or model-server tests ran. Browser/terminal interaction testing and broader activation workflows were not executed. Historical evidence and the original freeze remain unchanged.

## CW-121C remaining scope

Build operator-facing preview/enable/revoke controls in the existing CLI/TUI/web/MCP interfaces. Show stored pin contents, status, activation destinations, and possible future automatic sources before enabling. Consider explicit approval of specific content or a narrower future-generation scope. Make manual-creation feedback and inactive-state display consistent across clients. Keep hypothetical estimates distinct from observed provider usage. No full workflow, new Protect from Pruning feature, persistent pin exposure, or live-runner requalification is included here.
