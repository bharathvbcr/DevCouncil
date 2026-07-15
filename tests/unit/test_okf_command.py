"""CLI coverage for `dev okf` — export guards, ingest, validate, select, html."""

from __future__ import annotations

from types import SimpleNamespace

from typer.testing import CliRunner

import devcouncil.cli.commands.okf as okf_cmd
from devcouncil.cli.main import app

runner = CliRunner()


# --- helpers: config resolution fall-throughs -------------------------------------


def test_knowledge_okf_dir_config_error_falls_back(tmp_path, monkeypatch):
    import devcouncil.app.config as config_mod

    def boom(root):
        raise RuntimeError("no config")

    monkeypatch.setattr(config_mod, "load_config", boom)
    assert okf_cmd._knowledge_okf_dir(tmp_path) == tmp_path / ".devcouncil/knowledge" / "okf"


def test_knowledge_design_md_config_error_and_missing(tmp_path, monkeypatch):
    import devcouncil.app.config as config_mod

    def boom(root):
        raise RuntimeError("no config")

    monkeypatch.setattr(config_mod, "load_config", boom)
    assert okf_cmd._knowledge_design_md(tmp_path) is None


def test_knowledge_design_md_found(tmp_path):
    design = tmp_path / ".devcouncil" / "knowledge" / "design" / "design.md"
    design.parent.mkdir(parents=True)
    design.write_text("# design", encoding="utf-8")
    assert okf_cmd._knowledge_design_md(tmp_path) == design


# --- export -----------------------------------------------------------------------


