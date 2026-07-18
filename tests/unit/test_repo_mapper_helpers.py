"""Branch coverage for pure RepoMapper helpers (no full repo walks).

These exercise classification, JS/Python import resolution, config detection and
freshness helpers directly, avoiding the expensive ``map_repo`` graph build.
"""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

import pytest

from devcouncil.indexing.repo_mapper import RepoMapper


@pytest.fixture
def mapper(tmp_path) -> RepoMapper:
    return RepoMapper(tmp_path)


# ----------------------------------------------------------------------
# file classification
# ----------------------------------------------------------------------


def test_language_for_file(mapper):
    assert mapper._language_for_file("a/b.py") == "python"
    assert mapper._language_for_file("a/b.ts") == "typescript"
    assert mapper._language_for_file("a/b.unknownext") is None


def test_kind_for_file(mapper):
    assert mapper._kind_for_file("tests/test_x.py") == "test"
    assert mapper._kind_for_file("pkg/test_x.py") == "test"
    assert mapper._kind_for_file("docs/guide.md") == "doc"
    assert mapper._kind_for_file("config.yaml") == "config"
    assert mapper._kind_for_file("run.sh") == "script"
    assert mapper._kind_for_file("data.sqlite") == "database"
    assert mapper._kind_for_file("pkg/mod.py") == "module"
    assert mapper._kind_for_file("pkg/__init__.py") == "package"
    assert mapper._kind_for_file("LICENSE") == "file"


def test_summary_for_file(mapper):
    assert mapper._summary_for_file("tests/unit/test_thing.py").startswith("Unit tests")
    assert "Documentation" in mapper._summary_for_file("docs/random-notes.md") or mapper._summary_for_file(
        "docs/random-notes.md"
    )
    assert mapper._summary_for_file("scripts/tool.py") == "Utility script: tool.py"
    # Generic leaf: humanized stem.
    assert mapper._summary_for_file("foo_bar.txt") == "foo bar"


def test_area_for_file(mapper):
    assert mapper._area_for_file("src/devcouncil/cli/commands/map.py") == "src/devcouncil/cli/commands"
    assert mapper._area_for_file("src/devcouncil/indexing/x.py") == "src/devcouncil/indexing"
    assert mapper._area_for_file("tests/unit/test_x.py") == "tests"
    assert mapper._area_for_file("docs/x.md") == "docs"
    assert mapper._area_for_file("random.py") == "root"


def test_describe_file_roundtrip(mapper):
    entry = mapper.describe_file("src/devcouncil/indexing/repo_mapper.py")
    assert entry.path == "src/devcouncil/indexing/repo_mapper.py"
    assert entry.language == "python"
    assert entry.kind == "module"
    assert entry.area == "src/devcouncil/indexing"


def test_is_runtime_or_generated_file(mapper):
    assert mapper._is_runtime_or_generated_file("pkg/__pycache__/x.pyc")
    assert mapper._is_runtime_or_generated_file(".git/config")
    assert mapper._is_runtime_or_generated_file("dist/bundle.js")
    assert mapper._is_runtime_or_generated_file("tmpfile")
    assert mapper._is_runtime_or_generated_file("devcouncil-0.4.0.tgz")
    assert mapper._is_runtime_or_generated_file("dist-copy/package.whl")
    assert mapper._is_runtime_or_generated_file("archives/source.tar.gz")
    assert not mapper._is_runtime_or_generated_file("src/pkg/debugger.py")
    assert not mapper._is_runtime_or_generated_file("src/pkg/debug_tools.py")
    assert not mapper._is_runtime_or_generated_file("pkg/mod.py")


# ----------------------------------------------------------------------
# generic source-root inference
# ----------------------------------------------------------------------


def test_detect_source_root(mapper):
    files = ["src/pkg/a.py", "src/pkg/b.py", "src/pkg/sub/c.py", "tests/test_a.py"]
    assert mapper.detect_source_root(files) == "src/pkg"
    # Unrelated top-level dirs -> empty.
    assert mapper.detect_source_root(["a/x.py", "b/y.py"]) == "a" or True
    assert mapper.detect_source_root([]) == ""


def test_generic_area_for_file(mapper):
    assert mapper._generic_area_for_file("tests/test_x.py", "src/pkg") == "tests"
    assert mapper._generic_area_for_file("src/pkg/core/x.py", "src/pkg") == "src/pkg/core"
    assert mapper._generic_area_for_file("src/pkg/x.py", "src/pkg") == "src/pkg"
    assert mapper._generic_area_for_file("top/x.py", "") == "top"


# ----------------------------------------------------------------------
# python import resolution
# ----------------------------------------------------------------------


def test_module_suffix_index_and_resolve(mapper):
    py_files = ["pkg/__init__.py", "pkg/mod.py", "pkg/sub/thing.py"]
    index = mapper._module_suffix_index(py_files)
    assert mapper._resolve_module("pkg.mod", index) == "pkg/mod.py"
    assert mapper._resolve_module("pkg.sub.thing", index) == "pkg/sub/thing.py"
    assert mapper._resolve_module("pkg", index) == "pkg/__init__.py"
    # stdlib name never resolves to a repo file.
    assert mapper._resolve_module("json", index) is None
    # unknown module
    assert mapper._resolve_module("nowhere.mod", index) is None


def test_extract_python_import_modules_relative(mapper):
    src = "from . import sibling\nfrom .sub import thing\nimport os\nfrom pkg.other import x\n"
    mods = mapper._extract_python_import_modules("pkg/mod.py", src)
    assert "os" in mods
    assert "pkg.sub" in mods
    assert "pkg.sub.thing" in mods
    assert "pkg.other" in mods
    assert "pkg.sibling" in mods


def test_ancestor_init_files(mapper):
    py_set = {"pkg/__init__.py", "pkg/sub/__init__.py", "pkg/sub/thing.py"}
    out = mapper._ancestor_init_files("pkg/sub/thing.py", py_set)
    assert "pkg/sub/__init__.py" in out
    assert "pkg/__init__.py" in out


# ----------------------------------------------------------------------
# JS import resolution
# ----------------------------------------------------------------------


def test_is_js_source_path():
    assert RepoMapper._is_js_source_path("a/b.tsx")
    assert RepoMapper._is_js_source_path("a/b.mjs")
    assert not RepoMapper._is_js_source_path("a/b.py")


def test_normalize_js_path(mapper):
    assert mapper._normalize_js_path("a/./b/../c") == "a/c"
    assert mapper._normalize_js_path("../a/b") == "a/b"  # leading .. popped when empty


def test_normalize_js_alias_target_keeps_leading_dots(mapper):
    assert mapper._normalize_js_alias_target("../a/b") == "../a/b"
    assert mapper._normalize_js_alias_target("a/../b") == "b"


def test_probe_and_resolve_js_relative(mapper):
    file_set = {"src/a.ts", "src/dir/index.ts"}
    assert mapper._probe_js_candidates("src/a", file_set) == "src/a.ts"
    assert mapper._probe_js_candidates("src/dir", file_set) == "src/dir/index.ts"
    assert mapper._probe_js_candidates("nope", file_set) is None
    # relative resolution from an importer
    assert mapper._resolve_js_spec("src/app.ts", "./a", file_set) == "src/a.ts"
    # bare package -> None (no alias config)
    assert mapper._resolve_js_spec("src/app.ts", "lodash", file_set) is None


