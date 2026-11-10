"""Records a release binary's size and its median `--version` startup time,
and with a plugin directory, the size of each plugin.

Usage: measure.py <binary> <target> <json output> [<plugin dir>]
Writes the figures to the JSON file and Markdown tables to stdout.
"""

import json
import os
import statistics
import subprocess
import sys
import time

RUNS = 20

binary, target, output = sys.argv[1:4]
plugin_dir = sys.argv[4] if len(sys.argv) > 4 else None

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
if plugin_dir:
    plugins = {}
    for kind in sorted(os.listdir(plugin_dir)):
        for name in sorted(os.listdir(os.path.join(plugin_dir, kind))):
            size = 0
            for root, _, files in os.walk(os.path.join(plugin_dir, kind, name)):
                size += sum(os.path.getsize(os.path.join(root, f)) for f in files)
            plugins[f"{kind}/{name}"] = size
    result["plugins_bytes"] = plugins
with open(output, "w", encoding="utf-8") as f:
    json.dump(result, f, indent=2)

print("| Target | Size (bytes) | `--version` median (ms) |")
print("| --- | --- | --- |")
print(f"| {target} | {result['size_bytes']} | {result['startup_median_ms']} |")

if plugin_dir:
    print()
    print("| Plugin | Size (bytes) |")
    print("| --- | --- |")
    for plugin, size in result["plugins_bytes"].items():
        print(f"| {plugin} | {size} |")
