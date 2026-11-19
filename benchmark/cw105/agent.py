"""Build an isolated offline OMP launch configuration."""
import json
import os
from pathlib import Path

from routing import route

CONTEXT_WINDOW = 262144
MAX_OUTPUT_TOKENS = 4096
BUILTINS = ("read", "edit", "write", "bash", "grep", "glob")


def offline_agent(binary, state, port, prompt, *, mode="M0", proxy_port=None, workspace=None):
    selected = route(mode, port, proxy_port, workspace)
    state.mkdir(mode=0o700)
    home = state / "home"
    home.mkdir(mode=0o700)
    config = {
        "providers": {"cw105": {
            "baseUrl": selected.agent_base,
            "api": "openai-completions",
            "apiKey": "offline-fixture",
            "models": [{
                "id": "qwen3.8-27b-ud", "name": "CW-105 offline fixture",
                "contextWindow": CONTEXT_WINDOW, "maxTokens": MAX_OUTPUT_TOKENS,
            }],
        }},
    }
    (state / "models.json").write_text(json.dumps(config), encoding="utf-8")
    env = {
        "PATH": os.defpath,
        "HOME": str(home),
        "PI_CODING_AGENT_DIR": str(state),
        "XDG_CONFIG_HOME": str(home / "config"),
        "XDG_CACHE_HOME": str(home / "cache"),
    }
    return [
        str(binary), "--print", "--mode", "json", "--model", "cw105/qwen3.8-27b-ud",
        "--no-extensions", "--no-skills", "--no-rules", "--offline",
        "--tools", ",".join(BUILTINS), "--thinking", "off", prompt,
    ], env


def enable_memory(argv, state, *, binary, daemon_state, api_port, session_id, workspace):
    overlay = state / "mcp-settings.yml"
    overlay.write_text(
        "tools:\n  xdev: false\nmcp:\n"
        "  enableProjectConfig: true\n  startupTimeoutMs: 5000\n"
    )
    config = {"mcpServers": {"cw": {
        "command": str(binary),
        "args": ["--db", str(daemon_state / "cw.db"), "mcp", "--daemon-url",
                 f"http://127.0.0.1:{api_port}"],
        "env": {"CW_SESSION_ID": session_id, "HOME": str(daemon_state)},
    }}}
    path = Path(workspace) / "mcp.json"
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w", encoding="utf-8") as output:
        json.dump(config, output)
    argv[1:1] = ["--config", str(overlay)]
    return path
