"""Opt-in binary integration against a scripted loopback provider."""
import json
import os
import tempfile
import socket
import sys
import unittest
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

from daemon import daemon
from routing import route
from tool_metrics import tool_metrics
from agent import BUILTINS, enable_memory, offline_agent
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
                delta, finish = {"content": "cw105-mock-pass"}, "stop"
                steps = [
                    ("cw_pin", {"key": "fixture", "rule": "cw105-private-register"}),
                    ("cw_mount", {"uri": "repo://fixture.py", "mode": "full"}),
                    ("cw_unmount", {"uri": "repo://fixture.py"}),
                ]
                if mode == "M2" and len(requests) <= len(steps):
                    tool, arguments = steps[len(requests) - 1]
                    arguments["session_id"] = "cw105-smoke"
                    name = next((t["function"]["name"] for t in requests[0].get("tools", [])
                                 if t["function"]["name"].endswith(tool)), None)
                    if name is not None:
                        delta = {"tool_calls": [{"index": 0, "id": f"call_{tool}",
                                                 "type": "function", "function": {
                            "name": name,
                            "arguments": json.dumps(arguments),
                        }}]}
                        finish = "tool_calls"
                for chunk in (
                    {"choices": [{"index": 0, "delta": delta, "finish_reason": None}]},
                    {"choices": [{"index": 0, "delta": {}, "finish_reason": finish}]},
                    {"choices": [], "usage": {"prompt_tokens": 12, "completion_tokens": 3}},
                ):
                    self.wfile.write(f"data: {json.dumps(chunk)}\n\n".encode())
                self.wfile.write(b"data: [DONE]\n\n")

            def log_message(self, *_args):
                pass

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "fixture.py").write_text("def fixture():\n    return 'mounted-only-marker'\n")
            request_count = 4 if mode == "M2" else 1
            expected_requests = request_count + (1 if mode == "M2" else 0)
            budget = Budget(campaign_limit=expected_requests, seconds=30)
            budget.start("smoke", request_limit=expected_requests, seconds=30)
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
                            enable_memory(
                                argv, root / "state", binary=os.environ["CW105_CW"],
                                daemon_state=root / "cw", api_port=api_port,
                                session_id="cw105-smoke", workspace=root,
                            )
                        observer = Path(__file__).with_name("observe_agent.py")
                        argv = [sys.executable, str(observer), str(root / "tool-timing.jsonl"), *argv]
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
                            (root / "mcp.json").unlink(missing_ok=True)
            output = (root / "omp.log").read_text()
            if mode == "M2":
                names = {t["function"]["name"] for t in requests[0].get("tools", [])}
                self.assertTrue(any("cw_mount" in name for name in names), (names, output[-3000:]))
            parsed = tool_metrics(root / "omp.log", root / "tool-timing.jsonl")
            self.assertEqual(parsed["mcp_count"], 3 if mode == "M2" else 0, parsed)
            self.assertEqual(parsed["tool_timing_source"], "observer_monotonic")
            if mode == "M2":
                self.assertGreaterEqual(parsed["mcp_seconds"], 0)
            self.assertEqual(result.returncode, 0, output[-3000:])
            self.assertIn("cw105-mock-pass", output)
        self.assertEqual([request["model"] for request in requests], ["qwen3.8-27b-ud"] * expected_requests)
        self.assertEqual(len(metrics), expected_requests)
        self.assertTrue(metrics[0]["complete"])
        self.assertEqual(metrics[0]["usage"]["input_tokens"], 12)
        self.assertEqual("<workspace_symbol_map>" in json.dumps(requests), mode == "M2")
        self.assertEqual("cw105-private-register" in json.dumps(requests), mode == "M2")
        names = {t["function"]["name"] for t in requests[0].get("tools", [])}
        self.assertEqual(names.intersection(BUILTINS), set(BUILTINS))
        if mode != "M2":
            self.assertEqual(names, set(BUILTINS))
        if mode == "M2":
            self.assertIn("<workspace_symbol_map>", json.dumps(requests[4]))
            self.assertNotIn("cw105-private-register", json.dumps(requests[4]))
            system = [m["content"] for m in requests[1]["messages"] if m["role"] == "system"]
            self.assertIn("cw105-private-register", json.dumps(system))
            names = [t["function"]["name"] for t in requests[0].get("tools", [])]
            for name in ("cw_pin", "cw_mount", "cw_unmount"):
                self.assertTrue(any(name in n for n in names), names)
            def system_text(index):
                return json.dumps([m["content"] for m in requests[index]["messages"]
                                   if m["role"] == "system"])
            self.assertNotIn("mounted-only-marker", system_text(1))
            self.assertIn("mounted-only-marker", system_text(2))
            self.assertNotIn("mounted-only-marker", system_text(3))
