# CW-121C1: Operator PinContext consent controls

Status: implementation for independent review, based on merged PR #38. PinContext remains experimental, potentially harmful, and unproven as an optimization. This revision adds CLI/Web controls over the existing policy API. It does not add authorization rules, inference, persistence semantics, or per-content approval.

## CLI workflow

Use the full canonical session ID. These commands address the running daemon's control API, not the local database selected by other store commands. `--daemon-url` defaults to `http://127.0.0.1:4400` and accepts only a bare loopback HTTP(S) origin. The existing `CW_CONTROL_TOKEN` or private token file authenticates requests. Redirects and environment HTTP proxies are disabled for this client.

```sh
cw pin-policy --session <ID> preview
cw pin-policy --session <ID> enable --acknowledge-risk
cw pin-policy --session <ID> disable
```

`preview` shows effective consent, stored labels/contents/statuses, latest-request eligibility, possible future automatic generation, session scope, and the complete risk warning. It does not change consent.

`enable` fetches and displays a valid authenticated preview and the full warning before sending the existing grant request. Without `--acknowledge-risk`, it prints the preview and exits without granting consent. The flag is the operator's explicit acknowledgement of that session-wide risk and scope; it is not proof of approval of each pin. Invalid or mismatched responses cannot enable reinforcement. Terminated sessions and unsupported Gemini sessions are refused. Only a confirmed enabled response produces authorization feedback.

`disable` requests revocation directly, without requiring a successful preview first, and leaves rules stored. A failed or malformed response is reported as unconfirmed, not success. A request already in flight can finish; this control does not cancel provider requests.

`cw pin` still deliberately stores a committed rule without granting consent. Its help and success text now say that storage, authorization, eligibility, and provider-request application are separate. Estimates remain explicitly distinct from measured savings.

## Web workflow

The current session workspace includes a compact **PinContext reinforcement** section with:

- Current consent state and any terminated/provider restriction.
- Full risk warning and session-wide activation scope, including existing and future eligible pins.
- Stored pin labels, full scrollable contents, statuses, and latest-request preview eligibility. Missing request data remains unavailable.
- Disclosure of configured automatic reminder/recovery policies and possible future generation.
- A deliberate acknowledgement checkbox and **Enable reinforcement** button.
- **Disable reinforcement**, available even while a preview is pending or after a preview failure.
- **Refresh preview** to recover from errors and inspect current state.

Enable requires a valid current preview and acknowledgement. Session changes and graph revisions reload the preview and clear acknowledgement. Late responses from an old session cannot update or authorize the newly selected session. HTTP failures, invalid JSON, and wrong-session bodies show unavailable/unconfirmed state. They never become a guessed disabled or successful state. A failed update may have reached the server; refresh or explicitly disable to establish current state.

The section reuses the application's existing authenticated `apiFetch` path and the server's grant/revoke API. The server remains the authority. The read contract adds `session_terminated` and `reinforcement_supported` to explain why enablement is unavailable; neither is a new authorization mechanism. Support and per-pin authorization disclosure use the latest turn's normalization provider, falling back to stored session metadata, as does request eligibility. Automatic-creation scope retains the stored session provider used by the existing creation policy. Future request shapes and providers are checked again at application time.

## Meaning of feedback

| Feedback | Meaning |
|---|---|
| Rule stored | A record exists; it may remain inactive |
| Rule committed | Stored status permits consideration; it does not grant consent |
| Reinforcement authorized / consent enabled | The session grant exists; request/provider restrictions still apply |
| Eligible in latest-request preview | Hypothetical eligibility for that recorded request after opt-in, not future qualification |
| Actually applied to a provider request | Requires separate recorded provider-request/ledger evidence; this policy preview does not establish it |
| Estimated reduction | A hypothetical estimate, not measured savings |

Revocation deletes no rules. Strategies still cannot grant consent. CW-121A's application boundary, CW-121B's automatic-creation gate, session isolation, transcript immutability, terminated-session rejection, and Gemini restriction are unchanged. No Protect from Pruning feature is added.

## Verification

`crates/cw-cli/tests/pin_policy_controls.rs` drives the actual CLI binary against an isolated authenticated loopback API. It checks preview before enablement, refusal without acknowledgement, explicit grant, revocation with identical stored rules, inactive drafts, future-generation disclosure, separate sessions, invalid/missing sessions, wrong credentials, terminated/Gemini sessions, unavailable APIs, malformed responses, failed writes, and unconfirmed results. The client URL test rejects remote origins and credential-bearing URLs.

`web/tests/pinPolicy.test.mjs` exercises the workflow used by the Svelte component. It checks inactive content/scope display data, deliberate acknowledgement, revocation, stale session responses, refresh resets, invalid sessions, authentication/server/network errors, malformed bodies, provider/session restrictions, unconfirmed writes, and revocation while preview is pending. The component is checked and built by Svelte/Vite. These deterministic tests do not establish browser layout or click behavior.

A terminal operator smoke used a disposable daemon, isolated HOME/config/data/store/workspace and only control API routes. Preview, refusal without acknowledgement, explicit enable, and disable passed; the pin remained stored. The daemon was stopped. A sanitized record and terminal transcript hash are retained in [operator evidence](../.audit/cw121c1/operator-check.json).

Review regressions verify support and authorization for mismatched stored/latest-turn providers in both directions, plus actionable malformed-JSON errors for GET and POST. Both checks failed before their respective corrections.

Browser interaction is **UNVERIFIED**: computer-use inventory exposed no browsers, and opening an in-app browser returned `Browser is not available: iab`. No browser installation or production environment change was attempted.

Final offline validation: 269 Rust tests passed, 19 ignored; workspace Clippy and formatting passed. All 15 web tests passed, Svelte reported zero errors and warnings, and the production build passed. The 15 runner admission tests and default sanitized offline lifecycle fixture passed. Ignored harness/live suites were not executed.

Run the required offline checks:

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
cd web
npm test
npm run check
npm run build
```

No live provider, model-server, cloud-spend, or benchmark qualification is performed. Historical evidence and the original experiment freeze remain unchanged; only the current offline source freeze is refreshed.

## CW-121C2 remaining gaps

TUI and MCP still lack an operator activation workflow. Their explicit pin paths can store inactive records but cannot implicitly grant consent. Add preview/acknowledge/enable/revoke controls and consistent feedback there separately. Per-content or narrower future-generation approval would need its own contract. Browser interaction qualification remains outstanding. None of these gaps authorizes another inference experiment.
