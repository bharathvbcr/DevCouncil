#!/usr/bin/env python3
"""Run deterministic code-intelligence performance ratchets."""

from __future__ import annotations

import argparse
import importlib.util
import sys
import tempfile
from pathlib import Path
from types import ModuleType


def _load_harness(repo_root: Path) -> ModuleType:
    path = repo_root / "tests" / "performance" / "benchmark_harness.py"
    spec = importlib.util.spec_from_file_location("codeintel_benchmark_harness", path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load benchmark harness from {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def main() -> int:
    parser = argparse.ArgumentParser(
        description=(
            "Run the fast PR code-intelligence ratchets, or the explicit "
            "10,000-file heavy profile."
        )
    )
    parser.add_argument("--profile", choices=("fast", "heavy"), default="fast")
    parser.add_argument(
        "--output",
        type=Path,
        default=Path("artifacts/codeintel-benchmark.json"),
        help="JSON artifact path; a Markdown summary is written beside it",
    )
    parser.add_argument(
        "--fixture-root",
        type=Path,
        help="Retain generated sources at this path instead of using a temporary directory",
    )
    args = parser.parse_args()

    repo_root = Path(__file__).resolve().parents[1]
    sys.path.insert(0, str(repo_root / "src"))
    harness = _load_harness(repo_root)

    if args.fixture_root is not None:
        args.fixture_root.mkdir(parents=True, exist_ok=True)
        result = harness.run_benchmark(args.fixture_root, profile=args.profile)
    else:
        with tempfile.TemporaryDirectory(prefix=f"codeintel-{args.profile}-") as temporary:
            result = harness.run_benchmark(Path(temporary), profile=args.profile)

    output, summary = harness.write_artifacts(result, args.output)
    print(harness.render_summary(result), end="")
    print(f"JSON: {output}")
    print(f"Summary: {summary}")
    return 1 if result["violations"] else 0


if __name__ == "__main__":
    raise SystemExit(main())
