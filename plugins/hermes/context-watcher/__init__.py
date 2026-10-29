"""Hermes middleware for session-bound contextWatcher proxy requests."""

import os
import stat
from pathlib import Path
from urllib.parse import urlsplit


def _control_token():
    path = Path(os.environ.get(
        "CW_CONTROL_TOKEN_FILE",
        Path.home() / ".local/share/context-watcher/control-token",
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


def _request_headers(**kwargs):
    route = urlsplit(str(kwargs.get("base_url") or ""))
    try:
        is_cw_deepseek = (
            route.scheme == "http"
            and route.hostname == "127.0.0.1"
            and route.port is not None
            and route.username is None
            and route.password is None
            and route.path.rstrip("/") == "/deepseek"
        )
    except ValueError:
        is_cw_deepseek = False
    if not is_cw_deepseek:
        return None
    session_id = os.environ.get("CW_SESSION_ID") or kwargs.get("session_id", "")
    if not isinstance(session_id, str) or not (1 <= len(session_id) <= 128):
        return None
    if not all(char.isascii() and (char.isalnum() or char in "_-") for char in session_id):
        return None
    token = _control_token()
    if token is None:
        return None

    request = dict(kwargs.get("request") or {})
    headers = dict(request.get("extra_headers") or {})
    headers.update({
        "x-cw-session-id": session_id,
        "x-cw-control-token": token,
        "x-cw-workspace": os.getcwd(),
    })
    request["extra_headers"] = headers
    return {
        "request": request,
        "source": "context-watcher",
        "reason": "authenticated local proxy session",
    }


def register(ctx):
    ctx.register_middleware("llm_request", _request_headers)
