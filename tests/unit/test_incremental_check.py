"""Unit tests for the incremental gate runner (selector + cache orchestration)."""

from devcouncil.verification.gate_cache import GateResultCache
from devcouncil.verification.incremental_check import run_incremental_gates


def _make_repo(tmp_path):
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "a.py").write_text("x = 1\n", encoding="utf-8")
    return {"lint": ["ruff check ."], "test": ["pytest"]}


def test_no_changes_reports_no_changes(tmp_path):
    result = run_incremental_gates(tmp_path, commands={"lint": ["ruff check ."]}, changed_files=[])
    assert result.no_changes is True
    assert result.passed is True


def test_docs_only_change_selects_no_gates(tmp_path):
    (tmp_path / "readme.md").write_text("# hi\n", encoding="utf-8")
    result = run_incremental_gates(
        tmp_path, commands={"lint": ["ruff check ."], "test": ["pytest"]},
        changed_files=["readme.md"],
    )
    assert result.no_gates is True


def test_runs_selected_gates_and_records_results(tmp_path):
    commands = _make_repo(tmp_path)
    calls = []

    def runner(cmd):
        calls.append(cmd)
        return True, "ok"

    result = run_incremental_gates(
        tmp_path, commands=commands, changed_files=["src/a.py"], runner=runner,
    )
    assert result.passed is True
    assert len(result.ran) == 2  # lint + test
    assert result.cached == []
    assert len(calls) == 2


def test_second_run_uses_cache_when_inputs_unchanged(tmp_path):
    commands = _make_repo(tmp_path)
    cache = GateResultCache(tmp_path)
    call_count = {"n": 0}

    def runner(cmd):
        call_count["n"] += 1
        return True, "ok"

    first = run_incremental_gates(
        tmp_path, commands=commands, changed_files=["src/a.py"], runner=runner, cache=cache,
    )
    assert len(first.ran) == 2

    # Nothing changed on disk -> second run is fully cached (runner not invoked again).
    cache2 = GateResultCache(tmp_path)
    second = run_incremental_gates(
        tmp_path, commands=commands, changed_files=["src/a.py"], runner=runner, cache=cache2,
    )
    assert second.passed is True
    assert len(second.cached) == 2
    assert second.ran == []
    assert call_count["n"] == 2  # no additional calls


def test_editing_input_reruns_only_affected_gate(tmp_path):
    commands = _make_repo(tmp_path)

    def runner(cmd):
        return True, "ok"

    run_incremental_gates(
        tmp_path, commands=commands, changed_files=["src/a.py"], runner=runner,
    )
    # Edit the file -> its content hash changes -> gates re-run rather than cache-hit.
    (tmp_path / "src" / "a.py").write_text("x = 2\n", encoding="utf-8")
    again = run_incremental_gates(
        tmp_path, commands=commands, changed_files=["src/a.py"], runner=runner,
    )
    assert len(again.ran) == 2
    assert again.cached == []


def test_failing_gate_makes_result_fail_and_is_not_cached_green(tmp_path):
    commands = {"lint": ["ruff check ."]}
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "a.py").write_text("x=1\n", encoding="utf-8")

    def runner(cmd):
        return False, "lint error"

    result = run_incremental_gates(
        tmp_path, commands=commands, changed_files=["src/a.py"], runner=runner,
    )
    assert result.passed is False
    # A failure is re-run next time (never served as cached green).
    again = run_incremental_gates(
        tmp_path, commands=commands, changed_files=["src/a.py"], runner=runner,
    )
    assert again.cached == []


def test_use_cache_false_always_runs(tmp_path):
    commands = _make_repo(tmp_path)
    n = {"c": 0}

    def runner(cmd):
        n["c"] += 1
        return True, "ok"

    run_incremental_gates(tmp_path, commands=commands, changed_files=["src/a.py"], runner=runner)
    run_incremental_gates(
        tmp_path, commands=commands, changed_files=["src/a.py"], runner=runner, use_cache=False,
    )
    # Both runs execute both gates: 2 + 2.
    assert n["c"] == 4
