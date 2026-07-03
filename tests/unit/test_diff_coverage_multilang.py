"""Tests for multi-language diff coverage helpers."""

from pathlib import Path

from devcouncil.verification import diff_coverage as dc


def test_measurable_js_and_go_filters():
    changed = {
        "src/app.ts": {1: "x"},
        "src/main.go": {1: "y"},
        "tests/test_app.ts": {1: "z"},
        "src/mod.py": {1: "p"},
    }
    assert "src/app.ts" in dc.measurable_js_changes(changed)
    assert "tests/test_app.ts" not in dc.measurable_js_changes(changed)
    assert "src/main.go" in dc.measurable_go_changes(changed)


def test_parse_istanbul_json_extracts_lines(tmp_path: Path):
    data = {
        str(tmp_path / "src" / "app.js"): {
            "path": str(tmp_path / "src" / "app.js"),
            "statementMap": {
                "0": {"start": {"line": 10, "column": 0}, "end": {"line": 10, "column": 1}},
                "1": {"start": {"line": 11, "column": 0}, "end": {"line": 11, "column": 1}},
            },
            "s": {"0": 1, "1": 0},
        }
    }
    (tmp_path / "src").mkdir(parents=True)
    cov = dc.parse_istanbul_json(data, tmp_path)
    rel = "src/app.js"
    assert 10 in cov.executed[rel]
    assert 11 in cov.executable[rel]
    assert 11 not in cov.executed[rel]


def test_parse_go_coverprofile():
    text = "mode: set\npkg/foo.go:10.2,12.3 2 1 0\npkg/foo.go:14.1,14.5 1 0 0\n"
    cov = dc.parse_go_coverprofile(text, Path("/unused"))
    assert 10 in cov.executed.get("pkg/foo.go", set())
    assert 14 in cov.executable.get("pkg/foo.go", set())
    assert 14 not in cov.executed.get("pkg/foo.go", set())


def test_merge_diff_coverage_results():
    a = dc.DiffCoverageResult(
        measured=True, tool="coverage.py",
        changed_executable_lines=4, covered_changed_lines=3,
    )
    b = dc.DiffCoverageResult(
        measured=True, tool="c8",
        changed_executable_lines=2, covered_changed_lines=1,
    )
    merged = dc.merge_diff_coverage_results([a, b])
    assert merged.measured
    assert merged.changed_executable_lines == 6
    assert merged.covered_changed_lines == 4
    assert "coverage.py" in merged.tool and "c8" in merged.tool


def test_c8_and_go_argv_helpers():
    assert dc.c8_run_argv(["npm", "test"], reports_dir="/tmp/r") is not None
    assert "c8" in dc.c8_run_argv(["npm", "test"], reports_dir="/tmp/r")[1]
    go = dc.go_cover_run_argv(["go", "test", "./..."], "/tmp/c.out")
    assert go is not None
    assert "-coverprofile=/tmp/c.out" in go
    assert dc.go_cover_run_argv(["python", "-m", "pytest"], "/tmp/c.out") is None
