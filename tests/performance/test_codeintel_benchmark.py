from __future__ import annotations

import json
from pathlib import Path

from .benchmark_harness import (
    FixtureSpec,
    fixture_paths,
    ratchet_violations,
    render_summary,
    run_benchmark,
    write_artifacts,
)


def test_profiles_preserve_exact_fixture_size_and_monorepo_shape() -> None:
    for profile, expected_files, expected_packages in (
        ("fast", 256, 8),
        ("heavy", 10_000, 40),
    ):
        spec = FixtureSpec.load(profile)
        paths = fixture_paths(spec)

        assert spec.file_count == expected_files
        assert spec.package_count == expected_packages
        assert len(paths) == expected_files
        assert len(set(paths)) == expected_files
        assert len({path.split("/", 2)[1] for path in paths}) == expected_packages
        assert all(path.startswith("packages/pkg_") for path in paths)


def test_fast_profile_satisfies_codeintel_performance_ratchets(tmp_path: Path) -> None:
    result = run_benchmark(tmp_path / "fixture", profile="fast")

    assert ratchet_violations(result) == []
    assert result["violations"] == []
    assert result["schema_version"] == 2
    assert result["fixture"]["file_count"] == 256
    assert result["one_file"]["affected_files"] == 1
    assert result["one_file"]["payload_rows_written"] <= 8
    assert result["one_file"]["write_stats"]["node_memberships"] == result["cold"]["node_count"]


def test_benchmark_artifacts_are_machine_and_human_readable(tmp_path: Path) -> None:
    result = {
        "profile": "fast",
        "fixture": {"file_count": 256, "package_count": 8},
        "cold": {"wall_seconds": 1.25},
        "one_file": {
            "wall_seconds": 0.05,
            "affected_files": 1,
            "payload_rows_written": 2,
        },
        "memory": {"peak_rss_bytes": 64 * 1024 * 1024},
        "storage": {
            "database": {
                "allocated_bytes": 1024,
                "freelist_ratio": 0.0,
            },
            "compatibility_export_bytes": 2048,
            "database_to_compatibility_ratio": 0.5,
        },
        "queries_ms": {
            "search": {"p95": 1.0},
            "explore": {"p95": 2.0},
            "dead": {"p95": 3.0},
        },
        "violations": [],
    }

    json_path, summary_path = write_artifacts(result, tmp_path / "result.json")

    assert json.loads(json_path.read_text(encoding="utf-8"))["profile"] == "fast"
    summary = summary_path.read_text(encoding="utf-8")
    assert summary == render_summary(result)
    assert "Code-intelligence benchmark: PASS" in summary
    assert "Query p95: search=1.00ms, explore=2.00ms, dead=3.00ms" in summary
