"""Offline chat completion relay. Both sockets are confined to loopback."""
import hashlib
import os
import socket
import threading
import time
from dataclasses import asdict
from http.client import HTTPConnection
from http.server import BaseHTTPRequestHandler, HTTPServer

from budget import BudgetExceeded
from stream_metrics import StreamMetrics

PATH = "/v1/chat/completions"


def relay(upstream_port, budget, run_id, evidence, *, spend=None, guard=None, request_policy=None, connect=None, wire_dir=None):
    if connect is not None and (upstream_port is not None or
                                any(value is None for value in (spend, guard, request_policy))):
        raise ValueError("External transport requires spend, resource, and request guards")
    class Handler(BaseHTTPRequestHandler):
        def do_POST(self):
            if self.path != PATH:
                self.send_error(404)
                return
            try:
                size = int(self.headers.get("Content-Length", "0"))
                if not 0 < size <= 16 * 1024 * 1024:
                    raise ValueError("Invalid body size")
                self.connection.settimeout(5)
                body = self.rfile.read(size)
                if len(body) != size:
                    raise ValueError("Incomplete body")
                if request_policy is not None:
                    request_policy(body)
                if guard is not None:
                    guard()
                remaining = budget.admit(run_id)
                if spend is not None:
                    spend.reserve()
            except BudgetExceeded as exc:
                budget.stop(str(exc))
                evidence({"run_id": run_id, "admitted": False, "complete": False,
                          "usage": None, "ttft_seconds": None, "error": str(exc)})
                self.send_error(429)
                return
            except (ValueError, OSError):
                self.send_error(400)
                return
            metrics = StreamMetrics(time.monotonic())
            upstream = None
            response_digest = hashlib.sha256()
            response_bytes = 0
            error = None
            sent_headers = False
            finished = threading.Event()
            watcher = None
            wire = None
            request_path = response_path = None
            try:
                if wire_dir is not None:
                    sequence = len(list(wire_dir.glob("request-*.json"))) + 1
                    request_path = wire_dir / f"request-{sequence:04}.json"
                    response_path = wire_dir / f"response-{sequence:04}.sse"
                    fd = os.open(request_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
                    with os.fdopen(fd, "wb") as output:
                        output.write(body)
                        output.flush()
                        os.fsync(output.fileno())
                    fd = os.open(response_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
                    wire = os.fdopen(fd, "wb")
                upstream = (connect(remaining) if connect is not None else
                            HTTPConnection("127.0.0.1", upstream_port, timeout=remaining))
                upstream_socket = None

                def cancel_on_stop():
                    while not finished.wait(0.1):
                        try:
                            budget.check_running(run_id)
                        except BudgetExceeded:
                            connection = upstream_socket or upstream.sock
                            if connection is not None:
                                try:
                                    connection.shutdown(socket.SHUT_RDWR)
                                except OSError:
                                    pass
                                return

                watcher = threading.Thread(target=cancel_on_stop, daemon=True)
                watcher.start()
                upstream.request("POST", PATH, body, {"Content-Type": "application/json"})
                upstream_socket = upstream.sock
                response = upstream.getresponse()
                if response.status != 200 or response.headers.get_content_type() != "text/event-stream":
                    raise ValueError(f"Expected SSE response, got HTTP {response.status}")
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.end_headers()
                sent_headers = True
                for line in iter(response.readline, b""):
                    if response_bytes + len(line) > 32 * 1024 * 1024:
                        raise ValueError("Response exceeds wire capture limit")
                    if wire is not None:
                        wire.write(line)
                        wire.flush()
                    response_digest.update(line)
                    response_bytes += len(line)
                    now = time.monotonic()
                    if now - metrics.started >= remaining:
                        raise TimeoutError("Request deadline exceeded")
                    if line.startswith(b"data:"):
                        metrics.observe(line[5:].strip().decode(), now)
                    self.wfile.write(line)
                    self.wfile.flush()
                    if metrics.ended and not line.strip():
                        break
            except (OSError, ValueError, BudgetExceeded) as exc:
                error = str(exc)
                if not sent_headers:
                    self.send_error(502)
            finally:
                finished.set()
                if watcher is not None:
                    watcher.join()
                if upstream is not None:
                    upstream.close()
                if wire is not None:
                    try:
                        wire.flush()
                        os.fsync(wire.fileno())
                    except OSError as exc:
                        error = error or f"Wire capture failed: {exc}"
                    finally:
                        wire.close()
                cost = None
                if spend is not None:
                    try:
                        cost = spend.settle(metrics.usage if metrics.complete and error is None else None)
                    except BudgetExceeded as exc:
                        error = error or str(exc)
                if not metrics.complete and error is None:
                    error = "Completion ended without terminal usage"
                if error is not None:
                    budget.stop(error)
                evidence({
                    "admitted": True, "cost_nanodollars": cost,
                    "request_sha256": hashlib.sha256(body).hexdigest(),
                    "response_prefix_sha256": response_digest.hexdigest() if response_bytes else None,
                    "request_bytes": len(body), "response_bytes": response_bytes,
                    "request_file": request_path.name if request_path else None,
                    "response_file": response_path.name if response_path else None,
                    "run_id": run_id, "usage": asdict(metrics.usage) if metrics.usage else None,
                    "ttft_seconds": metrics.ttft_seconds,
                    "elapsed_seconds": time.monotonic() - metrics.started,
                    "complete": metrics.complete and error is None, "error": error,
                })

        def log_message(self, *_args):
            pass

    return HTTPServer(("127.0.0.1", 0), Handler)
