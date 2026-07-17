"""Unit coverage for indexing.wiring helpers (comment stripping, export parsing,
entry-root resolution, dynamic-import indexing, structural exemptions)."""

from __future__ import annotations

import ast

from devcouncil.indexing import wiring


# ----------------------------------------------------------------------
# path predicates
# ----------------------------------------------------------------------


def test_is_test_path_variants():
    assert wiring.is_test_path("tests/unit/test_foo.py")
    assert wiring.is_test_path("pkg/test_bar.py")
    assert wiring.is_test_path("conftest.py")
    assert wiring.is_test_path("web/foo.test.ts")
    assert wiring.is_test_path("web/foo.spec.tsx")
    assert wiring.is_test_path("svc/foo_test.go")
    assert not wiring.is_test_path("pkg/module.py")


def test_is_private_and_dunder():
    assert wiring.is_private_symbol("_x")
    assert not wiring.is_private_symbol("x")
    assert not wiring.is_private_symbol("")
    assert wiring.is_dunder_symbol("__init__")
    assert not wiring.is_dunder_symbol("_x")
    assert not wiring.is_dunder_symbol("__x")


def test_is_vendored_path():
    assert wiring.is_vendored_path("web/node_modules/lib/x.js")
    assert wiring.is_vendored_path("assets/vendor/force-graph.min.js")
    assert wiring.is_vendored_path("static/app.min.css")
    assert not wiring.is_vendored_path("src/app.py")


def test_is_liveness_code_file():
    assert wiring.is_liveness_code_file("pkg/mod.py")
    assert wiring.is_liveness_code_file("web/x.tsx")
    assert wiring.is_liveness_code_file("svc/a.go")
    assert not wiring.is_liveness_code_file("README.md")


def test_is_liveness_rust_gated(monkeypatch):
    monkeypatch.setattr(
        "devcouncil.indexing.ts_imports.tree_sitter_available", lambda: True
    )
    assert wiring.is_liveness_code_file("src/lib.rs") is True
    monkeypatch.setattr(
        "devcouncil.indexing.ts_imports.tree_sitter_available", lambda: False
    )
    assert wiring.is_liveness_code_file("src/lib.rs") is False


# ----------------------------------------------------------------------
# comment / string stripping
# ----------------------------------------------------------------------


def test_strip_py_comments_preserves_line_count():
    src = 'x = 1  # trailing\n# full line\ny = "# not a comment"\n'
    out = wiring.strip_py_comments(src)
    assert out.count("\n") == src.count("\n")
    assert "trailing" not in out
    assert "full line" not in out
    assert "# not a comment" in out  # inside a string literal, preserved


def test_strip_js_comments_block_and_line():
    src = "const a = 1; // note\n/* block\nspanning */\nconst b = 2;\n"
    out = wiring.strip_js_comments(src)
    assert out.count("\n") == src.count("\n")
    assert "note" not in out
    assert "block" not in out
    assert "const b = 2;" in out


def test_strip_string_literals_blanks_bodies():
    src = 'name = "cost_by_task"\ntpl = `hello`\n'
    out = wiring.strip_string_literals(src)
    assert "cost_by_task" not in out
    assert out.count("\n") == src.count("\n")


def test_strip_string_literals_triple_quoted():
    src = 'doc = """line one\nsecret_symbol\n"""\nz = 1\n'
    out = wiring.strip_string_literals(src)
    assert "secret_symbol" not in out
    assert "z = 1" in out
    assert out.count("\n") == src.count("\n")


# ----------------------------------------------------------------------
# python export parsing
# ----------------------------------------------------------------------


def test_parse_python_all_exports():
    src = "__all__ = ['a', 'b']\nx = 1\n"
    assert wiring.parse_python_all_exports(src) == {"a", "b"}
    assert wiring.parse_python_all_exports("def (:\n") == set()


