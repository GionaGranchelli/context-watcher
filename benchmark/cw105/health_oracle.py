"""Verify a running benchmark API through its public HTTP contract."""
import argparse
import json
from http.client import HTTPConnection


def verify(port):
    connection = HTTPConnection("127.0.0.1", port, timeout=5)
    try:
        for path, exact in (("/api/v1/health", True), ("/actuator/health", False)):
            connection.request("GET", path)
            response = connection.getresponse()
            if response.status != 200:
                raise ValueError(f"{path}: expected HTTP 200, got {response.status}")
            if response.headers.get_content_type() != "application/json" and not (
                not exact and response.headers.get_content_type().endswith("+json")
            ):
                raise ValueError(f"{path}: expected JSON")
            raw = response.read(65537)
            if len(raw) > 65536:
                raise ValueError(f"{path}: response exceeds oracle size limit")
            body = json.loads(raw)
            if not isinstance(body, dict) or body.get("status") != "UP":
                raise ValueError(f"{path}: expected status UP")
            if exact and body != {"status": "UP"}:
                raise ValueError(f"{path}: unexpected response fields")
    finally:
        connection.close()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, required=True)
    args = parser.parse_args()
    try:
        verify(args.port)
    except (OSError, ValueError) as error:
        parser.exit(1, f"Health oracle failed: {error}\n")
    print("Health oracle passed")
