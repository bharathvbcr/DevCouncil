import json
import subprocess

from typer.testing import CliRunner

from devcouncil.cli.commands.init import initialize_project
from devcouncil.cli.main import app

runner = CliRunner()


def _setup_project(tmp_path):
    subprocess.check_call(["git", "init"], cwd=tmp_path, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    initialize_project(tmp_path, quiet=True)
    src = tmp_path / "src"
    src.mkdir()
    (src / "app.py").write_text("def foo():\n    return 1\n", encoding="utf-8")
    return src


def test_semantic_help():
    result = runner.invoke(app, ["semantic", "--help"])
    assert result.exit_code == 0
    assert "snapshot" in result.stdout


def test_semantic_snapshot_writes_payload(tmp_path):
    _setup_project(tmp_path)

    result = runner.invoke(
        app,
        ["semantic", "snapshot", "TASK-9", "--stage", "before", "--json", "--project-root", str(tmp_path)],
    )

    assert result.exit_code == 0
    payload = json.loads(result.output)
    assert payload["task_id"] == "TASK-9"
    assert payload["stage"] == "before"
    snapshot_path = tmp_path / ".devcouncil" / "semantic" / "TASK-9" / "before.json"
    assert snapshot_path.exists()
    snapshot = json.loads(snapshot_path.read_text(encoding="utf-8"))
    assert any(entry["path"] == "src/app.py" for entry in snapshot["source_files"])


def test_semantic_snapshot_rejects_invalid_stage(tmp_path):
    _setup_project(tmp_path)

    result = runner.invoke(
        app,
        ["semantic", "snapshot", "TASK-9", "--stage", "during", "--project-root", str(tmp_path)],
    )

    assert result.exit_code == 2


def test_semantic_diff_detects_import_change(tmp_path):
    src = _setup_project(tmp_path)
    before = runner.invoke(
        app,
        ["semantic", "snapshot", "TASK-9", "--stage", "before", "--json", "--project-root", str(tmp_path)],
    )
    assert before.exit_code == 0

    (src / "app.py").write_text("import json\n\n\ndef foo():\n    return json.dumps({})\n", encoding="utf-8")
    after = runner.invoke(
        app,
        ["semantic", "snapshot", "TASK-9", "--stage", "after", "--json", "--project-root", str(tmp_path)],
    )
    assert after.exit_code == 0

    result = runner.invoke(
        app,
        ["semantic", "diff", "TASK-9", "--json", "--project-root", str(tmp_path)],
    )

    assert result.exit_code == 0
    payload = json.loads(result.output)
    types = {item["type"] for item in payload["classifications"]}
    assert "import_dependency_change" in types
    assert payload["summary"]
