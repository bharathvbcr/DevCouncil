#!/usr/bin/env python3
"""Generate golden baseline snapshots from frozen Python implementation (Task 1.3)."""

import json
import os
import re
import runpy
import shutil
import sys
from pathlib import Path
from typing import Any, Callable

REPO_ROOT = Path(__file__).resolve().parent.parent.parent.parent
sys.path.insert(0, str(REPO_ROOT / "src"))

try:
    from devcouncil.indexing.graph.build import build_code_graph
except ImportError:
    build_code_graph = None

try:
    from devcouncil.codeintel.languages.registry import LANGUAGE_SPECS
except ImportError:
    registry_globals = runpy.run_path(
        str(REPO_ROOT / "src" / "devcouncil" / "codeintel" / "languages" / "registry.py")
    )
    LANGUAGE_SPECS = tuple(registry_globals["LANGUAGE_SPECS"])


class SnapshotError(RuntimeError):
    """The frozen baseline could not be built authoritatively."""


def fixture_slug(language_name: str) -> str:
    aliases = {
        "Python": "python-lang",
        "C#": "csharp",
        "VB.NET": "vbnet",
        "C++": "cpp",
        "Objective-C": "objc",
        "Pascal/Delphi": "pascal",
        "Terraform/OpenTofu": "terraform",
    }
    if language_name in aliases:
        return aliases[language_name]
    return re.sub(r"[^a-z0-9]+", "-", language_name.lower()).strip("-")


def _value(value: Any) -> Any:
    return getattr(value, "value", value)


def normalize_nodes(nodes: list) -> list[dict]:
    def get_attr(n, name, default=""):
        if isinstance(n, dict):
            return n.get(name, default)
        return getattr(n, name, default)

    def key_fn(n):
        return (
            get_attr(n, "id", ""),
            str(_value(get_attr(n, "kind", ""))),
            get_attr(n, "path", ""),
            get_attr(n, "line", 0),
            get_attr(n, "name", ""),
        )

    sorted_nodes = sorted(nodes, key=key_fn)
    return [
        {
            "id": get_attr(n, "id"),
            "kind": _value(get_attr(n, "kind")),
            "path": get_attr(n, "path"),
            "line": get_attr(n, "line", 0),
            "name": get_attr(n, "name"),
            "exported": get_attr(n, "exported", False),
        }
        for n in sorted_nodes
    ]


def normalize_edges(edges: list) -> list[dict]:
    def get_attr(e, name, default=""):
        if isinstance(e, dict):
            return e.get(name, default)
        return getattr(e, name, default)

    def key_fn(e):
        return (
            get_attr(e, "source", ""),
            get_attr(e, "target", ""),
            get_attr(e, "kind", ""),
            str(_value(get_attr(e, "confidence", ""))),
        )

    sorted_edges = sorted(edges, key=key_fn)
    return [
        {
            "source": get_attr(e, "source"),
            "target": get_attr(e, "target"),
            "kind": get_attr(e, "kind"),
            "confidence": _value(get_attr(e, "confidence")),
        }
        for e in sorted_edges
    ]


def normalize_dead(dead: list) -> list[dict]:
    def get_attr(item, name, default=""):
        if isinstance(item, dict):
            return item.get(name, default)
        return getattr(item, name, default)

    rows = [
        {
            "id": get_attr(item, "id"),
            "confidence": _value(get_attr(item, "confidence")),
        }
        for item in dead
    ]
    return sorted(rows, key=lambda row: (row["id"], str(row["confidence"])))


def generate_fixture_snapshot(
    fixture_path: Path,
    output_dir: Path,
    *,
    builder: Callable[[Path], Any] | None = None,
) -> None:
    graph_builder = builder or build_code_graph
    if graph_builder is None:
        raise SnapshotError("frozen Python graph builder is unavailable")
    try:
        graph = graph_builder(fixture_path)
    except Exception as err:
        raise SnapshotError(f"graph build failed for {fixture_path}: {err}") from err
    finally:
        # The frozen builder maintains a parse cache inside the target repo.
        # Fixture inputs must remain immutable; only golden outputs are artifacts.
        shutil.rmtree(fixture_path / ".devcouncil" / "cache", ignore_errors=True)

    if hasattr(graph, "nodes"):
        nodes = list(graph.nodes.values()) if isinstance(graph.nodes, dict) else list(graph.nodes)
        edges = list(graph.edges)
        dead = list(getattr(graph, "dead_code", []))
    elif isinstance(graph, dict):
        nodes = graph.get("nodes", [])
        edges = graph.get("edges", [])
        dead = graph.get("dead", [])
    else:
        raise SnapshotError(f"unsupported graph result {type(graph).__name__}")

    norm_nodes = normalize_nodes(nodes)
    norm_edges = normalize_edges(edges)
    norm_dead = normalize_dead(dead)

    output_dir.mkdir(parents=True, exist_ok=True)
    _atomic_write_json(output_dir / "nodes.json", norm_nodes)
    _atomic_write_json(output_dir / "edges.json", norm_edges)
    _atomic_write_json(output_dir / "dead.json", norm_dead)

    print(f"Generated snapshot in {output_dir}: {len(norm_nodes)} nodes, {len(norm_edges)} edges, {len(norm_dead)} dead")


def _atomic_write_json(path: Path, payload: Any) -> None:
    temporary = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    try:
        with temporary.open("w", encoding="utf-8") as handle:
            json.dump(payload, handle, indent=2, ensure_ascii=False)
            handle.write("\n")
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def _ensure_hash_seed() -> None:
    if os.environ.get("PYTHONHASHSEED") == "0":
        return
    environment = dict(os.environ)
    environment["PYTHONHASHSEED"] = "0"
    os.execve(sys.executable, [sys.executable, *sys.argv], environment)


def main():
    _ensure_hash_seed()
    testdata_dir = REPO_ROOT / "rust-port" / "testdata"
    golden_dir = testdata_dir / "golden"

    if not LANGUAGE_SPECS:
        raise SnapshotError("frozen Python language registry is unavailable")

    fixtures = [
        testdata_dir / "fixtures" / "tier_a" / "ts_app",
        testdata_dir / "fixtures" / "tier_a" / "python_app",
        testdata_dir / "fixtures" / "tier_b" / "rust_app",
        testdata_dir / "fixtures" / "tier_b" / "go_app",
        testdata_dir / "fixtures" / "tier_c" / "solidity_app",
    ]

    for fix in fixtures:
        if fix.exists():
            out = golden_dir / fix.name
            generate_fixture_snapshot(fix, out)

    language_root = testdata_dir / "fixtures" / "languages"
    expected = {fixture_slug(spec.name) for spec in LANGUAGE_SPECS}
    actual = {path.name for path in language_root.iterdir() if path.is_dir()}
    if actual != expected:
        missing = sorted(expected - actual)
        extra = sorted(actual - expected)
        raise SnapshotError(f"language fixture mismatch: missing={missing}, extra={extra}")

    manifest = [
        {
            "name": spec.name,
            "grammar": spec.grammar,
            "extensions": list(spec.extensions),
            "embedded": list(spec.embedded),
            "fixture": fixture_slug(spec.name),
        }
        for spec in LANGUAGE_SPECS
    ]
    _atomic_write_json(golden_dir / "language_specs.json", manifest)
    for spec in LANGUAGE_SPECS:
        slug = fixture_slug(spec.name)
        generate_fixture_snapshot(
            language_root / slug,
            golden_dir / "languages" / slug,
        )


if __name__ == "__main__":
    main()