def test_parse_python_reexport_names_init_vs_module():
    src = "from .sub import thing\nfrom .other import helper as aliased\n"
    init_names = wiring.parse_python_reexport_names("pkg/__init__.py", src)
    assert init_names == {"thing", "aliased"}
    # Non-init module only re-exports names in __all__.
    mod_src = "__all__ = ['thing']\nfrom .sub import thing\nfrom .sub import hidden\n"
    mod_names = wiring.parse_python_reexport_names("pkg/mod.py", mod_src)
    assert mod_names == {"thing"}


# ----------------------------------------------------------------------
# JS export symbol iteration
# ----------------------------------------------------------------------


def test_iter_js_export_symbols():
    src = (
        "export function alpha() {}\n"
        "export const beta = 1;\n"
        "export { gamma, delta as renamed };\n"
        "export default function main() {}\n"
        "export type { TypeOnly } from './t';\n"
    )
    found = {name for _, name in wiring.iter_js_export_symbols(src)}
    assert "alpha" in found
    assert "beta" in found
    assert "gamma" in found
    assert "renamed" in found  # `delta as renamed`
    assert "main" in found


# ----------------------------------------------------------------------
# decorators
# ----------------------------------------------------------------------


def test_decorator_names_and_wiring_detection():
    tree = ast.parse(
        "import x\n"
        "@app.get('/health')\n"
        "def health():\n    pass\n"
        "@staticmethod\n"
        "def util():\n    pass\n"
    )
    funcs = [n for n in tree.body if isinstance(n, ast.FunctionDef)]
    health_decos = wiring.decorator_names(funcs[0])
    assert any("app.get" in d for d in health_decos)
    assert wiring.is_wiring_decorated(health_decos) is True
    assert wiring.is_wiring_decorated(wiring.decorator_names(funcs[1])) is False


def test_is_wiring_decorated_segment_match_not_substring():
    assert wiring.is_wiring_decorated(["task"]) is True
    assert wiring.is_wiring_decorated(["multitask"]) is False  # not a whole segment


# ----------------------------------------------------------------------
# structural exemptions
# ----------------------------------------------------------------------


def test_structural_exemptions():
    assert wiring.structural_exemptions("pkg/__main__.py")
    assert wiring.structural_exemptions("app/dashboard/page.tsx")
    assert wiring.structural_exemptions("db/migrations/0001_init.py")
    assert wiring.structural_exemptions("scripts/tool.py")
    assert wiring.structural_exemptions("types/foo.d.ts")
    assert not wiring.structural_exemptions("pkg/regular.py")


# ----------------------------------------------------------------------
# entry roots / entry-point symbols
# ----------------------------------------------------------------------


def test_entry_roots_from_pyproject_scripts(tmp_path):
    (tmp_path / "pyproject.toml").write_text(
        '[project]\nname = "x"\nversion = "0"\n'
        '[project.scripts]\nmycli = "pkg.cli:main"\n',
        encoding="utf-8",
    )
    (tmp_path / "pkg").mkdir()
    (tmp_path / "pkg" / "cli.py").write_text("def main():\n    pass\n", encoding="utf-8")
    roots = wiring.entry_roots(tmp_path, ["pkg/cli.py", "pyproject.toml"])
    assert "pkg/cli.py" in roots


def test_entry_roots_includes_main_module(tmp_path):
    roots = wiring.entry_roots(tmp_path, ["pkg/__main__.py", "pkg/other.py"])
    assert "pkg/__main__.py" in roots


def test_entry_point_symbols(tmp_path):
    (tmp_path / "pyproject.toml").write_text(
        '[project]\nname = "x"\nversion = "0"\n'
        '[project.scripts]\nmycli = "pkg.cli:main"\n',
        encoding="utf-8",
    )
    syms = wiring.entry_point_symbols(tmp_path, ["pkg/cli.py"])
    assert "pkg/cli.py::main" in syms


