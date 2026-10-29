# Supported operations and limits

This is the current safety contract. Fixture coverage is narrower than full real-provider qualification. Generic HTTP compatibility does not establish native transcript support.

| Input / operation | Observe | Active proxy mutation | Transcript export | In-place replacement |
|---|---|---|---|---|
| OpenAI Chat, known unsigned text shapes | Raw bytes and analysis | Explicit authenticated session; opt-in | Through supported harness adapter | Unsupported |
| OpenAI Responses, stateless text and function items | Raw bytes and known-item analysis | Known unsigned text only when the client sends authenticated session headers | Adapter dependent | Unsupported |
| Responses `previous_response_id` / `conversation` | Raw bytes; partial local view | Disabled | Unsupported | Unsupported |
| Anthropic known text/tool blocks | Raw bytes and analysis | Unsigned known shapes only | Exact tool-result mappings | Unsupported |
| Gemini known text/function blocks | Raw bytes and analysis | No authenticated session headers from `cw wrap`; observation only | Adapter dependent | Unsupported |
| Signed thinking / thought signatures / encrypted content | Opaque raw preservation | Disabled for whole payload | Source preserved | Unsupported |
| Multimodal or unknown content shapes | Raw preservation; partial analysis | Disabled | Unknown records preserved | Unsupported |
| Sampled transcript (> analysis size bound) | Marked incomplete | Disabled | No full-history optimization | Unsupported |
| OMP / Pi native JSONL | Recorded-fixture import; local OMP qualification separate | `cw wrap` strips session credentials pending safe shell isolation; otherwise direct authenticated headers only | Exact eligible tool results; resumability version dependent | Unsupported |
| Codex rollout / Claude JSONL | Recorded-format import | Codex wrapper: known unsigned Responses text, locally mock-verified; other transports depend on headers | Exactly mapped results only | Unsupported |
| AGY JSONL | Recorded-format import | Depends on actual HTTP transport | Unsupported folds left unchanged | Unsupported |
| OpenCode SQLite | Explicit import | Independent of import | No native DB write-back | Unsupported |
| Hermes SQLite | Explicit import | Authenticated `cw wrap hermes` proxy path for built-in DeepSeek; other Hermes routes unverified | No native DB write-back | Unsupported |
| Aider Markdown | Unsupported native import | Generic proxy observation only | Unsupported | Unsupported |

The watcher follows registered JSONL sources only, filtered by the selected workspace. It reparses changed bounded snapshots on a blocking worker. Native SQLite live watching and validated incremental append cursors are not implemented.

`cw init` installs and enables a Hermes plugin that adds authenticated session headers in Hermes' `llm_request` middleware. Launch Hermes through `cw wrap hermes chat ...` so its built-in DeepSeek provider uses the local `/deepseek` proxy route. The plugin reads the private CW token file in-process, only adds it for that loopback route, and the proxy strips CW headers before forwarding. A two-turn isolated Hermes run against a local mock verified session reuse and committed pin reapplication. A bounded real Hermes/DeepSeek request through an isolated CW daemon returned the expected answer and recorded the provider session; it does not qualify signatures, latency, or other provider routes. Native Hermes SQLite write-back remains unsupported. The old `post_tool_call` proposal was insufficient because Hermes ignores its return value; see [Hermes middleware](https://github.com/NousResearch/hermes-agent/blob/main/website/docs/developer-guide/middleware.md).

`CW_SESSION_ID` is child-process metadata, not proxy authentication: the proxy binds an overlay only when a request carries both authenticated `x-cw-session-id` and `x-cw-control-token` headers. OMP v18.12.6, Claude Code 2.1.126, and Codex CLI 0.165.0 passed local mock checks for authenticated session association. A follow-up Claude Code tool probe showed its Bash child can read `ANTHROPIC_CUSTOM_HEADERS`, including the control token. Therefore `cw wrap` now strips `x-cw-*` headers and control-token environment variables from every non-Codex child; OMP and Claude wrapper requests are observation-only. Codex keeps its authenticated path because its temporary CLI configuration excludes `CW_CONTROL_TOKEN` from shell subprocesses; local mock checks verified session reuse and overlay reapplication.

