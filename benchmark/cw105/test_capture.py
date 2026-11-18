import threading
import unittest
from contextlib import contextmanager
from http.client import HTTPConnection
from http.server import BaseHTTPRequestHandler, HTTPServer

from budget import Budget
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
        forwarded, evidence = [], []

        class Upstream(BaseHTTPRequestHandler):
            def do_POST(self):
                forwarded.append(self.rfile.read(int(self.headers["Content-Length"])))
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.end_headers()
                self.wfile.write(
                    b'data: {"choices":[{"delta":{"content":"hi"}}]}\n\n'
                    b'data: {"usage":{"prompt_tokens":3,"completion_tokens":1}}\n\n'
                    b'data: [DONE]\n\n'
                )

            def log_message(self, *_args):
                pass

        budget = Budget(campaign_limit=1, seconds=10)
        budget.start("run", request_limit=1, seconds=10)
        with serving(HTTPServer(("127.0.0.1", 0), Upstream)) as upstream:
            with serving(relay(upstream, budget, "run", evidence.append)) as port:
                for expected in (200, 429):
                    connection = HTTPConnection("127.0.0.1", port, timeout=3)
                    connection.request("POST", PATH, b'{}')
                    response = connection.getresponse()
                    self.assertEqual(response.status, expected)
                    response.read()
                    connection.close()
        self.assertEqual(forwarded, [b'{}'])
        self.assertTrue(evidence[0]["complete"])
        self.assertEqual(evidence[0]["usage"]["input_tokens"], 3)
        self.assertIsNone(evidence[0]["usage"]["cached_input_tokens"])
