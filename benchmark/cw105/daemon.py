"""Start an isolated Context-Watcher instance for offline replay."""
import os
import subprocess
import time
from contextlib import contextmanager
from http.client import HTTPConnection

from process import stop_group


@contextmanager
def daemon(binary, selected, *, workspace, state, api_port, proxy_port):
    if selected.daemon_env is None:
        yield
        return
    state.mkdir(mode=0o700)
    env = {"PATH": os.defpath, "HOME": str(state), **selected.daemon_env}
    with (state / "setup.log").open("xb") as setup:
        subprocess.run(
            [str(binary), "--db", str(state / "cw.db"), "autopilot", "--enable"],
            cwd=workspace, env=env, stdout=setup, stderr=subprocess.STDOUT,
            check=True, timeout=10,
        )
    log = state / "cw.log"
    with log.open("xb") as output:
        process = subprocess.Popen(
            [str(binary), "--db", str(state / "cw.db"), "serve",
             "--port", str(api_port), "--proxy-port", str(proxy_port),
             "--workspace", str(workspace)],
            cwd=workspace, env=env, stdin=subprocess.DEVNULL,
            stdout=output, stderr=subprocess.STDOUT, start_new_session=True,
        )
        try:
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline and process.poll() is None:
                connection = HTTPConnection("127.0.0.1", proxy_port, timeout=0.25)
                try:
                    connection.request("GET", "/health")
                    response = connection.getresponse()
                    if response.status == 200 and process.poll() is None:
                        break
                except OSError:
                    pass
                finally:
                    connection.close()
                time.sleep(0.05)
            else:
                raise RuntimeError(f"Context-Watcher failed to start; see {log}")
            yield (state / ".local/share/context-watcher/control-token").read_text().strip()
        finally:
            stop_group(process)