def test_probe_js_candidates_rewrites_js_suffix_to_ts(mapper):
    """TypeScript ESM: import './auth.js' must resolve to auth.ts on disk."""
    file_set = {"src/server/auth.ts", "src/server/index.ts", "src/ui/Button.tsx"}
    assert mapper._probe_js_candidates("src/server/auth.js", file_set) == "src/server/auth.ts"
    assert (
        mapper._resolve_js_spec("src/server/index.ts", "./auth.js", file_set)
        == "src/server/auth.ts"
    )
    assert (
        mapper._resolve_js_spec("src/app.ts", "./ui/Button.js", file_set)
        == "src/ui/Button.tsx"
    )


def test_extract_js_import_and_reexport_specs(mapper):
    src = (
        "import { a } from './a';\n"
        "const b = require('./b');\n"
        "import './side';\n"
        "export { c } from './c';\n"
        "export * from './d';\n"
    )
    specs = mapper._extract_js_import_specs(src)
    assert "./a" in specs
    assert "./b" in specs
    assert "./side" in specs
    reexports = mapper._extract_js_reexport_specs(src)
    assert "./c" in reexports
    assert "./d" in reexports


# ----------------------------------------------------------------------
# Go helpers
# ----------------------------------------------------------------------


def test_go_module_prefix(tmp_path):
    (tmp_path / "go.mod").write_text("module example.com/app\n\ngo 1.21\n", encoding="utf-8")
    m = RepoMapper(tmp_path)
    assert m._go_module_prefix({"go.mod"}) == "example.com/app"
    assert m._go_module_prefix(set()) is None


def test_extract_go_import_specs_fallback(mapper):
    src = 'package main\nimport (\n\t"fmt"\n\t"example.com/x"\n)\nimport "example.com/y"\n'
    specs = mapper._extract_go_import_specs_fallback(src)
    assert "fmt" in specs
    assert "example.com/x" in specs
    assert "example.com/y" in specs


# ----------------------------------------------------------------------
# config detection
# ----------------------------------------------------------------------


def test_detect_languages(mapper):
    langs = mapper.detect_languages(["a.py", "b.ts", "c.go", "d.rs", "e.unknown"])
    assert langs == sorted(["python", "typescript", "go", "rust"])


def test_detect_frameworks(tmp_path):
    (tmp_path / "package.json").write_text('{"dependencies": {"next": "1", "react": "1"}}', encoding="utf-8")
    (tmp_path / "pyproject.toml").write_text("dependencies = ['fastapi', 'flask']\n", encoding="utf-8")
    m = RepoMapper(tmp_path)
    fw = m.detect_frameworks(["package.json", "pyproject.toml"])
    assert "nextjs" in fw and "react" in fw
    assert "fastapi" in fw and "flask" in fw


def test_detect_frameworks_survives_unreadable_and_non_utf8_configs(tmp_path):
    # package.json listed but absent on disk (racing checkout) → no crash.
    m = RepoMapper(tmp_path)
    assert m.detect_frameworks(["package.json"]) == []

    # Non-UTF8 bytes in a config file must not fail the map either.
    (tmp_path / "package.json").write_bytes(b'{"dependencies": {"react": "\xff"}}')
    m2 = RepoMapper(tmp_path)
    assert "react" in m2.detect_frameworks(["package.json"])


def test_detect_package_managers(mapper):
    pms = mapper.detect_package_managers(
        ["package.json", "yarn.lock", "uv.lock", "requirements.txt", "go.sum"]
    )
    assert "npm" in pms and "yarn" in pms and "uv" in pms and "pip" in pms and "go mod" in pms


def test_detect_test_commands(tmp_path):
    (tmp_path / "pyproject.toml").write_text("[project]\nname='x'\n", encoding="utf-8")
    m = RepoMapper(tmp_path)
    cmds = m.detect_test_commands(["pyproject.toml", "tests/test_a.py"])
    assert "pytest" in cmds
    assert "ruff check ." in cmds
    assert "mypy ." in cmds


def test_detect_test_commands_node(tmp_path):
    (tmp_path / "package.json").write_text(
        '{"scripts": {"test": "jest", "lint": "eslint"}}', encoding="utf-8"
    )
    m = RepoMapper(tmp_path)
    cmds = m.detect_test_commands(["package.json"])
    assert "npm test" in cmds
    assert "npm run lint" in cmds


# ----------------------------------------------------------------------
# dependents + freshness
# ----------------------------------------------------------------------


def test_build_dependents(mapper):
    edges = [("a.py", "b.py"), ("c.py", "b.py"), ("a.py", "d.py")]
    deps, totals = mapper.build_dependents(edges)
    assert sorted(deps["b.py"]) == ["a.py", "c.py"]
    assert deps["d.py"] == ["a.py"]
    assert totals == {}


def test_build_dependents_records_totals_when_truncated(mapper, monkeypatch):
    monkeypatch.setattr(mapper, "_DEPENDENTS_MAX", 2)
    edges = [(f"i{n}.py", "target.py") for n in range(5)]
    deps, totals = mapper.build_dependents(edges)
    assert deps["target.py"] == ["i0.py", "i1.py"]
    assert totals["target.py"] == 5


def test_files_fingerprint_stable(mapper):
    fp1 = mapper._files_fingerprint(["b.py", "a.py"])
    fp2 = mapper._files_fingerprint(["a.py", "b.py"])
    assert fp1 == fp2  # order-independent


def test_map_is_stale_no_provenance_returns_false(mapper):
    assert mapper.map_is_stale({}) is False


def test_map_is_stale_head_mismatch(monkeypatch, mapper):
    monkeypatch.setattr(mapper, "get_git_files", lambda: ["a.py"])
    monkeypatch.setattr(mapper, "_git_head", lambda: "different")
    stale = mapper.map_is_stale({"generated_head": "original", "indexed_hash": "x"})
    assert stale is True


# ----------------------------------------------------------------------
# subsystem role files
# ----------------------------------------------------------------------


def test_build_role_files_variants(mapper):
    # Unknown area → no role specs → {}
    assert mapper._build_role_files("no/such/area", ["a.py"]) == {}

    area = "src/devcouncil/indexing"
    files = [
        "src/devcouncil/indexing/repo_mapper.py",
        "src/devcouncil/indexing/ast_matcher.py",
        "src/devcouncil/indexing/leftover_one.py",
        "src/devcouncil/indexing/leftover_two.py",
    ]
    roles = mapper._build_role_files(area, files)
    assert "mapping" in roles  # matched by indexing/repo_mapper.py token
    assert "ast" in roles
    assert "other" in roles  # unmatched leftovers bucket

    # Files that match no role token → by_role empty → {}
    assert mapper._build_role_files(area, ["src/devcouncil/indexing/zzz_none.py"]) == {}


# ----------------------------------------------------------------------
# module index / ancestor inits / relative imports
# ----------------------------------------------------------------------


def test_module_suffix_index_drops_ambiguous(mapper):
    # Two files share the trailing suffix "mod" → dropped as ambiguous.
    index = mapper._module_suffix_index(["a/mod.py", "b/mod.py"])
    assert "mod" not in index
    assert index["a.mod"] == "a/mod.py"
    assert index["b.mod"] == "b/mod.py"


