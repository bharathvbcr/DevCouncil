"""CLI wiring for `dev check`'s deterministic evidence-gate mode (--verify/--test)."""

import json
import subprocess

from typer.testing import CliRunner

from devcouncil.cli.main import app

runner = CliRunner()


def _commit_base(tmp_path):
    subprocess.run(["git", "init"], cwd=tmp_path, capture_output=True)
    (tmp_path / "app.py").write_text("def f():\n    return 1\n", encoding="utf-8")
    subprocess.run(["git", "add", "-A"], cwd=tmp_path, capture_output=True)
    subprocess.run(
        ["git", "-c", "user.email=t@t.com", "-c", "user.name=t", "commit", "-m", "init"],
        cwd=tmp_path,
        capture_output=True,
    )


def test_check_verify_clean_tree_passes(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    _commit_base(tmp_path)

    result = runner.invoke(app, ["check", "--verify", "--json"])

    assert result.exit_code == 0
    data = json.loads(result.output)
    assert data["verified"] is True
    assert data["reason"] == "no_changes"


def test_check_verify_emits_next_actions_contract(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    _commit_base(tmp_path)
    # An unverified change: a new function with a stated goal but (in this minimal
    # repo) no passing evidence wired up.
    (tmp_path / "app.py").write_text("def f():\n    return 1\n\ndef g():\n    return 2\n", encoding="utf-8")

    result = runner.invoke(app, ["check", "--verify", "--goal", "g returns 2", "--json"])

    # Wiring contract: the command runs and emits the typed structure regardless of
    # the host test environment.
    assert result.exit_code in (0, 1)
    data = json.loads(result.output)
    assert {"verified", "changed_files", "gaps", "next_actions", "diff_coverage"} <= set(data)
    assert "app.py" in data["changed_files"]
