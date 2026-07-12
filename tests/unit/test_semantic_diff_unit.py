"""Unit tests for verification.checks.semantic_diff helpers."""

import json
from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest

from devcouncil.domain.gap import Gap
from devcouncil.domain.requirement import AcceptanceCriterion, Requirement
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.verification.checks import semantic_diff as sd


def _gap_id(task_id: str, kind: str) -> str:
    return f"{task_id}-{kind}-1"


def _task(**kwargs) -> Task:
    defaults = {
        "id": "TASK-1",
        "title": "Remove old_api",
        "description": "Refactor old_api usage",
        "planned_files": [PlannedFile(path="src/app.py", reason="x", allowed_change="modify")],
        "acceptance_criterion_ids": ["AC-1"],
    }
    defaults.update(kwargs)
    return Task(**defaults)


def _requirement() -> Requirement:
    return Requirement(
        id="REQ-1",
        title="t",
        description="d",
        priority="high",
        source="user",
        acceptance_criteria=[
            AcceptanceCriterion(id="AC-1", description="keep old_api stable", verification_method="unit_test")
        ],
    )


def test_task_intent_text_includes_linked_acceptance_criteria():
    task = _task()
    text = sd.task_intent_text(task, [_requirement()])
    assert "keep old_api stable" in text
    assert "remove old_api" in text


def test_task_intent_text_without_requirements():
    task = Task(id="T", title="Hello", description="World")
    assert sd.task_intent_text(task, None) == "hello world"


@pytest.mark.parametrize(
    "statement,expected",
    [
        ("import json", "json"),
        ("import os.path as osp", "os"),
        ("from pathlib import Path", "pathlib"),
        ("from .local import x", None),
        ("from  import broken", "import broken"),
        ("not an import", None),
        ("", None),
    ],
)
def test_import_top_level(statement, expected):
    assert sd.import_top_level(statement) == expected


def test_stdlib_modules_returns_frozenset():
    result = sd.stdlib_modules()
    assert isinstance(result, frozenset)


def test_load_project_dependencies_pyproject_and_requirements(tmp_path):
    (tmp_path / "pyproject.toml").write_text(
        '[project]\ndependencies = ["requests>=2.0", "click"]\n'
        '[project.optional-dependencies]\ndev = ["pytest>=7"]\n',
        encoding="utf-8",
    )
    (tmp_path / "requirements.txt").write_text("numpy>=1.0\n# comment\n", encoding="utf-8")
    (tmp_path / "package.json").write_text(
        json.dumps({"dependencies": {"react": "18"}, "devDependencies": {"jest": "29"}}),
        encoding="utf-8",
    )
    deps = sd.load_project_dependencies(tmp_path)
    assert {"requests", "click", "pytest", "numpy", "react", "jest"} <= deps


def test_load_project_dependencies_tolerates_parse_errors(tmp_path):
    (tmp_path / "pyproject.toml").write_text("not valid {{{", encoding="utf-8")
    (tmp_path / "requirements.txt").write_text("bad line\n", encoding="utf-8")
    (tmp_path / "package.json").write_text("{broken", encoding="utf-8")
    deps = sd.load_project_dependencies(tmp_path)
    assert isinstance(deps, set)


def test_is_new_third_party_import_respects_stdlib_and_declared_deps():
    assert sd.is_new_third_party_import("json", project_deps=set()) is False
    assert sd.is_new_third_party_import("requests", project_deps={"requests"}) is False
    assert sd.is_new_third_party_import(None, project_deps=set()) is False


def test_is_new_third_party_import_detects_missing_package(monkeypatch):
    monkeypatch.setattr(
        "importlib.util.find_spec",
        lambda name: None,
    )
    assert sd.is_new_third_party_import("brand_new_pkg_xyz", project_deps=set()) is True


def test_is_new_third_party_import_installed_package_not_new(monkeypatch):
    monkeypatch.setattr(
        "importlib.util.find_spec",
        lambda name: object(),
    )
    assert sd.is_new_third_party_import("installed_pkg", project_deps=set()) is False


def test_detect_semantic_diff_gaps_no_after_snapshot(tmp_path):
    gaps = sd.detect_semantic_diff_gaps(
        project_root=tmp_path,
        task=_task(),
        requirements=[_requirement()],
        next_gap_id=_gap_id,
        project_deps=set(),
    )
    assert gaps == []


def test_detect_semantic_diff_gaps_handles_semantic_index_failure(tmp_path, monkeypatch):
    sem_dir = tmp_path / ".devcouncil" / "semantic" / "TASK-1"
    sem_dir.mkdir(parents=True)
    (sem_dir / "after.json").write_text("{}", encoding="utf-8")

    class _BoomIndex:
        def __init__(self, root):
            pass

        def diff(self, task_id):
            raise RuntimeError("boom")

    monkeypatch.setattr("devcouncil.indexing.semantic_index.SemanticIndex", _BoomIndex)
    gaps = sd.detect_semantic_diff_gaps(
        project_root=tmp_path,
        task=_task(),
        requirements=[_requirement()],
        next_gap_id=_gap_id,
        project_deps=set(),
    )
    assert gaps == []


def _write_after_snapshot(tmp_path: Path) -> None:
    sem_dir = tmp_path / ".devcouncil" / "semantic" / "TASK-1"
    sem_dir.mkdir(parents=True)
    (sem_dir / "after.json").write_text("{}", encoding="utf-8")


