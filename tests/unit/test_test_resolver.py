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
