"""Opt-in binary integration against a scripted loopback provider."""
import json
import os
import tempfile
import socket
import subprocess
import unittest
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

from daemon import daemon
from routing import route
from agent import offline_agent
from budget import Budget
from capture import relay
from process import run
from test_capture import serving


@unittest.skipUnless(os.environ.get("CW105_OMP"), "set CW105_OMP to the installed binary")
class OmpSmokeTest(unittest.TestCase):
    def test_real_omp_through_capture(self):
        self.check_mode("M0")

    @unittest.skipUnless(os.environ.get("CW105_CW"), "set CW105_CW to the CW binary")
    def test_real_cw_routes_m1_and_m2(self):
        for mode in ("M1", "M2"):
            with self.subTest(mode=mode):
                self.check_mode(mode)

    def check_mode(self, mode):
        requests, metrics = [], []

        class Mock(BaseHTTPRequestHandler):
            def do_POST(self):
                requests.append(json.loads(self.rfile.read(int(self.headers["Content-Length"]))))
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.end_headers()
                for chunk in (
                    {"choices": [{"index": 0, "delta": {"content": "cw105-mock-pass"},
                                  "finish_reason": None}]},
                    {"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]},
                    {"choices": [], "usage": {"prompt_tokens": 12, "completion_tokens": 3}},
                ):
                    self.wfile.write(f"data: {json.dumps(chunk)}\n\n".encode())
                self.wfile.write(b"data: [DONE]\n\n")

            def log_message(self, *_args):
                pass

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "fixture.py").write_text("def fixture(): pass\n")
            request_count = 2 if mode == "M2" else 1
            budget = Budget(campaign_limit=request_count, seconds=30)
            budget.start("smoke", request_limit=request_count, seconds=30)
            with serving(HTTPServer(("127.0.0.1", 0), Mock)) as upstream:
                with serving(relay(upstream, budget, "smoke", metrics.append)) as port:
                    with socket.socket() as api, socket.socket() as proxy:
                        api.bind(("127.0.0.1", 0))
                        proxy.bind(("127.0.0.1", 0))
                        api_port, proxy_port = api.getsockname()[1], proxy.getsockname()[1]
                    selected = route(mode, port, proxy_port, root)
                    argv, env = offline_agent(
                        os.environ["CW105_OMP"], root / "state", port, "Say hi",
                        mode=mode, proxy_port=proxy_port, workspace=root,
                    )
                    with daemon(os.environ.get("CW105_CW"), selected, workspace=root,
                                state=root / "cw", api_port=api_port, proxy_port=proxy_port) as control:
                        if control:
                            config_path = root / "state" / "models.json"
                            config = json.loads(config_path.read_text())
                            config["providers"]["cw105"]["headers"] = {
                                "x-cw-session-id": "cw105-smoke",
                                "x-cw-control-token": control,
                            }
                            config_path.write_text(json.dumps(config))
                        if mode == "M2":
                            rpc = {"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {
                                "name": "cw_pin", "arguments": {
                                    "session_id": "cw105-smoke", "key": "fixture",
                                    "rule": "cw105-private-register",
                                },
                            }}
                            pin = subprocess.run(
                                [os.environ["CW105_CW"], "--db", str(root / "cw/cw.db"), "mcp"],
                                input=json.dumps(rpc) + "\n", text=True, capture_output=True,
                                timeout=5, check=True, cwd=root,
                                env={"HOME": str(root / "cw"), "PATH": os.defpath},
                            )
                            reply = json.loads(pin.stdout)
                            self.assertIn("result", reply)
                            self.assertFalse(reply["result"].get("isError", False))
                        result = run(argv, cwd=root, env=env, log=root / "omp.log", timeout=15)
                        if mode == "M2":
                            other = root / "other"
                            argv, env = offline_agent(
                                os.environ["CW105_OMP"], other, port, "Say hi",
                                mode=mode, proxy_port=proxy_port, workspace=root,
                            )
                            config_path = other / "models.json"
                            config = json.loads(config_path.read_text())
                            config["providers"]["cw105"]["headers"] = {
                                "x-cw-session-id": "cw105-other",
                                "x-cw-control-token": control,
                            }
                            config_path.write_text(json.dumps(config))
                            second = run(argv, cwd=root, env=env,
                                         log=root / "other.log", timeout=10)
                            self.assertEqual(second.returncode, 0, (root / "other.log").read_text()[-3000:])
            output = (root / "omp.log").read_text()
            self.assertEqual(result.returncode, 0, output[-3000:])
            self.assertIn("cw105-mock-pass", output)
        self.assertEqual([request["model"] for request in requests], ["qwen3.8-27b-ud"] * request_count)
        self.assertEqual(len(metrics), request_count)
        self.assertTrue(metrics[0]["complete"])
        self.assertEqual(metrics[0]["usage"]["input_tokens"], 12)
        self.assertEqual("<workspace_symbol_map>" in json.dumps(requests), mode == "M2")
        self.assertEqual("cw105-private-register" in json.dumps(requests), mode == "M2")
        if mode == "M2":
            self.assertIn("<workspace_symbol_map>", json.dumps(requests[1]))
            self.assertNotIn("cw105-private-register", json.dumps(requests[1]))
