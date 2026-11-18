"""Extract tool activity from OMP JSON print events."""
import json

MEMORY_TOOLS = {"cw_mount", "cw_unmount", "cw_pin"}


def is_memory_tool(name):
    return name in MEMORY_TOOLS or (
        name.startswith("mcp__") and name.split("__")[-1] in MEMORY_TOOLS
    )


def tool_metrics(path):
    calls = {}
    with path.open(encoding="utf-8") as output:
        for line in output:
            try:
                event = json.loads(line)
            except ValueError:
                continue
            if not isinstance(event, dict):
                continue
            kind = event.get("type")
            if kind not in ("tool_execution_start", "tool_execution_end"):
                continue
            identifier = event.get("toolCallId")
            name = event.get("toolName")
            if not isinstance(identifier, str) or not isinstance(name, str):
                continue
            record = calls.setdefault(identifier, {"name": name, "finished": False})
            if kind == "tool_execution_end":
                record["finished"] = True
                record["error"] = event.get("isError")
                duration = event.get("durationMs")
                if type(duration) in (int, float) and 0 <= duration < float("inf"):
                    record["duration_seconds"] = duration / 1000

    memory = [call for call in calls.values() if is_memory_tool(call["name"])]

    def seconds(records):
        if not all("duration_seconds" in call for call in records):
            return None
        return sum(call["duration_seconds"] for call in records)

    return {
        "tool_calls": calls, "tool_count": len(calls), "mcp_count": len(memory),
        "tool_seconds": seconds(calls.values()), "mcp_seconds": seconds(memory),
    }
