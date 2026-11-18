"""Offline chat completion relay. Both sockets are confined to loopback."""
import time
from dataclasses import asdict
from http.client import HTTPConnection
from http.server import BaseHTTPRequestHandler, HTTPServer

from budget import BudgetExceeded
from stream_metrics import StreamMetrics

PATH = "/v1/chat/completions"


def relay(upstream_port, budget, run_id, evidence):
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
                remaining = budget.admit(run_id)
            except BudgetExceeded:
                self.send_error(429)
                return
            except (ValueError, OSError):
                self.send_error(400)
                return
            metrics = StreamMetrics(time.monotonic())
            upstream = HTTPConnection("127.0.0.1", upstream_port, timeout=remaining)
            error = None
            sent_headers = False
            try:
                upstream.request("POST", PATH, body, {"Content-Type": "application/json"})
                response = upstream.getresponse()
                if response.status != 200 or response.headers.get_content_type() != "text/event-stream":
                    raise ValueError(f"Expected SSE response, got HTTP {response.status}")
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.end_headers()
                sent_headers = True
                for line in iter(response.readline, b""):
                    now = time.monotonic()
                    if now - metrics.started >= remaining:
                        raise TimeoutError("Request deadline exceeded")
                    if line.startswith(b"data:"):
                        metrics.observe(line[5:].strip().decode(), now)
                    self.wfile.write(line)
                    self.wfile.flush()
                    if metrics.ended and not line.strip():
                        break
            except (OSError, ValueError) as exc:
                error = str(exc)
                if not sent_headers:
                    self.send_error(502)
            finally:
                upstream.close()
                evidence({
                    "run_id": run_id, "usage": asdict(metrics.usage) if metrics.usage else None,
                    "ttft_seconds": metrics.ttft_seconds,
                    "elapsed_seconds": time.monotonic() - metrics.started,
                    "complete": metrics.complete and error is None, "error": error,
                })

        def log_message(self, *_args):
            pass

    return HTTPServer(("127.0.0.1", 0), Handler)
