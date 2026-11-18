"""Build and verify the candidate Spring API from a benchmark worktree."""
import argparse
import json
import os
import socket
import subprocess
import time
from pathlib import Path

from board_oracle import FIXTURE, verify as verify_board
from health_oracle import verify as verify_health
from process import run, stop_group


def verify_api(worktree, evidence, task):
    app = worktree / "services/api"
    build = run([str(app / "gradlew"), "--offline", "--no-daemon", "bootJar"],
                cwd=app, env=os.environ, log=evidence / "build.log", timeout=180)
    if build.returncode != 0:
        raise RuntimeError("Spring build failed. See build.log")
    jars = list((app / "build/libs").glob("*.jar"))
    jars = [jar for jar in jars if not jar.name.endswith("-plain.jar")]
    if len(jars) != 1:
        raise RuntimeError("Expected one executable Spring JAR")
    env = {"PATH": os.defpath}
    if task == "board-query":
        fixture = evidence / "tasks-fixture.json"
        with fixture.open("x", encoding="utf-8") as output:
            json.dump(FIXTURE, output)
        env["CWK_TASK_FIXTURE"] = str(fixture)
    oracle = verify_board if task == "board-query" else verify_health
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        port = listener.getsockname()[1]
    with (evidence / "server.log").open("xb") as log:
        server = subprocess.Popen(
            ["java", "-jar", str(jars[0]), "--server.address=127.0.0.1",
             f"--server.port={port}"],
            cwd=app, env=env, stdout=log, stderr=subprocess.STDOUT, start_new_session=True,
        )
        try:
            deadline = time.monotonic() + 15
            while server.poll() is None and time.monotonic() < deadline:
                try:
                    oracle(port)
                    return True
                except ConnectionRefusedError:
                    time.sleep(0.1)
                except (ValueError, KeyError, TypeError) as error:
                    (evidence / "api-oracle.log").write_text(str(error) + "\n")
                    return False
            raise RuntimeError("Spring did not start. See server.log")
        finally:
            stop_group(server)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("task", choices=("health-contract", "board-query"))
    parser.add_argument("worktree", type=Path)
    parser.add_argument("evidence", type=Path)
    args = parser.parse_args()
    raise SystemExit(not verify_api(args.worktree.resolve(), args.evidence.resolve(), args.task))
