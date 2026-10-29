# Local usage

Start `cw serve`, open `http://127.0.0.1:4400`, and connect with the token printed by `cw auth-token`. The default configuration observes traffic. Upgrading a legacy configuration leaves automatic mutation disabled until deliberately enabled again.

Update an installed binary with `cw update`. By default this downloads the latest release, verifies its SHA-256 checksum, atomically installs it to `~/.local/bin/cw`, refreshes integrations, and restarts the daemon. `cw update --no-restart` skips the restart; `cw update --from-source` builds the latest `main` branch first. The release checksum is hosted with the release and does not provide signature-based publisher verification.

## Provider routing

Send requests through the loopback proxy, normally port 4401:

| Provider | Proxy path | Upstream setting |
|---|---|---|
| OpenAI Chat / Responses | `/openai/v1/chat/completions`, `/openai/v1/responses` | `CW_OPENAI_UPSTREAM` |
| Anthropic | `/anthropic/v1/messages` | `CW_ANTHROPIC_UPSTREAM` |
| Gemini | `/gemini/v1beta/models/...` | `CW_GEMINI_UPSTREAM` |

Pass the provider's own API credential as usual. No automatic endpoint fallback is performed. Unknown fields and signed blocks remain in the raw request; unsupported active formats remain observation-only.

## Session selection

An active proxy integration must send both `x-cw-session-id` and `x-cw-control-token`. The session ID accepts ASCII letters, digits, underscore and hyphen, up to 128 characters. Reusing a session with incompatible provider, model or workspace metadata is rejected.

`cw wrap [--session-id ID] <command>` injects proxy routing. For Hermes, `cw init` installs and enables the `llm_request` plugin; use `cw wrap hermes chat ...` to route built-in DeepSeek requests through CW. The wrapper supplies Hermes' `chat --base-url` because its configured provider URL overrides `DEEPSEEK_BASE_URL`. The plugin adds session headers only for CW's loopback endpoint and does not expose the CW token through the shell environment. Two-turn mock testing verified session binding and pin reapplication; a live Hermes/DeepSeek resume test confirmed the same session was recorded across turns. Other Hermes providers remain unverified. For other wrappers, `CW_SESSION_ID` alone does not authenticate a session. Codex supplies authenticated headers and excludes `CW_CONTROL_TOKEN` from shell subprocesses. Anthropic clients remain observation-only because a Claude Code probe showed its Bash tool inherits `ANTHROPIC_CUSTOM_HEADERS`; `cw wrap` strips `x-cw-*` credentials from their environment. Gemini CLI and Antigravity complete local API-key mock requests through `cw wrap`, but do not bind the wrapper session ID; their current paths are observation-only. OpenCode, Pi, and Aider each passed a local mock when their own provider settings supplied the CW headers, but `cw wrap` does not create those settings; use them only with OS-level isolation for untrusted tools. Aider receives its documented `OPENAI_API_BASE` endpoint alias. OpenRouter live checks verified Anthropic-routed signature streaming and Gemini normalized reasoning passthrough; native Gemini signature preservation and direct Anthropic connectivity remain unverified. Native Aider Markdown import is unsupported.

For direct Hermes launches, set `model.base_url` in `~/.hermes/config.yaml` to `http://127.0.0.1:4401/deepseek` and keep the context-watcher plugin enabled. A plain Hermes launch and resume were verified against CW on 2026-10-29; the test session was healthy, so no overlay fired.

CLI commands accept `--session <id>`. Without an explicit ID or `CW_SESSION_ID`, session selection requires a unique exact workspace match. MCP calls also accept an optional `session_id` and fall back to `CW_SESSION_ID` or a unique exact workspace match (returning candidate session IDs when multiple sessions match). Multiple matching IDE sessions require selection in the tool window.

## Import, inspect and preview

```sh
cw import /path/to/copy.jsonl --harness omp
cw status --session <id>
cw optimize min --session <id> --dry-run
cw optimize mid --session <id>
cw preview --session <id>
```

Import only the sources you intend to inspect. The daemon watcher follows registered JSONL sources in the selected workspace; it does not scan every harness home directory. Large sampled imports are marked incomplete and cannot authorize full-history mutation.

An optimization command stores overlays. `preview` applies them to an original in memory and prints an estimated turn and payload. It does not call a model or rewrite an actual forwarded turn. `min` (`cache_safe_boost`) selects only zero-KV-break candidates (`kv_break_tokens == 0`, preserving any `CachedHit` prefix). Other plans can invalidate a prefix and discard information; review their selected nodes.

A pin is a reminder, not proof of improved adherence. Generic fold markers are not factual summaries. Automatic trajectory folding and unused-tool removal are disabled.

## Export and resume

```sh
cw sync-back --session <id>
# Or export while creating selected rules:
cw optimize mid --session <id> --sync-back
```

The legacy command name now means **export a separate copy**. Read the printed export path. Original transcript bytes are unchanged, including long user instructions and unknown records. Only exactly mapped eligible tool-result changes are exported; unsupported changes are omitted. A proxy pin is not automatically an equivalent native transcript instruction.

No in-place replacement is supported. Resume an exported copy only with a harness/version you have tested, after the original writer has stopped. The importer does not establish a live harness reload boundary.

## Control API changes

All control endpoints except health require the private bearer token. Browser origins must match the UI host and port, or the explicitly configured `CW_UI_ORIGIN`. WebSockets require authenticated subprotocols and reconnect with a state reload.

Retrieve `/api/v1/sessions/<id>/graph`, then use its `revision` as `If-Match` for mutation creation, plan application, draft commit, clear and deletion. Missing revisions return 428; stale revisions return 412. A preview is returned separately from the actual latest turn. Persistence failures return an error instead of a successful empty result.

## Retention

```sh
cw purge --session <id> --keep-turns 20
cw purge --session <id>
```

The second command removes cw's session records and tracked exports only when export contents still match cw's recorded hash. Changed exports require manual review. Shared blobs referenced by another session are kept. Source transcripts are never deleted. During a schema safety migration, automatic `pre-safety-*.db` recovery snapshots are created only when the existing database is `<= 64 MB` and are skipped for larger databases (rather than creating a truncated backup); any created snapshots are separate private files and must be retained or deleted deliberately.

The store is plaintext. Private permissions reduce access by other users; they do not protect against software running as your user or provide secure erasure.

## Reading metrics

Provider-observed usage is stored separately from estimated spans. First-body-byte timing is not necessarily first-token latency. A completed request, failed request, blocked approval, imported snapshot and preview have different outcomes. Per-rule token attribution remains unknown when changes overlap.

Dollar, prefill, attention and KV memory calculations use heuristics. Physical allocation, real slot ownership and engine page unmapping are not measured or executed. Compare task continuation with independent tests before relying on a reduction.
