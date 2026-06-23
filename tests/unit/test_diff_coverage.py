from pathlib import Path

from devcouncil.verification.diff_coverage import (
    CoverageData,
    coverage_run_argv,
    coverage_run_script_argv,
    inline_python_code,
    inline_script_content,
    intersect,
    measurable_python_changes,
    parse_changed_lines,
    parse_coverage_json,
)


def test_parse_changed_lines_maps_added_lines_to_new_file_numbers():
    diff = (
        "diff --git a/calc.py b/calc.py\n"
        "--- a/calc.py\n"
        "+++ b/calc.py\n"
        "@@ -1,3 +1,5 @@\n"
        " def add(a, b):\n"
        "     return a + b\n"
        "+\n"
        "+def sub(a, b):\n"
        "+    return a - b\n"
        " # trailing context\n"
    )

    changed = parse_changed_lines(diff)

    assert set(changed) == {"calc.py"}
    # Added lines are 3 (blank), 4 (def sub), 5 (return) in the new file.
    assert set(changed["calc.py"].keys()) == {3, 4, 5}
    assert changed["calc.py"][4] == "def sub(a, b):"


def test_parse_changed_lines_handles_new_file_and_multiple_files():
    diff = (
        "diff --git a/new.py b/new.py\n"
        "new file mode 100644\n"
        "--- /dev/null\n"
        "+++ b/new.py\n"
        "@@ -0,0 +1,2 @@\n"
        "+print('one')\n"
        "+print('two')\n"
        "diff --git a/other.py b/other.py\n"
        "--- a/other.py\n"
        "+++ b/other.py\n"
        "@@ -10,2 +10,3 @@\n"
        " keep = 1\n"
        "+added = 2\n"
        " keep2 = 3\n"
    )

    changed = parse_changed_lines(diff)

    assert set(changed["new.py"].keys()) == {1, 2}
    assert set(changed["other.py"].keys()) == {11}


def test_parse_changed_lines_ignores_deletions_for_new_line_counter():
    diff = (
        "+++ b/m.py\n"
        "@@ -1,3 +1,2 @@\n"
        " a = 1\n"
        "-b = 2\n"
        "+b = 22\n"
    )

    changed = parse_changed_lines(diff)

    # Deletion does not advance the new-file counter; the replacement is line 2.
    assert set(changed["m.py"].keys()) == {2}


def test_intersect_flags_touched_but_not_exercised(tmp_path: Path):
    changed = {"calc.py": {4: "def sub(a, b):", 5: "    return a - b"}}
    coverage = CoverageData(
        executed={"calc.py": {1, 2}},
        executable={"calc.py": {1, 2, 4, 5}},
    )

    result = intersect(changed, coverage)

    assert result.measured
    assert result.changed_executable_lines == 2
    assert result.covered_changed_lines == 0
    assert result.ratio == 0.0
    assert result.uncovered_by_file["calc.py"] == [4, 5]


def test_intersect_counts_partial_coverage(tmp_path: Path):
    changed = {"calc.py": {4: "def sub(a, b):", 5: "    return a - b"}}
    coverage = CoverageData(
        executed={"calc.py": {4}},
        executable={"calc.py": {4, 5}},
    )

    result = intersect(changed, coverage)

    assert result.covered_changed_lines == 1
    assert result.changed_executable_lines == 2
    assert result.ratio == 0.5


def test_intersect_flags_absent_file_as_never_imported():
    changed = {"feature.py": {1: "import os", 2: "", 3: "# comment", 4: "def go():"}}
    coverage = CoverageData(executed={}, executable={})

    result = intersect(changed, coverage)

    assert result.measured
    # Only code-like lines (1, 4) count; blank (2) and comment (3) are excluded.
    assert result.changed_executable_lines == 2
    assert result.covered_changed_lines == 0
    assert result.absent_files == ["feature.py"]


def test_intersect_unmeasured_when_no_executable_changed_lines():
    changed = {"calc.py": {3: "# only a comment", 4: ""}}
    coverage = CoverageData(executed={}, executable={})

    result = intersect(changed, coverage)

    assert not result.measured
    assert "no changed executable lines" in result.reason


def test_measurable_python_changes_excludes_tests_and_non_python():
    changed = {
        "src/app.py": {1: "x = 1"},
        "tests/test_app.py": {1: "assert True"},
        "README.md": {1: "# docs"},
    }

    measurable = measurable_python_changes(changed)

    assert set(measurable) == {"src/app.py"}


def test_parse_coverage_json_relativizes_and_unions_lines(tmp_path: Path):
    data = {
        "files": {
            "calc.py": {"executed_lines": [1, 2], "missing_lines": [4]},
            str((tmp_path / "pkg" / "mod.py")): {"executed_lines": [7], "missing_lines": []},
        }
    }

    coverage = parse_coverage_json(data, tmp_path)

    assert coverage.executed["calc.py"] == {1, 2}
    assert coverage.executable["calc.py"] == {1, 2, 4}
    assert coverage.executed["pkg/mod.py"] == {7}


def test_coverage_run_argv_transforms_known_runners():
    assert coverage_run_argv(
        ["python", "-m", "pytest", "tests/test_x.py", "-q"],
        "python",
        append=False,
        data_file=".cov",
    ) == ["python", "-m", "coverage", "run", "--source=.", "--data-file=.cov", "-m", "pytest", "tests/test_x.py", "-q"]

    assert coverage_run_argv(["pytest", "-q"], "py", append=True, data_file=".cov") == [
        "py", "-m", "coverage", "run", "--source=.", "--data-file=.cov", "-a", "-m", "pytest", "-q",
    ]


def test_coverage_run_argv_returns_none_for_uninstrumentable():
    assert coverage_run_argv(["go", "test", "./..."], "python", append=False, data_file=".cov") is None
    # -c is handled separately (via a temp script), so the single-command transform declines it.
    assert coverage_run_argv(["python", "-c", "assert True"], "python", append=False, data_file=".cov") is None


def test_parse_changed_lines_ignores_unparseable_hunk_header():
    # A combined-merge header (@@@ ... @@@) cannot be numbered; added lines under it
    # must not be mis-attributed to line 0.
    diff = "+++ b/m.py\n@@@ -1,2 -1,2 +1,3 @@@\n+ added\n"
    assert parse_changed_lines(diff) == {}


def test_coverage_run_argv_handles_windows_exe():
    assert coverage_run_argv(
        ["python.exe", "-m", "pytest", "t.py"], "python.exe", append=False, data_file=".cov"
    ) == ["python.exe", "-m", "coverage", "run", "--source=.", "--data-file=.cov", "-m", "pytest", "t.py"]


def test_inline_python_code_detection():
    assert inline_python_code(["python", "-c", "import m; assert m.f()"]) == "import m; assert m.f()"
    assert inline_python_code(["python.exe", "-c", "x = 1"]) == "x = 1"
    assert inline_python_code(["python", "-m", "pytest"]) is None
    assert inline_python_code(["pytest", "-q"]) is None


def test_coverage_run_script_argv_builds_invocation():
    assert coverage_run_script_argv("s.py", "python", append=True, data_file=".cov") == [
        "python", "-m", "coverage", "run", "--source=.", "--data-file=.cov", "-a", "s.py",
    ]


def test_inline_script_content_injects_repo_root(tmp_path):
    content = inline_script_content("import calc", tmp_path)
    assert "sys.path.insert(0," in content
    assert "import calc" in content
