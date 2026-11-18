"""Summarize observed request evidence without filling missing measurements."""
from statistics import median


def request_metrics(records):
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
    return result