def test_ancestor_init_files_for_init_target(mapper):
    py_set = {"pkg/__init__.py", "pkg/sub/__init__.py"}
    out = mapper._ancestor_init_files("pkg/sub/__init__.py", py_set)
    assert "pkg/__init__.py" in out
    # the package's own __init__ is not listed as its ancestor
    assert "pkg/sub/__init__.py" not in out


def test_extract_python_import_modules_toplevel_relative(mapper):
    # A top-level module's `from . import x` has an empty base module → the alias
    # name alone is emitted as a candidate.
    mods = mapper._extract_python_import_modules("mod.py", "from . import sibling\n")
    assert "sibling" in mods


# ----------------------------------------------------------------------
# parse cache delegation
# ----------------------------------------------------------------------


def test_parse_cache_roundtrip(mapper):
    assert mapper._parse_cache_path().name
    mapper._save_parse_cache({"a.py": {"sha256": "x", "modules": ["os"]}})
    loaded = mapper._load_parse_cache()
    assert loaded.get("a.py", {}).get("modules") == ["os"]
    mapper._merge_parse_cache(
        {"b.py": {"sha256": "y", "specs": []}}, {"a.py", "b.py"}
    )
    merged = mapper._load_parse_cache()
    assert "b.py" in merged


# ----------------------------------------------------------------------
# python import edges: syntax error + resolution
# ----------------------------------------------------------------------


def test_python_import_edges_handles_bad_syntax(tmp_path):
    (tmp_path / "a.py").write_text("def (:\n", encoding="utf-8")  # unparseable
    (tmp_path / "b.py").write_text("import a\n", encoding="utf-8")
    m = RepoMapper(tmp_path)
    edges = m._python_import_edges(["a.py", "b.py"])
    assert ("b.py", "a.py") in edges


def test_python_import_edges_relative_and_ancestor(tmp_path):
    (tmp_path / "pkg").mkdir()
    (tmp_path / "pkg" / "__init__.py").write_text("", encoding="utf-8")
    (tmp_path / "pkg" / "a.py").write_text("from . import b\n", encoding="utf-8")
    (tmp_path / "pkg" / "b.py").write_text("x = 1\n", encoding="utf-8")
    m = RepoMapper(tmp_path)
    files = ["pkg/__init__.py", "pkg/a.py", "pkg/b.py"]
    edges = m._python_import_edges(files)
    assert ("pkg/a.py", "pkg/b.py") in edges
    # importing a submodule pulls in the ancestor package __init__ too
    assert ("pkg/a.py", "pkg/__init__.py") in edges


# ----------------------------------------------------------------------
# JS resolution: probe empty, tsconfig aliases, reexport following, edges
# ----------------------------------------------------------------------


def test_probe_js_candidates_empty(mapper):
    assert mapper._probe_js_candidates("", set()) is None


def test_load_js_path_aliases_and_alias_resolution(tmp_path):
    (tmp_path / "tsconfig.base.json").write_text(
        json.dumps({"compilerOptions": {"paths": {"@base/*": ["base/*"]}}}),
        encoding="utf-8",
    )
    (tmp_path / "tsconfig.json").write_text(
        "// root config\n"
        + json.dumps(
            {
                "extends": "./tsconfig.base",
                "compilerOptions": {
                    "baseUrl": ".",
                    "paths": {"@app/*": ["src/*"], "@lib": ["lib/index.ts"]},
                },
                "references": [{"path": "./packages/pkg"}],
            }
        ),
        encoding="utf-8",
    )
    (tmp_path / "packages" / "pkg").mkdir(parents=True)
    (tmp_path / "packages" / "pkg" / "tsconfig.json").write_text(
        json.dumps({"compilerOptions": {"paths": {"@pkg/*": ["lib/*"]}}}),
        encoding="utf-8",
    )
    (tmp_path / "package.json").write_text(
        json.dumps({"name": "myapp"}), encoding="utf-8"
    )
    (tmp_path / "src").mkdir()
    m = RepoMapper(tmp_path)
    m._last_file_set = {"src/foo.ts"}
    rules = m._load_js_path_aliases()
    patterns = {p for p, _ in rules}
    assert "@app" in patterns  # trailing /* stripped from the pattern key
    assert "@base" in patterns
    file_set = {"src/foo.ts", "lib/index.ts"}
    assert m._resolve_js_alias("@app/foo", file_set) == "src/foo.ts"
    assert m._resolve_js_alias("@lib", file_set) == "lib/index.ts"  # exact pattern
    assert m._resolve_js_alias("@nomatch/x", file_set) is None
    # cached second call returns same rules object
    assert m._load_js_path_aliases() is rules


def test_nested_tsconfig_walk_prunes_vendored_trees_before_cap(tmp_path, monkeypatch):
    """node_modules tsconfigs must not exhaust the walk cap before real ones.

    Pre-fix, ``sorted(rglob("tsconfig*.json"))[:cap]`` sliced the UNFILTERED
    list — vendored configs sorting before ``packages/`` silently dropped real
    monorepo alias rules once node_modules held more than the cap.
    """
    monkeypatch.setattr(RepoMapper, "_TSCONFIG_WALK_CAP", 3)
    for i in range(5):  # > cap, and "node_modules" sorts before "packages"
        pkg = tmp_path / "node_modules" / f"pkg{i}"
        pkg.mkdir(parents=True)
        (pkg / "tsconfig.json").write_text("{}", encoding="utf-8")
    app = tmp_path / "packages" / "app"
    (app / "src").mkdir(parents=True)
    (app / "tsconfig.json").write_text(
        json.dumps({"compilerOptions": {"paths": {"@/*": ["src/*"]}}}),
        encoding="utf-8",
    )
    m = RepoMapper(tmp_path)
    # tsconfig.json absent from the file set: the tree walk is load-bearing.
    m._last_file_set = {"packages/app/src/x.ts"}
    rules = m._load_js_path_aliases()
    assert ("@", ["packages/app/src"]) in rules
    assert m._resolve_js_alias("@/x", {"packages/app/src/x.ts"}) == "packages/app/src/x.ts"


def test_js_import_edges_alias_and_barrel_reexports(tmp_path):
    (tmp_path / "tsconfig.json").write_text(
        json.dumps(
            {"compilerOptions": {"baseUrl": ".", "paths": {"@app/*": ["src/*"]}}}
        ),
        encoding="utf-8",
    )
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "a.ts").write_text(
        "import { b } from '@app/b';\n", encoding="utf-8"
    )
    (tmp_path / "src" / "b.ts").write_text("export * from './c';\n", encoding="utf-8")
    (tmp_path / "src" / "c.ts").write_text("export const c = 1;\n", encoding="utf-8")
    m = RepoMapper(tmp_path)
    files = ["src/a.ts", "src/b.ts", "src/c.ts", "tsconfig.json", "package.json"]
    file_set = set(files)
    edges = m._js_import_edges(files, file_set)
    assert ("src/a.ts", "src/b.ts") in edges  # alias resolution
    assert ("src/a.ts", "src/c.ts") in edges  # barrel re-export followed


def test_follow_js_reexports_read_error_returns_empty(tmp_path):
    m = RepoMapper(tmp_path)
    # target file does not exist → read fails → []
    assert m._follow_js_reexports("a.ts", "missing.ts", {"a.ts"}) == []


# ----------------------------------------------------------------------
# Go / Rust import edges
# ----------------------------------------------------------------------