def test_detect_semantic_diff_exported_symbol_removed(tmp_path, monkeypatch):
    _write_after_snapshot(tmp_path)
    classifications = [
        {"type": "exported_symbol_removed", "path": "src/app.py", "name": "old_api"},
    ]

    mock_index = MagicMock()
    mock_index.diff.return_value = {"classifications": classifications}
    monkeypatch.setattr(
        "devcouncil.indexing.semantic_index.SemanticIndex",
        lambda root: mock_index,
    )

    gaps = sd.detect_semantic_diff_gaps(
        project_root=tmp_path,
        task=_task(),
        requirements=[_requirement()],
        next_gap_id=_gap_id,
        project_deps=set(),
    )
    assert len(gaps) == 1
    assert gaps[0].gap_type == "architecture_drift"
    assert "old_api" in gaps[0].description


def test_detect_semantic_diff_exported_symbol_intended_not_blocking(tmp_path, monkeypatch):
    _write_after_snapshot(tmp_path)
    classifications = [
        {"type": "exported_symbol_removed", "path": "src/app.py", "name": "old_api"},
        {"type": "exported_symbol_added", "path": "src/app.py", "name": "old_api"},
    ]
    mock_index = MagicMock()
    mock_index.diff.return_value = {"classifications": classifications}
    monkeypatch.setattr(
        "devcouncil.indexing.semantic_index.SemanticIndex",
        lambda root: mock_index,
    )

    gaps = sd.detect_semantic_diff_gaps(
        project_root=tmp_path,
        task=_task(title="Remove old_api", description="Drop old_api symbol"),
        requirements=[_requirement()],
        next_gap_id=_gap_id,
        project_deps=set(),
    )
    assert gaps[0].blocking is False


def test_detect_semantic_diff_public_api_unplanned(tmp_path, monkeypatch):
    _write_after_snapshot(tmp_path)
    classifications = [{"type": "public_api_change", "path": "src/other.py", "name": "fn"}]
    mock_index = MagicMock()
    mock_index.diff.return_value = {"classifications": classifications}
    monkeypatch.setattr(
        "devcouncil.indexing.semantic_index.SemanticIndex",
        lambda root: mock_index,
    )

    gaps = sd.detect_semantic_diff_gaps(
        project_root=tmp_path,
        task=_task(acceptance_criterion_ids=[]),
        requirements=[],
        next_gap_id=_gap_id,
        project_deps=set(),
    )
    assert gaps[0].severity == "high"
    assert gaps[0].blocking is True


def test_detect_semantic_diff_public_api_planned_medium(tmp_path, monkeypatch):
    _write_after_snapshot(tmp_path)
    classifications = [{"type": "public_api_change", "path": "src/app.py", "name": "fn"}]
    mock_index = MagicMock()
    mock_index.diff.return_value = {"classifications": classifications}
    monkeypatch.setattr(
        "devcouncil.indexing.semantic_index.SemanticIndex",
        lambda root: mock_index,
    )

    gaps = sd.detect_semantic_diff_gaps(
        project_root=tmp_path,
        task=_task(),
        requirements=[],
        next_gap_id=_gap_id,
        project_deps=set(),
    )
    assert gaps[0].severity == "medium"
    assert gaps[0].blocking is False


def test_detect_semantic_diff_new_third_party_import(tmp_path, monkeypatch):
    _write_after_snapshot(tmp_path)
    classifications = [
        {
            "type": "import_dependency_change",
            "path": "src/app.py",
            "statement": "import brand_new_pkg_xyz",
        }
    ]
    mock_index = MagicMock()
    mock_index.diff.return_value = {"classifications": classifications}
    monkeypatch.setattr(
        "devcouncil.indexing.semantic_index.SemanticIndex",
        lambda root: mock_index,
    )
    monkeypatch.setattr(sd, "is_new_third_party_import", lambda top, **kw: True)

    gaps = sd.detect_semantic_diff_gaps(
        project_root=tmp_path,
        task=_task(),
        requirements=[],
        next_gap_id=_gap_id,
        project_deps=set(),
    )
    assert gaps[0].gap_type == "dependency_risk"
    assert gaps[0].blocking is True


def test_detect_semantic_diff_import_change_unplanned_medium(tmp_path, monkeypatch):
    _write_after_snapshot(tmp_path)
    classifications = [
        {"type": "import_dependency_change", "path": "src/other.py", "statement": "import os"}
    ]
    mock_index = MagicMock()
    mock_index.diff.return_value = {"classifications": classifications}
    monkeypatch.setattr(
        "devcouncil.indexing.semantic_index.SemanticIndex",
        lambda root: mock_index,
    )
    monkeypatch.setattr(sd, "is_new_third_party_import", lambda top, **kw: False)

    gaps = sd.detect_semantic_diff_gaps(
        project_root=tmp_path,
        task=_task(),
        requirements=[],
        next_gap_id=_gap_id,
        project_deps=set(),
    )
    assert gaps[0].severity == "medium"


def test_detect_semantic_diff_config_schema_unplanned(tmp_path, monkeypatch):
    _write_after_snapshot(tmp_path)
    classifications = [{"type": "config_schema_dependency_change", "path": "config.yaml"}]
    mock_index = MagicMock()
    mock_index.diff.return_value = {"classifications": classifications}
    monkeypatch.setattr(
        "devcouncil.indexing.semantic_index.SemanticIndex",
        lambda root: mock_index,
    )

    gaps = sd.detect_semantic_diff_gaps(
        project_root=tmp_path,
        task=_task(),
        requirements=[],
        next_gap_id=_gap_id,
        project_deps=set(),
    )
    assert gaps[0].gap_type == "dependency_risk"
    assert gaps[0].blocking is True
