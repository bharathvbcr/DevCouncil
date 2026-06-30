from typer.testing import CliRunner

from devcouncil.cli.main import app
from devcouncil.telemetry.logging_setup import LOG_RELATIVE_PATH

runner = CliRunner()


def _write_log(root, lines):
    log = root / LOG_RELATIVE_PATH
    log.parent.mkdir(parents=True, exist_ok=True)
    log.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return log


def test_logs_path_reports_not_created(tmp_path):
    result = runner.invoke(app, ["logs", "path", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    # Rich may wrap the long path line, so normalize whitespace before matching.
    assert "not created yet" in " ".join(result.stdout.split())


def test_logs_tail_limit(tmp_path):
    _write_log(tmp_path, [f"line {i}" for i in range(10)])
    result = runner.invoke(app, ["logs", "tail", "-n", "3", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "line 9" in result.stdout
    assert "line 6" not in result.stdout  # only the last 3 lines


def test_logs_tail_grep(tmp_path):
    _write_log(tmp_path, ["alpha", "beta", "ALPHA again", "gamma"])
    result = runner.invoke(app, ["logs", "tail", "--grep", "alpha", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "alpha" in result.stdout
    assert "ALPHA again" in result.stdout  # case-insensitive
    assert "beta" not in result.stdout


def test_logs_runs_lists_per_run_logs(tmp_path):
    run_dir = tmp_path / ".devcouncil" / "runs" / "run-123"
    run_dir.mkdir(parents=True)
    (run_dir / "run.log").write_text("hello\n", encoding="utf-8")
    result = runner.invoke(app, ["logs", "runs", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "run-123" in result.stdout


def test_logs_tail_run_specific(tmp_path):
    run_dir = tmp_path / ".devcouncil" / "runs" / "run-xyz"
    run_dir.mkdir(parents=True)
    (run_dir / "run.log").write_text("per-run line\n", encoding="utf-8")
    result = runner.invoke(app, ["logs", "tail", "--run", "run-xyz", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "per-run line" in result.stdout
