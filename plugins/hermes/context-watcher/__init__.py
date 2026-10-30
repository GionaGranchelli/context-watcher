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
    token_file_env = os.environ.get("CW_CONTROL_TOKEN_FILE")
    try:
        from agent.session_client_env import bound_client_env
        bound = bound_client_env()
        if isinstance(bound, dict) and not token_file_env:
            token_file_env = bound.get("CW_CONTROL_TOKEN_FILE")
    except Exception:
        pass
    path = Path(token_file_env or (_data_dir() / "control-token")).expanduser()
    try:
        metadata = path.lstat()
        if not stat.S_ISREG(metadata.st_mode):
            return None
        if os.name == "posix" and metadata.st_mode & 0o077:
            return None
        token = path.read_text(encoding="utf-8").strip()
    except OSError:
        return None
    if not (32 <= len(token) <= 128) or not token.isascii() or not all(ch.isalnum() or ch in "_-" for ch in token):
        return None
    return token


def _valid_session_id(value):
    return (
        isinstance(value, str)
        and 1 <= len(value) <= 128
        and all(ch.isascii() and (ch.isalnum() or ch in "_-") for ch in value)
    )


def _consume_wrap_override(hermes_session_id, override_file=None):
    if hermes_session_id and hermes_session_id in _SESSION_OVERRIDES:
        return _SESSION_OVERRIDES[hermes_session_id]

    if not override_file:
        override_file = os.environ.get("CW_HERMES_OVERRIDE_FILE")
    if not override_file:
        return None

    # Security: Purge CW_HERMES_OVERRIDE_FILE from process.env immediately
    os.environ.pop("CW_HERMES_OVERRIDE_FILE", None)

    path = Path(override_file).expanduser()
    try:
        metadata = path.lstat()
        if stat.S_ISLNK(metadata.st_mode) or path.is_symlink():
            return None
        if not stat.S_ISREG(metadata.st_mode):
            return None
        if os.name == "posix" and (metadata.st_mode & 0o077):
            return None
        raw = path.read_text(encoding="utf-8")
        path.unlink(missing_ok=True)
        payload = json.loads(raw)
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
    if "control_token" in payload or "x-cw-control-token" in payload:
        return None

    entry = (sid, ws if isinstance(ws, str) and ws else None)
    if hermes_session_id:
        _SESSION_OVERRIDES[hermes_session_id] = entry
    return entry


def _resolve_session_and_workspace(kwargs):
    hermes_sid = kwargs.get("session_id") or ""
    override_env = os.environ.get("CW_HERMES_OVERRIDE_FILE")
    env_sid = os.environ.get("CW_SESSION_ID")
    client_ws = None
    try:
        from agent.session_client_env import bound_client_env
        bound = bound_client_env()
        if isinstance(bound, dict):
            if not override_env:
                override_env = bound.get("CW_HERMES_OVERRIDE_FILE")
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

    # If an override was already bound for this native session, reuse it
    if hermes_sid and hermes_sid in _SESSION_OVERRIDES:
        override = _SESSION_OVERRIDES[hermes_sid]
        return override[0], override[1] or client_ws or os.getcwd()

    # If an explicit override file was specified in environment:
    if override_env:
        override = _consume_wrap_override(hermes_sid, override_env)
        if override is not None:
            return override[0], override[1] or client_ws or os.getcwd()
        # Fail safe: An explicit override was configured but was invalid, missing, or stale.
        # Do NOT silently bind to an unrelated session!
        return None, None

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
