"""Tests for stub/TODO detection over diff-added lines."""

from pathlib import Path

from devcouncil.verification.stub_detector import (
    added_lines_by_file,
    detect_stubs,
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


class TestAddedLinesByFile:
    def test_parses_added_lines_with_line_numbers(self):
        diff = _diff("src/a.py", ["x = 1", "y = 2"], start=10)
        result = added_lines_by_file(diff)
        assert result == {"src/a.py": [(10, "x = 1"), (11, "y = 2")]}

    def test_skips_deleted_files(self):
        diff = (
            "--- a/src/gone.py\n"
            "+++ /dev/null\n"
            "@@ -1,2 +0,0 @@\n"
            "-x = 1\n"
        )
        assert added_lines_by_file(diff) == {}

    def test_context_lines_advance_counter(self):
        diff = (
            "--- a/src/a.py\n"
            "+++ b/src/a.py\n"
            "@@ -1,3 +1,4 @@\n"
            " def f():\n"
            "+    x = 1  # added at line 2\n"
            "     return x\n"
        )
        assert added_lines_by_file(diff) == {"src/a.py": [(2, "    x = 1  # added at line 2")]}

    def test_never_raises_on_garbage(self):
        assert added_lines_by_file("not a diff\n@@ broken @@\n+++ \n") == {}


class TestDetectStubs:
    def test_todo_marker_flagged(self, tmp_path: Path):
        diff = _diff("src/a.py", ["# TODO: finish this"])
        findings = detect_stubs(tmp_path, diff)
        assert len(findings) == 1
        assert findings[0].file == "src/a.py"
        assert "TODO" in findings[0].reason or "marker" in findings[0].reason

    def test_lowercase_prose_not_flagged(self, tmp_path: Path):
        diff = _diff("src/a.py", ["# this is a neat hack for speed is fine? no: lowercase"])
        # lowercase "hack" must not fire the shouty marker
        findings = [f for f in detect_stubs(tmp_path, diff) if "marker" in f.reason]
        assert findings == []

    def test_not_implemented_raise_flagged(self, tmp_path: Path):
        diff = _diff("src/a.py", ["    raise NotImplementedError"])
        findings = detect_stubs(tmp_path, diff)
        assert any("NotImplementedError" in f.reason for f in findings)

    def test_allow_stub_marker_suppresses_with_honor_flag(self, tmp_path: Path):
        diff = _diff("src/a.py", ["# TODO: scaffold  # devcouncil: allow-stub"])
        assert detect_stubs(tmp_path, diff, honor_allow_stub=True) == []
        assert detect_stubs(tmp_path, diff, honor_allow_stub=False) != []

    def test_markdown_files_ignored(self, tmp_path: Path):
        diff = _diff("docs/notes.md", ["- TODO: write more docs"])
        assert detect_stubs(tmp_path, diff) == []

    def test_rust_and_js_idioms(self, tmp_path: Path):
        diff = _diff("src/lib.rs", ["    todo!()"]) + _diff(
            "src/app.ts", ['  throw new Error("not implemented");']
        )
        findings = detect_stubs(tmp_path, diff)
        reasons = {f.reason for f in findings}
        assert any("todo!" in r for r in reasons)
        assert any("JS/TS" in r for r in reasons)

    def test_skipped_test_in_test_file_flagged(self, tmp_path: Path):
        diff = _diff("tests/test_a.py", ["@pytest.mark.skip", "def test_x():", "    assert True"])
        findings = detect_stubs(tmp_path, diff)
        assert any("skipped" in f.reason for f in findings)

    def test_skip_pattern_outside_test_file_not_flagged(self, tmp_path: Path):
        diff = _diff("src/runner.py", ["@pytest.mark.skip"])
        findings = [f for f in detect_stubs(tmp_path, diff) if "skipped" in f.reason]
        assert findings == []

    def test_stub_marker_flagged_case_insensitive(self, tmp_path: Path):
        diff = _diff("src/a.py", ["# stub implementation for now"])
        findings = detect_stubs(tmp_path, diff)
        assert any("stub" in f.reason.lower() or "marker" in f.reason for f in findings)

    def test_stub_identifier_in_code_not_flagged(self, tmp_path: Path):
        # "stub" as a legitimate CODE identifier (test doubles, class names) must not
        # fire the marker — it produced blocking false positives on hard tasks.
        diff = _diff("src/providers.py", [
            "class StubProvider:",
            "    stub_response = make_stub()",
            "def build_stub(config):",
        ])
        findings = [f for f in detect_stubs(tmp_path, diff) if "marker" in f.reason]
        assert findings == []

    def test_stub_comment_in_test_file_not_flagged(self, tmp_path: Path):
        # Talking about stubs in TESTS is standard practice (stub provider, stubbed
        # network), not agent laziness.
        diff = _diff("tests/test_api.py", ["    # use a stub provider for the network"])
        findings = [f for f in detect_stubs(tmp_path, diff) if "marker" in f.reason]
        assert findings == []

    def test_stub_comment_in_source_still_flagged(self, tmp_path: Path):
        diff = _diff("src/handler.py", ["    return None  // stub until API lands"])
        findings = [f for f in detect_stubs(tmp_path, diff) if "marker" in f.reason]
        assert findings != []

    def test_commented_out_assert_flagged_in_test_file(self, tmp_path: Path):
        diff = _diff("tests/test_a.py", ["    # assert result == 1"])
        findings = detect_stubs(tmp_path, diff)
        assert any("commented-out assert" in f.reason for f in findings)

    def test_empty_exported_function_body_flagged(self, tmp_path: Path):
        diff = _diff("src/app.ts", ["export function noop() {}"])
        findings = detect_stubs(tmp_path, diff)
        assert any("empty exported function" in f.reason for f in findings)

    def test_python_pass_only_body_flagged(self, tmp_path: Path):
        src = tmp_path / "src"
        src.mkdir()
        (src / "mod.py").write_text(
            "def real():\n    return 1\n\n\ndef stubbed():\n    pass\n",
            encoding="utf-8",
        )
        # Diff adds the stubbed function (lines 5-6 in the new file).
        diff = (
            "--- a/src/mod.py\n"
            "+++ b/src/mod.py\n"
            "@@ -2,0 +4,3 @@\n"
            "+\n"
            "+def stubbed():\n"
            "+    pass\n"
        )
        findings = detect_stubs(tmp_path, diff)
        assert any("stubbed" in f.reason for f in findings)

    def test_python_preexisting_stub_not_flagged(self, tmp_path: Path):
        src = tmp_path / "src"
        src.mkdir()
        (src / "mod.py").write_text(
            "def old_stub():\n    pass\n\n\ndef newly_added():\n    return 42\n",
            encoding="utf-8",
        )
        # Diff only adds the REAL function; the pre-existing stub must not fire.
        diff = (
            "--- a/src/mod.py\n"
            "+++ b/src/mod.py\n"
            "@@ -2,0 +4,2 @@\n"
            "+def newly_added():\n"
            "+    return 42\n"
        )
        assert detect_stubs(tmp_path, diff) == []

    def test_deduplicates_findings(self, tmp_path: Path):
        diff = _diff("src/a.py", ["# TODO not implemented"])
        findings = detect_stubs(tmp_path, diff)
        keys = [(f.file, f.line, f.reason) for f in findings]
        assert len(keys) == len(set(keys))

    def test_stub_keyword_flagged(self, tmp_path: Path):
        diff = _diff("src/a.py", ["# stub implementation for now"])
        findings = detect_stubs(tmp_path, diff)
        assert any("marker" in f.reason for f in findings)

    def test_commented_out_assert_in_test_flagged(self, tmp_path: Path):
        diff = _diff("tests/test_a.py", ["    # assert result == 1"])
        findings = detect_stubs(tmp_path, diff)
        assert any("commented-out assert" in f.reason for f in findings)

    def test_empty_exported_function_flagged(self, tmp_path: Path):
        diff = _diff("src/app.ts", ["export function handler() {}"])
        findings = detect_stubs(tmp_path, diff)
        assert any("empty exported function" in f.reason for f in findings)
