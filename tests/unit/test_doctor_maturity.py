from devcouncil.cli.commands.doctor import _subsystem_maturity_rows, render_doctor_check
from io import StringIO
from rich.console import Console


def test_subsystem_maturity_includes_preview_features():
    rows = _subsystem_maturity_rows()
    tiers = {area: tier for area, tier, _ in rows}
    assert tiers["CLI & Storage"] == "stable"
    assert tiers["Repo Map & Code Graph"] == "stable"
    assert tiers["Live Dashboard"] == "stable"
    assert tiers["Coding CLI Executors"] == "preview"
    # Promoted out of Experimental once it joined the lease-gated write path + shared
    # verify/next-actions loop (parity with MCP), backed by test_native_closed_loop.py.
    assert tiers["Native Executor"] == "preview"


def test_doctor_renders_maturity_section(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    tmp_path.mkdir(parents=True, exist_ok=True)
    console = Console(file=StringIO(), width=120)
    from devcouncil.cli.commands import doctor as doctor_mod

    original = doctor_mod.console
    doctor_mod.console = console
    try:
        render_doctor_check(tmp_path)
    finally:
        doctor_mod.console = original
    output = console.file.getvalue()
    assert "Maturity:" in output or "Subsystem Maturity" in output
    assert "Preview" in output
    assert "Semantic layer" in output


def test_check_unknown_indexing_keys_flags_stale_option(tmp_path):
    from devcouncil.cli.commands.doctor import check_unknown_indexing_keys

    dc = tmp_path / ".devcouncil"
    dc.mkdir()
    (dc / "config.yaml").write_text(
        "indexing:\n  compact_graph_json: true\n  no_such_option: 5\n",
        encoding="utf-8",
    )
    rows = check_unknown_indexing_keys(tmp_path)
    assert len(rows) == 1
    assert "no_such_option" in rows[0][2]
    assert "compact_graph_json" not in rows[0][2]


def test_check_unknown_indexing_keys_silent_when_clean(tmp_path):
    from devcouncil.cli.commands.doctor import check_unknown_indexing_keys

    dc = tmp_path / ".devcouncil"
    dc.mkdir()
    (dc / "config.yaml").write_text("indexing:\n  lsp_refs: false\n", encoding="utf-8")
    assert check_unknown_indexing_keys(tmp_path) == []


def test_check_grammar_coverage_warns_on_missing_repo_language(tmp_path, monkeypatch):
    """Display-cased spec names must match lowercase repo_map languages, and a
    missing python grammar must NOT warn (extraction is native stdlib-ast)."""
    from devcouncil.cli.commands.doctor import check_grammar_coverage

    dc = tmp_path / ".devcouncil"
    dc.mkdir()
    (dc / "repo_map.json").write_text('{"languages": ["python", "go"]}', encoding="utf-8")

    def _fake_status():
        return {
            "languages": [
                {"language": "Python", "grammar": "python", "missing_grammars": ["python"]},
                {"language": "Go", "grammar": "go", "missing_grammars": ["go"]},
                {"language": "Java", "grammar": "java", "missing_grammars": ["java"]},
            ],
            "action": "Install the grammars wheel.",
        }

    import devcouncil.codeintel.languages as langs_mod

    monkeypatch.setattr(langs_mod, "grammar_status", _fake_status)
    rows = check_grammar_coverage(tmp_path)
    assert len(rows) == 1
    status_markup = rows[0][1]
    assert "WARN" in status_markup
    detail = rows[0][2]
    assert "Go" in detail
    # Python is native-extraction; Java is not a repo language — neither warns.
    assert "Python" not in detail
    assert "Java" not in detail


def test_check_grammar_coverage_ok_when_only_native_missing(tmp_path, monkeypatch):
    from devcouncil.cli.commands.doctor import check_grammar_coverage

    dc = tmp_path / ".devcouncil"
    dc.mkdir()
    (dc / "repo_map.json").write_text('{"languages": ["python"]}', encoding="utf-8")

    def _fake_status():
        return {
            "languages": [
                {"language": "Python", "grammar": "python", "missing_grammars": ["python"]},
            ],
            "action": "Install the grammars wheel.",
        }

    import devcouncil.codeintel.languages as langs_mod

    monkeypatch.setattr(langs_mod, "grammar_status", _fake_status)
    rows = check_grammar_coverage(tmp_path)
    assert len(rows) == 1
    assert "OK" in rows[0][1]


def test_check_lsp_reference_confirmation_reports_missing_servers(tmp_path, monkeypatch):
    import shutil

    from devcouncil.cli.commands.doctor import check_lsp_reference_confirmation

    (tmp_path / "pkg").mkdir()
    (tmp_path / "pkg" / "svc.go").write_text("package pkg\n", encoding="utf-8")
    monkeypatch.setattr(shutil, "which", lambda *_a, **_k: None)
    rows = check_lsp_reference_confirmation(tmp_path)
    assert len(rows) == 1
    assert "go" in rows[0][2]
