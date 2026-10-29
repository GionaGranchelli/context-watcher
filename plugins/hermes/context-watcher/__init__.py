"""Hermes middleware for session-bound contextWatcher proxy requests."""

import json
import os
import stat
import time
from pathlib import Path
from urllib.parse import urlsplit

_SESSION_OVERRIDES = {}
_CW_ROUTES = frozenset({
    "/deepseek",
    "/openai",
    "/openai/v1",
    "/anthropic",
    "/gemini",
    "/ollama",
    "/llamacpp",
    "/vllm",
})


def _data_dir():
    return Path.home() / ".local/share/context-watcher"


def _control_token():
    path = Path(os.environ.get(
        "CW_CONTROL_TOKEN_FILE",
        _data_dir() / "control-token",
    )).expanduser()
    try:
        metadata = path.lstat()
        if not stat.S_ISREG(metadata.st_mode):
            return None
        if os.name == "posix" and metadata.st_mode & 0o077:
            return None
        token = path.read_text(encoding="utf-8").strip()
    except OSError:
        return None
    if not (32 <= len(token) <= 128) or not token.isascii() or not token.isalnum():
        return None
    return token


def _valid_session_id(value):
    return (
        isinstance(value, str)
        and 1 <= len(value) <= 128
        and all(ch.isascii() and (ch.isalnum() or ch in "_-") for ch in value)
    )


def _consume_wrap_override(hermes_session_id):
    if hermes_session_id and hermes_session_id in _SESSION_OVERRIDES:
        return _SESSION_OVERRIDES[hermes_session_id]
    path = _data_dir() / "hermes-session-override.json"
    try:
        metadata = path.lstat()
        if not stat.S_ISREG(metadata.st_mode):
            return None
        if os.name == "posix" and metadata.st_mode & 0o077:
            return None
        payload = json.loads(path.read_text(encoding="utf-8"))
        path.unlink(missing_ok=True)
    except (OSError, ValueError, TypeError):
        return None
    if not isinstance(payload, dict):
        return None
    ts = payload.get("ts")
    if not isinstance(ts, (int, float)) or abs(time.time() - ts) > 120:
        return None
    sid = payload.get("session_id")
    ws = payload.get("workspace")
    if not _valid_session_id(sid):
        return None
    entry = (sid, ws if isinstance(ws, str) and ws else None)
    if hermes_session_id:
        _SESSION_OVERRIDES[hermes_session_id] = entry
    return entry


def _resolve_session_and_workspace(kwargs):
    hermes_sid = kwargs.get("session_id") or ""
    env_sid = os.environ.get("CW_SESSION_ID")
    client_ws = None
    try:
        from agent.session_client_env import bound_client_env
        bound = bound_client_env()
        if isinstance(bound, dict):
            if not env_sid:
                env_sid = bound.get("CW_SESSION_ID")
            client_ws = bound.get("CW_WORKSPACE")
    except Exception:
        pass
    try:
        from gateway.session_policy import _ACTIVE_POLICY
        policy = _ACTIVE_POLICY.get()
        if policy is not None and getattr(policy, "cwd", None):
            client_ws = str(policy.cwd)
    except Exception:
        pass
    override = _consume_wrap_override(hermes_sid)
    if override is not None:
        return override[0], override[1] or client_ws or os.getcwd()
    session_id = env_sid or hermes_sid
    workspace = client_ws or os.getcwd()
    return session_id, workspace


def _request_headers(**kwargs):
    route = urlsplit(str(kwargs.get("base_url") or ""))
    try:
        is_cw_route = (
            route.scheme == "http"
            and route.hostname in ("127.0.0.1", "localhost")
            and route.port is not None
            and route.username is None
            and route.password is None
            and route.path.rstrip("/") in _CW_ROUTES
        )
    except ValueError:
        is_cw_route = False
    if not is_cw_route:
        return None
    session_id, workspace = _resolve_session_and_workspace(kwargs)
    if not _valid_session_id(session_id):
        return None
    token = _control_token()
    if token is None:
        return None

    request = dict(kwargs.get("request") or {})
    headers = dict(request.get("extra_headers") or {})
    headers.update({
        "x-cw-session-id": session_id,
        "x-cw-control-token": token,
        "x-cw-workspace": workspace,
    })
    request["extra_headers"] = headers
    return {
        "request": request,
        "source": "context-watcher",
        "reason": "authenticated local proxy session",
    }


def register(ctx):
    ctx.register_middleware("llm_request", _request_headers)
