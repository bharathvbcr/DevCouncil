"""CLI + helper coverage for `dev check` — list-gates, evidence gate, LLM audit
human path, and the --watch plumbing (gate/evidence/snapshot helpers)."""

import json
import subprocess
from types import SimpleNamespace

import devcouncil.cli.commands.check as check_cmd
from devcouncil.cli.main import app
from devcouncil.domain.gap import Gap
from devcouncil.verification.ad_hoc_check import AdHocCheckResult
from devcouncil.verification.next_actions import NextAction
from typer.testing import CliRunner

runner = CliRunner()


def _git_repo(tmp_path):
    subprocess.run(["git", "init"], cwd=tmp_path, capture_output=True)
    (tmp_path / "app.py").write_text("x = 1\n", encoding="utf-8")
    subprocess.run(["git", "add", "-A"], cwd=tmp_path, capture_output=True)
    subprocess.run(
        ["git", "-c", "user.email=t@t.com", "-c", "user.name=t", "commit", "-m", "init"],
        cwd=tmp_path, capture_output=True,
    )


def _blocking_gap():
    return Gap(
        id="G1",
        severity="high",
        gap_type="diff_not_exercised",
        description="Changed lines are not exercised by tests.",
        recommended_fix="Add a test.",
        blocking=True,
        file="app.py",
        line=2,
    )


# --- list-gates -------------------------------------------------------------------


