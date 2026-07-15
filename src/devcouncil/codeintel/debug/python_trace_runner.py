"""Child-process entrypoint for exact Python call/return tracing."""

from __future__ import annotations

import argparse
import json
import runpy
import sys
import time
from collections import Counter
from pathlib import Path
from types import FrameType
from typing import Any


def _frame_id(frame: FrameType, root: Path) -> str | None:
    path = Path(frame.f_code.co_filename).resolve()
    try:
        rel = path.relative_to(root).as_posix()
    except ValueError:
        return None
    return f"{rel}::{frame.f_code.co_qualname}"


def run_trace(root: Path, output: Path, script: Path, argv: list[str]) -> int:
    root = root.resolve()
    calls: Counter[tuple[str, str]] = Counter()
    first_seen: dict[tuple[str, str], float] = {}
    last_seen: dict[tuple[str, str], float] = {}

    def profile(frame: FrameType, event: str, _arg: Any):
        if event != "call" or frame.f_back is None:
            return profile
        caller = _frame_id(frame.f_back, root)
        callee = _frame_id(frame, root)
        if caller and callee:
            key = (caller, callee)
            now = time.time()
            calls[key] += 1
            first_seen.setdefault(key, now)
            last_seen[key] = now
        return profile

    previous_argv = sys.argv
    sys.argv = [str(script), *argv]
    sys.setprofile(profile)
    exit_code = 0
    try:
        runpy.run_path(str(script), run_name="__main__")
    except SystemExit as exc:
        exit_code = int(exc.code) if isinstance(exc.code, int) else 1
    finally:
        sys.setprofile(None)
        sys.argv = previous_argv
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("w", encoding="utf-8") as handle:
        for (source, target), count in sorted(calls.items()):
            handle.write(json.dumps({
                "source": source,
                "target": target,
                "kind": "observed_calls",
                "count": count,
                "first_seen": first_seen[(source, target)],
                "last_seen": last_seen[(source, target)],
                "evidence": {"provider": "python-sys-setprofile"},
            }, sort_keys=True) + "\n")
    return exit_code


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("script", type=Path)
    parser.add_argument("args", nargs=argparse.REMAINDER)
    ns = parser.parse_args()
    return run_trace(ns.root, ns.output, ns.script, list(ns.args))


if __name__ == "__main__":
    raise SystemExit(main())
