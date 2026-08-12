"""Unit tests for liveness hardening features."""

from pathlib import Path
from devcouncil.indexing.wiring import (
    DiscoveryReport,
    entry_roots_with_report,
    is_wiring_decorated,
    _add_module_file,
    _expand_roots_via_dynamic_imports,
)
from devcouncil.verification.checks.liveness_ratchet import (
    detect_liveness_regressions,
    LIVENESS_SCHEMA_VERSION,
)
from devcouncil.indexing.wiring import LIVENESS_SCAN_VERSION


def test_discovery_report_structure(tmp_path: Path):
    file_set = {"src/main.py", "pyproject.toml"}
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "main.py").write_text("def main(): pass\nif __name__ == '__main__': main()")
    (tmp_path / "pyproject.toml").write_text("[project.scripts]\napp = 'src.main:main'\n")

    roots, report = entry_roots_with_report(tmp_path, file_set)
    assert isinstance(report, DiscoveryReport)
    assert "config" in report.sources_attempted
    assert "pyproject" in report.sources_attempted
    assert len(roots) > 0


def test_expanded_wiring_decorators():
    assert is_wiring_decorated(["@receiver(post_save)"])
    assert is_wiring_decorated(["@api_view(['GET'])"])
    assert is_wiring_decorated(["@action(detail=True)"])
    assert is_wiring_decorated(["@subscriber('events')"])
    assert is_wiring_decorated(["@dramatiq.actor"])
    assert is_wiring_decorated(["@huey.task()"])


def test_add_module_file_namespace_package():
    file_set = {"plugins/auth/provider.py", "src/main.py"}
    out = set()
    _add_module_file("plugins.auth", file_set, out)
    assert "plugins/auth/provider.py" in out


def test_expand_roots_python_dynamic_import(tmp_path: Path):
    file_set = {"src/app.py", "src/plugins/eval.py"}
    (tmp_path / "src").mkdir(parents=True)
    (tmp_path / "src" / "plugins").mkdir(parents=True)
    (tmp_path / "src" / "app.py").write_text("import importlib\nimportlib.import_module('src.plugins.eval')")
    (tmp_path / "src" / "plugins" / "eval.py").write_text("def run(): pass")

    seeds = {"src/app.py"}
    expanded = _expand_roots_via_dynamic_imports(tmp_path, file_set, seeds)
    assert "src/plugins/eval.py" in expanded


test_task = type("TaskMock", (), {"id": "TEST-123"})()


def test_ratchet_renamed_file_not_stranded():
    baseline = {
        "complete": True,
        "scan_version": LIVENESS_SCAN_VERSION,
        "schema_version": LIVENESS_SCHEMA_VERSION,
        "unwired_candidates": ["old_path/helper.py"],
        "unreachable_files": [],
        "dead_symbol_candidates": [],
        "symbol_index": [],
    }
    current = {
        "complete": True,
        "scan_version": LIVENESS_SCAN_VERSION,
        "schema_version": LIVENESS_SCHEMA_VERSION,
        "unwired_candidates": ["new_path/helper.py"],
        "unreachable_files": [],
        "dead_symbol_candidates": [],
        "symbol_index": [],
    }

    gaps = detect_liveness_regressions(
        baseline,
        current,
        task=test_task,
    )
    file_gaps = [g for g in gaps if g.gap_type == "stranded_code"]
    assert len(file_gaps) == 0


def test_ratchet_schema_version_mismatch_skips_diff():
    baseline = {
        "complete": True,
        "scan_version": LIVENESS_SCAN_VERSION,
        "schema_version": LIVENESS_SCHEMA_VERSION,
        "unwired_candidates": [],
        "unreachable_files": [],
        "dead_symbol_candidates": [],
        "symbol_index": [],
    }
    current = {
        "complete": True,
        "scan_version": LIVENESS_SCAN_VERSION,
        "schema_version": 999,
        "unwired_candidates": ["stranded.py"],
        "unreachable_files": [],
        "dead_symbol_candidates": [],
        "symbol_index": [],
    }

    gaps = detect_liveness_regressions(
        baseline,
        current,
        task=test_task,
    )
    assert len(gaps) == 0
