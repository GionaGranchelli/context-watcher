import os
import tempfile
import time
import unittest
from pathlib import Path
from types import SimpleNamespace

from budget import BudgetExceeded
from daemon import daemon


class DaemonTest(unittest.TestCase):
    def test_campaign_deadline_stops_daemon_during_startup(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / "fake-cw"
            binary.write_text('''#!/usr/bin/python3
import os, sys, time
if "serve" in sys.argv:
    open("daemon.pid", "w").write(str(os.getpid()))
    time.sleep(30)
''')
            binary.chmod(0o755)
            deadline = time.monotonic() + 0.4

            def remaining():
                left = deadline - time.monotonic()
                if left <= 0:
                    raise BudgetExceeded("Campaign deadline exceeded")
                return left

            with self.assertRaisesRegex(BudgetExceeded, "deadline"):
                with daemon(binary, SimpleNamespace(daemon_env={}), workspace=root,
                            state=root / "state", api_port=1, proxy_port=1,
                            remaining=remaining):
                    self.fail("A daemon that never starts must not be accepted")
            pid = int((root / "daemon.pid").read_text())
            with self.assertRaises(ProcessLookupError):
                os.kill(pid, 0)