OpenCode 1.17.3, Pi 1.0.4, and Aider 0.86.2 each sent authenticated session headers when configured through their native provider settings and bound the mock request to the configured session. The proxy stripped those headers before forwarding. These tests used throwaway values in temporary config files; they do not establish safety for untrusted tools running as the same OS user, and `cw wrap` does not create these configurations. Aider receives the documented `OPENAI_API_BASE` endpoint alias from `cw wrap`.

Gemini CLI 0.42.0 and Antigravity CLI 1.4.10 completed local mock calls through `cw wrap` with a dummy API key and local upstream. Antigravity required `modelProvider: "gemini"` in its temporary settings. With explicit `--session-id` values, the store recorded new `sess_…` observation sessions rather than either requested ID, so authenticated session binding is not supported by these wrapper paths. The installed Gemini CLI 0.42.0 source supports `GEMINI_CLI_CUSTOM_HEADERS` for provider requests, but its shell environment sanitizer explicitly preserves all `GEMINI_CLI_*` variables. Carrying a CW session proof there would expose it to model-invoked shell commands, so `cw wrap` must not use this mechanism for authenticated binding. `cw wrap gemini` now strips inherited `x-cw-*` headers from this setting; the focused wrapper test passes. See [Gemini CLI configuration](https://github.com/google-gemini/gemini-cli/blob/main/docs/reference/configuration.md) and [Antigravity CLI settings](https://www.agy.dev/docs/settings?tab=cli). The proxy forwards a signed Anthropic request body unchanged with a committed pin and forwards a signed SSE response byte-for-byte in deterministic fixture tests. This is fixture-level preservation evidence, not validation of a cryptographic provider signature. A live OpenRouter-to-Anthropic signature check passed as described below; direct Anthropic connectivity and Gemini signatures remain unverified.

The control token protects the local API from unauthenticated callers; it does not isolate against a hostile process running as the same OS user with access to the token file. Run untrusted harnesses under an OS sandbox or separate account.

Opt-in live signature checks are available in `crates/cw-proxy/tests/live_provider.rs`. Run them separately so each provider call is explicit:

```sh
cargo test -p cw-proxy --test live_provider real_openrouter_gemini_reasoning_details_survive_proxy_forwarding -- --ignored
cargo test -p cw-proxy --test live_provider real_openrouter_anthropic_signature_event_survives_proxy_streaming -- --ignored
cargo test -p cw-proxy --test live_provider real_anthropic_signature_event_survives_proxy_streaming -- --ignored
cargo test -p cw-proxy --test live_provider real_gemini_function_signature_survives_proxy_forwarding -- --ignored
```

Both OpenRouter cases need `OPENROUTER_API_KEY`. The Anthropic case optionally accepts `CW_R7_OPENROUTER_ANTHROPIC_MODEL` (defaults to `anthropic/claude-haiku-5.5`); it pins routing to Anthropic, disables fallback, and checks for a nonempty streaming `signature_delta`. It passed on 2026-10-29. The Gemini case optionally accepts `CW_R7_OPENROUTER_GEMINI_MODEL` (defaults to `google/gemini-3.8-flash`); it pins routing to Google AI Studio and checks for a tool call with nonempty OpenRouter `reasoning_details`. It passed on 2026-10-29. That normalized OpenAI-compatible response did not contain Gemini's native `thoughtSignature`, so it does not qualify native Gemini signature preservation. The direct Anthropic case needs `ANTHROPIC_API_KEY`, `CW_R7_ANTHROPIC_MODEL`, and optionally `CW_R7_ANTHROPIC_THINKING` (`adaptive` by default; `enabled` for models that support manual extended thinking). The native Gemini case needs `GEMINI_API_KEY` and `CW_R7_GEMINI_MODEL` set to a Gemini model with function-call signatures; it checks for `thoughtSignature`. Tests omit provider error bodies and signature values from assertion output.

The local proxy safety suite verifies that valid session headers select a committed overlay, that the overlay reaches the upstream request, and that the internal session and control headers are stripped before upstream forwarding. Run it with `cargo test -p cw-proxy --test safety`.

## Mutation rules

A pointer locates a target but does not establish its identity. A fingerprint or real call ID must match; when both are supplied both must match. System-fragment writes are disabled without an exact span map. All file-read results and current unread results are protected from generic pruning/folding/truncation. Covered-read dedupe additionally requires matching successful contents and covering ranges. A later write alone does not establish coverage.

Automatic folds, automatic spills, automatic transcript writes, unused-tool removal and local-engine parameter injection are disabled. User-created manual folds require supplied summary text and remain potentially lossy. Pins can change a prefix and are not equivalent to provider-native instructions or native transcript exports.

## Measurements and engine features

Only explicit provider usage is observed cache evidence. Radix/node spans, salience, model-family memory, cost and prefill calculations remain estimates. Unknown cache state is not a cache hit. Minimum optimization (`cache_safe_boost` / `min`) selects only zero-KV-break candidates (`kv_break_tokens == 0`), leaving any `CachedHit` prefix untouched.

Engine slot listing uses actual engine responses; offline slot lists are empty. Splicing and page-alignment calculations are previews. Execute requests for physical page manipulation return unsupported. Replay is a non-network preview. Real task continuation is a separate explicit benchmark operation.

## Sources checked for boundary behavior

Anthropic permits consecutive roles and combines them into a turn; local validation must not reject this documented input. See the [Messages API reference](https://platform.claude.com/docs/en/api/messages/create). Local checks do not validate signatures or all provider schemas.

Anthropic thinking modes vary by model, so the live test defaults to adaptive thinking and allows `enabled` for models that support manual extended thinking. See [Anthropic thinking configuration](https://platform.claude.com/docs/en/build-with-claude/thinking). Gemini 3 and 4 attach thought signatures to function-call responses; the live test forces a function call and checks for its signature. See [Gemini thought signatures](https://ai.google.dev/gemini-api/docs/generate-content/thought-signatures).

OpenRouter exposes an Anthropic Messages-compatible endpoint and supports provider routing controls; the live test uses these to require Anthropic and disable provider fallback. See the [OpenRouter Anthropic Messages API](https://openrouter.ai/docs/api/api-reference/anthropic-messages/create-messages?explorer=true), [provider routing docs](https://openrouter.ai/docs/guides/routing/provider-selection), and [Claude Haiku 5.5 model page](https://openrouter.ai/anthropic/claude-haiku-5.5/). The model currently documents adaptive thinking by default; model availability and pricing may change.

OpenRouter serves Google models through its OpenAI-compatible API and returns normalized `reasoning_details`; the live proxy test confirms those details pass through with a tool call. This is distinct from Google's native Gemini `thoughtSignature` field, which was absent from the observed normalized response. See [OpenRouter Google models](https://openrouter.ai/google/) and [reasoning details](https://openrouter.ai/docs/guides/best-practices/reasoning-tokens).

Gemini CLI documents `GOOGLE_GEMINI_BASE_URL` as its API-key endpoint override; `cw wrap` now sets that variable. See [Gemini CLI configuration](https://github.com/google-gemini/gemini-cli/blob/main/docs/reference/configuration.md). Aider documents `OPENAI_API_BASE` for OpenAI-compatible endpoints; `cw wrap` sets that alias. Aider 0.86.2 also completed a local mock request bound to its configured session when its native provider settings supplied authenticated `x-cw-*` headers. That manual configuration is not provisioned by `cw wrap` and does not establish credential isolation from same-user tools. Correct routing alone does not provide authenticated headers needed for overlay sessions.

The OMP continuation runner uses documented `compact` RPC and isolated model configuration, with `qwen-chat-template` reasoning-off shaping. See [OMP RPC](https://github.com/can1357/oh-my-pi/blob/main/docs/rpc.md) and [model configuration](https://github.com/can1357/oh-my-pi/blob/main/docs/models.md). Installed-version behavior is checked during the run; upstream documentation alone is not runtime evidence.
