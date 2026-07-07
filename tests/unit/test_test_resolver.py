from devcouncil.domain.task import Task
from devcouncil.verification.test_resolver import TestResolver


def test_auth_maps_to_test_auth(tmp_path):
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "auth.py").write_text("def f(): pass", encoding="utf-8")
    (tmp_path / "tests").mkdir()
    (tmp_path / "tests" / "test_auth.py").write_text("def test_x(): pass", encoding="utf-8")
    task = Task(id="T", title="t", description="d")
    suggestions = TestResolver(tmp_path).suggest_for_task(task, ["src/auth.py"])
    assert any("tests/test_auth.py" in s.command for s in suggestions)
    assert suggestions[0].confidence == "high"


def test_apply_does_not_duplicate(tmp_path):
    task = Task(id="T", title="t", description="d", expected_tests=["pytest tests/unit"])
    suggestions = TestResolver(tmp_path).suggest_for_task(task, ["src/foo.py"])
    existing = set(task.expected_tests)
    for item in suggestions:
        if item.confidence == "high" and item.command not in existing:
            task.expected_tests.append(item.command)
    assert len(task.expected_tests) == 1


def test_map_dependents_suggests_importing_test(tmp_path):
    (tmp_path / "src" / "service").mkdir(parents=True)
    (tmp_path / "src" / "service" / "foo.py").write_text("def f(): pass", encoding="utf-8")
    (tmp_path / "tests" / "unit").mkdir(parents=True)
    (tmp_path / "tests" / "unit" / "test_foo_behaviour.py").write_text(
        "import src.service.foo\n\ndef test_x(): pass\n", encoding="utf-8"
    )
    repo_map = {
        "files": [
            {"path": "src/service/foo.py", "area": "service"},
            {"path": "tests/unit/test_foo_behaviour.py", "area": "tests"},
        ],
        "dependents": {"src/service/foo.py": ["tests/unit/test_foo_behaviour.py"]},
    }
    task = Task(id="T", title="t", description="d")
    suggestions = TestResolver(tmp_path, repo_map).suggest_for_task(task, ["src/service/foo.py"])
    top = suggestions[0]
    assert top.command == "pytest tests/unit/test_foo_behaviour.py"
    assert top.confidence == "high"
    assert "imports the changed file" in top.reason


def test_map_ignores_non_test_importers(tmp_path):
    repo_map = {
        "files": [{"path": "src/service/foo.py", "area": "service"}],
        # a non-test importer must not be proposed as a test command
        "dependents": {"src/service/foo.py": ["src/service/bar.py"]},
    }
    task = Task(id="T", title="t", description="d")
    suggestions = TestResolver(tmp_path, repo_map).suggest_for_task(task, ["src/service/foo.py"])
    assert all("bar.py" not in s.command for s in suggestions)


def test_map_role_files_fallback_by_area(tmp_path):
    (tmp_path / "tests").mkdir()
    (tmp_path / "tests" / "test_core.py").write_text("def test_x(): pass", encoding="utf-8")
    repo_map = {
        "files": [{"path": "src/core/engine.py", "area": "core"}],
        "dependents": {},  # no direct test importer -> fall back to the area's tests
        "subsystems": [
            {"area": "core", "role_files": {"tests": ["tests/test_core.py"]}},
        ],
    }
    task = Task(id="T", title="t", description="d")
    suggestions = TestResolver(tmp_path, repo_map).suggest_for_task(task, ["src/core/engine.py"])
    assert any(
        s.command == "pytest tests/test_core.py" and s.confidence == "medium" for s in suggestions
    )


def test_no_map_is_backward_compatible(tmp_path):
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "auth.py").write_text("def f(): pass", encoding="utf-8")
    (tmp_path / "tests").mkdir()
    (tmp_path / "tests" / "test_auth.py").write_text("def test_x(): pass", encoding="utf-8")
    task = Task(id="T", title="t", description="d")
    # No repo_map passed → identical behavior to before (name-based mapping).
    suggestions = TestResolver(tmp_path).suggest_for_task(task, ["src/auth.py"])
    assert any("tests/test_auth.py" in s.command for s in suggestions)
    assert suggestions[0].confidence == "high"
