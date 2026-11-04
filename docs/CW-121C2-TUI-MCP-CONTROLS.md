# CW-121C2: TUI controls and agent-readable policy

PinContext is experimental, potentially harmful, and unproven as an optimization. This change completes operator entry points after merged PR #39. The daemon policy API remains authoritative. No new consent storage, persistence, per-pin approval, or protection from pruning is introduced.

## Use the human TUI controls

```sh
cw tui --daemon-url http://127.0.0.1:4400
```

Press **c** to enter the daemon policy workspace. This uses the existing control credential and accepts only a bare loopback HTTP(S) daemon origin. Requests have a five-second timeout. Redirects and environment HTTP proxies are disabled.

The main TUI still opens its local `--db` store for existing optimization workflows. The policy workspace never reads or writes consent through that store. It lists sessions from the selected daemon and labels that source on screen. If the selected local session is absent from the daemon's list, controls refuse entry rather than choosing a different session silently. With an empty local workspace, the first daemon session is selected. **Tab** switches among daemon sessions.

The scrollable policy snapshot displays:

- Consent enabled/disabled, session restrictions and provider support.
- Complete stored pin contents, labels and statuses.
- Latest-request preview eligibility, including unavailable values.
- Automatic-generation configuration and future session-wide scope.
- The full risk warning and policy events.

Use **j/k**, arrow keys, or **Page Down/Page Up** to inspect the complete snapshot. JSON escaping keeps control characters in pin contents literal. Consent is distinct from stored/committed rules, preview eligibility and observed provider application. Measured savings remain unavailable.

After reaching the end of the snapshot, press **e**. Type `ENABLE <full-session-id>` and press **Enter**. A single hotkey, an empty confirmation or an incorrect phrase cannot grant consent. **Esc** cancels confirmation. Session switching and refresh clear the reviewed state and typed confirmation.

Before posting a grant, the TUI reads the policy again. A changed or failed snapshot requires a new review. The daemon still checks termination and all CW-121A/B restrictions. This is session-level consent for existing and future eligible rules, not atomic approval of specific contents. Configuration or content can change after the final read; request-time enforcement remains authoritative.

Press **d** to revoke consent without deleting rules, including when a policy read has failed. Every attempted operation is followed by a new policy read. Failed, malformed or inconsistent responses leave consent unconfirmed and never report successful authorization. **r** refreshes policy and session listing. **q/Esc** returns to the local optimization workspace. **q** there exits the TUI.

Normal exits, errors and Rust panic unwinding restore terminal mode and leave the alternate screen through a drop guard. Forced process termination cannot guarantee cleanup.

## Inspect policy through MCP

`cw_pin_policy_status` accepts only an optional canonical `session_id`. It returns the validated daemon policy snapshot with an interpretation note. It uses the existing MCP session-resolution and authenticated API transport. It does not require a recorded turn: missing request eligibility remains null.

The tool is read-only. Extra enable/acknowledgement arguments are rejected. There is no registered or dispatched policy enable, disable or revoke tool. Agents must direct a human to CLI, Web or the trusted TUI controls. MCP revocation is deliberately not added: a read-only policy surface avoids letting the agent change the operator's consent decision.

`cw_pin_context` still stores a committed rule without consent and says so in its result. `cw_unpin_context` deletes stored rules, optionally all mutation rules under its existing contract. It does not revoke session consent. `cw_optimize_strategy` retains the existing consent-aware creation gate. Its description distinguishes rule creation and estimated reductions from measured savings.

This is a tool-dispatch boundary, not isolation from the operating-system account. Existing MCP mutations still carry the daemon control credential. An agent with separate unrestricted shell access or access to that credential is outside this tool-only restriction. No new credential authority or shell sandbox is claimed.

## Entry-point security inventory

| Entry point | Policy authority and permitted action | Human requirement |
|---|---|---|
| CLI `pin-policy` | Existing authenticated daemon API; inspect/grant/revoke | Preview and explicit risk flag for grant |
| Web policy section | Same API; inspect/grant/revoke | Current preview and acknowledgement checkbox |
| TUI **c** workspace | Same API; daemon-listed sessions; inspect/grant/revoke | End-of-snapshot review plus typed session-specific confirmation |
| MCP `cw_pin_policy_status` | Authenticated GET only; inspect | Cannot grant or revoke |
| MCP `cw_pin_context` / TUI Space | Existing manual creation; stores committed rule | Storage does not grant consent |
| MCP `cw_unpin_context` | Existing mutation deletion | Deletion does not revoke consent |
| Optimization strategies | Existing CW-121B creation and CW-121A application guards | Cannot grant consent |
| Proxy/native transform | Existing request-time enforcement | Default-off, session grant, committed rule and qualified request required |

No core application, automatic-creation or transcript-write behavior changes. Gemini, terminated sessions, tool tails and unsupported shapes retain their existing guards. Support disclosure follows the latest effective provider as in CW-121C1; automatic-creation scope retains its stored-session-provider meaning.

## Verification and limitations

Validation: 277 Rust tests passed, 19 ignored; workspace Clippy and formatting passed. All 15 web tests, Svelte check and build passed. The 12 Hermes plugin tests, 15 offline runner admission tests and sanitized offline lifecycle fixture passed. No ignored live-provider or harness-binary suite was invoked.

TUI state tests use an isolated authenticated daemon API. They cover review/typed confirmation, revocation preserving stored rules, session switching, stale snapshots, terminated/Gemini/invalid sessions, API failures, malformed responses and loopback URL restrictions. Ratatui TestBackend tests exercise long-content scrolling and the confirmation prompt.

MCP tests exercise real dispatch against the daemon API: tool registration, complete policy formatting, forged acknowledgement/enable attempts, malformed arguments and responses, unavailable/unauthenticated requests, manual pin creation without consent, and non-pin dedupe preservation.

The shared policy validator was moved from the CLI to `cw-core::control`; CLI/TUI/MCP now reuse it without changing the API or consent rules. Existing CLI/Web tests and proxy lifecycle/safety suites remain applicable.

`scripts/cw121c2_terminal_smoke.py` drives the actual built TUI in a pseudo-terminal against a disposable daemon with isolated HOME/config/data/workspaces. It verifies typed grant, revocation preserving a pin, separate local/daemon stores, and terminal-attribute restoration on both normal and API-failure paths. This is scripted real-terminal interaction, not human visual usability qualification. The first smoke attempts exposed an overly strict assertion against incremental terminal output; attempt output was retained privately and the assertion was corrected. No product success is claimed from those failed script attempts.

Run it with a new evidence directory:

```sh
cargo build -p cw-cli
python3 scripts/cw121c2_terminal_smoke.py --evidence-dir /tmp/cw121c2-new-run
```

Only the [sanitized result and output hashes](../.audit/cw121c2/terminal-smoke.json) are committed. Raw fixture-only terminal output is retained locally. No provider inference, model-server execution, cloud spending, production configuration changes or historical benchmark rewrites occurred. Browser interaction was not repeated for this TUI/MCP-only change. Ignored harness/live suites remain outside default workspace coverage.

After independent review, stop PinContext expansion and return to CW-122 pruning-only qualification. There are no new per-content approval or sustained-exposure claims.