def test_package_json_entry_targets(tmp_path):
    (tmp_path / "package.json").write_text(
        '{"name": "app", "main": "src/index.js", "bin": {"app": "bin/cli.js"}}\n',
        encoding="utf-8",
    )
    roots = wiring.entry_roots(tmp_path, ["src/index.js", "bin/cli.js", "package.json"])
    assert "src/index.js" in roots
    assert "bin/cli.js" in roots


# ----------------------------------------------------------------------
# module tokens + import spec matching
# ----------------------------------------------------------------------


def test_module_tokens_for():
    tokens = wiring.module_tokens_for("src/pkg/mod.py")
    assert "src/pkg/mod" in tokens
    assert "pkg.mod" in tokens  # src. prefix stripped from dotted form


def test_import_spec_matches():
    tokens = wiring.module_tokens_for("src/pkg/mod.py")
    assert wiring.import_spec_matches("pkg.mod", tokens)
    assert wiring.import_spec_matches("pkg/mod", tokens)
    assert not wiring.import_spec_matches("other.module", tokens)
    assert not wiring.import_spec_matches("", tokens)


# ----------------------------------------------------------------------
# allow-unwired marker + dynamic import index
# ----------------------------------------------------------------------


def test_has_allow_unwired(tmp_path):
    (tmp_path / "a.py").write_text("# devcouncil: allow-unwired\nx = 1\n", encoding="utf-8")
    (tmp_path / "b.py").write_text("x = 1\n", encoding="utf-8")
    assert wiring.has_allow_unwired(tmp_path, "a.py") is True
    assert wiring.has_allow_unwired(tmp_path, "b.py") is False
    assert wiring.has_allow_unwired(tmp_path, "missing.py") is False


def test_build_dynamic_import_index_and_reference_cleared(tmp_path):
    (tmp_path / "plugins").mkdir()
    (tmp_path / "plugins" / "impl.py").write_text("x = 1\n", encoding="utf-8")
    (tmp_path / "loader.py").write_text(
        "import importlib\nmod = importlib.import_module('plugins.impl')\n",
        encoding="utf-8",
    )
    git_files = ["plugins/impl.py", "loader.py"]
    index = wiring.build_dynamic_import_index(tmp_path, git_files=git_files)
    assert any("plugins" in form and "impl" in form for form in index)
    # reference_cleared should see loader.py referencing plugins/impl.py.
    assert wiring.reference_cleared(
        tmp_path, "plugins/impl.py", git_files=git_files, dynamic_index=index
    ) is True
    # An unreferenced module is not cleared.
    (tmp_path / "plugins" / "orphan.py").write_text("y = 1\n", encoding="utf-8")
    assert wiring.reference_cleared(
        tmp_path,
        "plugins/orphan.py",
        git_files=["plugins/orphan.py", "loader.py"],
        dynamic_index=index,
    ) is False


def test_reference_cleared_ignores_test_referrers(tmp_path):
    (tmp_path / "plugins").mkdir()
    (tmp_path / "plugins" / "impl.py").write_text("x = 1\n", encoding="utf-8")
    (tmp_path / "tests").mkdir()
    (tmp_path / "tests" / "test_it.py").write_text(
        "import importlib\nimportlib.import_module('plugins.impl')\n", encoding="utf-8"
    )
    git_files = ["plugins/impl.py", "tests/test_it.py"]
    # Dynamic import only in a test file → does not clear (parity with static rule).
    assert wiring.reference_cleared(tmp_path, "plugins/impl.py", git_files=git_files) is False


# ----------------------------------------------------------------------
# _norm + is_liveness rust exception branch
# ----------------------------------------------------------------------


def test_norm_strips_leading_dot_slash():
    assert wiring._norm("./a/./b") == "a/./b"
    assert wiring._norm(".././a") == ".././a"


def test_is_liveness_code_file_rust_exception(monkeypatch):
    def boom():
        raise RuntimeError("import failed")

    monkeypatch.setattr(
        "devcouncil.indexing.ts_imports.tree_sitter_available", boom
    )
    assert wiring.is_liveness_code_file("src/lib.rs") is False