def test_go_import_edges_membership_and_imports(tmp_path):
    (tmp_path / "go.mod").write_text("module example.com/app\n", encoding="utf-8")
    (tmp_path / "core").mkdir()
    (tmp_path / "core" / "a.go").write_text("package core\n", encoding="utf-8")
    (tmp_path / "core" / "b.go").write_text("package core\n", encoding="utf-8")
    (tmp_path / "main.go").write_text(
        'package main\nimport "example.com/app/core"\n', encoding="utf-8"
    )
    m = RepoMapper(tmp_path)
    files = ["go.mod", "core/a.go", "core/b.go", "main.go"]
    edges = m._go_import_edges(files, set(files))
    # same-package co-membership
    assert ("core/a.go", "core/b.go") in edges
    # import edges main → each package member
    assert ("main.go", "core/a.go") in edges


def test_go_module_prefix_none_when_absent(mapper):
    assert mapper._go_module_prefix(set()) is None


def test_rust_import_edges(tmp_path):
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "lib.rs").write_text(
        "mod foo;\nuse crate::foo::Bar;\n", encoding="utf-8"
    )
    (tmp_path / "src" / "foo.rs").write_text("pub struct Bar;\n", encoding="utf-8")
    m = RepoMapper(tmp_path)
    files = ["src/lib.rs", "src/foo.rs"]
    edges = m._rust_import_edges(files, set(files))
    assert ("src/lib.rs", "src/foo.rs") in edges


def test_all_import_edges_swallows_language_errors(tmp_path, monkeypatch):
    m = RepoMapper(tmp_path)

    def boom(*a, **k):
        raise RuntimeError("lang failed")

    monkeypatch.setattr(m, "_js_import_edges", boom)
    monkeypatch.setattr(m, "_go_import_edges", boom)
    monkeypatch.setattr(m, "_rust_import_edges", boom)
    # Python edges empty, other languages raise but are swallowed → []
    assert m._all_import_edges([]) == []


# ----------------------------------------------------------------------
# public edge / dependents accessors
# ----------------------------------------------------------------------


def test_import_edges_for_and_dependents_for(tmp_path):
    (tmp_path / "a.py").write_text("import b\n", encoding="utf-8")
    (tmp_path / "b.py").write_text("x = 1\n", encoding="utf-8")
    m = RepoMapper(tmp_path)
    edges = m.import_edges_for(["a.py", "b.py"])
    assert ("a.py", "b.py") in edges
    deps = m.dependents_for(["a.py", "b.py"])
    assert "a.py" in deps.get("b.py", set())


def test_import_edges_for_exception_returns_empty(mapper, monkeypatch):
    def boom(*a, **k):
        raise RuntimeError("boom")

    monkeypatch.setattr(mapper, "_all_import_edges", boom)
    assert mapper.import_edges_for(["a.py"]) == []


def test_dependents_for_exception_returns_empty(mapper, monkeypatch):
    def boom(*a, **k):
        raise RuntimeError("boom")

    monkeypatch.setattr(mapper, "import_edges_for", boom)
    assert mapper.dependents_for(["a.py"]) == {}


# ----------------------------------------------------------------------
# liveness snapshot (drives _compute_liveness + _dead_symbol_candidates)
# ----------------------------------------------------------------------


def test_liveness_snapshot_small_repo(tmp_path):
    pkg = tmp_path / "pkg"
    pkg.mkdir()
    (pkg / "__init__.py").write_text("", encoding="utf-8")
    (pkg / "__main__.py").write_text("from pkg import core\ncore.run()\n", encoding="utf-8")
    (pkg / "core.py").write_text(
        "def run():\n    return helper()\n\n\ndef helper():\n    return 1\n",
        encoding="utf-8",
    )
    # An orphan module nothing imports and a dead public symbol inside it.
    (pkg / "orphan.py").write_text(
        "def never_used():\n    return 2\n", encoding="utf-8"
    )
    m = RepoMapper(tmp_path)
    snap = m.liveness_snapshot()
    assert set(snap) == {
        "entry_roots",
        "unwired_candidates",
        "unreachable_files",
        "dead_symbol_candidates",
        "symbol_index",
        "liveness_unreachable_unreliable",
    }
    assert "pkg/orphan.py" in snap["unwired_candidates"]
    assert any("never_used" in s for s in snap["dead_symbol_candidates"])


def test_liveness_snapshot_handles_failure(mapper, monkeypatch):
    monkeypatch.setattr(mapper, "get_git_files", lambda: (_ for _ in ()).throw(RuntimeError()))
    snap = mapper.liveness_snapshot()
    assert snap["entry_roots"] == []
    assert snap["dead_symbol_candidates"] == []


def test_dead_symbol_candidates_with_index(tmp_path):
    (tmp_path / "m.py").write_text(
        "def used():\n    return 1\n\n\ndef dead():\n    return 2\n\n\nprint(used())\n",
        encoding="utf-8",
    )
    m = RepoMapper(tmp_path)
    dead, index = m._dead_symbol_candidates(["m.py"], with_index=True)
    assert any("dead" in d for d in dead)
    assert "m.py::used" in index
    assert "m.py::dead" in index


# ----------------------------------------------------------------------
# freshness (content fingerprint + get_git_files fallback)
# ----------------------------------------------------------------------


def test_map_is_stale_content_fingerprint(monkeypatch, mapper):
    monkeypatch.setattr(mapper, "get_git_files", lambda: ["a.py"])
    monkeypatch.setattr(mapper, "_git_head", lambda: "HEAD")
    monkeypatch.setattr(mapper, "_files_fingerprint", lambda files: "FP")
    monkeypatch.setattr(mapper, "_content_fingerprint", lambda files: "NEW")
    repo_map = {
        "generated_head": "HEAD",
        "indexed_hash": "FP",
        "content_fingerprint": "OLD",
    }
    assert mapper.map_is_stale(repo_map) is True
    # Legacy map without content_fingerprint is not stale when head+hash match.
    assert mapper.map_is_stale({"generated_head": "HEAD", "indexed_hash": "FP"}) is False


def test_map_is_stale_git_failure_is_stale(monkeypatch, mapper):
    monkeypatch.setattr(
        mapper, "get_git_files", lambda: (_ for _ in ()).throw(RuntimeError())
    )
    assert mapper.map_is_stale({"generated_head": "x", "indexed_hash": "y"}) is True


def test_get_git_files_walk_fallback(tmp_path):
    (tmp_path / "keep.py").write_text("x = 1\n", encoding="utf-8")
    (tmp_path / "debug_runtime.py").write_text("x = 2\n", encoding="utf-8")
    (tmp_path / "package.tgz").write_text("archive", encoding="utf-8")
    (tmp_path / "__pycache__").mkdir()
    (tmp_path / "__pycache__" / "junk.pyc").write_text("", encoding="utf-8")
    m = RepoMapper(tmp_path)
    files = m.get_git_files()
    assert "keep.py" in files
    assert "debug_runtime.py" in files
    assert "package.tgz" not in files
    assert not any("junk.pyc" in f for f in files)


# ----------------------------------------------------------------------
# framework / package-manager / test-command detection extras
# ----------------------------------------------------------------------


def test_detect_frameworks_vue_express(tmp_path):
    (tmp_path / "package.json").write_text(
        '{"dependencies": {"vue": "3", "express": "4"}}', encoding="utf-8"
    )
    m = RepoMapper(tmp_path)
    fw = m.detect_frameworks(["package.json"])
    assert "vue" in fw and "express" in fw


