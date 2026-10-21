#!/usr/bin/env bash
# Launch Pi / OpenCode / Aider routed through contextWatcher intercepting proxy
set -euo pipefail
HARNESS="${1:-pi}"
shift || true
exec cw wrap "$HARNESS" "$@"