# ----------------------------------------------------------------------
# python export parsing edge branches
# ----------------------------------------------------------------------


def test_parse_python_all_exports_ignores_non_list_and_non_str():
    # __all__ assigned a non list/tuple value, plus an unrelated tuple target.
    src = "x, y = 1, 2\n__all__ = 'not-a-list'\n"
    assert wiring.parse_python_all_exports(src) == set()
    # list with a non-constant element is skipped.
    assert wiring.parse_python_all_exports("__all__ = [foo, 'kept']\n") == {"kept"}


def test_parse_python_reexport_names_syntax_error_and_star():
    assert wiring.parse_python_reexport_names("pkg/__init__.py", "def (:\n") == set()
    # star import contributes nothing.
    assert wiring.parse_python_reexport_names("pkg/__init__.py", "from .m import *\n") == set()


# ----------------------------------------------------------------------
# JS export symbol edge forms
# ----------------------------------------------------------------------


def test_iter_js_export_symbols_edge_forms():
    src = (
        "export function _hidden() {}\n"
        "export default main;\n"
        "export { type, default as keep, Renamed as R };\n"
        "export * as ns from './m';\n"
    )
    found = {name for _, name in wiring.iter_js_export_symbols(src)}
    assert "_hidden" not in found  # private skipped
    assert "main" in found         # export default identifier
    assert "keep" in found         # `default as keep`
    assert "R" in found            # `Renamed as R`
    assert "type" not in found     # bare `type` token skipped
    assert "ns" in found           # `export * as ns`


# ----------------------------------------------------------------------
# string-literal stripping edge cases
# ----------------------------------------------------------------------


def test_strip_string_literals_escapes_and_backtick_newline():
    src = 'a = "line\\nwith esc"\nb = `multi\nline`\nc = 1\n'
    out = wiring.strip_string_literals(src)
    assert "with esc" not in out
    assert "multi" not in out
    assert "c = 1" in out
    assert out.count("\n") == src.count("\n")


def test_strip_string_literals_no_trailing_newline():
    src = 'x = "abc"'  # no trailing newline
    out = wiring.strip_string_literals(src)
    assert "abc" not in out


def test_strip_string_literals_empty():
    assert wiring.strip_string_literals("") == ""


# ----------------------------------------------------------------------
# decorator unparse fallback
# ----------------------------------------------------------------------


def test_decorator_names_unparse_fallback():
    node = ast.parse("def f():\n    pass\n").body[0]
    # A malformed Attribute node makes ast.unparse raise -> fallback to .attr.
    node.decorator_list = [ast.Attribute(attr="wired_route")]
    names = wiring.decorator_names(node)
    assert "wired_route" in names


def test_is_wiring_decorated_empty_base_and_dotted_prefix():
    assert wiring.is_wiring_decorated(["("]) is False  # empty base -> skip
    assert wiring.is_wiring_decorated(["router.post"]) is True  # dotted prefix hint


# ----------------------------------------------------------------------
# structural exemptions extra branches
# ----------------------------------------------------------------------


def test_structural_exemptions_stories_and_non_py_migration():
    assert wiring.structural_exemptions("web/Button.stories.tsx")
    # A non-.py file under migrations/ is NOT auto-exempt.
    assert wiring.structural_exemptions("db/migrations/data.json") is False
    assert wiring.structural_exemptions("app/api/route.ts")


# ----------------------------------------------------------------------
# pyproject entry targets: gui-scripts, entry-points, plugins, fallback
# ----------------------------------------------------------------------


