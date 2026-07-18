"""Deterministic code-intelligence benchmark harness.

The fast profile is suitable for pull-request ratchets. The heavy profile is an
explicit 10,000-file run intended for scheduled or release qualification.
"""

from __future__ import annotations

import json
import math
import os
import sqlite3
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable

try:
    import resource
except ImportError:  # Windows has no resource module
    resource = None  # type: ignore[assignment]

from devcouncil.codeintel.query import CodeIntelQueryEngine
from devcouncil.codeintel.service import get_codeintel_service
from devcouncil.codeintel.sync.incremental import sync_affected_paths
from devcouncil.indexing.graph.build import build_code_graph, graph_path, write_code_graph

THRESHOLDS_PATH = Path(__file__).with_name("thresholds.json")


@dataclass(frozen=True)
class FixtureSpec:
    profile: str
    file_count: int
    package_count: int

    @classmethod
    def load(cls, profile: str) -> FixtureSpec:
        config = load_thresholds(profile)
        return cls(
            profile=profile,
            file_count=int(config["file_count"]),
            package_count=int(config["package_count"]),
        )


def load_thresholds(profile: str) -> dict[str, Any]:
    all_thresholds = json.loads(THRESHOLDS_PATH.read_text(encoding="utf-8"))
    if profile not in all_thresholds:
        choices = ", ".join(sorted(all_thresholds))
        raise ValueError(f"unknown benchmark profile {profile!r}; choose {choices}")
    return dict(all_thresholds[profile])


def fixture_paths(spec: FixtureSpec) -> list[str]:
    """Return a stable monorepo layout without touching disk."""
    width = max(4, len(str(spec.file_count)))
    return [
        f"packages/pkg_{index % spec.package_count:03d}/src/"
        f"{'main' if index < spec.package_count else f'module_{index:0{width}d}'}.py"
        for index in range(spec.file_count)
    ]


def materialize_fixture(root: Path, spec: FixtureSpec) -> list[str]:
    paths = fixture_paths(spec)
    (root / ".devcouncil").mkdir(parents=True, exist_ok=True)
    for index, rel in enumerate(paths):
        path = root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        import_line = ""
        call = str(index)
        next_index = index + spec.package_count
        if next_index < spec.file_count:
            module = Path(paths[next_index]).stem
            import_line = f"from .{module} import benchmark_symbol_{next_index}\n\n"
            call = f"benchmark_symbol_{next_index}() + 1"
        path.write_text(
            f"{import_line}def benchmark_symbol_{index}():\n"
            f"    return {call}\n",
            encoding="utf-8",
        )
    return paths


def _peak_rss_bytes() -> int:
    if resource is not None:
        value = int(resource.getrusage(resource.RUSAGE_SELF).ru_maxrss)
        # Darwin reports bytes; Linux and the BSDs used in CI report KiB.
        return value if sys.platform == "darwin" else value * 1024
    import ctypes

    class _ProcessMemoryCounters(ctypes.Structure):
        _fields_ = [
            ("cb", ctypes.c_uint32),
            ("PageFaultCount", ctypes.c_uint32),
            ("PeakWorkingSetSize", ctypes.c_size_t),
            ("WorkingSetSize", ctypes.c_size_t),
            ("QuotaPeakPagedPoolUsage", ctypes.c_size_t),
            ("QuotaPagedPoolUsage", ctypes.c_size_t),
            ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t),
            ("QuotaNonPagedPoolUsage", ctypes.c_size_t),
            ("PagefileUsage", ctypes.c_size_t),
            ("PeakPagefileUsage", ctypes.c_size_t),
        ]

    counters = _ProcessMemoryCounters()
    counters.cb = ctypes.sizeof(counters)
    handle = ctypes.windll.kernel32.GetCurrentProcess()  # type: ignore[attr-defined]
    ok = ctypes.windll.psapi.GetProcessMemoryInfo(  # type: ignore[attr-defined]
        handle, ctypes.byref(counters), counters.cb
    )
    return int(counters.PeakWorkingSetSize) if ok else 0


def _percentile(values: list[float], percentile: float) -> float:
    ordered = sorted(values)
    rank = max(0, math.ceil(percentile * len(ordered)) - 1)
    return ordered[rank]


