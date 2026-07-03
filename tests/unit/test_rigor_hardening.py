"""Tests for rigor hardening: allow-stub policy, assert-free tests, declarations."""

from pathlib import Path

from devcouncil.domain.task import Task
from devcouncil.verification.stub_detector import (
    detect_stub_declarations,
    detect_stubs,
    task_allows_scaffolding,
)


def _diff(path: str, added: list[str], start: int = 1) -> str:
    body = "\n".join(f"+{line}" for line in added)
    return (
        f"diff --git a/{path} b/{path}\n"
        f"--- a/{path}\n"
        f"+++ b/{path}\n"
        f"@@ -0,0 +{start},{len(added)} @@\n"
        f"{body}\n"
    )


class TestAllowStubPolicy:
    def test_allow_stub_suppresses_only_with_scaffolding_task(self, tmp_path: Path):
        diff = _diff("src/a.py", ["# TODO: scaffold  # devcouncil: allow-stub"])
        assert detect_stubs(tmp_path, diff, honor_allow_stub=False) != []
        assert detect_stubs(tmp_path, diff, honor_allow_stub=True) == []

    def test_task_allows_scaffolding_from_description(self):
        task = Task(id="T", title="X", description="This task is scaffolding only.")
        assert task_allows_scaffolding(task)
        assert not task_allows_scaffolding(
            Task(id="T2", title="X", description="Implement the feature.")
        )

    def test_stub_declarations_always_reported(self, tmp_path: Path):
        diff = _diff("src/a.py", ["pass  # devcouncil: allow-stub"])
        decls = detect_stub_declarations(diff)
        assert len(decls) == 1
        assert "allow-stub" in decls[0].reason


class TestAssertFreeTests:
    def test_python_test_without_assertions_flagged(self, tmp_path: Path):
        tests = tmp_path / "tests"
        tests.mkdir()
        (tests / "test_a.py").write_text(
            "def test_calls_only():\n    import mod\n    mod.run()\n",
            encoding="utf-8",
        )
        diff = (
            "--- a/tests/test_a.py\n"
            "+++ b/tests/test_a.py\n"
            "@@ -0,0 +1,3 @@\n"
            "+def test_calls_only():\n"
            "+    import mod\n"
            "+    mod.run()\n"
        )
        findings = detect_stubs(tmp_path, diff)
        assert any("no assertions" in f.reason for f in findings)

    def test_python_test_with_assert_not_flagged(self, tmp_path: Path):
        tests = tmp_path / "tests"
        tests.mkdir()
        (tests / "test_a.py").write_text(
            "def test_ok():\n    assert 1 == 1\n",
            encoding="utf-8",
        )
        diff = (
            "--- a/tests/test_a.py\n"
            "+++ b/tests/test_a.py\n"
            "@@ -0,0 +1,2 @@\n"
            "+def test_ok():\n"
            "+    assert 1 == 1\n"
        )
        findings = [f for f in detect_stubs(tmp_path, diff) if "no assertions" in f.reason]
        assert findings == []
