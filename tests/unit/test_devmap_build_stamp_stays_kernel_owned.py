"""The kernel's artifact stamp must survive a build on a root without git.

`build_map_result` fuses the build and the manifest into one kernel call and
lets the kernel stamp `generated_head` / `indexed_hash` / `content_fingerprint`
from its own inventory. When the root is not a git repository the kernel
reports that inventory as *unavailable*, and the seam used to fall back to
`stamp_freshness`: a Python read-modify-write of `repo_map.json` and
`code_graph.json` that re-serialised both (26 % larger, pretty-printed) and,
worse, left the kernel's `<db>.artifacts.json` describing files that no longer
existed byte for byte. Observed on 2026-09-06 on a `git archive` corpus copy:
every later build regenerated the manifest because the stamp could never be
current, and the doctor called the writer unverified — for artifacts the
kernel had written.

The kernel accepts the three digests as flags on `manifest`, and the seam
already passes them that way on the non-fused path. The fused path must do
the same instead of patching: one writer, one stamp.
"""

from __future__ import annotations

import json
import os
from pathlib import Path

import pytest


def _stamp_matches_disk(root: Path) -> list[tuple[str, bool]]:
    sidecar = root / ".devcouncil" / "codeintel" / "devmap.sqlite.artifacts.json"
    stamp = json.loads(sidecar.read_text(encoding="utf-8"))
    out = []
    for record in stamp.get("outputs", []):
        stat = os.stat(record["path"])
        out.append(
            (
                record.get("role", record["path"]),
                record.get("len") == stat.st_size and record.get("mtime_ns") == stat.st_mtime_ns,
            )
        )
    return out


def test_a_root_without_git_keeps_the_kernel_stamp_current(tmp_path: Path) -> None:
    from tests.unit.graph_fixtures import _have_kernel

    if not _have_kernel():
        pytest.skip("devmap kernel not built (cargo build --release -p devmap-cli)")
    from devcouncil.devmap_engine import build_map_result

    (tmp_path / "pkg").mkdir()
    (tmp_path / "pkg" / "a.py").write_text("def a():\n    return b()\n\ndef b():\n    return 1\n", encoding="utf-8")
    (tmp_path / ".devcouncil").mkdir()
    # Deliberately no `git init`: this is the case the kernel cannot stamp itself.

    result = build_map_result(tmp_path, output=tmp_path / ".devcouncil" / "repo_map.json")

    repo_map = json.loads((tmp_path / ".devcouncil" / "repo_map.json").read_text(encoding="utf-8"))
    assert repo_map.get("content_fingerprint"), "the map must still carry a fingerprint without git"
    assert result.freshness_source != "python", (
        "the digests must reach the artifacts through the kernel, not a Python rewrite"
    )
    matches = _stamp_matches_disk(tmp_path)
    assert matches and all(ok for _, ok in matches), (
        f"the kernel's stamp must describe the bytes on disk: {matches}"
    )
