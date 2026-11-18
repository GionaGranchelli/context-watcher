"""Parse observed usage from OpenAI-compatible chat completion responses."""
from dataclasses import dataclass


@dataclass(frozen=True)
class Usage:
    input_tokens: int
    output_tokens: int
    cached_input_tokens: int | None


def count(value, name):
    if type(value) is not int or value < 0:
        raise ValueError(f"Invalid {name}")
    return value


def parse_usage(event):
    """Return None on a chunk without usage. Reject malformed usage records."""
    usage = event.get("usage")
    if usage is None:
        return None
    if not isinstance(usage, dict):
        raise ValueError("Invalid usage object")
    input_tokens = count(usage.get("prompt_tokens"), "prompt_tokens")
    output_tokens = count(usage.get("completion_tokens"), "completion_tokens")
    if "total_tokens" in usage:
        total = count(usage["total_tokens"], "total_tokens")
        if total != input_tokens + output_tokens:
            raise ValueError("Inconsistent total_tokens")
    details = usage.get("prompt_tokens_details")
    cached = None
    if details is not None:
        if not isinstance(details, dict):
            raise ValueError("Invalid prompt_tokens_details")
        if details.get("cached_tokens") is not None:
            cached = count(details["cached_tokens"], "cached_tokens")
            if cached > input_tokens:
                raise ValueError("Cached tokens exceed input tokens")
    return Usage(input_tokens, output_tokens, cached)
