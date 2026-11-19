"""Validate resource samples before admitting another provider request."""
import math
from dataclasses import dataclass

from budget import BudgetExceeded


@dataclass(frozen=True)
class GpuSample:
    pci: str
    watts: float
    junction_c: float


def check_sample(samples, *, expected_devices, max_watts, max_junction_c, age, max_age):
    samples = tuple(samples)
    limits = (max_watts, max_junction_c, max_age)
    if not all(math.isfinite(value) and value > 0 for value in limits):
        raise ValueError("Resource limits must be finite and positive")
    if not expected_devices or {row.pci for row in samples} != set(expected_devices):
        raise BudgetExceeded("GPU telemetry has missing or unexpected devices")
    if len(samples) != len(set(expected_devices)):
        raise BudgetExceeded("GPU telemetry contains duplicate devices")
    if not 0 <= age <= max_age:
        raise BudgetExceeded("GPU telemetry is stale or invalid")
    for row in samples:
        if not math.isfinite(row.watts) or row.watts < 0:
            raise BudgetExceeded("GPU power reading is invalid")
        if not math.isfinite(row.junction_c) or not 0 < row.junction_c < max_junction_c:
            raise BudgetExceeded("GPU temperature is invalid or exceeds its ceiling")
    total = sum(row.watts for row in samples)
    if total > max_watts:
        raise BudgetExceeded("GPU power exceeds the campaign ceiling")
    return total
