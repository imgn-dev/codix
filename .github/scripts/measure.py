"""Records a release binary's size and its median `--version` startup time.

Usage: measure.py <binary> <target> <json output>
Writes the figures to the JSON file and a Markdown table to stdout.
"""

import json
import os
import statistics
import subprocess
import sys
import time

RUNS = 20

binary, target, output = sys.argv[1:4]

durations = []
for _ in range(RUNS):
    start = time.perf_counter()
    subprocess.run([binary, "--version"], check=True, stdout=subprocess.DEVNULL)
    durations.append(time.perf_counter() - start)

result = {
    "target": target,
    "size_bytes": os.path.getsize(binary),
    "startup_median_ms": round(statistics.median(durations) * 1000, 2),
    "runs": RUNS,
}
with open(output, "w", encoding="utf-8") as f:
    json.dump(result, f, indent=2)

print("| Target | Size (bytes) | `--version` median (ms) |")
print("| --- | --- | --- |")
print(f"| {target} | {result['size_bytes']} | {result['startup_median_ms']} |")
