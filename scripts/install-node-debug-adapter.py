"""Install the pinned Node debug adapter used by real DAP integration tests."""

from __future__ import annotations

import argparse
import gzip
import hashlib
import io
import os
import tempfile
import urllib.request
import zipfile
from pathlib import Path

VERSION = "1.42.5"
URL = (
    "https://marketplace.visualstudio.com/_apis/public/gallery/publishers/"
    f"ms-vscode/vsextensions/node-debug2/{VERSION}/vspackage"
)
SHA256 = "a4d4017994eca3dea0aa605de5cf7c48e757bbce6a81514297bb1432c435e026"
ADAPTER_RELATIVE_PATH = Path("extension/out/src/nodeDebug.js")


def install(output_dir: Path) -> Path:
    """Download, verify, and extract the adapter into *output_dir*."""
    adapter_path = output_dir / ADAPTER_RELATIVE_PATH
    with urllib.request.urlopen(URL, timeout=60) as response:
        archive = response.read()
    actual = hashlib.sha256(archive).hexdigest()
    if actual != SHA256:
        raise RuntimeError(f"node-debug2 archive checksum mismatch: {actual} != {SHA256}")

    output_dir.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(io.BytesIO(gzip.decompress(archive))) as package:
        package.extractall(output_dir)
    if not adapter_path.is_file():
        raise RuntimeError(f"node-debug2 archive is missing {ADAPTER_RELATIVE_PATH}")
    return adapter_path


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output-dir", type=Path)
    parser.add_argument("--github-env", type=Path)
    args = parser.parse_args()

    output_dir = args.output_dir or Path(os.environ.get("RUNNER_TEMP", tempfile.gettempdir())) / "node-debug2"
    github_env = args.github_env
    if github_env is None and os.environ.get("GITHUB_ENV"):
        github_env = Path(os.environ["GITHUB_ENV"])

    adapter_path = install(output_dir.expanduser().resolve())
    values = {
        "DEVCOUNCIL_NODE_DEBUG2_PATH": str(adapter_path),
        "DEVCOUNCIL_NODE_DEBUG2_VERSION": VERSION,
    }
    if github_env:
        with github_env.open("a", encoding="utf-8") as env_file:
            for name, value in values.items():
                env_file.write(f"{name}={value}\n")
    for name, value in values.items():
        print(f"{name}={value}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
