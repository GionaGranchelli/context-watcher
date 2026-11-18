"""Observe chat completion SSE events without rewriting the stream."""
import json
from dataclasses import dataclass

from usage import Usage, parse_usage


@dataclass
class StreamMetrics:
    started: float
    ttft_seconds: float | None = None
    usage: Usage | None = None
    ended: bool = False

    def observe(self, data, now):
        """Consume a decoded SSE data field with its monotonic arrival time."""
        if self.ended:
            raise ValueError("Unexpected event after stream terminator")
        if data == "[DONE]":
            self.ended = True
            return
        event = json.loads(data)
        if not isinstance(event, dict) or event.get("error") is not None:
            raise ValueError("Invalid or failed completion event")
        usage = parse_usage(event)
        if usage is not None:
            if self.usage is not None:
                raise ValueError("Multiple usage records for one completion")
            self.usage = usage
        if self.ttft_seconds is None:
            for choice in event.get("choices", []):
                delta = choice.get("delta", {})
                if any(delta.get(field) for field in (
                    "content", "reasoning_content", "tool_calls"
                )):
                    self.ttft_seconds = now - self.started
                    break

    @property
    def complete(self):
        return self.ended and self.usage is not None