def test_pyproject_gui_scripts_entry_points_and_plugins(tmp_path):
    (tmp_path / "pkg").mkdir()
    (tmp_path / "pkg" / "gui.py").write_text("def run():\n    pass\n", encoding="utf-8")
    (tmp_path / "pkg" / "ep.py").write_text("def hook():\n    pass\n", encoding="utf-8")
    (tmp_path / "pkg" / "plug.py").write_text("x = 1\n", encoding="utf-8")
    (tmp_path / "pyproject.toml").write_text(
        '[project]\nname = "x"\nversion = "0"\n'
        '[project.gui-scripts]\napp = "pkg.gui:run"\n'
        '[project.entry-points."some.group"]\nname = "pkg.ep:hook"\n'
        '[tool.pytest.ini_options]\npytest_plugins = ["pkg.plug"]\n',
        encoding="utf-8",
    )
    files = ["pkg/gui.py", "pkg/ep.py", "pkg/plug.py", "pyproject.toml"]
    roots = wiring.entry_roots(tmp_path, files)
    assert "pkg/gui.py" in roots
    assert "pkg/ep.py" in roots
    assert "pkg/plug.py" in roots


def test_pyproject_plugins_as_string(tmp_path):
    (tmp_path / "pkg").mkdir()
    (tmp_path / "pkg" / "plug.py").write_text("x = 1\n", encoding="utf-8")
    (tmp_path / "pyproject.toml").write_text(
        '[project]\nname = "x"\nversion = "0"\n'
        '[tool.pytest.ini_options]\npytest_plugins = "pkg.plug"\n',
        encoding="utf-8",
    )
    roots = wiring.entry_roots(tmp_path, ["pkg/plug.py", "pyproject.toml"])
    assert "pkg/plug.py" in roots


def test_pyproject_regex_fallback_on_bad_toml(tmp_path):
    # Invalid TOML forces tomllib to raise → regex fallback picks up module:attr.
    (tmp_path / "pkg").mkdir()
    (tmp_path / "pkg" / "cli.py").write_text("def main():\n    pass\n", encoding="utf-8")
    (tmp_path / "pyproject.toml").write_text(
        'this is = = not valid toml [[[\nmycli = "pkg.cli:main"\n',
        encoding="utf-8",
    )
    roots = wiring.entry_roots(tmp_path, ["pkg/cli.py", "pyproject.toml"])
    assert "pkg/cli.py" in roots


def test_add_module_file_soft_suffix_and_dotted(tmp_path):
    # A dotted module resolves to a nested file via the soft suffix match.
    (tmp_path / "pyproject.toml").write_text(
        '[project]\nname = "x"\nversion = "0"\n'
        '[project.scripts]\ncli = "mod:main"\n',
        encoding="utf-8",
    )
    roots = wiring.entry_roots(tmp_path, ["deep/nested/mod.py", "pyproject.toml"])
    assert "deep/nested/mod.py" in roots


def test_add_module_file_relative_module_ignored():
    out: set = set()
    wiring._add_module_file(".relative", {"relative.py"}, out)
    assert out == set()


# ----------------------------------------------------------------------
# package.json entry targets: extensions, bin str, exports variants
# ----------------------------------------------------------------------


def test_package_json_invalid_json_returns_empty(tmp_path):
    (tmp_path / "package.json").write_text("{not valid json", encoding="utf-8")
    assert wiring._package_json_entry_targets(tmp_path, {"package.json"}) == set()


def test_package_json_extension_probing_and_exports(tmp_path):
    (tmp_path / "package.json").write_text(
        '{"name": "app",'
        ' "main": "src/index",'          # needs extension probing
        ' "bin": "bin/run.js",'          # bin as string
        ' "exports": {".": {"import": "lib/mod.js"}}}\n',
        encoding="utf-8",
    )
    files = ["src/index.js", "bin/run.js", "lib/mod.js", "package.json"]
    roots = wiring.entry_roots(tmp_path, files)
    assert "src/index.js" in roots
    assert "bin/run.js" in roots
    assert "lib/mod.js" in roots


def test_package_json_exports_string(tmp_path):
    (tmp_path / "package.json").write_text(
        '{"name": "app", "exports": "dist/entry.js"}\n', encoding="utf-8"
    )
    roots = wiring.entry_roots(tmp_path, ["dist/entry.js", "package.json"])
    assert "dist/entry.js" in roots