def test_okf_export_db_unavailable(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    monkeypatch.setattr(okf_cmd, "get_db", lambda root: None)
    result = runner.invoke(app, ["okf", "export"])
    assert result.exit_code == 1
    assert "state is unavailable" in result.output


def test_okf_export_writes_bundle(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    import devcouncil.reporting.okf_bundle_writer as writer_mod
    monkeypatch.setattr(
        writer_mod.OKFBundleWriter, "generate",
        staticmethod(lambda *a, **k: ["index.md", "doc1.md"]),
    )
    result = runner.invoke(app, ["okf", "export", "--no-skills", "--no-design", "-o", str(tmp_path / "b")])
    assert result.exit_code == 0
    assert "Exported 2 OKF documents" in result.output


def test_okf_export_includes_skills_and_design(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    design = tmp_path / ".devcouncil" / "knowledge" / "design" / "design.md"
    design.parent.mkdir(parents=True)
    design.write_text("# design", encoding="utf-8")

    import devcouncil.reporting.okf_bundle_writer as writer_mod
    import devcouncil.knowledge.design as design_mod
    import devcouncil.skills.registry as reg_mod

    monkeypatch.setattr(reg_mod, "load_skills", lambda project_root: [SimpleNamespace(name="s1")])
    monkeypatch.setattr(design_mod, "parse_design_md", lambda p: SimpleNamespace(tokens={}))
    monkeypatch.setattr(
        writer_mod.OKFBundleWriter, "generate",
        staticmethod(lambda *a, **k: ["index.md"]),
    )
    result = runner.invoke(app, ["okf", "export", "-o", str(tmp_path / "b")])
    assert result.exit_code == 0
    assert "engineering skill document" in result.output
    assert "design system document" in result.output


# --- ingest -----------------------------------------------------------------------


def _fake_fetched(directory, suggested="bundle"):
    return SimpleNamespace(directory=directory, suggested_name=suggested, cleanup=lambda: None)


def test_okf_ingest_fetch_error(tmp_path, monkeypatch):
    import devcouncil.knowledge.fetch as fetch_mod

    def boom(bundle):
        raise RuntimeError("network down")

    monkeypatch.setattr(fetch_mod, "fetch_bundle", boom)
    result = runner.invoke(app, ["okf", "ingest", "https://example.com/b.git", "--project-root", str(tmp_path)])
    assert result.exit_code == 1
    assert "Could not fetch bundle" in result.output


def test_okf_ingest_not_a_directory(tmp_path, monkeypatch):
    import devcouncil.knowledge.fetch as fetch_mod
    missing = tmp_path / "nope"
    monkeypatch.setattr(fetch_mod, "fetch_bundle", lambda bundle: _fake_fetched(missing))
    result = runner.invoke(app, ["okf", "ingest", str(missing), "--project-root", str(tmp_path)])
    assert result.exit_code == 1
    assert "Not a directory" in result.output


def test_okf_ingest_no_documents(tmp_path, monkeypatch):
    import devcouncil.knowledge.fetch as fetch_mod
    src = tmp_path / "src"
    src.mkdir()
    monkeypatch.setattr(fetch_mod, "fetch_bundle", lambda bundle: _fake_fetched(src))
    monkeypatch.setattr(okf_cmd, "read_bundle", lambda s: SimpleNamespace(documents=[]))
    result = runner.invoke(app, ["okf", "ingest", str(src), "--project-root", str(tmp_path)])
    assert result.exit_code == 1
    assert "No OKF documents" in result.output


def test_okf_ingest_copies_docs_with_validation_warnings(tmp_path, monkeypatch):
    import devcouncil.knowledge.fetch as fetch_mod
    src = tmp_path / "src"
    (src / "sub").mkdir(parents=True)
    (src / "index.md").write_text("# a", encoding="utf-8")
    (src / "sub" / "doc.md").write_text("# b", encoding="utf-8")

    monkeypatch.setattr(fetch_mod, "fetch_bundle", lambda bundle: _fake_fetched(src, "mybundle"))
    monkeypatch.setattr(okf_cmd, "read_bundle", lambda s: SimpleNamespace(documents=["x"]))
    monkeypatch.setattr(okf_cmd, "validate_bundle", lambda parsed: ["missing type on doc.md"])

    result = runner.invoke(app, ["okf", "ingest", str(src), "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "Ingested 2 OKF document(s)" in result.output
    assert (tmp_path / ".devcouncil" / "knowledge" / "okf" / "mybundle" / "index.md").exists()


# --- validate ---------------------------------------------------------------------


def test_okf_validate_not_a_directory(tmp_path):
    f = tmp_path / "file.md"
    f.write_text("x", encoding="utf-8")
    result = runner.invoke(app, ["okf", "validate", str(f)])
    assert result.exit_code == 1
    assert "Not a directory" in result.output


def test_okf_validate_valid(tmp_path, monkeypatch):
    src = tmp_path / "b"
    src.mkdir()
    monkeypatch.setattr(okf_cmd, "read_bundle", lambda s: SimpleNamespace(documents=["a", "b"]))
    monkeypatch.setattr(okf_cmd, "validate_bundle", lambda parsed: [])
    result = runner.invoke(app, ["okf", "validate", str(src)])
    assert result.exit_code == 0
    assert "Valid OKF bundle" in result.output


def test_okf_validate_problems(tmp_path, monkeypatch):
    src = tmp_path / "b"
    src.mkdir()
    monkeypatch.setattr(okf_cmd, "read_bundle", lambda s: SimpleNamespace(documents=["a"]))
    monkeypatch.setattr(okf_cmd, "validate_bundle", lambda parsed: ["link broken"])
    result = runner.invoke(app, ["okf", "validate", str(src)])
    assert result.exit_code == 1
    assert "problem(s)" in result.output


# --- select -----------------------------------------------------------------------


def test_okf_select_human(tmp_path, monkeypatch):
    import devcouncil.knowledge.knowledge_select as ks_mod
    monkeypatch.setattr(
        ks_mod, "select_knowledge_payload",
        lambda root, goal: {
            "sources": [{"kind": "okf", "name": "n1", "description": "d1"}],
            "preamble": "use these",
        },
    )
    result = runner.invoke(app, ["okf", "select", "--goal", "do it", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "n1" in result.output
    assert "use these" in result.output


def test_okf_select_json(tmp_path, monkeypatch):
    import devcouncil.knowledge.knowledge_select as ks_mod
    monkeypatch.setattr(
        ks_mod, "select_knowledge_payload",
        lambda root, goal: {"sources": [], "preamble": ""},
    )
    result = runner.invoke(app, ["okf", "select", "--goal", "x", "--json", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "sources" in result.output


# --- html -------------------------------------------------------------------------


def test_okf_html_not_a_directory(tmp_path):
    f = tmp_path / "file.md"
    f.write_text("x", encoding="utf-8")
    result = runner.invoke(app, ["okf", "html", str(f)])
    assert result.exit_code == 1
    assert "Not a directory" in result.output


def test_okf_html_no_documents(tmp_path, monkeypatch):
    src = tmp_path / "b"
    src.mkdir()
    monkeypatch.setattr(okf_cmd, "read_bundle", lambda s: SimpleNamespace(documents=[]))
    result = runner.invoke(app, ["okf", "html", str(src)])
    assert result.exit_code == 1
    assert "No OKF documents" in result.output


def test_okf_html_renders(tmp_path, monkeypatch):
    src = tmp_path / "b"
    src.mkdir()
    monkeypatch.setattr(okf_cmd, "read_bundle", lambda s: SimpleNamespace(documents=["a"]))
    import devcouncil.reporting.okf_html as html_mod
    monkeypatch.setattr(html_mod, "write_bundle_html", lambda parsed, out_dir: ["index.html", "a.html"])
    result = runner.invoke(app, ["okf", "html", str(src), "-o", str(tmp_path / "site")])
    assert result.exit_code == 0
    assert "Rendered 2 page(s)" in result.output
