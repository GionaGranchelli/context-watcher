import json
import threading
import unittest
from http.server import BaseHTTPRequestHandler, HTTPServer

from health_oracle import verify


class HealthOracleTest(unittest.TestCase):
    def check(self, api, actuator):
        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                status, body = {
                    "/api/v1/health": api,
                    "/actuator/health": actuator,
                }[self.path]
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.end_headers()
                self.wfile.write(json.dumps(body).encode())

            def log_message(self, *_args):
                pass

        with HTTPServer(("127.0.0.1", 0), Handler) as server:
            thread = threading.Thread(target=server.serve_forever, args=(0.01,))
            thread.start()
            try:
                verify(server.server_port)
            finally:
                server.shutdown()
                thread.join()

    def test_accepts_healthy_api_and_actuator(self):
        self.check((200, {"status": "UP"}), (200, {"status": "UP", "components": {}}))

    def test_rejects_unexpected_fields(self):
        with self.assertRaisesRegex(ValueError, "unexpected response fields"):
            self.check((200, {"status": "UP", "extra": True}), (200, {"status": "UP"}))

    def test_requires_existing_actuator_route(self):
        with self.assertRaisesRegex(ValueError, "actuator/health.*404"):
            self.check((200, {"status": "UP"}), (404, {}))