def test_package_json_index_fallback(tmp_path):
    (tmp_path / "package.json").write_text(
        '{"name": "app", "main": "src"}\n', encoding="utf-8"
    )
    roots = wiring.entry_roots(tmp_path, ["src/index.js", "package.json"])
    assert "src/index.js" in roots


# ----------------------------------------------------------------------
# entry_roots / entry_point_symbols exception + edge branches
# ----------------------------------------------------------------------


def test_entry_roots_returns_empty_on_exception(tmp_path):
    def bad_files():
        raise ValueError("boom")
        yield  # pragma: no cover

    assert wiring.entry_roots(tmp_path, bad_files()) == []


def test_entry_point_symbols_entry_points_and_skips(tmp_path):
    (tmp_path / "pkg").mkdir()
    (tmp_path / "pkg" / "ep.py").write_text("def hook():\n    pass\n", encoding="utf-8")
    (tmp_path / "pyproject.toml").write_text(
        '[project]\nname = "x"\nversion = "0"\n'
        '[project.entry-points."grp"]\nname = "pkg.ep:hook"\n'
        'nocolon = "pkg.ep"\n',
        encoding="utf-8",
    )
    syms = wiring.entry_point_symbols(tmp_path, ["pkg/ep.py"])
    assert "pkg/ep.py::hook" in syms


def test_entry_point_symbols_bad_toml_returns_empty(tmp_path):
    (tmp_path / "pyproject.toml").write_text("= = invalid [[[\n", encoding="utf-8")
    assert wiring.entry_point_symbols(tmp_path, ["pkg/ep.py"]) == set()


def test_entry_point_symbols_no_pyproject(tmp_path):
    assert wiring.entry_point_symbols(tmp_path, ["pkg/ep.py"]) == set()


# ----------------------------------------------------------------------
# module tokens: long top-level stem
# ----------------------------------------------------------------------


def test_module_tokens_for_long_top_level_stem():
    tokens = wiring.module_tokens_for("configuration_module.py")
    assert "configuration_module" in tokens


def test_module_tokens_for_init_package():
    tokens = wiring.module_tokens_for("src/pkg/__init__.py")
    assert "pkg" in tokens  # package dotted form, src. stripped


# ----------------------------------------------------------------------
# build_dynamic_import_index: default git files + exceptions
# ----------------------------------------------------------------------


def test_build_dynamic_import_index_default_git_files(tmp_path):
    (tmp_path / "loader.py").write_text(
        "import importlib\nimportlib.import_module('plugins.impl')\n"
        "getattr(obj, 'do_thing')\n",
        encoding="utf-8",
    )
    (tmp_path / "notes.md").write_text("not code\n", encoding="utf-8")
    # git_files=None → falls back to RepoMapper().get_git_files() (os.walk here).
    index = wiring.build_dynamic_import_index(tmp_path)
    assert any("plugins" in form for form in index)
    assert any(form.startswith(wiring.GETATTR_INDEX_PREFIX) for form in index)


def test_build_dynamic_import_index_repomapper_error(tmp_path, monkeypatch):
    monkeypatch.setattr(
        "devcouncil.indexing.repo_mapper.RepoMapper.get_git_files",
        lambda self: (_ for _ in ()).throw(RuntimeError("boom")),
    )
    assert wiring.build_dynamic_import_index(tmp_path) == {}


def test_build_dynamic_import_index_skips_relative_dynamic_import(tmp_path):
    (tmp_path / "m.ts").write_text("const x = import('./local');\n", encoding="utf-8")
    index = wiring.build_dynamic_import_index(tmp_path, git_files=["m.ts"])
    assert not any("local" in form for form in index)


# ----------------------------------------------------------------------
# reference_cleared: fallback scan + edge branches
# ----------------------------------------------------------------------


def test_reference_cleared_empty_target_returns_false(tmp_path):
    assert wiring.reference_cleared(tmp_path, "", git_files=[]) is False


