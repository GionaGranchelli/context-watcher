"""Verify candidate Nuxt behavior using the independent browser oracle."""
import argparse
import os
import socket
import subprocess
import time
from http.client import HTTPConnection
from pathlib import Path

from process import run, stop_group


def verify_web(worktree, evidence, task):
    env = dict(os.environ, NUXT_TELEMETRY_DISABLED="1")
    install = run(["pnpm", "install", "--offline", "--frozen-lockfile"],
                  cwd=worktree, env=env, log=evidence / "web-install.log", timeout=90)
    if install.returncode:
        raise RuntimeError("Offline dependency install failed. See web-install.log")
    with socket.socket() as web, socket.socket() as api:
        web.bind(("127.0.0.1", 0))
        api.bind(("127.0.0.1", 0))
        web_port, api_port = web.getsockname()[1], api.getsockname()[1]
    web_origin = f"http://127.0.0.1:{web_port}"
    api_origin = f"http://127.0.0.1:{api_port}"
    env["NUXT_PUBLIC_API_BASE_URL"] = api_origin
    with (evidence / "web-server.log").open("xb") as log:
        server = subprocess.Popen(
            ["pnpm", "exec", "nuxt", "dev", "--host", "127.0.0.1", "--port", str(web_port)],
            cwd=worktree / "apps/web", env=env, stdout=log, stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        try:
            deadline = time.monotonic() + 30
            while server.poll() is None and time.monotonic() < deadline:
                connection = HTTPConnection("127.0.0.1", web_port, timeout=1)
                try:
                    connection.request("GET", "/")
                    if connection.getresponse().status == 200:
                        break
                except OSError:
                    pass
                finally:
                    connection.close()
                time.sleep(0.2)
            else:
                raise RuntimeError("Nuxt did not become ready. See web-server.log")
            oracle = Path(__file__).with_name("browser_oracle.mjs").resolve()
            result = run(["node", str(oracle), task, web_origin, api_origin],
                         cwd=worktree, env=env, log=evidence / "browser.log", timeout=30)
            return result.returncode == 0
        finally:
            stop_group(server)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("task", choices=("web-api-base-url", "board-query"))
    parser.add_argument("worktree", type=Path)
    parser.add_argument("evidence", type=Path)
    args = parser.parse_args()
    raise SystemExit(not verify_web(args.worktree.resolve(), args.evidence.resolve(), args.task))
