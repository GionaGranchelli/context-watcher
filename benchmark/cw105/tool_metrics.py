"""Extract tool activity from OMP JSON print events."""
import json

MEMORY_TOOLS = {"cw_mount", "cw_unmount", "cw_pin"}


def is_memory_tool(name):
    return name in MEMORY_TOOLS or (
        name.startswith("mcp__") and name.split("__")[-1] in MEMORY_TOOLS
    )


def tool_metrics(path, timing_path=None):
    calls = {}
    event_keys = []
    ended = False
    malformed = False
    with path.open("rb") as output:
        for line in output:
            try:
                event = json.loads(line)
            except UnicodeError:
                malformed = True
                continue
            except ValueError:
                continue
            if not isinstance(event, dict):
                continue
            kind = event.get("type")
            if kind == "agent_end":
                ended = True
            if kind not in ("tool_execution_start", "tool_execution_end"):
                continue
            identifier = event.get("toolCallId")
            name = event.get("toolName")
            if not isinstance(identifier, str) or not isinstance(name, str):
                malformed = True
                continue
            event_keys.append((kind, identifier))
            record = calls.setdefault(identifier, {"name": name, "started": False, "finished": False})
            if record["name"] != name:
                malformed = True
            if kind == "tool_execution_start":
                if record["started"] or record["finished"]:
                    malformed = True
                record["started"] = True
            if kind == "tool_execution_end":
                if not record["started"] or record["finished"]:
                    malformed = True
                record["finished"] = True
                record["error"] = event.get("isError")
                duration = event.get("durationMs")
                if type(duration) in (int, float) and 0 <= duration < float("inf"):
                    record["duration_seconds"] = duration / 1000

    timing_valid = False
    if timing_path is not None and timing_path.is_file():
        try:
            timings = [json.loads(line) for line in timing_path.read_text().splitlines()]
            keys = [(event["type"], event["toolCallId"]) for event in timings]
            times = [event["observed_ns"] for event in timings]
            timing_valid = (keys == event_keys and all(type(value) is int and value >= 0 for value in times)
                            and times == sorted(times))
            if timing_valid:
                starts = {}
                for event in timings:
                    name, identifier, timestamp = event["type"], event["toolCallId"], event["observed_ns"]
                    if name == "tool_execution_start":
                        starts[identifier] = timestamp
                    elif identifier in starts:
                        calls[identifier]["duration_seconds"] = (timestamp - starts.pop(identifier)) / 1e9
        except (OSError, ValueError, TypeError, KeyError):
            timing_valid = False

    memory = [call for call in calls.values() if is_memory_tool(call["name"])]
    complete = ended and not malformed and all(call["finished"] for call in calls.values())

    def seconds(records):
        if not complete or not all("duration_seconds" in call for call in records):
            return None
        return sum(call["duration_seconds"] for call in records)

    return {
        "tool_calls": calls, "tool_events_complete": complete,
        "tool_timing_source": "observer_monotonic" if timing_valid and complete else None,
        "observed_tool_count": len(calls), "observed_mcp_count": len(memory),
        "tool_count": len(calls) if complete else None,
        "mcp_count": len(memory) if complete else None,
        "tool_seconds": seconds(calls.values()), "mcp_seconds": seconds(memory),
    }
