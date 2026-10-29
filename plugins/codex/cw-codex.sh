#!/usr/bin/env bash
# Launch Codex CLI through cw's authenticated local Responses provider.
set -euo pipefail
exec cw wrap codex "$@"