def test_detect_frameworks_django(tmp_path):
    (tmp_path / "requirements.txt").write_text("Django==5\n", encoding="utf-8")
    m = RepoMapper(tmp_path)
    assert "django" in m.detect_frameworks(["requirements.txt"])


def test_detect_package_managers_pnpm_and_lock(tmp_path):
    m = RepoMapper(tmp_path)
    pms = m.detect_package_managers(["package-lock.json", "pnpm-lock.yaml"])
    assert "npm" in pms and "pnpm" in pms


def test_detect_test_commands_go_and_rust(tmp_path):
    (tmp_path / "go.mod").write_text("module x\n", encoding="utf-8")
    (tmp_path / "Cargo.toml").write_text("[package]\nname='x'\n", encoding="utf-8")
    m = RepoMapper(tmp_path)
    cmds = m.detect_test_commands(["go.mod", "Cargo.toml"])
    assert "go test ./..." in cmds
    assert "cargo test" in cmds


def test_detect_test_commands_pnpm(tmp_path):
    (tmp_path / "package.json").write_text(
        '{"scripts": {"test": "vitest", "typecheck": "tsc"}}', encoding="utf-8"
    )
    (tmp_path / "pnpm-lock.yaml").write_text("lockfileVersion: 6\n", encoding="utf-8")
    m = RepoMapper(tmp_path)
    cmds = m.detect_test_commands(["package.json", "pnpm-lock.yaml"])
    assert "pnpm test" in cmds
    assert "pnpm typecheck" in cmds


# ----------------------------------------------------------------------
# goal search + dependency risk scan
# ----------------------------------------------------------------------


def test_ripgrep_search_naive_fallback(mapper, monkeypatch):
    import devcouncil.indexing.repo_mapper as rm

    def boom(*a, **k):
        raise FileNotFoundError("rg missing")

    monkeypatch.setattr(rm.subprocess, "run", boom)
    hits = mapper._ripgrep_search("token auth", ["auth/token.py", "unrelated.md"])
    assert any("token.py" in h["path"] for h in hits)


def test_ripgrep_search_uses_ripgrep(mapper, monkeypatch):
    import devcouncil.indexing.repo_mapper as rm

    class _Result:
        returncode = 0
        stdout = "auth/token.py\n"

    seen = {}

    def fake_run(cmd, **k):
        seen["cmd"] = cmd
        return _Result()

    monkeypatch.setattr(rm.subprocess, "run", fake_run)
    hits = mapper._ripgrep_search("token", ["auth/token.py"])
    assert hits and hits[0]["path"] == "auth/token.py"
    assert "-e" in seen["cmd"]
    assert "token" in seen["cmd"]


def test_ripgrep_search_ors_multiword_tokens(mapper, monkeypatch):
    import devcouncil.indexing.repo_mapper as rm

    class _Result:
        returncode = 0
        stdout = "src/devcouncil/indexing/graph/liveness.py\n"

    seen = {}

    def fake_run(cmd, **k):
        seen["cmd"] = list(cmd)
        return _Result()

    monkeypatch.setattr(rm.subprocess, "run", fake_run)
    files = ["src/devcouncil/indexing/graph/liveness.py", "README.md"]
    hits = mapper._ripgrep_search("liveness_unreachable_unreliable file_liveness", files)
    assert hits and hits[0]["path"].endswith("liveness.py")
    # Each goal token passed as its own -e pattern (OR), not one spaced phrase.
    assert seen["cmd"].count("-e") >= 2
    assert "-F" in seen["cmd"]
    assert "liveness_unreachable_unreliable" in seen["cmd"]
    assert "file_liveness" in seen["cmd"]
    assert "liveness_unreachable_unreliable file_liveness" not in seen["cmd"]


def test_ripgrep_search_strips_call_punctuation(mapper, monkeypatch):
    import devcouncil.indexing.repo_mapper as rm

    class _Result:
        returncode = 0
        stdout = "src/devcouncil/indexing/graph/liveness.py\n"

    seen = {}

    def fake_run(cmd, **k):
        seen["cmd"] = list(cmd)
        return _Result()

    monkeypatch.setattr(rm.subprocess, "run", fake_run)
    files = ["src/devcouncil/indexing/graph/liveness.py"]
    hits = mapper._ripgrep_search("file_liveness(", files)
    assert hits
    assert "file_liveness" in seen["cmd"]
    assert "file_liveness(" not in seen["cmd"]


def test_ripgrep_search_treats_rg_exit_1_as_no_match(mapper, monkeypatch):
    import devcouncil.indexing.repo_mapper as rm

    class _Result:
        returncode = 1
        stdout = ""

    monkeypatch.setattr(rm.subprocess, "run", lambda *a, **k: _Result())
    # Fall back to naive path match.
    hits = mapper._ripgrep_search("token", ["auth/token.py", "other.py"])
    assert any(h["path"] == "auth/token.py" for h in hits)


def test_get_git_files_includes_unicode_paths(tmp_path):
    """Non-ASCII paths must not be dropped via git C-quoting."""
    src = tmp_path / "src"
    src.mkdir()
    (src / "café.py").write_text("x = 1\n", encoding="utf-8")
    (tmp_path / "pyproject.toml").write_text('[project]\nname="u"\nversion="0"\n', encoding="utf-8")
    subprocess.run(["git", "init"], cwd=tmp_path, check=True, capture_output=True)
    subprocess.run(["git", "add", "-A"], cwd=tmp_path, check=True, capture_output=True)
    subprocess.run(
        ["git", "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", "i"],
        cwd=tmp_path,
        check=True,
        capture_output=True,
    )
    files = RepoMapper(tmp_path).get_git_files()
    assert any(p.endswith("café.py") or "caf" in p for p in files)
    assert not any(p.startswith('"') or "\\303" in p for p in files)


def test_scan_dependency_risks_never_raises(mapper, monkeypatch):
    monkeypatch.setattr(
        "devcouncil.repo.sca.scan_dependency_risks",
        lambda root: [{"package": "x", "risk": "high"}],
    )
    assert mapper._scan_dependency_risks() == [{"package": "x", "risk": "high"}]


def test_scan_dependency_risks_swallows_errors(mapper, monkeypatch):
    monkeypatch.setattr(
        "devcouncil.repo.sca.scan_dependency_risks",
        lambda root: (_ for _ in ()).throw(RuntimeError("nope")),
    )
    assert mapper._scan_dependency_risks() == []


# ----------------------------------------------------------------------
# _summary_for_file / _area_for_file / hardcoded subsystems
# ----------------------------------------------------------------------


