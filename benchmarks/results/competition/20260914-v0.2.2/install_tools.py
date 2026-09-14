#!/usr/bin/env python3
"""Reinstall the pinned competitor tools the 2026-09-13 cleanup pass deleted.

Every tarball is verified against the sha256 recorded by the build-48cd3c7 run
(`installed-binaries.json`) BEFORE it is extracted, and the extracted executable
is verified against that run's `binary_sha256`. A digest mismatch is a failure,
never a warning: an unverified tool would silently turn a version comparison
into a comparison of two unknowns.

Graphify is a Python package, so it is pinned by the same requirements file the
prior run recorded rather than by a binary digest.
"""

import hashlib
import json
import shutil
import subprocess
import sys
import tarfile
import urllib.request
from pathlib import Path

OUT = Path(__file__).resolve().parent
PRIOR = OUT.parent / "20260913-48cd3c7"
TOOLS = Path("/Users/bharath/Code/devtools/DevCouncil/.devcouncil/benchmarks/"
             "20260914-v0.2.2/tools")

# Relative path of the executable inside each extracted tarball.
EXECUTABLE = {
    "zzet/gortex": "gortex",
    "DeusData/codebase-memory-mcp": "codebase-memory-mcp",
    "colbymchenry/codegraph": "codegraph-darwin-arm64/bin/codegraph",
}
SLUG = {
    "zzet/gortex": "gortex",
    "DeusData/codebase-memory-mcp": "cbm",
    "colbymchenry/codegraph": "codegraph",
}


def digest(path):
    h = hashlib.sha256()
    with open(path, "rb") as stream:
        for chunk in iter(lambda: stream.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def fetch(pin, dest_dir):
    """Download, verify, extract one pinned release. Returns the executable path."""
    slug = SLUG[pin["repo"]]
    dest_dir.mkdir(parents=True, exist_ok=True)
    tarball = dest_dir / pin["name"]

    if not (tarball.exists() and digest(tarball) == pin["sha256"]):
        print("  downloading %s (%.1f MB)" % (pin["url"], pin["bytes"] / 1e6), flush=True)
        urllib.request.urlretrieve(pin["url"], tarball)

    got = digest(tarball)
    if got != pin["sha256"]:
        raise SystemExit("%s: tarball sha256 %s != pinned %s" % (slug, got, pin["sha256"]))
    if tarball.stat().st_size != pin["bytes"]:
        raise SystemExit("%s: size %d != pinned %d" % (slug, tarball.stat().st_size, pin["bytes"]))
    print("  tarball sha256 verified: %s" % got[:16], flush=True)

    extracted = dest_dir / "x"
    if extracted.exists():
        shutil.rmtree(extracted)
    extracted.mkdir()
    with tarfile.open(tarball) as archive:
        archive.extractall(extracted)

    executable = extracted / EXECUTABLE[pin["repo"]]
    if not executable.exists():
        raise SystemExit("%s: %s not in tarball" % (slug, EXECUTABLE[pin["repo"]]))
    executable.chmod(0o755)

    got_bin = digest(executable)
    if got_bin != pin["binary_sha256"]:
        raise SystemExit("%s: executable sha256 %s != pinned %s"
                         % (slug, got_bin, pin["binary_sha256"]))
    print("  executable sha256 verified: %s" % got_bin[:16], flush=True)
    return executable


def graphify():
    """Recreate the pinned Graphify venv from the prior run's requirements."""
    venv = TOOLS / "graphify-venv"
    executable = venv / "bin" / "graphify"
    requirements = PRIOR / "graphify-installed-requirements.txt"
    if executable.exists():
        print("  already present", flush=True)
        return executable
    subprocess.run([sys.executable, "-m", "venv", str(venv)], check=True)
    subprocess.run([str(venv / "bin" / "pip"), "install", "--quiet",
                    "--disable-pip-version-check", "-r", str(requirements)], check=True)
    if not executable.exists():
        raise SystemExit("graphify: venv built but no bin/graphify")
    return executable


def main():
    pins = json.loads((PRIOR / "installed-binaries.json").read_text())
    resolved = {}
    for pin in pins:
        slug = SLUG[pin["repo"]]
        print("%s %s" % (slug, pin["version"]), flush=True)
        resolved[slug] = str(fetch(pin, TOOLS / slug))

    print("graphify 0.9.59 (pinned requirements)", flush=True)
    resolved["graphify"] = str(graphify())

    resolved["gitnexus"] = "/opt/homebrew/bin/gitnexus"
    (OUT / "installed-tools.json").write_text(json.dumps(resolved, indent=2) + "\n")
    print("\nwrote installed-tools.json")
    for slug in sorted(resolved):
        print("  %-10s %s" % (slug, resolved[slug]))


if __name__ == "__main__":
    main()
