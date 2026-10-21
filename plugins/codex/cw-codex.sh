#!/usr/bin/env bash
# Launch Codex CLI routed through contextWatcher intercepting proxy
set -euo pipefail
export OPENAI_BASE_URL="${CW_OPENAI_PROXY_URL:-http://127.0.0.1:4401/openai/v1}"
exec cw wrap codex "$@"
