"""Internal subprocess entry point for supervised full graph builds.

devcouncil: allow-unwired — launched with ``python -m`` by build_control.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path


def _emit(**payload: object) -> None:
    print(json.dumps(payload, separators=(",", ":")), flush=True)


def main() -> int:
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument("--root", required=True)
    parser.add_argument("--build-id", required=True)
    parser.add_argument("--changed-path", action="append", default=[])
    parser.add_argument("--no-liveness", action="store_true")
    args = parser.parse_args()
    root = Path(args.root).expanduser().resolve()

    def progress(phase: str, completed: int, total: int) -> None:
        _emit(
            build_id=args.build_id,
            state="building",
            phase=phase,
            completed=completed,
            total=total,
        )

    from devcouncil.codeintel.build_control import graph_build_session
    from devcouncil.indexing.graph.build import (
        CompatibilityGraphTooLarge,
        build_code_graph,
        write_code_graph,
    )

    # Acquire the cross-process writer lease in the child so an orphaned worker
    # (parent crash) still serializes against watchers/MCP writers.
    with graph_build_session(root):
        graph = build_code_graph(
            root,
            changed_paths=set(args.changed_path),
            liveness=not args.no_liveness,
            progress=progress,
        )
        graph.meta.update({
            "incremental": False,
            "changed_paths": sorted(set(args.changed_path)),
            "affected_paths": sorted(set(args.changed_path)),
            "affected_fraction": 1.0,
            "resolution_scope": "full",
        })
        compatibility_export = "healthy"
        reason = ""
        try:
            write_code_graph(root, graph, _lease_held=True)
        except CompatibilityGraphTooLarge as exc:
            compatibility_export = "degraded"
            reason = str(exc)
        _emit(
            build_id=args.build_id,
            state="degraded" if reason else "complete",
            phase="complete",
            completed=1,
            total=1,
            compatibility_export=compatibility_export,
            reason=reason,
        )
        return 0


if __name__ == "__main__":
    raise SystemExit(main())
