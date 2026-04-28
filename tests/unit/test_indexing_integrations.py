import json

from devcouncil.indexing.ast_matcher import AstMatcher
from devcouncil.indexing.lsp import LspInspector


def test_lsp_inspector_detects_language_and_initialize_payload(tmp_path):
    (tmp_path / "app.py").write_text("def hello():\n    return 'world'\n", encoding="utf-8")

    summary = LspInspector(tmp_path).summary(["app.py"])

    assert summary["languages"] == ["python"]
    assert summary["servers"][0]["language"] == "python"
    assert summary["initialize_requests"]["python"]["method"] == "initialize"


def test_ast_matcher_finds_python_symbols(tmp_path):
    (tmp_path / "app.py").write_text(
        "class Service:\n"
        "    pass\n\n"
        "def handle_request():\n"
        "    return Service()\n",
        encoding="utf-8",
    )

    matches = AstMatcher(tmp_path).match(query="handle", language="python")

    assert [match.name for match in matches] == ["handle_request"]
    assert matches[0].kind == "function"
    assert matches[0].engine in {"fallback-ast", "tree-sitter-optional"}


def test_ast_cli_outputs_matches(tmp_path):
    from typer.testing import CliRunner
    from devcouncil.cli.main import app

    runner = CliRunner()
    (tmp_path / "app.py").write_text("def target_symbol():\n    pass\n", encoding="utf-8")

    result = runner.invoke(app, ["ast", "match", "target", "--project-root", str(tmp_path)])

    assert result.exit_code == 0
    assert json.loads(result.output)["matches"][0]["name"] == "target_symbol"


def test_ast_cli_clamps_zero_limit(tmp_path):
    from typer.testing import CliRunner
    from devcouncil.cli.main import app

    runner = CliRunner()
    (tmp_path / "app.py").write_text("def target_symbol():\n    pass\n", encoding="utf-8")

    result = runner.invoke(app, ["ast", "match", "target", "--limit", "0", "--project-root", str(tmp_path)])

    assert result.exit_code == 0
    assert json.loads(result.output)["matches"][0]["name"] == "target_symbol"


def test_lsp_cli_reports_missing_project_root(tmp_path):
    from typer.testing import CliRunner
    from devcouncil.cli.main import app

    runner = CliRunner()
    result = runner.invoke(app, ["lsp", "inspect", "--project-root", str(tmp_path / "missing")])

    assert result.exit_code == 1
    assert "does not exist" in result.output


def test_ast_matcher_ignores_dependency_dirs_and_finds_typescript_arrows(tmp_path):
    (tmp_path / "node_modules").mkdir()
    (tmp_path / "node_modules" / "ignored.ts").write_text("export const targetSymbol = () => 1;\n", encoding="utf-8")
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "app.ts").write_text("export const targetSymbol = () => 2;\n", encoding="utf-8")

    matches = AstMatcher(tmp_path).match(query="target", language="TypeScript")

    assert [match.path for match in matches] == ["src/app.ts"]
    assert matches[0].name == "targetSymbol"


def test_lsp_inspector_ignores_dependency_dirs(tmp_path):
    (tmp_path / "node_modules").mkdir()
    (tmp_path / "node_modules" / "ignored.ts").write_text("export const ignored = true;\n", encoding="utf-8")
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "app.py").write_text("def app(): pass\n", encoding="utf-8")

    summary = LspInspector(tmp_path).summary()

    assert summary["languages"] == ["python"]


def test_lsp_inspector_ignores_dependency_dirs_from_explicit_file_lists(tmp_path):
    summary = LspInspector(tmp_path).summary(["node_modules/ignored.ts", "src/app.py"])

    assert summary["languages"] == ["python"]


def test_indexing_normalizes_uppercase_extensions(tmp_path):
    (tmp_path / "Service.PY").write_text("def target_symbol():\n    pass\n", encoding="utf-8")

    lsp = LspInspector(tmp_path).summary()
    matches = AstMatcher(tmp_path).match(query="target", language="python")

    assert lsp["languages"] == ["python"]
    assert matches[0].path == "Service.PY"
