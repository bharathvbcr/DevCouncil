import json

from typer.testing import CliRunner

from devcouncil.cli.main import app


runner = CliRunner()


def test_watch_sessions_empty_state_json(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    import devcouncil.cli.commands.watch as watch_cmd

    monkeypatch.setattr(watch_cmd, "discover_sessions", lambda root, client="claude": [])

    result = runner.invoke(app, ["watch", "sessions", "--json"])

    assert result.exit_code == 0
    payload = json.loads(result.stdout)
    assert payload["sessions"] == []


def test_watch_cards_empty_state_json(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    import devcouncil.cli.commands.watch as watch_cmd

    monkeypatch.setattr(watch_cmd, "load_cards", lambda root: [])

    result = runner.invoke(app, ["watch", "cards", "--json"])

    assert result.exit_code == 0
    payload = json.loads(result.stdout)
    assert payload["cards"] == []
    assert payload["total"] == 0


def test_watch_status_empty_review_state_json(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    import devcouncil.cli.commands.watch as watch_cmd

    monkeypatch.setattr(
        watch_cmd,
        "live_review_summary",
        lambda root, task_id=None: {
            "active_task_id": None,
            "scope_task_id": None,
            "pending_signals": 0,
            "pending_signal_items": [],
            "blocking_cards": [],
            "cards": {"total": 0, "open": 0, "critical_open": 0},
        },
    )

    result = runner.invoke(app, ["watch", "status", "--json"])

    assert result.exit_code == 0
    payload = json.loads(result.stdout)
    assert payload["blocking_cards"] == []
    assert payload["cards"]["total"] == 0
