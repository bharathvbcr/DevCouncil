from types import SimpleNamespace

from devcouncil.cli.commands import run as runmod


def test_verify_executor_output_if_present_runs_when_tree_changed(tmp_path, monkeypatch):
    task = SimpleNamespace(id="TASK-1")
    calls = {"verify": 0, "live": 0}

    monkeypatch.setattr(runmod, "_current_changed_files", lambda root: ["stats.py"])
    monkeypatch.setattr(runmod, "_verify_after_execution", lambda *a, **k: calls.__setitem__("verify", calls["verify"] + 1) or True)
    monkeypatch.setattr(runmod, "_record_project_phase", lambda *a, **k: None)
    monkeypatch.setattr(runmod, "TaskRepository", lambda session: SimpleNamespace(save=lambda t: None))
    monkeypatch.setattr(runmod, "_record_agent_verification", lambda *a, **k: None)
    monkeypatch.setattr(runmod, "_run_live_review_after_execution", lambda *a, **k: calls.__setitem__("live", calls["live"] + 1))
    monkeypatch.setattr(runmod, "_log_exec_outcome", lambda *a, **k: None)
    monkeypatch.setattr(runmod, "_build_verification_router", lambda root: None)

    ok = runmod._verify_executor_output_if_present(
        session=object(),
        task=task,
        reqs=[],
        root=tmp_path,
        executor_label="CLAUDE",
        cli_client="claude",
        cli_executor=SimpleNamespace(last_run_id="run-1"),
    )
    assert ok is True
    assert calls == {"verify": 1, "live": 1}


def test_verify_executor_output_if_present_skips_clean_tree(tmp_path, monkeypatch):
    monkeypatch.setattr(runmod, "_current_changed_files", lambda root: [])
    ok = runmod._verify_executor_output_if_present(
        session=object(),
        task=SimpleNamespace(id="TASK-1"),
        reqs=[],
        root=tmp_path,
        executor_label="CLAUDE",
    )
    assert ok is False