def test_reference_cleared_dynamic_index_skips_self_and_test(tmp_path):
    index = {form: {"plugins/impl.py"} for form in wiring._module_forms("plugins.impl")}
    # only reference is the target itself → not cleared.
    assert wiring.reference_cleared(
        tmp_path, "plugins/impl.py", dynamic_index=index
    ) is False


def test_reference_cleared_fallback_scan_matches(tmp_path):
    (tmp_path / "plugins").mkdir()
    (tmp_path / "plugins" / "impl.py").write_text("x = 1\n", encoding="utf-8")
    (tmp_path / "loader.py").write_text(
        "import importlib\nimportlib.import_module('plugins.impl')\n", encoding="utf-8"
    )
    # No dynamic_index → targeted fallback scan over git_files.
    assert wiring.reference_cleared(
        tmp_path, "plugins/impl.py", git_files=["plugins/impl.py", "loader.py"]
    ) is True


def test_reference_cleared_fallback_default_git_files(tmp_path):
    (tmp_path / "plugins").mkdir()
    (tmp_path / "plugins" / "impl.py").write_text("y = 1\n", encoding="utf-8")
    (tmp_path / "loader.py").write_text(
        "mod = __import__('plugins.impl')\n", encoding="utf-8"
    )
    # git_files=None and no dynamic_index → RepoMapper fallback (os.walk).
    assert wiring.reference_cleared(tmp_path, "plugins/impl.py") is True


def test_reference_cleared_fallback_dynamic_import_relative_skipped(tmp_path):
    (tmp_path / "impl.ts").write_text("x = 1\n", encoding="utf-8")
    (tmp_path / "loader.ts").write_text("const m = import('./impl');\n", encoding="utf-8")
    assert wiring.reference_cleared(
        tmp_path, "impl.ts", git_files=["impl.ts", "loader.ts"]
    ) is False


def test_config_yaml_entry_roots_merged(tmp_path):
    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / ".devcouncil" / "config.yaml").write_text(
        "indexing:\n  entry_roots:\n    - pkg/seed.py\n",
        encoding="utf-8",
    )
    (tmp_path / "pkg").mkdir()
    (tmp_path / "pkg" / "seed.py").write_text("def run():\n    pass\n", encoding="utf-8")
    files = ["pkg/seed.py", "pkg/other.py"]
    assert "pkg/seed.py" in wiring.entry_roots(tmp_path, files, production_only=True)


# ----------------------------------------------------------------------
# advisory corpus index
# ----------------------------------------------------------------------


def test_build_corpus_markdown_headings_and_links(tmp_path):
    (tmp_path / "docs").mkdir()
    (tmp_path / "docs" / "guide.md").write_text(
        "# Overview\n\nSee [map](../README.md) and `src/devcouncil/cli/main.py`.\n",
        encoding="utf-8",
    )
    (tmp_path / "README.md").write_text("# Root\n", encoding="utf-8")
    graph = wiring.build_corpus(tmp_path)
    assert any(n.kind == "document" for n in graph.nodes)
    assert any(n.kind == "section" and n.label == "Overview" for n in graph.nodes)
    assert any(n.kind == "code_ref" for n in graph.nodes)
    assert wiring.corpus_graph_path(tmp_path).is_file()


def test_query_corpus_matches(tmp_path):
    (tmp_path / "docs").mkdir()
    (tmp_path / "docs" / "alpha.md").write_text("# Alpha topic\n", encoding="utf-8")
    wiring.build_corpus(tmp_path)
    result = wiring.query_corpus(tmp_path, "alpha")
    assert result["count"] >= 1
    assert any("alpha" in (m["label"] or "").lower() for m in result["matches"])


def test_corpus_status_before_build(tmp_path):
    status = wiring.corpus_status(tmp_path)
    assert status["enabled"] is True
    assert status["verify_gates"] is False
    assert status["node_count"] == 0
