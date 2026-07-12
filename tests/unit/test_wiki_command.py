"""CLI coverage for `dev wiki` — update/generate/read/status/install-action.

The heavy wiki builder (`refresh_wiki`) and reader (`read_wiki_page`) are stubbed;
the tests exercise the command's rendering, exit codes, and file-writing paths.
"""

from pathlib import Path
from types import SimpleNamespace

import devcouncil.knowledge.wiki as wiki_pkg
import devcouncil.knowledge.wiki_read as wiki_read_pkg
from devcouncil.cli.main import app
from typer.testing import CliRunner

runner = CliRunner()


def _result(**kw):
    defaults = dict(
        changed=True,
        created=["subsystems/a.md"],
        updated=[],
        skipped=[],
        enriched=[],
        problems=[],
        wiki_dir=Path(".devcouncil/knowledge/okf/wiki"),
    )
    defaults.update(kw)
    return SimpleNamespace(**defaults)


def test_wiki_update_reports_created(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    monkeypatch.setattr(wiki_pkg, "refresh_wiki", lambda root, **kw: _result())

    result = runner.invoke(app, ["wiki", "update", "--no-llm", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "Wiki updated" in result.output
    assert "1 created" in result.output


def test_wiki_update_reports_up_to_date(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    monkeypatch.setattr(
        wiki_pkg, "refresh_wiki",
        lambda root, **kw: _result(changed=False, created=[], skipped=["a.md", "b.md"]),
    )

    result = runner.invoke(app, ["wiki", "update", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "up to date" in result.output


def test_wiki_update_reports_enriched_and_problems(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    monkeypatch.setattr(
        wiki_pkg, "refresh_wiki",
        lambda root, **kw: _result(enriched=["a.md"], problems=["bad link in b.md"]),
    )

    result = runner.invoke(app, ["wiki", "update", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "LLM-enriched" in result.output
    assert "validation issue" in result.output
    assert "bad link" in result.output


def test_wiki_generate_alias(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    captured = {}

    def fake_refresh(root, **kw):
        captured.update(kw)
        return _result()

    monkeypatch.setattr(wiki_pkg, "refresh_wiki", fake_refresh)
    result = runner.invoke(app, ["wiki", "generate", "--force", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert captured["force"] is True


def test_wiki_read_json(tmp_path, monkeypatch):
    monkeypatch.setattr(
        wiki_read_pkg, "read_wiki_page",
        lambda root, page=None, query=None: {"ok": True, "pages": [{"page": "index.md", "title": "Home"}]},
    )
    result = runner.invoke(app, ["wiki", "read", "--json", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "index.md" in result.output


def test_wiki_read_lists_pages_human(tmp_path, monkeypatch):
    monkeypatch.setattr(
        wiki_read_pkg, "read_wiki_page",
        lambda root, page=None, query=None: {"ok": True, "pages": [{"page": "index.md", "title": "Home"}]},
    )
    result = runner.invoke(app, ["wiki", "read", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "index.md" in result.output
    assert "Home" in result.output


def test_wiki_read_single_page_body(tmp_path, monkeypatch):
    monkeypatch.setattr(
        wiki_read_pkg, "read_wiki_page",
        lambda root, page=None, query=None: {"ok": True, "body": "# Page body"},
    )
    result = runner.invoke(app, ["wiki", "read", "--page", "index.md", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "Page body" in result.output


def test_wiki_read_error_exits(tmp_path, monkeypatch):
    monkeypatch.setattr(
        wiki_read_pkg, "read_wiki_page",
        lambda root, page=None, query=None: {"ok": False, "error": "no wiki"},
    )
    result = runner.invoke(app, ["wiki", "read", "--project-root", str(tmp_path)])
    assert result.exit_code == 1
    assert "no wiki" in result.output


def test_wiki_status_missing_wiki_exits(tmp_path, monkeypatch):
    result = runner.invoke(app, ["wiki", "status", "--project-root", str(tmp_path)])
    assert result.exit_code == 1
    assert "No wiki found" in result.output


def test_wiki_status_missing_wiki_json(tmp_path):
    result = runner.invoke(app, ["wiki", "status", "--json", "--project-root", str(tmp_path)])
    assert result.exit_code == 1
    assert '"exists": false' in result.output.lower() or "false" in result.output.lower()


def test_wiki_install_action_writes_workflow(tmp_path):
    result = runner.invoke(app, ["wiki", "install-action", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    workflow = tmp_path / ".github" / "workflows" / "devcouncil-wiki-update.yml"
    assert workflow.is_file()
    assert "DevCouncil Wiki Update" in workflow.read_text(encoding="utf-8")


def test_wiki_install_action_refuses_overwrite_without_force(tmp_path):
    assert runner.invoke(app, ["wiki", "install-action", "--project-root", str(tmp_path)]).exit_code == 0
    result = runner.invoke(app, ["wiki", "install-action", "--project-root", str(tmp_path)])
    assert result.exit_code == 1
    assert "already exists" in result.output


def test_wiki_install_action_force_overwrites_and_unignores(tmp_path):
    (tmp_path / ".gitignore").write_text(".devcouncil/\n", encoding="utf-8")
    assert runner.invoke(app, ["wiki", "install-action", "--project-root", str(tmp_path)]).exit_code == 0

    result = runner.invoke(app, ["wiki", "install-action", "--force", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    gitignore = (tmp_path / ".gitignore").read_text(encoding="utf-8")
    assert "!.devcouncil/knowledge/okf/wiki/" in gitignore
