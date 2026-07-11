from pathlib import Path

from devcouncil.cli.commands.doctor import (
    STATUS_DOC_FLAT_TEST_PREFIXES,
    _subsystem_has_unit_tests,
    check_coverage_floor,
    check_mypy_status,
)


def test_subsystem_has_unit_tests_gating_subdir(tmp_path: Path) -> None:
    unit = tmp_path / "tests" / "unit"
    (unit / "gating").mkdir(parents=True)
    (unit / "gating" / "test_scan.py").write_text("def test_x(): pass\n", encoding="utf-8")
    assert _subsystem_has_unit_tests(unit, "gating")


def test_subsystem_has_unit_tests_flat_prefixes(tmp_path: Path) -> None:
    unit = tmp_path / "tests" / "unit"
    unit.mkdir(parents=True)
    (unit / "test_json_report.py").write_text("def test_x(): pass\n", encoding="utf-8")
    assert _subsystem_has_unit_tests(unit, "reporting")


def test_check_coverage_floor_ok_when_configured(tmp_path: Path) -> None:
    (tmp_path / "pyproject.toml").write_text(
        "[tool.coverage.report]\nfail_under = 18\n",
        encoding="utf-8",
    )
    rows = check_coverage_floor(tmp_path)
    assert rows[0][0] == "Coverage floor"
    assert "OK" in rows[0][1]
    assert "fail_under=18" in rows[0][2]


def test_check_coverage_floor_warns_when_missing(tmp_path: Path) -> None:
    (tmp_path / "pyproject.toml").write_text("[project]\nname = 'x'\n", encoding="utf-8")
    rows = check_coverage_floor(tmp_path)
    assert "WARN" in rows[0][1]
    assert "fail_under" in rows[0][2]


def test_check_mypy_status_skips_without_src(tmp_path: Path) -> None:
    (tmp_path / "pyproject.toml").write_text("[project]\nname='x'\n", encoding="utf-8")
    rows = check_mypy_status(tmp_path)
    assert rows[0][0] == "mypy green"
    assert "WARN" in rows[0][1]


def test_status_doc_flat_prefix_backs_reporting_subsystem(tmp_path: Path) -> None:
    unit = tmp_path / "tests" / "unit"
    unit.mkdir(parents=True)
    (unit / "test_json_report.py").write_text("def test_x(): pass\n", encoding="utf-8")
    assert _subsystem_has_unit_tests(unit, "reporting")


def test_flat_prefix_map_covers_requested_subsystems() -> None:
    for subsystem in ("gating", "execution", "indexing", "reporting", "executors"):
        assert subsystem in STATUS_DOC_FLAT_TEST_PREFIXES or subsystem in {
            area for _, area in __import__(
                "devcouncil.cli.commands.doctor", fromlist=["STATUS_DOC_UNIT_TEST_DIRS"]
            ).STATUS_DOC_UNIT_TEST_DIRS
        }
