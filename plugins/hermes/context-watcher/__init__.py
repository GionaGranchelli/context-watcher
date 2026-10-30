"""Hermes middleware for session-bound contextWatcher proxy requests."""

import json
import os
import stat
import time
import uuid
from pathlib import Path
from urllib.parse import urlsplit

_SESSION_OVERRIDES = {}
_MAX_SESSION_OVERRIDES = 500
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


def _bound_client_env():
    """Retrieve client-scoped environment if running inside the Hermes gateway."""
    try:
        from agent.session_client_env import bound_client_env
        bound = bound_client_env()
        if isinstance(bound, dict):
            return bound
    except Exception:
        pass
    return None


def _get_env_var(var_name, client_env=None):
    """Retrieve an environment variable, prioritizing client-scoped env over process-global env.

    Inside a long-lived Hermes gateway daemon, client_env or bound_client_env()
    isolates variables per client/request. We NEVER mutate os.environ.
    """
    if isinstance(client_env, dict) and var_name in client_env:
        return client_env[var_name]
    bound = _bound_client_env()
    if isinstance(bound, dict) and var_name in bound:
        return bound[var_name]
    return os.environ.get(var_name)


def _control_token(client_env=None):
    token_file_env = _get_env_var("CW_CONTROL_TOKEN_FILE", client_env)
    path = Path(token_file_env or (_data_dir() / "control-token")).expanduser()
    path_str = str(path)

    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0)
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW

    try:
        fd = os.open(path_str, flags)
    except OSError:
        return None

    try:
        st = os.fstat(fd)
        if not stat.S_ISREG(st.st_mode):
            return None
        if hasattr(os, "getuid") and st.st_uid != os.getuid():
            return None
        if os.name == "posix" and (st.st_mode & 0o077):
            return None
        if st.st_size > 4096 or st.st_size == 0:
            return None
        raw_bytes = os.read(fd, 4096)
    finally:
        os.close(fd)

    token = raw_bytes.decode("utf-8", errors="replace").strip()
    if not (32 <= len(token) <= 128) or not token.isascii() or not all(ch.isalnum() or ch in "_-" for ch in token):
        return None
    return token


def _valid_session_id(value):
    return (
        isinstance(value, str)
        and 1 <= len(value) <= 128
        and all(ch.isascii() and (ch.isalnum() or ch in "_-") for ch in value)
    )


def _consume_handoff_file(override_file):
    """Safely and atomically consume a handoff file using atomic claim and descriptor reads.

    Prevents symlink traversal, TOCTOU races, and arbitrary file reads.
    Uses atomic os.rename claim to ensure strictly single consumption even under
    concurrent race conditions. Verifies ownership, permissions (0600), bounds size,
    and returns parsed (session_id, workspace) tuple.
    """
    if not override_file or not isinstance(override_file, str):
        return None

    path_obj = Path(override_file).expanduser()
    path_str = str(path_obj)

    # Atomically claim the handoff file so exactly one consumer wins the race
    claimed_str = f"{path_str}.claim.{uuid.uuid4().hex}"
    try:
        os.rename(path_str, claimed_str)
    except OSError:
        return None

    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0)
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW

    try:
        fd = os.open(claimed_str, flags)
    except OSError:
        try:
            os.unlink(claimed_str)
        except OSError:
            pass
        return None

    try:
        st = os.fstat(fd)
        # 1. Must be a regular file
        if not stat.S_ISREG(st.st_mode):
            return None
        # 2. Must be owned by current UID on POSIX
        if hasattr(os, "getuid") and st.st_uid != os.getuid():
            return None
        # 3. Mode must be strictly 0600 (no group or other permissions)
        if os.name == "posix" and (st.st_mode & 0o077):
            return None
        # 4. Bounded size
        if st.st_size == 0 or st.st_size > 65536:
            return None

        raw_bytes = os.read(fd, 65536)
    finally:
        os.close(fd)
        try:
            os.unlink(claimed_str)
        except OSError:
            pass

    try:
        payload = json.loads(raw_bytes.decode("utf-8"))
    except (ValueError, UnicodeDecodeError):
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

    return (sid, ws if isinstance(ws, str) and ws else None)


def _prune_session_overrides():
    """Prune stale session entries from _SESSION_OVERRIDES to prevent unbounded growth."""
    now = time.time()
    stale = [sid for sid, val in _SESSION_OVERRIDES.items() if (now - val[2]) > 3600]
    for sid in stale:
        _SESSION_OVERRIDES.pop(sid, None)
    if len(_SESSION_OVERRIDES) > _MAX_SESSION_OVERRIDES:
        sorted_by_age = sorted(_SESSION_OVERRIDES.items(), key=lambda item: item[1][2])
        for sid, _ in sorted_by_age[:len(_SESSION_OVERRIDES) - _MAX_SESSION_OVERRIDES]:
            _SESSION_OVERRIDES.pop(sid, None)


def on_session_terminated(**kwargs):
    """Lifecycle hook: clear cached binding when a Hermes session terminates or resets."""
    sid = kwargs.get("session_id") or kwargs.get("old_session_id")
    if sid:
        _SESSION_OVERRIDES.pop(str(sid), None)


def _resolve_session_and_workspace(kwargs):
    hermes_sid = kwargs.get("session_id") or ""
    client_env = kwargs.get("client_env")

    # If an override was already bound for this native session, reuse it
    if hermes_sid and hermes_sid in _SESSION_OVERRIDES:
        override = _SESSION_OVERRIDES[hermes_sid]
        client_ws = _get_env_var("CW_WORKSPACE", client_env)
        return override[0], override[1] or client_ws or os.getcwd()

    override_env = _get_env_var("CW_HERMES_OVERRIDE_FILE", client_env)
    env_sid = _get_env_var("CW_SESSION_ID", client_env)
    client_ws = _get_env_var("CW_WORKSPACE", client_env)

    try:
        from gateway.session_policy import _ACTIVE_POLICY
        policy = _ACTIVE_POLICY.get()
        if policy is not None and getattr(policy, "cwd", None):
            client_ws = str(policy.cwd)
    except Exception:
        pass

    # If an explicit override file was specified in environment:
    if override_env:
        parsed = _consume_handoff_file(override_env)
        if parsed is not None:
            sid, ws = parsed
            final_ws = ws or client_ws or os.getcwd()
            if hermes_sid:
                _prune_session_overrides()
                _SESSION_OVERRIDES[hermes_sid] = (sid, final_ws, time.time())
            return sid, final_ws
        # Fail safe: An explicit override was configured but was invalid, missing, stale, or symlink.
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

    client_env = kwargs.get("client_env")
    session_id, workspace = _resolve_session_and_workspace(kwargs)
    if not _valid_session_id(session_id):
        return None
    token = _control_token(client_env)
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
    for hook_name in ("on_session_reset", "on_session_end", "on_session_finalize"):
        try:
            if hasattr(ctx, "register_hook"):
                ctx.register_hook(hook_name, on_session_terminated)
        except Exception:
            pass
