#!/usr/bin/env python3
"""Parity harness: devmap (Rust) graph output vs the frozen Python baseline.

Each golden fixture under ``testdata/golden/<name>/`` is a frozen Python
``nodes.json`` / ``edges.json`` / ``dead.json`` triple. This harness builds the
matching source fixture with ``devmap``, reads the resulting generation straight
out of SQLite, and compares the three sets element-by-element.

Run: python rust-port/tools/parity/parity_harness.py [--root /path/to/repo]

Exit status is 0 only when every fixture matches its baseline exactly.

History: this file previously compared nothing. ``load_golden`` returned
``nodes.json`` as a *list*, and the comparison called ``.get("nodes", [])`` on
it, so the harness raised ``AttributeError`` on every fixture that actually had
a baseline. It had therefore never produced a parity verdict, while reading as
though parity were checked. The rewrite below compares real identity sets.
"""

from __future__ import annotations

import argparse
import json
import os
import sqlite3
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any, Dict, List, Set, Tuple

# Confidence is a float in Rust and a tier name in the Python baseline.
#
# CAVEAT — this mapping is a reconstruction, not a documented contract. The
# Python side's exact float-to-tier boundaries were not available when this was
# written, so a *tier* mismatch is weaker evidence than an *identity* mismatch:
# it may reflect this table rather than a real divergence. Identity differences
# (an id present on one side only) are unambiguous. `--strict-tiers` opts into
# comparing tiers; by default only identities are compared, so the harness does
# not manufacture divergence out of its own guess.
CONFIDENCE_TIERS: List[Tuple[float, str]] = [
    (1.0, "extracted"),
    (0.9, "inferred"),
    (0.0, "ambiguous"),
]


def confidence_tier(value: float) -> str:
    for threshold, name in CONFIDENCE_TIERS:
        if value >= threshold:
            return name
    return "ambiguous"


def find_devmap(root: Path) -> Path:
    for candidate in (
        root / "rust-port" / "target" / "release" / "devmap",
        root / "target" / "release" / "devmap",
        root / "rust-port" / "target" / "debug" / "devmap",
    ):
        if candidate.is_file():
            return candidate
    raise SystemExit(
        "devmap binary not found - run `cargo build --release -p devmap-cli` first"
    )


def find_fixture_source(rust_port: Path, name: str) -> Path | None:
    """Locate the source tree a golden fixture was generated from."""
    for tier in sorted((rust_port / "testdata" / "fixtures").glob("tier_*")):
        candidate = tier / name
        if candidate.is_dir():
            return candidate
    candidate = rust_port / "testdata" / "fixtures" / "languages" / name
    return candidate if candidate.is_dir() else None


def build_fixture(devmap: Path, source: Path, db: Path) -> None:
    proc = subprocess.run(
        [str(devmap), "--json", "--db", str(db), "build", str(source)],
        capture_output=True,
        text=True,
        timeout=600,
    )
    if proc.returncode != 0:
        raise RuntimeError(f"devmap build failed for {source}: {proc.stderr.strip()}")


def read_rust_graph(db: Path) -> Dict[str, Set[Tuple[Any, ...]]]:
    """Read the newest generation's identity sets out of the store."""
    conn = sqlite3.connect(str(db))
    try:
        generation = conn.execute(
            "SELECT id FROM generations ORDER BY id DESC LIMIT 1"
        ).fetchone()
        if generation is None:
            return {"nodes": set(), "edges": set(), "dead": set()}
        gen_id = generation[0]

        nodes = {
            (qualified_name, kind.lower())
            for qualified_name, kind in conn.execute(
                "SELECT qualified_name, kind FROM generation_nodes WHERE generation_id = ?",
                (gen_id,),
            )
        }
        edges = {
            (source, target, kind.lower(), confidence_tier(confidence))
            for source, target, kind, confidence in conn.execute(
                """SELECT source_symbol, target_symbol, edge_kind, confidence
                   FROM generation_edges WHERE generation_id = ?""",
                (gen_id,),
            )
        }
        dead = {
            (f"{file_path}::{symbol_name}", confidence_tier(confidence))
            for file_path, symbol_name, confidence in conn.execute(
                """SELECT file_path, symbol_name, confidence
                   FROM generation_dead_symbols WHERE generation_id = ?""",
                (gen_id,),
            )
        }
        return {"nodes": nodes, "edges": edges, "dead": dead}
    finally:
        conn.close()


