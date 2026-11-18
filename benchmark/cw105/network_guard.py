"""Limit TCP connect for the agent and its children using Linux Landlock."""
import ctypes
import os
import platform
import sys


class Ruleset(ctypes.Structure):
    _fields_ = [("filesystem", ctypes.c_uint64), ("network", ctypes.c_uint64)]


class PortRule(ctypes.Structure):
    _fields_ = [("access", ctypes.c_uint64), ("port", ctypes.c_uint64)]


def restrict_tcp(port):
    if platform.system() != "Linux" or platform.machine() not in ("x86_64", "aarch64"):
        raise RuntimeError("CW-105 requires Linux Landlock")
    if not 1024 <= port <= 65535 or port == 1234:
        raise ValueError("Invalid relay port")
    libc = ctypes.CDLL(None, use_errno=True)

    def checked(result):
        if result < 0:
            errno = ctypes.get_errno()
            raise OSError(errno, os.strerror(errno))
        return result

    if checked(libc.syscall(444, 0, 0, 1)) < 4:
        raise RuntimeError("Landlock ABI 4 is required for TCP restrictions")
    rules = Ruleset(0, 3)  # Handle TCP bind and connect.
    fd = checked(libc.syscall(444, ctypes.byref(rules), ctypes.sizeof(rules), 0))
    try:
        allowed = PortRule(2, port)  # Allow connect only.
        checked(libc.syscall(445, fd, 2, ctypes.byref(allowed), 0))
        checked(libc.prctl(38, 1, 0, 0, 0))  # PR_SET_NO_NEW_PRIVS
        checked(libc.syscall(446, fd, 0))
    finally:
        os.close(fd)


if __name__ == "__main__":
    restrict_tcp(int(sys.argv[1]))
    os.execv(sys.argv[2], sys.argv[2:])
