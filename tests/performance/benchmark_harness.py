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
from devcouncil.indexing.graph.build import graph_path, load_code_graph
from devcouncil.indexing.map_artifacts import refresh_map_artifacts

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

    # The cold build is the kernel's: `build_code_graph` (the Python engine)
    # was retired on 2026-09-02 and the kernel is the only writer of the export.
    started = time.perf_counter()
    refresh_map_artifacts(root, root / ".devcouncil" / "repo_map.json", quiet=True)
    cold_seconds = time.perf_counter() - started
    # Fill the Python query cache from that export so the storage ratio below
    # compares the cache against the artifact it was built from.
    graph = load_code_graph(root)
    assert graph is not None, "the kernel's export must load into the query cache"

    service = get_codeintel_service(root)
    # Storage is measured against the cold build's own artifacts, before the
    # kernel rewrites `code_graph.json`: a SQLite-to-JSON ratio across two
    # different engines' outputs would not be a ratio of anything.
    compatibility_path = graph_path(root)
    database = _database_metrics(service.store.path)
    compatibility_bytes = compatibility_path.stat().st_size
    database_ratio = float(database["allocated_bytes"]) / max(1, compatibility_bytes)

    # One-file refresh, measured against the *kernel* — the only writer of
    # `repo_map.json` / `code_graph.json`. This stage used to time
    # ``sync_affected_paths`` (the retired Python incremental engine) and
    # ratchet its per-edit SQLite payload rows; neither the function nor the
    # store it wrote to is on the edit path any more. The untimed warm-up
    # commits the kernel's own baseline generation so the timed call measures
    # an incremental build, not a cold one.
    map_path = root / ".devcouncil" / "repo_map.json"
    refresh_map_artifacts(root, map_path, quiet=True)

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
    refreshed = refresh_map_artifacts(root, map_path, quiet=True, paths=[changed_path])
    one_file_seconds = time.perf_counter() - started

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
        "schema_version": graph.schema_version,
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
            "engine": refreshed.mode,
            "generation": refreshed.generation,
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
    result["timing_enforced"] = timing_enforced()
    result["violations"] = ratchet_violations(result)
    return result


def timing_enforced() -> bool:
    """Whether timing ratchets (wall clock + query p95) are enforced here.

    Timing budgets are only meaningful on the CI runners they were tuned on.
    Windows runners' file I/O and fsync are highly variable (cold build has
    been observed at 15s and 29s in back-to-back runs), and local developer
    machines are noisier still (background load, thermals, Spotlight), so
    enforcing them there produces false failures on unchanged code. The
    deterministic structural ratchets (db ratio, RSS, fixture shape) still run
    everywhere and catch real regressions. The per-edit SQLite payload-row and
    affected-file ratchets went with ``sync_affected_paths``: they measured the
    retired Python incremental engine, which no longer runs on any edit path.
    Timing actuals are always recorded in the result/summary either way.
    Override with DEVCOUNCIL_BENCH_ENFORCE_TIMING=1 (force on) or =0
    (force off)."""
    override = os.environ.get("DEVCOUNCIL_BENCH_ENFORCE_TIMING")
    if override is not None:
        return override.strip().lower() in {"1", "true", "yes", "on"}
    return bool(os.environ.get("CI")) and sys.platform != "win32"


def ratchet_violations(result: dict[str, Any]) -> list[str]:
    threshold = result["thresholds"]
    violations: list[str] = []

    def maximum(metric: str, actual: float, limit: float) -> None:
        if actual > limit:
            violations.append(f"{metric}: {actual:.4f} > {limit:.4f}")

    enforce_timing = timing_enforced()

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

    if enforce_timing:
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
    if enforce_timing:
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
        f"- One-file kernel refresh: {one_file['wall_seconds']:.3f}s "
        f"({one_file.get('engine', 'unknown')}, generation {one_file.get('generation')})",
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
    if not result.get("timing_enforced", True):
        lines.append(
            "- Timing ratchets NOT enforced on this host (non-CI or Windows); "
            "actuals recorded above. Force with DEVCOUNCIL_BENCH_ENFORCE_TIMING=1."
        )
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
