"""Exercise the task list via HTTP, with independently seeded records."""
import argparse
import json
from http.client import HTTPConnection
from pathlib import Path
from urllib.parse import urlencode

FIXTURE = [
    {"key": f"CWK-{i:03}", "title": f"Task {i}", "status": status, "priority": priority}
    for i, status, priority in [
        (5, "Ready", "P0"), (2, "In Progress", "P0"), (4, "Ready", "P1"),
        (1, "Ready", "P0"), (3, "Ready", "P0"),
    ]
] + [
    {"key": f"CWK-{i:03}", "title": f"Task {i}", "status": "In Progress", "priority": "P0"}
    for i in range(6, 56)
]


def verify(port):
    conn = HTTPConnection("127.0.0.1", port, timeout=5)

    def query(expected_status=200, **params):
        conn.request("GET", "/api/v1/tasks?" + urlencode(params))
        response = conn.getresponse()
        raw = response.read(65537)
        if response.status != expected_status or len(raw) > 65536:
            raise ValueError(f"Unexpected HTTP response for {params}: {response.status}")
        if expected_status == 200:
            body = json.loads(raw)
            items = body["items"]
            expected = {item["key"]: item for item in FIXTURE}
            for item in items:
                reference = expected.get(item["key"])
                if reference is None or any(item.get(field) != value for field, value in reference.items()):
                    raise ValueError("Task fields differ from the injected source")
            return [item["key"] for item in items], body["nextCursor"]

    try:
        filters = {"status": "Ready", "priority": "P0", "limit": 2}
        keys, cursor = query(**filters)
        if keys != ["CWK-001", "CWK-003"] or not isinstance(cursor, str) or not cursor:
            raise ValueError("First filtered page has incorrect ordering or cursor")
        if query(**filters, cursor=cursor) != (["CWK-005"], None):
            raise ValueError("Continuation duplicates or omits a task")
        if query(status="In Progress", priority="P1") != ([], None):
            raise ValueError("Nonmatching filters must yield an empty page")
        keys, cursor = query()
        if keys != [f"CWK-{i:03}" for i in range(1, 21)] or not cursor:
            raise ValueError("Default page must contain 20 ordered items and a cursor")
        keys, cursor = query(cursor=cursor)
        if keys != [f"CWK-{i:03}" for i in range(21, 41)] or not cursor:
            raise ValueError("Default continuation has incorrect items")
        if query(cursor=cursor) != ([f"CWK-{i:03}" for i in range(41, 56)], None):
            raise ValueError("Default final page has incorrect items")
        keys, cursor = query(limit=50)
        if keys != [f"CWK-{i:03}" for i in range(1, 51)] or not cursor:
            raise ValueError("Maximum page must contain 50 ordered items and a cursor")
        if query(limit=50, cursor=cursor) != ([f"CWK-{i:03}" for i in range(51, 56)], None):
            raise ValueError("Maximum-page continuation has incorrect items")
        for params in ({"status": "sentinel"}, {"priority": "sentinel"},
                       {"limit": 0}, {"limit": 51}, {"cursor": "!!invalid!!"}):
            query(400, **params)
    finally:
        conn.close()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    options = parser.add_mutually_exclusive_group(required=True)
    options.add_argument("--fixture", type=Path)
    options.add_argument("--port", type=int)
    args = parser.parse_args()
    if args.fixture:
        with args.fixture.open("x", encoding="utf-8") as output:
            json.dump(FIXTURE, output)
    else:
        verify(args.port)
        print("Board API oracle passed")
