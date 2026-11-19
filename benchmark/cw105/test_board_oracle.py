import json
import threading
import unittest
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import parse_qs, urlsplit

from board_oracle import FIXTURE, verify


class BoardOracleTest(unittest.TestCase):
    def check(self, continuation, *, default_limit=20, wrong_title=False):
        source = {row["key"]: row for row in FIXTURE}
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
                    keys, cursor = list(range(1, default_limit + 1)), "default-next"
                elif params == {"cursor": ["default-next"]}:
                    keys, cursor = list(range(21, 41)), "later"
                elif params == {"cursor": ["later"]}:
                    keys = list(range(41, 56))
                elif params == {"limit": ["50"]}:
                    keys, cursor = list(range(1, 51)), "max-next"
                elif params == {"limit": ["50"], "cursor": ["max-next"]}:
                    keys = list(range(51, 56))
                else:
                    status, keys = 400, []
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.end_headers()
                self.wfile.write(json.dumps({
                    "items": [dict(source[f"CWK-{i:03}"], **({"title": "wrong"} if wrong_title else {}))
                              for i in keys],
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

    def test_rejects_wrong_default_page_size(self):
        with self.assertRaisesRegex(ValueError, "Default page"):
            self.check([5], default_limit=10)

    def test_rejects_changed_task_fields(self):
        with self.assertRaisesRegex(ValueError, "Task fields"):
            self.check([5], wrong_title=True)