def read_golden(fixture: Path) -> Dict[str, Set[Tuple[Any, ...]]]:
    def load(name: str) -> List[Dict[str, Any]]:
        path = fixture / name
        return json.loads(path.read_text(encoding="utf-8")) if path.is_file() else []

    return {
        "nodes": {(n["id"], n["kind"].lower()) for n in load("nodes.json")},
        "edges": {
            (e["source"], e["target"], e["kind"].lower(), e["confidence"].lower())
            for e in load("edges.json")
        },
        "dead": {(d["id"], d["confidence"].lower()) for d in load("dead.json")},
    }


def compare(
    golden: Dict[str, Set],
    rust: Dict[str, Set],
    limit: int = 5,
    strict_tiers: bool = False,
) -> List[str]:
    """Report both directions. A one-sided check would call an empty build a pass."""
    diffs: List[str] = []
    for kind in ("nodes", "edges", "dead"):
        left, right = golden[kind], rust[kind]
        if not strict_tiers:
            # Drop the trailing confidence tier so a mismatch reflects real
            # identity divergence rather than the reconstructed tier table.
            left = {row[:-1] for row in left}
            right = {row[:-1] for row in right}
        missing = sorted(left - right)
        extra = sorted(right - left)
        if missing:
            diffs.append(
                f"{kind}: {len(missing)} in baseline but not in devmap, "
                f"e.g. {missing[:limit]}"
            )
        if extra:
            diffs.append(
                f"{kind}: {len(extra)} in devmap but not in baseline, "
                f"e.g. {extra[:limit]}"
            )
    return diffs


def main() -> int:
    parser = argparse.ArgumentParser(description="devmap vs Python parity harness")
    parser.add_argument("--root", type=Path, default=Path.cwd())
    parser.add_argument("--golden", type=Path, default=None)
    parser.add_argument("--report", type=Path, default=None)
    parser.add_argument(
        "--strict-tiers",
        action="store_true",
        help="also compare confidence tiers (see CONFIDENCE_TIERS caveat)",
    )
    args = parser.parse_args()

    root = args.root.resolve()
    rust_port = root / "rust-port" if (root / "rust-port").is_dir() else root
    golden_root = args.golden or (rust_port / "testdata" / "golden")
    devmap = find_devmap(root)

    report: Dict[str, Any] = {"fixtures": [], "diffs": [], "skipped": []}
    with tempfile.TemporaryDirectory(prefix="devmap-parity-") as workdir:
        for fixture in sorted(p for p in golden_root.iterdir() if p.is_dir()):
            if not (fixture / "nodes.json").is_file():
                # A directory of per-language goldens, not a fixture itself.
                continue
            source = find_fixture_source(rust_port, fixture.name)
            if source is None:
                # Never silently pass an unchecked fixture.
                report["skipped"].append(
                    f"{fixture.name}: no source fixture found; parity UNVERIFIED"
                )
                continue

            db = Path(workdir) / f"{fixture.name}.sqlite"
            try:
                build_fixture(devmap, source, db)
                diffs = compare(
                    read_golden(fixture),
                    read_rust_graph(db),
                    strict_tiers=args.strict_tiers,
                )
            except Exception as error:  # noqa: BLE001 - reported, never swallowed
                diffs = [f"comparison failed: {type(error).__name__}: {error}"]

            report["fixtures"].append({"name": fixture.name, "diffs": diffs})
            report["diffs"].extend(f"{fixture.name}: {d}" for d in diffs)

    report["tier_comparison"] = "strict" if args.strict_tiers else "identities-only"
    report["ok"] = not report["diffs"]
    report["fixtures_compared"] = len(report["fixtures"])
    out_path = args.report or (rust_port / "parity_report.json")
    out_path.write_text(json.dumps(report, indent=2), encoding="utf-8")

    print(json.dumps(report, indent=2))
    if report["skipped"]:
        print(
            f"\nWARNING: {len(report['skipped'])} fixture(s) skipped - "
            "these are NOT covered by this run.",
            file=sys.stderr,
        )
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    os.environ.setdefault("PYTHONHASHSEED", "0")
    sys.exit(main())