def test_check_list_gates_json(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    _git_repo(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    (tmp_path / "app.py").write_text("x = 2\n", encoding="utf-8")

    result = runner.invoke(app, ["check", "--list-gates", "--json"])
    assert result.exit_code == 0
    data = json.loads(result.stdout)
    assert "changed_files" in data
    assert "gates" in data


def test_check_list_gates_human_no_changes(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    _git_repo(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    result = runner.invoke(app, ["check", "--list-gates"])
    assert result.exit_code == 0
    assert "No working-tree changes" in result.output


# --- evidence gate (--verify) -----------------------------------------------------


def test_check_verify_json_blocking(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    result_obj = AdHocCheckResult(
        requirement="Do the thing",
        changed_files=["app.py"],
        gaps=[_blocking_gap()],
        next_actions=[],
        passed=False,
    )
    monkeypatch.setattr(check_cmd, "run_working_tree_check", lambda *a, **k: result_obj)

    result = runner.invoke(app, ["check", "--verify", "--json"])
    assert result.exit_code == 1
    data = json.loads(result.stdout)
    assert data["verified"] is False
    assert data["blocking_gap_count"] == 1


def test_check_verify_human_render_with_gaps(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    action = NextAction(
        gap_id="G1", gap_type="diff_not_exercised", category="add_test",
        severity="high", blocking=True, action="Write a test for app.py", file="app.py", line=2,
    )
    result_obj = AdHocCheckResult(
        requirement="Do the thing",
        changed_files=["app.py"],
        gaps=[_blocking_gap()],
        next_actions=[action],
        passed=False,
    )
    monkeypatch.setattr(check_cmd, "run_working_tree_check", lambda *a, **k: result_obj)

    result = runner.invoke(app, ["check", "--verify"])
    assert result.exit_code == 1
    assert "Findings" in result.output
    assert "Next actions" in result.output
    assert "Not verified" in result.output


def test_check_verify_passing(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    result_obj = AdHocCheckResult(requirement="ok", changed_files=["app.py"], gaps=[], passed=True)
    monkeypatch.setattr(check_cmd, "run_working_tree_check", lambda *a, **k: result_obj)

    result = runner.invoke(app, ["check", "--verify"])
    assert result.exit_code == 0
    assert "Verified" in result.output


def test_check_watch_rejects_json(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    result = runner.invoke(app, ["check", "--watch", "--json"])
    assert result.exit_code != 0
    assert "cannot be combined with --json" in result.output


# --- LLM audit human path (no provider key configured) ----------------------------


def test_check_llm_audit_human_reports_secrets_and_note(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    monkeypatch.delenv("OPENROUTER_API_KEY", raising=False)
    _git_repo(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    (tmp_path / "app.py").write_text(
        'x = 1\nAPI_KEY = "sk-1234567890abcdefghijklmnopqrstuv"\n', encoding="utf-8"
    )

    result = runner.invoke(app, ["check"])
    assert result.exit_code == 0
    assert "Changed files" in result.output
    # Provider key missing → LLM review degrades to a note rather than erroring.
    assert "Possible secrets" in result.output or "secrets" in result.output.lower()


# --- helper: _render_gate ---------------------------------------------------------


def test_render_gate_no_changes(capsys):
    check_cmd._render_gate(AdHocCheckResult(requirement="", reason="no_changes"))
    out = capsys.readouterr().out
    assert "No changes to verify" in out


def test_render_gate_clean_verified(capsys):
    check_cmd._render_gate(
        AdHocCheckResult(requirement="do it", changed_files=["a.py"], gaps=[], passed=True)
    )
    out = capsys.readouterr().out
    assert "backed by passing evidence" in out


# --- helper: watch plumbing -------------------------------------------------------


def test_watch_snapshot_lists_files(tmp_path):
    (tmp_path / "a.py").write_text("x\n", encoding="utf-8")
    ignored = tmp_path / "__pycache__"
    ignored.mkdir()
    (ignored / "junk.pyc").write_text("x", encoding="utf-8")

    snapshot = check_cmd._watch_snapshot(tmp_path)
    assert any(p.endswith("a.py") for p in snapshot)
    assert not any("__pycache__" in p for p in snapshot)


def test_watch_evidence_once_prints_verdict(tmp_path, monkeypatch, capsys):
    monkeypatch.setattr(
        check_cmd, "run_working_tree_check",
        lambda *a, **k: AdHocCheckResult(requirement="r", changed_files=["a.py"], gaps=[], passed=True),
    )
    check_cmd._watch_evidence_once(tmp_path, "goal", [], False, 0.0, "12:00:00")
    assert "PASS" in capsys.readouterr().out


def test_watch_evidence_once_handles_error(tmp_path, monkeypatch, capsys):
    def boom(*a, **k):
        raise RuntimeError("git hiccup")

    monkeypatch.setattr(check_cmd, "run_working_tree_check", boom)
    check_cmd._watch_evidence_once(tmp_path, "goal", [], False, 0.0, "12:00:00")
    assert "ERROR" in capsys.readouterr().out


def test_watch_gate_once_no_changes(tmp_path, monkeypatch, capsys):
    monkeypatch.setattr(
        check_cmd, "run_incremental_gates",
        lambda *a, **k: SimpleNamespace(no_changes=True, no_gates=False),
    )
    check_cmd._watch_gate_once(tmp_path, "goal", [], False, 0.0, cache=None)
    assert "no working-tree changes" in capsys.readouterr().out


def test_watch_gate_once_runs_gates(tmp_path, monkeypatch, capsys):
    outcome = SimpleNamespace(passed=False, kind="pytest")
    monkeypatch.setattr(
        check_cmd, "run_incremental_gates",
        lambda *a, **k: SimpleNamespace(
            no_changes=False, no_gates=False, passed=False,
            ran=["pytest"], cached=[], outcomes=[outcome], duration_s=0.5, narrowed=True,
        ),
    )
    check_cmd._watch_gate_once(tmp_path, "goal", [], False, 0.0, cache=None)
    out = capsys.readouterr().out
    assert "FAIL" in out
    assert "failed: pytest" in out
    assert "narrowed" in out


def test_watch_gate_once_falls_back_to_evidence_when_no_gates(tmp_path, monkeypatch, capsys):
    monkeypatch.setattr(
        check_cmd, "run_incremental_gates",
        lambda *a, **k: SimpleNamespace(no_changes=False, no_gates=True),
    )
    monkeypatch.setattr(
        check_cmd, "run_working_tree_check",
        lambda *a, **k: AdHocCheckResult(requirement="r", changed_files=["a.py"], gaps=[], passed=True),
    )
    check_cmd._watch_gate_once(tmp_path, "goal", [], False, 0.0, cache=None)
    assert "evidence gate" in capsys.readouterr().out


# --- _diff with a base ref --------------------------------------------------------


def test_diff_with_base_uses_git_output(tmp_path, monkeypatch):
    import devcouncil.utils.proc as proc
    monkeypatch.setattr(proc, "git_output", lambda *a, **k: "diff --git a b\n")
    assert "diff --git" in check_cmd._diff(tmp_path, "HEAD~1")


# --- list-gates with test commands + populated gates ------------------------------


def test_list_gates_json_with_test_command(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    _git_repo(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    (tmp_path / "app.py").write_text("x = 2\n", encoding="utf-8")

    result = runner.invoke(app, ["check", "--list-gates", "--test", "pytest -q", "--json"])
    assert result.exit_code == 0
    data = json.loads(result.stdout)
    assert "gates" in data


def test_list_gates_human_with_gates(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    _git_repo(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    (tmp_path / "app.py").write_text("x = 2\n", encoding="utf-8")

    gate = SimpleNamespace(name="pytest", kind="python", command=["pytest", "-q"], narrowed=True)
    monkeypatch.setattr(check_cmd, "selected_gate_specs", lambda *a, **k: [gate])
    result = runner.invoke(app, ["check", "--list-gates"])
    assert result.exit_code == 0
    assert "gate(s) for" in result.output
    assert "python/pytest" in result.output


def test_list_gates_human_changed_but_no_gates(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    _git_repo(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    (tmp_path / "app.py").write_text("x = 2\n", encoding="utf-8")

    monkeypatch.setattr(check_cmd, "selected_gate_specs", lambda *a, **k: [])
    result = runner.invoke(app, ["check", "--list-gates"])
    assert result.exit_code == 0
    assert "no matching stack gates" in result.output


# --- watch mode dispatch ----------------------------------------------------------


def test_check_watch_invokes_watch_gate(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    called = {}
    monkeypatch.setattr(check_cmd, "_watch_gate", lambda root, goal, **k: called.setdefault("hit", True))
    result = runner.invoke(app, ["check", "--watch"])
    assert result.exit_code == 0
    assert called["hit"] is True


# --- LLM audit success path (mocked provider + reviewer) --------------------------


def test_check_llm_audit_success_renders_findings_and_blast(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    monkeypatch.setenv("OPENROUTER_API_KEY", "sk-test")
    _git_repo(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    (tmp_path / "app.py").write_text("x = 2\n", encoding="utf-8")

    monkeypatch.setattr(check_cmd, "validate_model_provider", lambda provider: None)
    monkeypatch.setattr(check_cmd, "get_api_key", lambda provider, root: "sk-test")
    monkeypatch.setattr(check_cmd, "create_provider", lambda *a, **k: object())
    monkeypatch.setattr(check_cmd, "ModelRouter", lambda *a, **k: object())

    finding = SimpleNamespace(severity="high", description="risky change")

    class _Reviewer:
        def __init__(self, router):
            pass

        async def review_changes(self, task, reqs, diff):
            return SimpleNamespace(findings=[finding])

    monkeypatch.setattr(check_cmd, "ImplementationReviewer", _Reviewer)
    monkeypatch.setattr(
        check_cmd, "CodeReviewGraphAdapter",
        lambda root: SimpleNamespace(
            get_context=lambda files: SimpleNamespace(
                available=True, impacted_files=["b.py"], related_tests=["test_b.py"],
            )
        ),
    )

    result = runner.invoke(app, ["check", "--goal", "make it work"])
    assert result.exit_code == 0
    assert "Review findings" in result.output
    assert "Blast radius" in result.output


def test_check_llm_audit_provider_error_note(tmp_path, monkeypatch):
    from devcouncil.llm.provider import ProviderRequestError

    monkeypatch.chdir(tmp_path)
    monkeypatch.setenv("OPENROUTER_API_KEY", "sk-test")
    _git_repo(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    (tmp_path / "app.py").write_text("x = 2\n", encoding="utf-8")

    def boom(*a, **k):
        raise ProviderRequestError("provider down")

    monkeypatch.setattr(check_cmd, "validate_model_provider", lambda provider: None)
    monkeypatch.setattr(check_cmd, "get_api_key", lambda provider, root: "sk-test")
    monkeypatch.setattr(check_cmd, "create_provider", boom)
    monkeypatch.setattr(
        check_cmd, "CodeReviewGraphAdapter",
        lambda root: SimpleNamespace(
            get_context=lambda files: SimpleNamespace(available=False, impacted_files=[], related_tests=[])
        ),
    )

    result = runner.invoke(app, ["check"])
    assert result.exit_code == 0
    assert "LLM review unavailable" in result.output
    assert "No secrets or review concerns" in result.output


def test_check_clean_tree_json(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    _git_repo(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    result = runner.invoke(app, ["check", "--json"])
    assert result.exit_code == 0
    assert json.loads(result.stdout)["ok"] is True


# --- _render_gate coverage line + next actions + non-blocking pass ----------------


def test_render_gate_with_coverage_and_next_actions(capsys):
    from devcouncil.domain.evidence import DiffCoverageEvidence
    from devcouncil.verification.next_actions import NextAction

    action = NextAction(
        gap_id="G1", gap_type="diff_not_exercised", category="add_test",
        severity="low", blocking=False, action="add a test", file="app.py", line=3,
    )
    result = AdHocCheckResult(
        requirement="do it",
        changed_files=["app.py"],
        gaps=[_blocking_gap()],
        next_actions=[action],
        passed=True,
        diff_coverage=DiffCoverageEvidence(task_id="CHECK", tool="coverage", measured=True, summary="80%"),
    )
    check_cmd._render_gate(result)
    out = capsys.readouterr().out
    assert "Diff coverage" in out
    assert "Next actions" in out
    assert "non-blocking signals only" in out


# --- watch helpers: evidence no-changes + incremental error -----------------------


def test_watch_evidence_once_no_changes(tmp_path, monkeypatch, capsys):
    monkeypatch.setattr(
        check_cmd, "run_working_tree_check",
        lambda *a, **k: AdHocCheckResult(requirement="r", reason="no_changes", gaps=[], passed=True),
    )
    check_cmd._watch_evidence_once(tmp_path, "goal", [], False, 0.0, "12:00:00")
    assert "no working-tree changes" in capsys.readouterr().out


def test_watch_gate_once_incremental_error(tmp_path, monkeypatch, capsys):
    def boom(*a, **k):
        raise RuntimeError("incremental hiccup")

    monkeypatch.setattr(check_cmd, "run_incremental_gates", boom)
    check_cmd._watch_gate_once(tmp_path, "goal", [], False, 0.0, cache=None)
    assert "ERROR" in capsys.readouterr().out


# --- _watch_gate loop (poll/debounce until KeyboardInterrupt) ----------------------


def test_watch_gate_loop_runs_and_stops(tmp_path, monkeypatch, capsys):
    monkeypatch.setattr(check_cmd, "GateResultCache", lambda root: None)
    monkeypatch.setattr(check_cmd, "_watch_gate_once", lambda *a, **k: None)
    snapshots = iter([{"a": 1}, {"a": 2}, {"a": 2}])
    monkeypatch.setattr(check_cmd, "_watch_snapshot", lambda root: next(snapshots, {"a": 2}))

    counter = {"n": 0}

    def fake_sleep(_seconds):
        counter["n"] += 1
        if counter["n"] >= 3:
            raise KeyboardInterrupt

    monkeypatch.setattr(check_cmd.time, "sleep", fake_sleep)
    check_cmd._watch_gate(tmp_path, "goal", test_commands=[], enforce_coverage=False, min_coverage=0.0)
    assert "Watch stopped" in capsys.readouterr().out
