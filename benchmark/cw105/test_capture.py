import hashlib
import threading
import unittest
from contextlib import contextmanager
from http.client import HTTPConnection
from http.server import BaseHTTPRequestHandler, HTTPServer

from budget import Budget, BudgetExceeded
from spend import Spend
from capture import PATH, relay


@contextmanager
def serving(server):
    with server:
        thread = threading.Thread(target=server.serve_forever, args=(0.01,))
        thread.start()
        try:
            yield server.server_port
        finally:
            server.shutdown()
            thread.join()


class CaptureTest(unittest.TestCase):
    def test_forwards_one_request_and_records_observed_usage(self):
        self.check_admission(missing_usage=False)

    def test_missing_usage_prevents_further_admission(self):
        self.check_admission(missing_usage=True)

    def test_resource_fault_denies_request_before_provider_access(self):
        evidence = []
        budget = Budget(campaign_limit=1, seconds=10)
        budget.start("run", request_limit=1, seconds=10)

        def reject():
            raise BudgetExceeded("Stale GPU telemetry")

        with serving(relay(1, budget, "run", evidence.append, guard=reject)) as port:
            connection = HTTPConnection("127.0.0.1", port, timeout=3)
            try:
                connection.request("POST", PATH, b"{}")
                self.assertEqual(connection.getresponse().status, 429)
            finally:
                connection.close()
        self.assertEqual(len(evidence), 1)
        self.assertFalse(evidence[0]["admitted"])
        self.assertEqual(evidence[0]["error"], "Stale GPU telemetry")
        with self.assertRaisesRegex(BudgetExceeded, "Stale GPU telemetry"):
            budget.admit("run")

    def check_admission(self, missing_usage):
        forwarded, evidence = [], []

        class Upstream(BaseHTTPRequestHandler):
            def do_POST(self):
                forwarded.append(self.rfile.read(int(self.headers["Content-Length"])))
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.end_headers()
                self.wfile.write(
                    b'data: {"choices":[{"delta":{"content":"hi"}}]}\n\n'
                    + (b"" if missing_usage else
                       b'data: {"usage":{"prompt_tokens":3,"completion_tokens":1}}\n\n')
                    + b'data: [DONE]\n\n'
                )

            def log_message(self, *_args):
                pass

        cap = 2 if missing_usage else 1
        budget = Budget(campaign_limit=cap, seconds=10)
        budget.start("run", request_limit=cap, seconds=10)
        spend = Spend(cap=100, input_rate=2, output_rate=3, max_input=10, max_output=4)
        with serving(HTTPServer(("127.0.0.1", 0), Upstream)) as upstream:
            with serving(relay(upstream, budget, "run", evidence.append, spend=spend)) as port:
                for expected in (200, 429):
                    connection = HTTPConnection("127.0.0.1", port, timeout=3)
                    connection.request("POST", PATH, b'{}')
                    response = connection.getresponse()
                    self.assertEqual(response.status, expected)
                    response.read()
                    connection.close()
        self.assertEqual(forwarded, [b'{}'])
        self.assertTrue(evidence[0]["admitted"])
        self.assertEqual(evidence[0]["request_sha256"], hashlib.sha256(b"{}").hexdigest())
        self.assertGreater(evidence[0]["response_bytes"], 0)
        self.assertRegex(evidence[0]["response_prefix_sha256"], r"^[a-f0-9]{64}$")
        self.assertFalse(evidence[1]["admitted"])
        with self.assertRaisesRegex(RuntimeError, "denied|Unsettled"):
            budget.start("next", request_limit=1, seconds=10)
        if missing_usage:
            self.assertFalse(evidence[0]["complete"])
            self.assertIsNone(evidence[0]["cost_nanodollars"])
            return
        self.assertTrue(evidence[0]["complete"])
        self.assertEqual(evidence[0]["cost_nanodollars"], 9)
        self.assertEqual(evidence[0]["usage"]["input_tokens"], 3)
        self.assertIsNone(evidence[0]["usage"]["cached_input_tokens"])

    def test_external_transport_requires_guards_and_follows_policy(self):
        budget = Budget(campaign_limit=1, seconds=10)
        budget.start("run", request_limit=1, seconds=10)
        events = []

        def connect(_):
            self.fail("Must reject before connecting")

        def reject(_):
            raise BudgetExceeded("Unexpected model")

        with self.assertRaisesRegex(ValueError, "requires"):
            relay(None, budget, "run", events.append, connect=connect)
        spend = Spend(cap=0, input_rate=0, output_rate=0, max_input=10, max_output=2)
        with serving(relay(None, budget, "run", events.append, connect=connect,
                           spend=spend, guard=lambda: None, request_policy=reject)) as port:
            from contextlib import closing
            with closing(HTTPConnection("127.0.0.1", port, timeout=3)) as conn:
                conn.request("POST", PATH, b"{}")
                self.assertEqual(conn.getresponse().status, 429)
        self.assertEqual(events[0]["error"], "Unexpected model")

    def test_interrupted_stream_preserves_prefix_and_stops_campaign(self):
        import tempfile
        from pathlib import Path

        evidence = []
        partial = b'data: {"choices":[{"delta":{"content":"partial"}}]}\n\n'

        class Interrupted(BaseHTTPRequestHandler):
            def do_POST(self):
                self.rfile.read(int(self.headers["Content-Length"]))
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.end_headers()
                self.wfile.write(partial)

            def log_message(self, *_args):
                pass

        budget = Budget(campaign_limit=2, seconds=10)
        budget.start("run", request_limit=2, seconds=10)
        with tempfile.TemporaryDirectory() as directory:
            wire = Path(directory)
            with serving(HTTPServer(("127.0.0.1", 0), Interrupted)) as upstream:
                with serving(relay(upstream, budget, "run", evidence.append, wire_dir=wire)) as port:
                    connection = HTTPConnection("127.0.0.1", port, timeout=3)
                    connection.request("POST", PATH, b"{}")
                    response = connection.getresponse()
                    self.assertEqual(response.read(), partial)
                    connection.close()
            self.assertEqual((wire / evidence[0]["response_file"]).read_bytes(), partial)
            self.assertFalse(evidence[0]["complete"])
            with self.assertRaises(BudgetExceeded):
                budget.admit("run")

    def test_capture_creation_failure_prevents_forwarding(self):
        from pathlib import Path
        from tempfile import TemporaryDirectory

        evidence = []
        budget = Budget(campaign_limit=1, seconds=10)
        budget.start("run", request_limit=1, seconds=10)
        with TemporaryDirectory() as directory:
            with serving(relay(1, budget, "run", evidence.append,
                               wire_dir=Path(directory) / "missing")) as port:
                connection = HTTPConnection("127.0.0.1", port, timeout=3)
                connection.request("POST", PATH, b"{}")
                response = connection.getresponse()
                self.assertEqual(response.status, 502)
                response.read()
                connection.close()
        self.assertFalse(evidence[0]["complete"])
        with self.assertRaises(BudgetExceeded):
            budget.admit("run")

    def test_response_write_failure_stops_admission(self):
        import os
        from pathlib import Path
        from tempfile import TemporaryDirectory
        from unittest.mock import patch

        evidence = []
        real_fdopen = os.fdopen

        class FailingWriter:
            def __init__(self, wrapped):
                self.wrapped = wrapped

            def write(self, data):
                raise OSError("synthetic disk full")

            def flush(self):
                self.wrapped.flush()

            def fileno(self):
                return self.wrapped.fileno()

            def close(self):
                self.wrapped.close()

        def fdopen(fd, mode):
            wrapped = real_fdopen(fd, mode)
            target = os.readlink(f"/proc/self/fd/{fd}")
            return FailingWriter(wrapped) if "response-" in target else wrapped

        class Upstream(BaseHTTPRequestHandler):
            def do_POST(self):
                self.rfile.read(int(self.headers["Content-Length"]))
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.end_headers()
                self.wfile.write(b'data: {"choices":[]}\n\n')

            def log_message(self, *_args):
                pass

        budget = Budget(campaign_limit=2, seconds=10)
        budget.start("run", request_limit=2, seconds=10)
        with TemporaryDirectory() as directory, patch("capture.os.fdopen", side_effect=fdopen):
            with serving(HTTPServer(("127.0.0.1", 0), Upstream)) as upstream:
                with serving(relay(upstream, budget, "run", evidence.append,
                                   wire_dir=Path(directory))) as port:
                    connection = HTTPConnection("127.0.0.1", port, timeout=3)
                    connection.request("POST", PATH, b"{}")
                    response = connection.getresponse()
                    response.read()
                    connection.close()
            self.assertIn("synthetic disk full", evidence[0]["error"])
            self.assertFalse(evidence[0]["complete"])
            with self.assertRaises(BudgetExceeded):
                budget.admit("run")

    def test_campaign_stop_interrupts_stalled_upstream(self):
        import threading
        from contextlib import closing

        started = threading.Event()
        release = threading.Event()
        evidence = []

        class Stalled(BaseHTTPRequestHandler):
            def do_POST(self):
                self.rfile.read(int(self.headers["Content-Length"]))
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.end_headers()
                self.wfile.flush()
                started.set()
                release.wait(3)

            def log_message(self, *_args):
                pass

        budget = Budget(campaign_limit=1, seconds=30)
        budget.start("run", request_limit=1, seconds=30)
        with serving(HTTPServer(("127.0.0.1", 0), Stalled)) as upstream:
            try:
                with serving(relay(upstream, budget, "run", evidence.append)) as port:
                    with closing(HTTPConnection("127.0.0.1", port, timeout=3)) as connection:
                        connection.request("POST", PATH, b"{}")
                        response = connection.getresponse()
                        self.assertTrue(started.wait(2))
                        budget.stop("resource fault")
                        self.assertEqual(response.read(), b"")
                self.assertFalse(evidence[0]["complete"])
            finally:
                release.set()