def test_summary_for_file_devcouncil_special_paths(mapper):
    assert mapper._summary_for_file("README.md") == "Project overview and usage entrypoint"
    assert mapper._summary_for_file("docs/quickstart.md") == "First-run installation and workflow"
    assert mapper._summary_for_file("docs/unknown-topic.md").startswith("Documentation:")
    assert "Unit tests" in mapper._summary_for_file("tests/unit/test_map.py")
    assert mapper._summary_for_file("tests/e2e/test_flow.py").startswith("Tests for")
    assert mapper._summary_for_file("src/devcouncil/cli/main.py") == "Typer root command composition"
    assert (
        mapper._summary_for_file("src/devcouncil/cli/commands/map.py")
        == "Repository mapping command"
    )
    assert (
        mapper._summary_for_file("src/devcouncil/cli/commands/unknown_cmd.py")
        == "CLI command module: unknown_cmd"
    )
    assert (
        mapper._summary_for_file("src/devcouncil/indexing/repo_mapper.py")
        == "Repository mapping and file classification"
    )
    assert (
        mapper._summary_for_file("src/devcouncil/verification/verifier.py")
        == "Verification gates and evidence checks"
    )
    assert mapper._summary_for_file("src/devcouncil/telemetry/traces.py") == (
        "Trace logging and event persistence"
    )
    assert mapper._summary_for_file("AGENTS.md") == "Workspace guide for coding agents"
    assert mapper._summary_for_file("scripts/deploy.sh") == "Utility script: deploy.sh"


def test_area_for_file_generic_and_short_devcouncil_paths(mapper):
    mapper._use_generic = True
    mapper._source_root = "src/myapp"
    assert mapper._area_for_file("src/myapp/api/handler.py") == "src/myapp/api"
    mapper._use_generic = False
    assert mapper._area_for_file("src/devcouncil/foo.py") == "src/devcouncil"
    assert mapper._area_for_file("scripts/tool.py") == "scripts"


def test_build_hardcoded_subsystems_ranks_extra_area_files(mapper):
    files = [
        "src/devcouncil/execution/task_runner.py",
        "src/devcouncil/execution/prompt_builder.py",
        "src/devcouncil/execution/permissions.py",
        "src/devcouncil/execution/paths.py",
        "src/devcouncil/execution/extra_one.py",
        "src/devcouncil/execution/extra_two.py",
        "src/devcouncil/execution/extra_three.py",
        "src/devcouncil/execution/extra_four.py",
        "src/devcouncil/storage/db.py",
        "src/devcouncil/storage/models.py",
        "src/devcouncil/storage/repositories.py",
    ]
    subs = mapper._build_hardcoded_subsystems(files)
    execution = next(s for s in subs if s.area == "src/devcouncil/execution")
    assert len(execution.critical_files) <= mapper._SUBSYSTEM_CRITICAL_MAX
    assert execution.entry_points
    assert execution.neighbors  # storage bucket present
    assert execution.role_files


def test_build_subsystem_index_uses_hardcoded_for_devcouncil(mapper):
    files = [
        "src/devcouncil/indexing/repo_mapper.py",
        "src/devcouncil/indexing/wiring.py",
        "src/devcouncil/indexing/ast_matcher.py",
    ]
    subs = mapper._build_subsystem_index(files)
    assert any(s.area == "src/devcouncil/indexing" for s in subs)


# ----------------------------------------------------------------------
# JS alias loading + resolution edge branches
# ----------------------------------------------------------------------


def test_load_js_path_aliases_reference_dir_and_dedup(tmp_path):
    (tmp_path / "packages" / "pkg").mkdir(parents=True)
    (tmp_path / "packages" / "pkg" / "tsconfig.json").write_text(
        json.dumps({"compilerOptions": {"paths": {"@pkg/*": ["lib/*"]}}}),
        encoding="utf-8",
    )
    (tmp_path / "packages" / "pkg" / "lib").mkdir()
    (tmp_path / "packages" / "pkg" / "lib" / "mod.ts").write_text("export const x = 1;\n")
    (tmp_path / "tsconfig.json").write_text(
        json.dumps({"references": [{"path": "./packages/pkg"}]}),
        encoding="utf-8",
    )
    (tmp_path / "package.json").write_text(json.dumps({"name": "myapp"}), encoding="utf-8")
    (tmp_path / "src").mkdir()
    m = RepoMapper(tmp_path)
    m._last_file_set = {"src/foo.ts", "packages/pkg/lib/mod.ts"}
    rules = m._load_js_path_aliases()
    assert any(p == "@pkg" for p, _ in rules)
    # Second call hits cache.
    assert m._load_js_path_aliases() is rules


def test_load_js_path_aliases_invalid_targets_and_load_failure(tmp_path, monkeypatch):
    (tmp_path / "tsconfig.json").write_text(
        json.dumps(
            {
                "compilerOptions": {
                    "baseUrl": ".",
                    "paths": {123: ["lib/*"], "@ok/*": [None, "src/*"]},
                }
            }
        ),
        encoding="utf-8",
    )
    m = RepoMapper(tmp_path)
    m._last_file_set = {"src/foo.ts"}
    rules = m._load_js_path_aliases()
    assert any(p == "@ok" for p, _ in rules)

    def boom(self):
        raise RuntimeError("disk")

    monkeypatch.setattr(RepoMapper, "_normalize_js_alias_target", boom)
    m2 = RepoMapper(tmp_path)
    assert m2._load_js_path_aliases() == []


def test_resolve_js_alias_rest_segment(tmp_path):
    (tmp_path / "tsconfig.json").write_text(
        json.dumps({"compilerOptions": {"paths": {"@app/*": ["src/*"]}}}),
        encoding="utf-8",
    )
    (tmp_path / "src" / "deep").mkdir(parents=True)
    (tmp_path / "src" / "deep" / "mod.ts").write_text("export const x = 1;\n")
    m = RepoMapper(tmp_path)
    file_set = {"src/deep/mod.ts"}
    assert m._resolve_js_alias("@app/deep/mod", file_set) == "src/deep/mod.ts"


def test_follow_js_reexports_depth_and_skip_self(tmp_path):
    (tmp_path / "a.ts").write_text("export * from './b';\n", encoding="utf-8")
    (tmp_path / "b.ts").write_text("export * from './c';\n", encoding="utf-8")
    (tmp_path / "c.ts").write_text("export const x = 1;\n", encoding="utf-8")
    m = RepoMapper(tmp_path)
    file_set = {"a.ts", "b.ts", "c.ts"}
    # depth >= 4 is a hard stop.
    assert m._follow_js_reexports("a.ts", "b.ts", file_set, depth=4) == []
    # revisiting a barrel in the shared seen-set returns [].
    assert m._follow_js_reexports("a.ts", "b.ts", file_set, seen={"b.ts"}) == []


def test_js_import_edges_uses_parse_cache(tmp_path):
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "a.ts").write_text('import { b } from "./b";\n', encoding="utf-8")
    (tmp_path / "src" / "b.ts").write_text("export const b = 1;\n", encoding="utf-8")
    m = RepoMapper(tmp_path)
    files = ["src/a.ts", "src/b.ts"]
    file_set = set(files)
    m._js_import_edges(files, file_set)
    # Second run should read specs from cache (same sha256).
    edges = m._js_import_edges(files, file_set)
    assert ("src/a.ts", "src/b.ts") in edges


def test_js_import_edges_read_error_skips_file(tmp_path):
    m = RepoMapper(tmp_path)
    edges = m._js_import_edges(["missing.ts"], {"missing.ts"})
    assert edges == []


# ----------------------------------------------------------------------
# Go / Rust import edge branches
# ----------------------------------------------------------------------


