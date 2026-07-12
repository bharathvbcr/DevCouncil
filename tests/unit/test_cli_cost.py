import json
from pathlib import Path
from typer.testing import CliRunner

from devcouncil.cli.main import app

runner = CliRunner()


def _setup_cost_env(tmp_path: Path, monkeypatch) -> Path:
    monkeypatch.chdir(tmp_path)
    runner.invoke(app, ["init"])
    
    # Force log directory to be inside our tmp_path to avoid global test isolation interference
    log_dir = tmp_path / ".devcouncil" / "logs"
    log_dir.mkdir(parents=True, exist_ok=True)
    monkeypatch.setenv("DEVCOUNCIL_LOG_DIR", str(log_dir))
    return tmp_path


def test_cli_cost_show_empty(tmp_path, monkeypatch):
    _setup_cost_env(tmp_path, monkeypatch)
    
    res = runner.invoke(app, ["cost", "show"])
    assert res.exit_code == 0
    assert "Total Cost: $0.0000" in res.output


def test_cli_cost_show_with_data(tmp_path, monkeypatch):
    root = _setup_cost_env(tmp_path, monkeypatch)
    
    # Write mock calls ledger to our venv logs directory
    ledger = root / ".devcouncil" / "logs" / "model_calls.jsonl"
    records = [
        {"model": "gpt-4", "usage": {"prompt_tokens": 1000, "completion_tokens": 500}, "task_id": "TASK-1", "run_id": "run-1"},
        {"model": "gpt-4", "usage": {"prompt_tokens": 2000, "completion_tokens": 1000}, "task_id": "TASK-1", "run_id": "run-1"},
        {"model": "gpt-4", "usage": {"prompt_tokens": 500, "completion_tokens": 200}},  # unattributed
    ]
    with open(ledger, "w", encoding="utf-8") as f:
        for r in records:
            f.write(json.dumps(r) + "\n")
            
    res = runner.invoke(app, ["cost", "show"])
    assert res.exit_code == 0
    assert "Total Cost:" in res.output
    assert "TASK-1" in res.output
    
    # JSON output
    res_json = runner.invoke(app, ["cost", "show", "--json"])
    assert res_json.exit_code == 0
    data = json.loads(res_json.output)
    assert data["total_calls"] == 3
    assert data["by_task"]["TASK-1"]["calls"] == 2
    assert data["by_task"]["(unattributed)"]["calls"] == 1


def test_cli_cost_budget(tmp_path, monkeypatch):
    root = _setup_cost_env(tmp_path, monkeypatch)
    
    # Set budget
    res = runner.invoke(app, ["cost", "budget", "--set", "10.00"])
    assert res.exit_code == 0
    assert "Set telemetry.cost_budget_usd = 10.00" in res.output
    
    # Show budget
    res_show = runner.invoke(app, ["cost", "budget"])
    assert res_show.exit_code == 0
    assert "Budget: $10.00" in res_show.output
    
    # Budget JSON
    res_json = runner.invoke(app, ["cost", "budget", "--json"])
    assert res_json.exit_code == 0
    data = json.loads(res_json.output)
    assert data["budget_usd"] == 10.0
    
    # Clear budget
    res_clear = runner.invoke(app, ["cost", "budget", "--clear"])
    assert res_clear.exit_code == 0
    assert "Cleared telemetry.cost_budget_usd" in res_clear.output
    
    # Show budget cleared
    res_show_cleared = runner.invoke(app, ["cost", "budget"])
    assert res_show_cleared.exit_code == 0
    assert "Budget: not configured" in res_show_cleared.output


def test_cli_cost_budget_validation(tmp_path, monkeypatch):
    _setup_cost_env(tmp_path, monkeypatch)
    
    # Both set and clear should fail
    res = runner.invoke(app, ["cost", "budget", "--set", "5.00", "--clear"])
    assert res.exit_code != 0
    
    # Negative budget should fail
    res2 = runner.invoke(app, ["cost", "budget", "--set", "-1.00"])
    assert res2.exit_code != 0
