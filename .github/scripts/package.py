"""Packs the modular flavour built by `cargo xtask dist` into archives: one
for the core and one per plugin, each laid out as it installs under a prefix.

Usage: package.py <dist dir> <target> <archive dir>
Archives are .tar.gz, or .zip for Windows targets:
  codix-<target>                 bin/codix
  codix-<kind>-<name>-<target>   lib/codix/plugins/<kind>/<name>/
"""

import os
import shutil
import sys
import tarfile
import zipfile

dist, target, out = sys.argv[1:4]
windows = "windows" in target
os.makedirs(out, exist_ok=True)
KINDS = {"language": "lang", "build": "build", "vcs": "vcs"}


def archive(name, paths):
    """Archives `paths`, relative to the dist directory."""
    if windows:
        with zipfile.ZipFile(os.path.join(out, name + ".zip"), "w", zipfile.ZIP_DEFLATED) as z:
            for path in paths:
                for root, _, files in os.walk(os.path.join(dist, path)):
                    for file in files:
                        full = os.path.join(root, file)
                        z.write(full, os.path.relpath(full, dist))
                if os.path.isfile(os.path.join(dist, path)):
                    z.write(os.path.join(dist, path), path)
    else:
        with tarfile.open(os.path.join(out, name + ".tar.gz"), "w:gz") as t:
            for path in paths:
                t.add(os.path.join(dist, path), path)


binary = "codix.exe" if windows else "codix"
archive(f"codix-{target}", [os.path.join("bin", binary)])
plugins = os.path.join("lib", "codix", "plugins")
for kind in sorted(os.listdir(os.path.join(dist, plugins))):
    for name in sorted(os.listdir(os.path.join(dist, plugins, kind))):
        archive(f"codix-{KINDS[kind]}-{name}-{target}", [os.path.join(plugins, kind, name)])
print("\n".join(sorted(os.listdir(out))))
