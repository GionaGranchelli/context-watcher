# CW-121A: Session consent at the PinContext application boundary

Status: implementation for review. This is the first runtime boundary for the [roadmap in PR #34](https://github.com/GionaGranchelli/context-watcher/pull/34), not completion of CW-121B/C. PR #34 merged on 2026-11-03 and is included in this branch. No inference or production changes are part of this work.

## Runtime contract

A committed rule does not authorize reinforcement. Missing consent, a revoked grant, a failed policy read, or consent for another session prevents PinContext injection. Other mutation operators retain their existing eligibility checks and behavior. Existing pin rules remain stored with their original status and contents.

`apply_mutations_to_payload()` defaults to pin-disabled. Its policy-aware variant checks session consent before collecting any pin text, then applies the existing signed-content, provider, stateful-request, and tail-shape guards. `PinContextPolicy` cannot be deserialized from a provider request or mutation. Trusted callers obtain it from explicit stored consent. Gemini reinforcement abstains even with consent until [issue #35](https://github.com/GionaGranchelli/context-watcher/issues/35) is resolved and qualified.

Policy events are appended to SQLite for an existing session. No existing session receives a grant during schema initialization or import. Grants and revocations survive restart; ended sessions cannot reinforce or receive a new grant. Session refresh and auto-pilot configuration cannot undo a revocation. Enabling a session authorizes its eligible stored and future pins, including configured automatic rules. It does not create a pin, enable global mutation mode, authorize inference, or guarantee sustained exposure.

Revocation applies to subsequent transformations. A transformation already running with a policy snapshot can finish; this is not cancellation of an in-flight request. Existing text in an incoming request is preserved, including text previously supplied by the harness. Revocation prevents CW adding reinforcement; it does not rewrite the harness's history.

## Authenticated operator controls

The loopback control API requires the existing control credential and origin checks for all policy operations. The session must already exist and meet the authenticated session-ID contract.

- `GET /api/v1/sessions/:id/pin-context-policy` returns effective enabled state, grant/revocation events, and the risk warning.
- `POST` to the same route with `{"enabled":true,"acknowledge_risk":true}` explicitly enables reinforcement for that session.
- `POST` with `{"enabled":false}` revokes it without deleting or changing any mutation rule.

An enable request without explicit risk acknowledgement is rejected. Enabling a terminated session returns HTTP 409 with a JSON error identifying session termination and a separate risk warning; further acknowledgement cannot reopen it. Unknown request fields are rejected. Review `GET /api/v1/sessions/:id/mutations` before opting in; those rule contents are what the policy permits. Audit events identify control operations and their timestamps, not independently verified human identities. Broader clients and user-facing controls remain CW-121B/C work. CW-121B must make automatic creation policy-aware and show pin contents and scope before consent, including which stored pins would become active.

PinContext can add tokens, increase local latency or cloud cost, disturb cache reuse, and change agent behavior. Its benefits remain unproven. Reinforcement is not protection from pruning. No new Protect from Pruning feature is implemented.

## Application and activation inventory

| Path | CW-121A boundary | Remaining CW-121B/C work |
|---|---|---|
| Proxy forwarding and native OMP context-transform API | Load current session policy; blocked pins report no applied ID | Operator-facing grant/revoke controls in harness clients |
| Auto-pilot `auto_cache_safe_pin` and low-health strategy generation | Generated rules cannot grant consent; final application checks policy; reports distinguish generated from applied | Make automatic creation policy-aware; show existing and proposed pin contents and scope before opt-in |
| Optimization presets and lint suggestions | Can create stored rules; applying them still requires consent; REST preview reports creation separately from application | Policy-aware plans, warnings, and activation flow |
| REST mutation creation and draft commit | Committing is not consent; no automatic grant | Visible inactive state and risk disclosure before manual activation |
| CLI pin, optimize, and batch optimize | Store rules only; model-bound application checks the session policy; CLI preview uses policy | CLI grant/revoke commands and truthful activation messages |
| TUI pin key and web pin buttons | Store through existing mutation paths; cannot bypass application guard | Warnings, consent controls, and current-state display |
| MCP `cw_pin_context` and optimization tools | Create rules through REST; cannot grant consent implicitly | Explicit consent workflow and response wording |
| REST ingest, fork preview, and graph recomputation | Use session policy; previews are not provider execution | Consistent UI application/abstention display |
| Harness imports, transcript sync, and auto-heal disk paths | Legacy application entry point remains pin-disabled, even for an opted-in session; no reinforcement written to source transcripts | Separate any future disk-persistence contract from model-bound reinforcement |
| Core optimizer estimates and direct callers without session policy | Remain pin-disabled; cannot infer permission from a candidate or committed rule | Policy-aware estimates and entry-point migration where qualified |
| Gemini contents path | Reinforcement blocked regardless of grant | Issue #35 and separate provider qualification |

The final mutation guard is shared by every application call above. Strategy confirmations distinguish created or staged rules, preview eligibility, rule estimates, and unavailable measured savings. Other CLI/TUI/web/MCP success wording may still describe a created rule as pinned; that interface migration is explicitly unfinished. No new CLI, TUI, web, or MCP consent controls are claimed here.

## Deterministic evidence

The new `committed_pin_requires_explicit_session_opt_in` test failed before the fix because a committed legacy pin changed the request. It passes after the guard.

`pin_lifecycle` covers authenticated grant/revoke, rejected missing acknowledgement and invalid credentials, preserved rules/source messages, the actual proxy dry-run pipeline and stored ledger, strategy creation without consent, and existing tool-tail lifecycle protection with explicit consent. Core tests cover a wrong-session grant, unchanged pruning alongside a blocked pin, OpenAI Chat/Responses, Anthropic text, and Gemini abstention. Store tests cover default-off opening/reopening, persistent grants/revocations, absent sessions, refresh, and failed policy reads.

Reproduce without provider inference:

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
python3 -B -m unittest discover -s benchmark/cw104b -p test_smoke.py
python3 -B benchmark/cw104b/smoke.py --manifest-dir /tmp/cw121a-offline-check
```

Local validation: 257 Rust tests passed, 19 ignored; workspace Clippy and formatting passed. All 15 Python runner-admission tests and the default offline preflight passed. The ignored suites, live-provider execution, and production server behavior were not run.

The default smoke command runs the offline API fixture only. Its current source freeze is refreshed to cover this policy revision; historical result manifests and the original CW-104B freeze remain unchanged. This is not live-runner requalification. Existing PinContext inference runners need an explicit policy-aware activation design before reuse. No pin-persistence or benchmark campaign is implemented.
