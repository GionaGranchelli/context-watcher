"""Summarize observed request evidence without filling missing measurements."""
from statistics import median


def request_metrics(records, *, context_window=None):
    records = list(records)
    complete = bool(records) and all(
        record["complete"] and record["usage"] is not None for record in records
    )
    result = {"request_count": len(records), "telemetry_complete": complete}
    for field in ("input_tokens", "output_tokens", "cached_input_tokens"):
        values = [record["usage"][field] for record in records if record["usage"] is not None]
        result[field] = sum(values) if complete and all(v is not None for v in values) else None
    timings = [record["ttft_seconds"] for record in records if record["ttft_seconds"] is not None]
    result["ttft_seconds"] = median(timings) if timings else None
    result["ttft_sample_count"] = len(timings)
    result["admitted_requests"] = sum(record.get("admitted") is True for record in records)
    cost = [record.get("cost_nanodollars") for record in records]
    result["provider_cost_usd"] = (
        sum(cost) / 1_000_000_000 if complete and all(value is not None for value in cost)
        else None
    )
    result["context_window_tokens"] = context_window
    result["peak_observed_context_fraction"] = (
        max(record["usage"]["input_tokens"] for record in records) / context_window
        if complete and context_window else None
    )
    return result
