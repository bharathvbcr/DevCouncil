"""CLI coverage for `dev cost show` and `dev cost budget`.

The commands read the local ``model_calls.jsonl`` ledger and (for budget) the
project config — both fully offline — so these tests seed a real ledger and
config rather than mocking, exercising the JSON, human, and error output paths.
"""

import json

import yaml
from typer.testing import CliRunner

from devcouncil.cli.main import app

runner = CliRunner()


def _write_ledger(root, entries):
    logs = root / ".devcouncil" / "logs"
    logs.mkdir(parents=True, exist_ok=True)
    ledger = logs / "model_calls.jsonl"
    ledger.write_text(
        "\n".join(json.dumps(e) for e in entries) + "\n", encoding="utf-8"
    )
    return ledger


def _record(task_id="TASK-001", run_id="run-1", prompt=1000, completion=500):
    return {
        "task_id": task_id,
        "run_id": run_id,
        "timestamp": "2026-01-01T00:00:00Z",
        "provider": "openrouter",
        "response": {"model": "openai/gpt-4o"},
        "usage": {"prompt_tokens": prompt, "completion_tokens": completion},
    }


def test_cost_show_json_aggregates_ledger(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    monkeypatch.delenv("DEVCOUNCIL_LOG_DIR", raising=False)
    assert runner.invoke(app, ["init"]).exit_code == 0
    _write_ledger(tmp_path, [_record(), _record(task_id="TASK-002", run_id="run-2")])

    result = runner.invoke(app, ["cost", "show", "--json"])

    assert result.exit_code == 0
    data = json.loads(result.stdout)
    assert data["total_calls"] == 2
    assert data["total_cost"] > 0
    assert "TASK-001" in data["by_task"]
    assert "run-2" in data["by_run"]


def test_cost_show_human_renders_totals_and_tables(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    monkeypatch.delenv("DEVCOUNCIL_LOG_DIR", raising=False)
    assert runner.invoke(app, ["init"]).exit_code == 0
    _write_ledger(tmp_path, [_record()])

    result = runner.invoke(app, ["cost", "show"])

    assert result.exit_code == 0
    assert "Total Cost" in result.output
    assert "Cost by Task" in result.output
    assert "Cost by Run" in result.output


def test_cost_show_empty_ledger_reports_zero(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    monkeypatch.delenv("DEVCOUNCIL_LOG_DIR", raising=False)
    assert runner.invoke(app, ["init"]).exit_code == 0

    result = runner.invoke(app, ["cost", "show", "--json"])

    assert result.exit_code == 0
    data = json.loads(result.stdout)
    assert data["total_calls"] == 0
    assert data["total_cost"] == 0


def test_cost_budget_set_writes_config(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    monkeypatch.delenv("DEVCOUNCIL_LOG_DIR", raising=False)
    assert runner.invoke(app, ["init"]).exit_code == 0

    result = runner.invoke(app, ["cost", "budget", "--set", "5.00"])

    assert result.exit_code == 0
    assert "cost_budget_usd = 5.00" in result.output
    raw = yaml.safe_load((tmp_path / ".devcouncil" / "config.yaml").read_text(encoding="utf-8"))
    assert raw["telemetry"]["cost_budget_usd"] == 5.0


def test_cost_budget_clear_removes_value(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    monkeypatch.delenv("DEVCOUNCIL_LOG_DIR", raising=False)
    assert runner.invoke(app, ["init"]).exit_code == 0
    assert runner.invoke(app, ["cost", "budget", "--set", "5.00"]).exit_code == 0

    result = runner.invoke(app, ["cost", "budget", "--clear"])

    assert result.exit_code == 0
    assert "Cleared" in result.output
    raw = yaml.safe_load((tmp_path / ".devcouncil" / "config.yaml").read_text(encoding="utf-8"))
    assert "cost_budget_usd" not in (raw.get("telemetry") or {})


def test_cost_budget_json_reports_remaining(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    monkeypatch.delenv("DEVCOUNCIL_LOG_DIR", raising=False)
    assert runner.invoke(app, ["init"]).exit_code == 0
    assert runner.invoke(app, ["cost", "budget", "--set", "10.00"]).exit_code == 0
    _write_ledger(tmp_path, [_record()])

    result = runner.invoke(app, ["cost", "budget", "--json"])

    assert result.exit_code == 0
    data = json.loads(result.stdout)
    assert data["budget_usd"] == 10.0
    assert data["spend_usd"] > 0
    assert data["remaining_usd"] < 10.0
    assert data["over_budget"] is False


def test_cost_budget_human_no_budget_configured(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    monkeypatch.delenv("DEVCOUNCIL_LOG_DIR", raising=False)
    assert runner.invoke(app, ["init"]).exit_code == 0

    result = runner.invoke(app, ["cost", "budget"])

    assert result.exit_code == 0
    assert "not configured" in result.output
    assert "Spend to date" in result.output


def test_cost_budget_over_budget_is_flagged(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    monkeypatch.delenv("DEVCOUNCIL_LOG_DIR", raising=False)
    assert runner.invoke(app, ["init"]).exit_code == 0
    assert runner.invoke(app, ["cost", "budget", "--set", "0.001"]).exit_code == 0
    _write_ledger(tmp_path, [_record(prompt=100000, completion=100000)])

    human = runner.invoke(app, ["cost", "budget"])
    assert human.exit_code == 0
    assert "over budget" in human.output

    js = runner.invoke(app, ["cost", "budget", "--json"])
    assert json.loads(js.stdout)["over_budget"] is True


def test_cost_budget_rejects_set_and_clear_together(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    result = runner.invoke(app, ["cost", "budget", "--set", "5.00", "--clear"])

    assert result.exit_code == 2
    assert "not both" in result.output


def test_cost_budget_rejects_non_positive_set(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    result = runner.invoke(app, ["cost", "budget", "--set", "-1"])

    assert result.exit_code == 2
    assert "positive USD amount" in result.output


def test_cost_budget_set_without_config_errors(tmp_path, monkeypatch):
    project = tmp_path / "empty"
    project.mkdir()
    monkeypatch.chdir(tmp_path)

    result = runner.invoke(app, ["cost", "budget", "--set", "5.00", "--project-root", str(project)])

    assert result.exit_code == 1
    assert "Config not found" in result.output


# --- `--json` contract: exactly one JSON object on stdout, diagnostics on stderr ---
#
# These assert on `result.stdout`, never `result.output`. Under Click 8.4 `.output` is
# the stdout+stderr streams merged, so it parses identically whether or not a banner
# leaked onto stdout — it cannot detect this bug at all, which is why the pre-existing
# tests above passed while `dev cost budget --json --set 5.00` emitted unparseable
# stdout. `tests/e2e/test_json_stdout_contract.py` re-proves the same contract through a
# real subprocess, where the streams are genuinely separate rather than emulated.


def _init(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    monkeypatch.delenv("DEVCOUNCIL_LOG_DIR", raising=False)
    assert runner.invoke(app, ["init"]).exit_code == 0


def test_cost_budget_json_set_keeps_confirmation_off_stdout(tmp_path, monkeypatch):
    _init(tmp_path, monkeypatch)

    result = runner.invoke(app, ["cost", "budget", "--json", "--set", "5.00"])

    assert result.exit_code == 0
    data = json.loads(result.stdout)
    assert data["budget_usd"] == 5.0
    assert "cost_budget_usd = 5.00" in result.stderr
    assert "cost_budget_usd = 5.00" not in result.stdout


def test_cost_budget_json_clear_keeps_confirmation_off_stdout(tmp_path, monkeypatch):
    _init(tmp_path, monkeypatch)
    assert runner.invoke(app, ["cost", "budget", "--set", "5.00"]).exit_code == 0

    result = runner.invoke(app, ["cost", "budget", "--json", "--clear"])

    assert result.exit_code == 0
    assert json.loads(result.stdout)["budget_usd"] is None
    assert "Cleared" in result.stderr
    assert "Cleared" not in result.stdout


def test_cost_budget_human_set_confirmation_is_a_diagnostic(tmp_path, monkeypatch):
    """The confirmation is routed unconditionally, so human mode reports it on stderr too.

    Both streams land on the same terminal, so this is invisible to an interactive user —
    and it is what makes the contract structural rather than dependent on a flag check.
    """
    _init(tmp_path, monkeypatch)

    result = runner.invoke(app, ["cost", "budget", "--set", "5.00"])

    assert result.exit_code == 0
    assert "cost_budget_usd = 5.00" in result.stderr


def test_cost_budget_json_set_and_clear_together_still_emits_one_object(tmp_path, monkeypatch):
    """An error path is a `--json` path: zero objects on stdout breaks the contract too."""
    _init(tmp_path, monkeypatch)

    result = runner.invoke(app, ["cost", "budget", "--json", "--set", "5.00", "--clear"])

    assert result.exit_code == 2
    data = json.loads(result.stdout)
    assert data["ok"] is False
    assert "not both" in data["error"]
    assert "not both" in result.stderr


def test_cost_budget_json_non_positive_set_still_emits_one_object(tmp_path, monkeypatch):
    _init(tmp_path, monkeypatch)

    result = runner.invoke(app, ["cost", "budget", "--json", "--set", "-1"])

    assert result.exit_code == 2
    data = json.loads(result.stdout)
    assert data["ok"] is False
    assert "positive USD amount" in data["error"]


def test_cost_budget_json_missing_config_still_emits_one_object(tmp_path, monkeypatch):
    project = tmp_path / "empty"
    project.mkdir()
    monkeypatch.chdir(tmp_path)

    result = runner.invoke(
        app, ["cost", "budget", "--json", "--set", "5.00", "--project-root", str(project)]
    )

    assert result.exit_code == 1
    data = json.loads(result.stdout)
    assert data["ok"] is False
    assert "Config not found" in data["error"]
