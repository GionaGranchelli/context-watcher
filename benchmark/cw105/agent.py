"""Build an isolated offline OMP launch configuration."""
import json
import os

from routing import route


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
                "contextWindow": 262144, "maxTokens": 4096,
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
        "--no-tools", "--thinking", "off", prompt,
    ], env


def enable_memory(argv, state, *, binary, daemon_state, api_port, session_id):
    overlay = state / "mcp-settings.yml"
    overlay.write_text("tools:\n  xdev: false\nmcp:\n  startupTimeoutMs: 5000\n")
    config = {"mcpServers": {"cw": {
        "command": str(binary),
        "args": ["--db", str(daemon_state / "cw.db"), "mcp", "--daemon-url",
                 f"http://127.0.0.1:{api_port}"],
        "env": {"CW_SESSION_ID": session_id, "HOME": str(daemon_state)},
    }}}
    (state / "mcp.json").write_text(json.dumps(config), encoding="utf-8")
    argv[1:1] = ["--config", str(overlay)]
    argv.remove("--no-tools")