def _latencies_ms(operation: Callable[[], object], iterations: int) -> dict[str, float]:
    samples = []
    for _ in range(iterations):
        started = time.perf_counter()
        operation()
        samples.append((time.perf_counter() - started) * 1000.0)
    return {
        "min": min(samples),
        "p50": _percentile(samples, 0.50),
        "p95": _percentile(samples, 0.95),
        "max": max(samples),
    }


def _database_metrics(path: Path) -> dict[str, int | float]:
    with sqlite3.connect(path) as conn:
        conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
        page_size = int(conn.execute("PRAGMA page_size").fetchone()[0])
        page_count = int(conn.execute("PRAGMA page_count").fetchone()[0])
        freelist_count = int(conn.execute("PRAGMA freelist_count").fetchone()[0])
    allocated = page_size * page_count
    return {
        "file_bytes": path.stat().st_size,
        "allocated_bytes": allocated,
        "page_size": page_size,
        "page_count": page_count,
        "freelist_count": freelist_count,
        "freelist_bytes": page_size * freelist_count,
        "freelist_ratio": freelist_count / max(1, page_count),
    }


def run_benchmark(root: Path, *, profile: str = "fast") -> dict[str, Any]:
    root = root.expanduser().resolve()
    spec = FixtureSpec.load(profile)
    thresholds = load_thresholds(profile)
    paths = materialize_fixture(root, spec)

    started = time.perf_counter()
    graph = build_code_graph(root, paths, liveness=True)
    write_code_graph(root, graph)
    cold_seconds = time.perf_counter() - started

    service = get_codeintel_service(root)
    changed_path = paths[0]
    changed_index = 0
    next_index = spec.package_count
    next_module = Path(paths[next_index]).stem
    changed_source = root / changed_path
    changed_source.write_text(
        f"from .{next_module} import benchmark_symbol_{next_index}\n\n"
        f"def benchmark_symbol_{changed_index}():\n"
        f"    return benchmark_symbol_{next_index}() + 2\n",
        encoding="utf-8",
    )
    started = time.perf_counter()
    updated = sync_affected_paths(service, [changed_path], liveness=True)
    one_file_seconds = time.perf_counter() - started

    write_stats = dict(service.store.last_write_stats)
    payload_rows = sum(
        int(write_stats.get(key, 0))
        for key in (
            "node_payloads_written",
            "edge_payloads_written",
            "dead_payloads_written",
        )
    )
    compatibility_path = graph_path(root)
    database = _database_metrics(service.store.path)
    compatibility_bytes = compatibility_path.stat().st_size
    database_ratio = float(database["allocated_bytes"]) / max(1, compatibility_bytes)

    query = CodeIntelQueryEngine(service)
    iterations = int(thresholds["query_iterations"])
    target = f"benchmark_symbol_{changed_index}"
    query_metrics = {
        "search": _latencies_ms(
            lambda: service.store.search(target, limit=20),
            iterations,
        ),
        "explore": _latencies_ms(
            lambda: query.explore(target, limit=20),
            iterations,
        ),
        "dead": _latencies_ms(
            lambda: query.dead(minimum_confidence="inferred"),
            iterations,
        ),
    }

    result: dict[str, Any] = {
        "schema_version": updated.schema_version,
        "profile": profile,
        "fixture": {
            "file_count": len(paths),
            "package_count": spec.package_count,
            "shape": "round-robin-monorepo",
            "changed_path": changed_path,
        },
        "cold": {
            "wall_seconds": cold_seconds,
            "node_count": len(graph.nodes),
            "edge_count": len(graph.edges),
        },
        "one_file": {
            "wall_seconds": one_file_seconds,
            "affected_files": len(updated.meta.get("affected_paths") or []),
            "affected_fraction": float(updated.meta.get("affected_fraction") or 0.0),
            "payload_rows_written": payload_rows,
            "write_stats": write_stats,
        },
        "memory": {"peak_rss_bytes": _peak_rss_bytes()},
        "storage": {
            "database": database,
            "compatibility_export_bytes": compatibility_bytes,
            "database_to_compatibility_ratio": database_ratio,
        },
        "queries_ms": query_metrics,
        "thresholds": thresholds,
    }
    result["violations"] = ratchet_violations(result)
    return result


