"""Explicit per-mode routing for loopback qualification."""
from dataclasses import dataclass


@dataclass(frozen=True)
class Route:
    agent_base: str
    daemon_env: dict[str, str] | None
    virtual_memory: bool


def route(mode, capture_port, proxy_port, workspace):
    if mode == "M0":
        return Route(f"http://127.0.0.1:{capture_port}/v1", None, False)
    if mode not in ("M1", "M2"):
        raise ValueError(f"Unknown benchmark mode: {mode}")
    virtual_memory = mode == "M2"
    return Route(
        f"http://127.0.0.1:{proxy_port}/openai/v1",
        {
            "CW_OPENAI_UPSTREAM": f"http://127.0.0.1:{capture_port}",
            "CW_ENABLE_SYMBOL_MAP": "1" if virtual_memory else "0",
            "CW_WORKSPACE_ROOT": str(workspace),
        },
        virtual_memory,
    )
