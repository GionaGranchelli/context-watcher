import socket
import subprocess
import sys
import unittest
from pathlib import Path


@unittest.skipUnless(sys.platform == "linux", "Linux Landlock required")
class NetworkGuardTest(unittest.TestCase):
    def test_child_inherits_tcp_port_restriction(self):
        wrapper = Path(__file__).with_name("network_guard.py")
        with socket.socket() as relay, socket.socket() as denied:
            relay.bind(("127.0.0.1", 0))
            denied.bind(("127.0.0.1", 0))
            relay.listen()
            denied.listen()
            allowed, blocked = relay.getsockname()[1], denied.getsockname()[1]
            script = """
import errno, socket, subprocess, sys
with socket.create_connection(("127.0.0.1", int(sys.argv[1])), timeout=2):
    pass
code = 'import errno,socket,sys; s=socket.socket(); assert s.connect_ex(("127.0.0.1",int(sys.argv[1]))) == errno.EACCES'
subprocess.run([sys.executable, "-c", code, sys.argv[2]], check=True)
"""
            subprocess.run([sys.executable, str(wrapper), str(allowed),
                            sys.executable, "-c", script, str(allowed), str(blocked)],
                           check=True, timeout=10)
