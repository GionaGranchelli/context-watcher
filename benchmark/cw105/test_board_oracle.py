import json
import threading
import unittest
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import parse_qs, urlsplit

from board_oracle import verify


class BoardOracleTest(unittest.TestCase):
    def check(self, continuation):
        filters = {"status": ["Ready"], "priority": ["P0"], "limit": ["2"]}

        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                request = urlsplit(self.path)
                params = parse_qs(request.query)
                status = 200
                cursor = None
                if request.path != "/api/v1/tasks":
                    status, keys = 404, []
                elif params == filters:
                    keys, cursor = [1, 3], "opaque-token"
                elif params == dict(filters, cursor=["opaque-token"]):
                    keys = continuation
                elif params == {"status": ["In Progress"], "priority": ["P1"]}:
                    keys = []
                elif not params:
                    keys = [1, 2, 3, 4, 5]
                else:
                    status, keys = 400, []
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.end_headers()
                self.wfile.write(json.dumps({
                    "items": [{"key": f"CWK-{i:03}"} for i in keys],
                    "nextCursor": cursor,
                }).encode())

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

    def test_accepts_complete_contract(self):
        self.check([5])

    def test_rejects_repeated_item_on_continuation(self):
        with self.assertRaisesRegex(ValueError, "duplicates or omits"):
            self.check([3, 5])