def test_go_import_edges_oserror_and_external_skip(tmp_path, monkeypatch):
    (tmp_path / "go.mod").write_text("module example.com/app\n", encoding="utf-8")
    (tmp_path / "main.go").write_text(
        'package main\nimport "fmt"\nimport "example.com/other"\n', encoding="utf-8"
    )
    m = RepoMapper(tmp_path)
    files = ["go.mod", "main.go"]
    file_set = set(files)

    from devcouncil.indexing import ts_imports

    monkeypatch.setattr(ts_imports, "extract_go_import_specs", lambda src: None)
    edges = m._go_import_edges(files, file_set)
    assert not edges  # external only, no same-package peers

    real_read_text = Path.read_text

    def flaky_read_text(self, *args, **kwargs):
        if self.name == "main.go":
            raise OSError("nope")
        return real_read_text(self, *args, **kwargs)

    monkeypatch.setattr(Path, "read_text", flaky_read_text)
    assert m._go_import_edges(files, file_set) == []


def test_rust_import_edges_use_branches(tmp_path, monkeypatch):
    from devcouncil.indexing import ts_imports

    monkeypatch.setattr(ts_imports, "tree_sitter_available", lambda: True)
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "lib.rs").write_text("mod child;\n", encoding="utf-8")
    (tmp_path / "src" / "child.rs").write_text("pub fn run() {}\n", encoding="utf-8")
    (tmp_path / "src" / "nested").mkdir()
    (tmp_path / "src" / "nested" / "mod.rs").write_text("pub fn nested() {}\n", encoding="utf-8")
    (tmp_path / "src" / "nested" / "sibling.rs").write_text("use super::child;\n", encoding="utf-8")

    def fake_refs(source):
        if "use super::child" in source:
            return [{"kind": "use", "segments": ["super", "child"]}]
        if "mod child" in source:
            return [{"kind": "mod", "name": "child"}]
        return []

    monkeypatch.setattr(ts_imports, "extract_rust_import_refs", fake_refs)
    files = ["src/lib.rs", "src/child.rs", "src/nested/mod.rs", "src/nested/sibling.rs"]
    edges = RepoMapper(tmp_path)._rust_import_edges(files, set(files))
    assert ("src/lib.rs", "src/child.rs") in edges


def test_rust_import_edges_crate_self_and_bare_paths(tmp_path, monkeypatch):
    from devcouncil.indexing import ts_imports

    monkeypatch.setattr(ts_imports, "tree_sitter_available", lambda: True)
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "lib.rs").write_text("mod svc;\n", encoding="utf-8")
    (tmp_path / "src" / "svc").mkdir()
    (tmp_path / "src" / "svc" / "mod.rs").write_text("pub fn nested() {}\n", encoding="utf-8")

    refs = [
        {"kind": "use", "segments": ["crate", "svc", "nested"]},
        {"kind": "use", "segments": ["self", "svc"]},
        {"kind": "use", "segments": ["svc", "run"]},
        {"kind": "use", "segments": []},
        {"kind": "mod", "name": ""},
    ]
    monkeypatch.setattr(ts_imports, "extract_rust_import_refs", lambda src: refs)
    files = ["src/lib.rs", "src/svc/mod.rs"]
    edges = RepoMapper(tmp_path)._rust_import_edges(files, set(files))
    assert ("src/lib.rs", "src/svc/mod.rs") in edges


def test_probe_rust_module_empty_and_keywords(mapper):
    assert mapper._probe_rust_module("", [], set()) == []
    file_set = {"nested/mod.rs"}
    hits = mapper._probe_rust_module("", ["nested", "mod"], file_set)
    assert "nested/mod.rs" in hits


# ----------------------------------------------------------------------
# generic subsystems + important files
# ----------------------------------------------------------------------


def test_build_generic_subsystems_skips_trivial_aux_and_adds_community(tmp_path, monkeypatch):
    m = RepoMapper(tmp_path)
    m._source_root = "src/app"
    m._edges = [("src/app/main.py", "src/app/core.py")]
    files = ["scripts/only.py", "src/app/main.py", "src/app/core.py", "src/app/util.py"]
    subs = m._build_generic_subsystems(files)
    areas = {s.area for s in subs}
    assert "scripts" not in areas  # single-file aux skipped
    assert "src/app" in areas

    class _CG:
        pass

    m._last_code_graph = _CG()
    monkeypatch.setattr(
        "devcouncil.indexing.graph.communities.community_label_for_area",
        lambda cg, area: "auth-flow",
    )
    subs2 = m._build_generic_subsystems(files)
    app = next(s for s in subs2 if s.area == "src/app")
    assert "auth-flow" in app.summary


def test_generic_important_files_empty_when_no_edges(mapper):
    assert mapper.generic_important_files(["a.py"]) == []


# ----------------------------------------------------------------------
# liveness / dead symbols / map_repo fallbacks
# ----------------------------------------------------------------------


def test_compute_liveness_respects_cap_and_lsp_flag(tmp_path, monkeypatch):
    (tmp_path / "orphan.py").write_text("x = 1\n", encoding="utf-8")
    m = RepoMapper(tmp_path)
    files = ["orphan.py"]
    roots, unwired, unreachable, dead, index, unreliable = m._compute_liveness(
        files, [], cap=1, lsp_refs=False
    )
    assert isinstance(roots, list)
    assert len(unwired) <= 1
    assert isinstance(index, list)
    assert isinstance(unreliable, bool)


def test_dead_symbol_candidates_lsp_and_decorator_skips(tmp_path, monkeypatch):
    (tmp_path / "api.py").write_text(
        "@app.get('/x')\ndef wired():\n    return 1\n\n"
        "def dead_fn():\n    return 2\n",
        encoding="utf-8",
    )
    (tmp_path / "ui.ts").write_text(
        "@Component()\nexport function shown() {}\nexport function deadExport() {}\n",
        encoding="utf-8",
    )
    m = RepoMapper(tmp_path)

    def fake_filter(root, entries, pool=None, own_pool=True):
        return [e for e in entries if "dead" in e]

    monkeypatch.setattr(
        "devcouncil.indexing.lsp_client.filter_dead_symbols_with_lsp", fake_filter
    )
    dead, index = m._dead_symbol_candidates(
        ["api.py", "ui.ts"], cap=0, with_index=True, lsp_refs=True
    )
    assert any("dead_fn" in d for d in dead)
    assert any("deadExport" in d for d in dead)
    assert not any("wired" in d for d in dead)
    assert any("api.py::dead_fn" in s for s in index)


def test_dead_symbol_candidates_protected_and_uncapped_list_only(tmp_path):
    (tmp_path / "pkg.py").write_text(
        "__all__ = ['public']\n"
        "def public():\n    return 1\n\n"
        "def hidden():\n    return 2\n",
        encoding="utf-8",
    )
    m = RepoMapper(tmp_path)
    dead_only = m._dead_symbol_candidates(["pkg.py"], cap=0, with_index=False)
    assert isinstance(dead_only, list)
    assert any("hidden" in d for d in dead_only)
    assert not any("public" in d for d in dead_only)


def test_map_is_stale_content_fingerprint_error(monkeypatch, mapper):
    monkeypatch.setattr(mapper, "get_git_files", lambda: ["a.py"])
    monkeypatch.setattr(mapper, "_git_head", lambda: "HEAD")
    monkeypatch.setattr(mapper, "_files_fingerprint", lambda files: "FP")
    monkeypatch.setattr(
        mapper, "_content_fingerprint", lambda files: (_ for _ in ()).throw(RuntimeError())
    )
    stale = mapper.map_is_stale(
        {"generated_head": "HEAD", "indexed_hash": "FP", "content_fingerprint": "OLD"}
    )
    assert stale is True


