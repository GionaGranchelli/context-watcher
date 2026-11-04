# CW-122B1 — Codex mock qualification

**Status:** mock adapter qualification passed; live execution remains **NO-GO**.

CW-122B1 qualifies the installed Codex CLI and Context-Watcher's authenticated Responses path without contacting a model provider. It does not qualify coding-agent product value or authorize provider spend.

## Run contract

`benchmark/cw122b1/run_mock_campaign.py` runs the ignored Rust integration test through the real `codex` executable. It requires an empty evidence directory outside the checkout with mode `0700`, records the Codex version and frozen CW-122A digest, and launches Cargo with `--offline --locked` under the CW-122A minimal environment allowlist. There is no live mode; `--live` is rejected.

Each of 12 counterbalanced attempts gets a fresh workspace, `HOME`, `CODEX_HOME`, temporary directory, session, control credential and store. The initial and resumed invocations use the same task prompts, Codex version, model label and configuration. The Codex child gets a dummy API key only to authenticate to the loopback proxy. The shell tool environment is separately filtered; a real Codex tool call checked for `OPENAI_API_KEY`, `CW_CONTROL_TOKEN`, and proxy variables and observed none. Proxy credentials are removed before provider egress.

The mock endpoint is an in-process loopback server. Codex is configured to use only `http://127.0.0.1:<port>/openai/v1`; user configuration is ignored, `workspace-write` tool networking is explicitly disabled, Codex analytics and the built-in metrics exporter are disabled, and outbound request metadata is checked for `analytics_enabled: false`. The [Codex configuration reference](https://learn.chatgpt.com/docs/config-file/config-advanced) documents these analytics and sandbox settings. The Context-Watcher HTTP client has redirects disabled; an integration test confirms a mock 307 is returned without contacting a loopback tripwire. No provider fallback is configured.

Each run has a 600-second total deadline across both Codex invocations; the campaign has a 7,200-second deadline. The fixed 12 attempts and existing per-session SpendGuard limit of 30 admitted requests bound the campaign at 360 requests. A timed-out process group, including tool descendants, is killed. A failed attempt stops the campaign and is retained in the private evidence directory. There are no automatic retries of campaign attempts.

## Mock campaign result

The completed host run used `codex-cli 0.166.0` and passed all 12 attempts (six pairs across F1 and F2, both arm orders), with 48 local mock requests. Each attempt made four requests. All 12 independent fixture oracles passed. M1 had no applied pruning. Every M2 attempt recorded two verified applications to the completed historical probe output in the resumed and following request; original transcripts retained the unmodified output.

For every request, the test compared the complete egress JSON with the exact expected payload, checked persisted egress against the mock's received bytes, reconciled provider-reported mock usage with the turn ledger, and required matching mutation IDs. The first probe tool result was complete and consumed in a subsequent assistant turn before M2 committed its pruning rule. The final implementation prompt contains no repeated task constraints.

Observed request sampling fields matched across all attempts: temperature and top-p were unset, and reasoning summary was `auto`. Codex emitted a model-catalog fallback warning for the synthetic `gpt-4o-mini` label. This is recorded as a mock-runtime limitation; the campaign does not qualify a production model selection.

The run's provider call count and provider spend were both zero. Mock usage is synthetic. Cache hits, cache fairness, real token economics, provider latency, inference behavior and product value are **NOT_EVALUATED**. Both arms passing the scripted oracle cannot establish a correctness advantage. The mock's command responses include frozen fixture repairs, so this test validates CLI routing, resume lifecycle, treatment delivery and evidence accounting—not autonomous coding quality.

## Preserved evidence and first-run corrections

Raw requests, egress, SSE responses and Codex rollouts are stored with restrictive permissions outside the checkout. Do not commit them; they contain complete agent payloads and local environment details. The successful manifest hashes every request, response and rollout. Earlier failed harness attempts remain in separate private temporary evidence directories and are summarized in the local append-only decision log. The failures were test harness defects: unsupported resume flags, evidence-directory reuse, and an oracle that treated the canonical tool-result object pointer as the nested `output` string. They did not indicate a Context-Watcher mutation defect.

## Gate to CW-122B live preparation

The installed CLI, Responses protocol, session resume, proxy routing, tool-environment filtering, mock request accounting and pruning lifecycle are now mock-qualified. Before any live campaign, independently review this PR and its private evidence hashes, choose a supported real model profile, freeze real-model sampling and task acceptance, prove actual provider endpoint identity, and obtain fresh written spend authorization that states the reservation-overrun risk. This result is not authorization. Do not treat synthetic usage or the internal mock SpendGuard budget as money spent or approved.
