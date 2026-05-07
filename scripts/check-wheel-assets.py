from __future__ import annotations

import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path


REQUIRED_FILES = {
    "devcouncil/llm/model_defaults.yaml",
    "devcouncil/telemetry/model_pricing.yaml",
}


def main() -> int:
    repo_root = Path(__file__).resolve().parents[1]
    with tempfile.TemporaryDirectory(prefix="devcouncil-wheel-") as tmp:
        out_dir = Path(tmp)
        subprocess.check_call(
            ["uv", "build", "--wheel", "--out-dir", str(out_dir)],
            cwd=repo_root,
        )
        wheels = list(out_dir.glob("*.whl"))
        if len(wheels) != 1:
            print(f"Expected exactly one wheel, found {len(wheels)}.", file=sys.stderr)
            return 1
        with zipfile.ZipFile(wheels[0]) as wheel:
            names = set(wheel.namelist())
        missing = sorted(REQUIRED_FILES - names)
        if missing:
            print("Wheel is missing required packaged assets:", file=sys.stderr)
            for name in missing:
                print(f"  - {name}", file=sys.stderr)
            return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