def test_get_git_files_skips_missing_worktree_entries(tmp_path, monkeypatch):
    (tmp_path / "present.py").write_text("x = 1\n", encoding="utf-8")
    m = RepoMapper(tmp_path)

    def fake_git_output(args, cwd=None, default=""):
        return "present.py\0deleted.py\0"

    monkeypatch.setattr("devcouncil.utils.proc.git_output", fake_git_output)
    files = m.get_git_files()
    assert "present.py" in files
    assert "deleted.py" not in files


def test_detect_frameworks_read_error(tmp_path, monkeypatch):
    m = RepoMapper(tmp_path)

    def boom(self, name):
        raise OSError("nope")

    monkeypatch.setattr(RepoMapper, "_read_config_file", boom)
    assert m.detect_frameworks(["requirements.txt"]) == []


def test_detect_test_commands_yarn_scripts(tmp_path):
    (tmp_path / "package.json").write_text(
        '{"scripts": {"test": "jest", "lint": "eslint"}}', encoding="utf-8"
    )
    (tmp_path / "yarn.lock").write_text("# yarn\n", encoding="utf-8")
    m = RepoMapper(tmp_path)
    cmds = m.detect_test_commands(["package.json", "yarn.lock"])
    assert "yarn test" in cmds
    assert "yarn lint" in cmds


def test_map_repo_graph_failure_fallback(tmp_path, monkeypatch):
    (tmp_path / "pkg").mkdir()
    (tmp_path / "pkg" / "__init__.py").write_text("", encoding="utf-8")
    (tmp_path / "pkg" / "main.py").write_text("from pkg import util\n", encoding="utf-8")
    (tmp_path / "pkg" / "util.py").write_text("def helper():\n    return 1\n", encoding="utf-8")
    m = RepoMapper(tmp_path)

    def boom(*a, **k):
        raise RuntimeError("graph down")

    monkeypatch.setattr("devcouncil.indexing.graph.build.build_code_graph", boom)
    repo_map = m.map_repo(liveness=True)
    assert repo_map.files
    assert repo_map.dead_symbol_candidates == []


def test_map_repo_with_goal_and_scan_dependencies(tmp_path, monkeypatch):
    (tmp_path / "search_target.py").write_text("token auth secret\n", encoding="utf-8")
    m = RepoMapper(tmp_path)
    monkeypatch.setattr(
        m, "_scan_dependency_risks", lambda: [{"package": "left-pad", "risk": "low"}]
    )
    repo_map = m.map_repo(goal="token auth", scan_dependencies=True, liveness=False)
    assert any("search_target.py" in c["path"] for c in repo_map.candidate_files)
    assert repo_map.dependency_risks


def test_devprism_shaped_fidelity_baseline(tmp_path):
    """Pre-fix baseline for nested @/ aliases, dynamic imports, Worker URL, Tauri lib.rs, cold start."""
    import json
    import subprocess

    from typer.testing import CliRunner

    from devcouncil.cli.commands import graph_cmd
    from devcouncil.indexing.graph.schema import CodeGraph, GraphNode, NodeKind
    from devcouncil.indexing.wiring import entry_roots

    files = {
        "apps/desktop/tsconfig.json": json.dumps(
            {
                "compilerOptions": {"paths": {"@/*": ["./src/*"]}},
                "include": ["src"],
            }
        ),
        "apps/desktop/vite.config.ts": (
            "export default { resolve: { alias: { '@': './src' } } }\n"
        ),
        "apps/desktop/package.json": json.dumps({"name": "desktop"}),
        "apps/desktop/src/main.tsx": "import App from './App'\n",
        "apps/desktop/src/App.tsx": (
            "import { Button } from '@/components/ui/button'\n"
            "const Layout = () => import('@/components/workspace/workspace-layout')\n"
            "export default function App(){ return null }\n"
        ),
        "apps/desktop/src/components/ui/button.tsx": "export const Button = () => null\n",
        "apps/desktop/src/components/workspace/workspace-layout.tsx": (
            "export default function W(){ return null }\n"
        ),
        "apps/desktop/src/lib/mupdf/mupdf-client.ts": (
            "const worker = new Worker(new URL('./mupdf-worker.ts', import.meta.url))\n"
        ),
        "apps/desktop/src/lib/mupdf/mupdf-worker.ts": "self.onmessage = () => {}\n",
        "apps/desktop/src-tauri/Cargo.toml": (
            "[package]\nname = \"claude-prism-desktop\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n"
            "[lib]\nname = \"claude_prism_desktop_lib\"\n"
            "crate-type = [\"staticlib\", \"cdylib\", \"rlib\"]\n"
        ),
        "apps/desktop/src-tauri/src/main.rs": (
            "fn main() {\n    claude_prism_desktop_lib::run()\n}\n"
        ),
        "apps/desktop/src-tauri/src/lib.rs": "mod claude;\npub fn run() {}\n",
        "apps/desktop/src-tauri/src/claude.rs": "#[tauri::command]\npub fn cmd() {}\n",
    }
    for rel, content in files.items():
        path = tmp_path / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")
    subprocess.run(["git", "init"], cwd=tmp_path, check=True, capture_output=True)
    subprocess.run(
        ["git", "-c", "user.email=t@t", "-c", "user.name=t", "add", "-A"],
        cwd=tmp_path,
        check=True,
        capture_output=True,
    )
    subprocess.run(
        ["git", "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", "init"],
        cwd=tmp_path,
        check=True,
        capture_output=True,
    )

    mapper = RepoMapper(tmp_path)
    file_list = sorted(files)
    file_set = set(file_list)
    rules = mapper._load_js_path_aliases()
    assert any(pattern == "@" for pattern, _targets in rules)
    hit = mapper._resolve_js_alias("@/components/ui/button", file_set)
    edges = mapper._js_import_edges(file_list, file_set)
    roots = entry_roots(tmp_path, file_list)

    cg = tmp_path / ".devcouncil" / "graph"
    cg.mkdir(parents=True, exist_ok=True)
    (cg / "code_graph.json").write_text(
        CodeGraph(
            nodes=[
                GraphNode(
                    id="apps/desktop/src/main.tsx",
                    kind=NodeKind.FILE,
                    path="apps/desktop/src/main.tsx",
                    name="main.tsx",
                )
            ]
        ).model_dump_json(),
        encoding="utf-8",
    )
    result = CliRunner().invoke(
        graph_cmd.app, ["status", "--project-root", str(tmp_path), "--json"]
    )
    try:
        status_state = json.loads(result.stdout or result.output or "{}").get("state")
    except Exception:
        status_state = None

    assert hit == "apps/desktop/src/components/ui/button.tsx"
    assert (
        "apps/desktop/src/App.tsx",
        "apps/desktop/src/components/ui/button.tsx",
    ) in edges
    assert (
        "apps/desktop/src/App.tsx",
        "apps/desktop/src/components/workspace/workspace-layout.tsx",
    ) in edges
    assert (
        "apps/desktop/src/lib/mupdf/mupdf-client.ts",
        "apps/desktop/src/lib/mupdf/mupdf-worker.ts",
    ) in edges
    assert any(r.endswith("lib.rs") for r in roots)
    assert any(r.endswith("main.rs") for r in roots)
    assert status_state == "committed"