def ratchet_violations(result: dict[str, Any]) -> list[str]:
    threshold = result["thresholds"]
    violations: list[str] = []

    def maximum(metric: str, actual: float, limit: float) -> None:
        if actual > limit:
            violations.append(f"{metric}: {actual:.4f} > {limit:.4f}")

    # Windows runners' file I/O and fsync are highly variable (cold build has
    # been observed at 15s and 29s in back-to-back runs), so the wall-clock
    # timers are too noisy to enforce there. The deterministic structural
    # ratchets below (payload_rows_written, affected_files, db ratio) still run
    # on every platform and catch real regressions — e.g. a fall-back-to-full-
    # rebuild shows up as payload_rows_written jumping from 0 to ~1500.
    enforce_wall_clock = sys.platform != "win32"

    if int(result["fixture"]["file_count"]) != int(threshold["file_count"]):
        violations.append(
            "fixture.file_count: "
            f"{result['fixture']['file_count']} != {threshold['file_count']}"
        )
    if int(result["fixture"]["package_count"]) != int(threshold["package_count"]):
        violations.append(
            "fixture.package_count: "
            f"{result['fixture']['package_count']} != {threshold['package_count']}"
        )
    if int(result["schema_version"]) != 2:
        violations.append(f"schema_version: {result['schema_version']} != 2")

    if enforce_wall_clock:
        maximum(
            "cold.wall_seconds",
            float(result["cold"]["wall_seconds"]),
            float(threshold["cold_wall_seconds_max"]),
        )
        maximum(
            "one_file.wall_seconds",
            float(result["one_file"]["wall_seconds"]),
            float(threshold["one_file_wall_seconds_max"]),
        )
    maximum(
        "memory.peak_rss_bytes",
        float(result["memory"]["peak_rss_bytes"]),
        float(threshold["peak_rss_bytes_max"]),
    )
    maximum(
        "storage.database_to_compatibility_ratio",
        float(result["storage"]["database_to_compatibility_ratio"]),
        float(threshold["database_to_compatibility_ratio_max"]),
    )
    maximum(
        "storage.database.freelist_ratio",
        float(result["storage"]["database"]["freelist_ratio"]),
        float(threshold["freelist_ratio_max"]),
    )
    maximum(
        "one_file.payload_rows_written",
        float(result["one_file"]["payload_rows_written"]),
        float(threshold["incremental_payload_rows_max"]),
    )
    maximum(
        "one_file.affected_files",
        float(result["one_file"]["affected_files"]),
        float(threshold["affected_files_max"]),
    )
    for query_name, limit in threshold["query_p95_ms_max"].items():
        maximum(
            f"queries_ms.{query_name}.p95",
            float(result["queries_ms"][query_name]["p95"]),
            float(limit),
        )
    return violations


def render_summary(result: dict[str, Any]) -> str:
    status = "PASS" if not result["violations"] else "FAIL"
    storage = result["storage"]
    one_file = result["one_file"]
    lines = [
        f"# Code-intelligence benchmark: {status}",
        "",
        f"- Profile: `{result['profile']}` "
        f"({result['fixture']['file_count']} files / {result['fixture']['package_count']} packages)",
        f"- Cold index: {result['cold']['wall_seconds']:.3f}s; "
        f"peak RSS: {result['memory']['peak_rss_bytes'] / (1024 ** 2):.1f} MiB",
        f"- One-file sync: {one_file['wall_seconds']:.3f}s; "
        f"{one_file['affected_files']} affected; "
        f"{one_file['payload_rows_written']} payload rows written",
        f"- Storage: {storage['database']['allocated_bytes']} SQLite bytes / "
        f"{storage['compatibility_export_bytes']} JSON bytes "
        f"({storage['database_to_compatibility_ratio']:.3f}x); "
        f"freelist {storage['database']['freelist_ratio']:.3%}",
        "- Query p95: "
        + ", ".join(
            f"{name}={metrics['p95']:.2f}ms"
            for name, metrics in result["queries_ms"].items()
        ),
    ]
    if result["violations"]:
        lines.extend(["", "## Ratchet failures", ""])
        lines.extend(f"- {violation}" for violation in result["violations"])
    return "\n".join(lines) + "\n"


def write_artifacts(result: dict[str, Any], output: Path) -> tuple[Path, Path]:
    output = output.expanduser().resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(
        json.dumps(result, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    summary = output.with_suffix(".md")
    summary.write_text(render_summary(result), encoding="utf-8")
    return output, summary


def isolated_root(base: Path) -> Path:
    """Create a process-unique benchmark root below an existing directory."""
    root = base / f"codeintel-{os.getpid()}-{time.time_ns()}"
    root.mkdir(parents=True)
    return root
