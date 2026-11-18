"""Read Machinist GPU telemetry over SSH without invoking the model."""
import subprocess
import time

from budget import BudgetExceeded
from resources import GpuSample, check_sample

DEVICES = {"0000:04:00.0", "0000:85:00.0", "0000:88:00.0"}

READ_SENSORS = r"""
for sensor in /sys/class/drm/card[0-9]/device/hwmon/hwmon*; do
    [ -r "$sensor/name" ] && [ "$(cat "$sensor/name")" = amdgpu ] || continue
    pci=$(basename "$(readlink -f "$sensor/device")")
    power="$sensor/power1_input"
    [ -r "$power" ] || power="$sensor/power1_average"
    watts=$(cat "$power") || exit 1
    junction=$(cat "$sensor/temp2_input") || exit 1
    printf '%s %s %s\n' "$pci" "$watts" "$junction"
done
"""


def read_sample(*, max_watts, max_junction_c, max_age):
    started = time.monotonic()
    try:
        result = subprocess.run(
            ["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=3", "machinist", READ_SENSORS],
            capture_output=True, text=True, check=True, timeout=max_age,
        )
        rows = []
        for line in result.stdout.splitlines():
            pci, power, temp = line.split()
            rows.append(GpuSample(pci, int(power) / 1_000_000, int(temp) / 1000))
    except (OSError, ValueError, subprocess.SubprocessError) as exc:
        raise BudgetExceeded("Machinist telemetry could not be read") from exc
    total = check_sample(rows, expected_devices=DEVICES, max_watts=max_watts,
                         max_junction_c=max_junction_c, age=time.monotonic() - started,
                         max_age=max_age)
    return rows, total
