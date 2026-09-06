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

from devcouncil.indexing.map_artifacts import refresh_map_artifacts
from tests.unit.support_maps import stub_kernel

@pytest.fixture
def mapper(tmp_path) -> RepoMapper:
    return RepoMapper(tmp_path)


# ----------------------------------------------------------------------
# file classification
# ----------------------------------------------------------------------


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


# ----------------------------------------------------------------------
# dependents + freshness
# ----------------------------------------------------------------------


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


def test_map_repo_with_goal_and_scan_dependencies(tmp_path, monkeypatch):
    (tmp_path / "token_auth_search_target.py").write_text("token auth secret\n", encoding="utf-8")
    m = RepoMapper(tmp_path)
    monkeypatch.setattr(
        m, "_scan_dependency_risks", lambda: [{"package": "left-pad", "risk": "low"}]
    )
    monkeypatch.setattr(
        RepoMapper, "_scan_dependency_risks", lambda self: [{"package": "left-pad", "risk": "low"}]
    )
    stub_kernel(monkeypatch)
    repo_map = refresh_map_artifacts(
        m.project_root, m.project_root / ".devcouncil" / "repo_map.json", "token auth",
        scan_dependencies=True, quiet=True,
    ).repo_map
    assert any("token_auth_search_target.py" in c["path"] for c in repo_map.candidate_files)
    assert repo_map.dependency_risks


def test_devprism_shaped_fidelity_baseline(tmp_path):
    """Pre-fix baseline for nested @/ aliases, dynamic imports, Worker URL, Tauri lib.rs, cold start."""
    import json
    import subprocess


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
    # `dev map status` reports the kernel store; a hand-written code_graph.json
    # is not a kernel commit, so the old "committed" claim went with the Python engine.


def test_generated_trees_are_excluded_from_the_inventory(mapper: RepoMapper) -> None:
    """node_modules / build output / binaries must never enter the file index.

    Regression for a 136k-file, 175MB repo map: untracked trees are only as
    bounded as a repo's ignore rules, which the mapper cannot assume.
    """
    excluded = [
        "node_modules/react/index.js",
        "web/node_modules/left-pad/index.js",
        "coverage/lcov-report/index.html",
        "target/debug/build.rs",
        "vendor/github.com/pkg/errors/errors.go",
        ".gitnexus/index.db",
        "Pods/Alamofire/Source/Alamofire.swift",
        ".next/static/chunks/main.js",
        "assets/logo.png",
        "dist/bundle.min.js",
        "lib/native.so",
    ]
    for path in excluded:
        assert mapper._is_runtime_or_generated_file(path), path

    kept = [
        "src/app.py",
        # A source directory that merely happens to be named `build`.
        "src/devcouncil/indexing/graph/build.py",
        "packages/core/src/index.ts",
        "docs/architecture.md",
    ]
    for path in kept:
        assert not mapper._is_runtime_or_generated_file(path), path


def test_inventory_cap_drops_untracked_first(mapper: RepoMapper) -> None:
    tracked = [f"src/mod_{index}.py" for index in range(10)]
    untracked = [f"scratch/tmp_{index}.py" for index in range(100)]

    capped = mapper._cap_inventory(tracked, untracked, 20)

    assert len(capped) == 20
    assert set(tracked) <= set(capped)
    # Overflow came out of the untracked half, not the tracked half.
    assert len([p for p in capped if p.startswith("scratch/")]) == 10


def test_inventory_cap_below_tracked_count_truncates_tracked(mapper: RepoMapper) -> None:
    tracked = [f"src/mod_{index:03d}.py" for index in range(50)]

    capped = mapper._cap_inventory(tracked, ["scratch/x.py"], 10)

    assert len(capped) == 10
    assert all(path.startswith("src/") for path in capped)


def test_get_git_files_can_exclude_untracked(tmp_path, monkeypatch) -> None:
    subprocess.run(["git", "init", "-q"], cwd=tmp_path, check=True)
    (tmp_path / "tracked.py").write_text("x = 1\n", encoding="utf-8")
    subprocess.run(["git", "add", "tracked.py"], cwd=tmp_path, check=True)
    (tmp_path / "untracked.py").write_text("y = 2\n", encoding="utf-8")

    mapper = RepoMapper(tmp_path)
    monkeypatch.setattr(RepoMapper, "_inventory_limits", lambda self: (True, 50_000))
    assert set(mapper.get_git_files()) == {"tracked.py", "untracked.py"}

    monkeypatch.setattr(RepoMapper, "_inventory_limits", lambda self: (False, 50_000))
    assert mapper.get_git_files() == ["tracked.py"]


# ----------------------------------------------------------------------
# the inventory covers exactly what `dev map` can index
#
# `dev map`'s discovery walk honours the Cache Directory Tagging Standard: a
# directory holding a `CACHEDIR.TAG` with the standard's signature is pruned
# whole, never walked, never indexed. cargo, pip, uv, ccache, tox, ruff and
# pytest all write one, and a build cache is not source.
#
# `get_git_files` did not, so a tagged cache that no `.gitignore` happened to
# cover was skipped by the walk and counted by `_content_fingerprint`, which then
# moved on every write a build made into its own cache. `map_is_stale` answered
# True on files the map had deliberately declined to index, so `--if-stale`,
# `--watch` and `verify` rebuilt forever without converging — each rebuild
# re-stamping a fingerprint the next build would disagree with again.
#
# Measured on this workspace: `rust-port/.gitignore` carries `/target/`, which
# does not match the `target-lane*` directories beside it, and 7,465 of the 8,867
# paths `get_git_files()` returned came from them.
# ----------------------------------------------------------------------

_CACHEDIR_TAG_BODY = (
    "Signature: 8a477f597d28d172789f06886806bc55\n"
    "# This file is a cache directory tag created by a build tool.\n"
)


def _repo_with_a_tagged_cache(tmp_path, *, signature: str = _CACHEDIR_TAG_BODY):
    """One real source file plus a tagged cache directory, and **no** `.gitignore`.

    An ignored cache is invisible to `git ls-files` already and proves nothing.
    The defect lives in the gap between "the build tool declared this a cache"
    and "git was never told".
    """
    root = tmp_path / "repo"
    (root / "src").mkdir(parents=True)
    (root / "target-lane9" / "debug").mkdir(parents=True)
    (root / "src" / "app.py").write_text("def app():\n    return 1\n", encoding="utf-8")
    (root / "target-lane9" / "CACHEDIR.TAG").write_text(signature, encoding="utf-8")
    (root / "target-lane9" / "debug" / "artifact.py").write_text(
        "def artifact():\n    return 2\n", encoding="utf-8"
    )
    subprocess.run(["git", "init", "-q"], cwd=root, check=True)
    subprocess.run(["git", "config", "user.email", "t@example.invalid"], cwd=root, check=True)
    subprocess.run(["git", "config", "user.name", "t"], cwd=root, check=True)
    subprocess.run(["git", "add", "src/app.py"], cwd=root, check=True)
    subprocess.run(["git", "commit", "-qm", "one real source file"], cwd=root, check=True)
    return root


def test_a_tagged_cache_directory_is_absent_from_the_inventory(tmp_path) -> None:
    root = _repo_with_a_tagged_cache(tmp_path)

    # The premise, stated rather than assumed: git lists the cache, because
    # nothing ignores it.
    listed = subprocess.run(
        ["git", "ls-files", "--others", "--exclude-standard"],
        cwd=root,
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    assert "target-lane9/debug/artifact.py" in listed, listed

    files = RepoMapper(root).get_git_files()

    assert "src/app.py" in files, files
    counted = [path for path in files if path.startswith("target-lane9/")]
    assert not counted, (
        f"freshness counted {len(counted)} path(s) inside a tagged cache directory that "
        f"`dev map` will never index, so the map can never stop being stale: {counted}"
    )


def test_a_cache_only_write_does_not_make_the_map_stale(tmp_path) -> None:
    root = _repo_with_a_tagged_cache(tmp_path)
    mapper = RepoMapper(root, persist_content_cache=False)

    files = mapper.get_git_files()
    stamped = {
        "generated_head": mapper._git_head(),
        "indexed_hash": mapper._files_fingerprint(files),
        "content_fingerprint": mapper._content_fingerprint(files),
    }
    assert mapper.map_is_stale(stamped) is False, "precondition: a just-stamped map is fresh"

    # A build writes into its own cache. Nothing `dev map` would index changed,
    # so nothing the map answers can have changed either.
    (root / "target-lane9" / "debug" / "artifact.py").write_text(
        "def artifact():\n    return 3\n", encoding="utf-8"
    )
    (root / "target-lane9" / "debug" / "fresh.json").write_text("{}\n", encoding="utf-8")

    assert mapper.map_is_stale(stamped) is False, (
        "a write no walk will ever read marked the map stale; every rebuild re-stamps a "
        "fingerprint the next build will disagree with again"
    )

    # …and the check still has teeth: an edit to indexed source is still stale.
    (root / "src" / "app.py").write_text("def app():\n    return 99\n", encoding="utf-8")
    assert mapper.map_is_stale(stamped) is True


def test_only_the_standards_signature_hides_a_subtree(tmp_path) -> None:
    """A file merely *named* `CACHEDIR.TAG` must not delete a subtree.

    The tag is checked by its 43-byte signature, not by its name, precisely so
    that a source file with that name cannot silently shrink the inventory —
    which would make the map read fresh while missing real code.
    """
    root = _repo_with_a_tagged_cache(tmp_path, signature="not the standard's signature\n")

    files = RepoMapper(root).get_git_files()

    assert "target-lane9/debug/artifact.py" in files, files


def test_the_cache_lookup_matches_the_kernels_verdicts(tmp_path) -> None:
    """The transcription `freshness_parity.rs` depends on, checked directly.

    Mirrors `devmap-extract/tests/a_cache_verdict_answers_for_the_root_it_was_asked.rs`:
    the root is exempt, and a path that is not repo-relative is answered without
    opening anything — `root / ".."` is not a containment operation, so `src/..`
    *is* the root (which would defeat the exemption) and `../sibling` leaves the
    repository entirely.
    """
    from devcouncil.indexing.repo_mapper import _CacheDirectoryCache

    root = tmp_path / "root"
    (root / "pkg" / "deep").mkdir(parents=True)
    (root / "src").mkdir()
    (root / "pkg" / "CACHEDIR.TAG").write_text(_CACHEDIR_TAG_BODY, encoding="utf-8")
    (root / "src" / "main.py").write_text("x = 1\n", encoding="utf-8")
    # The root itself is tagged and still exempt: pointing `dev map` at a tagged
    # directory is a request, not an accident.
    (root / "CACHEDIR.TAG").write_text(_CACHEDIR_TAG_BODY, encoding="utf-8")

    caches = _CacheDirectoryCache(root)

    assert caches.is_inside_tagged_cache("pkg/mod.py") is True
    assert caches.is_inside_tagged_cache("pkg/deep/mod.py") is True
    assert caches.is_inside_tagged_cache("src/main.py") is False
    # `""` and `"."` mean the root, which is exempt.
    assert caches.is_inside_tagged_cache("") is False
    assert caches.is_inside_tagged_cache(".") is False
    # Not repo-relative: excluded rather than cleared, so a path that was never
    # evaluated can never read as one that was evaluated and passed.
    assert caches.is_inside_tagged_cache("src/../src/main.py") is True
    assert caches.is_inside_tagged_cache("../sibling/artifact.py") is True
    assert caches.is_inside_tagged_cache("/etc/passwd") is True
    # A repeated question is answered from the memo, not from a second `open`.
    assert caches.is_inside_tagged_cache("pkg/mod.py") is True


def test_the_walk_fallback_also_skips_tagged_cache_directories(tmp_path) -> None:
    """The no-git fallback feeds the same consumers, so it obeys the same rule."""
    plain = tmp_path / "plain"
    (plain / "pkg").mkdir(parents=True)
    (plain / "pkg" / "a.py").write_text("x = 1\n", encoding="utf-8")
    (plain / "build-cache" / "inner").mkdir(parents=True)
    (plain / "build-cache" / "CACHEDIR.TAG").write_text(_CACHEDIR_TAG_BODY, encoding="utf-8")
    (plain / "build-cache" / "inner" / "gen.py").write_text("y = 2\n", encoding="utf-8")

    # No `git init`: this is the `os.walk` branch.
    assert RepoMapper(plain).get_git_files() == ["pkg/a.py"]


def _tiny_git_repo(tmp_path):
    """A repo whose gitignored file exists on disk — the walk fallback lists it,
    `git ls-files` does not, so the two inventories are distinguishable."""
    import subprocess

    root = tmp_path / "repo"
    (root / "pkg").mkdir(parents=True)
    (root / "pkg" / "a.py").write_text("x = 1\n")
    (root / ".gitignore").write_text("ignored.py\n")
    (root / "ignored.py").write_text("x = 2\n")
    subprocess.run(["git", "init", "-q", "."], cwd=root, check=True)
    subprocess.run(["git", "add", "-A"], cwd=root, check=True)
    subprocess.run(
        ["git", "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "init"],
        cwd=root,
        check=True,
    )
    return root


def test_a_str_project_root_yields_the_same_inventory_as_a_path(tmp_path):
    """The root's *spelling* must not change the answer.

    `str / str` raises TypeError, the blanket except caught it, and the
    inventory silently degraded from `git ls-files` to an `os.walk` that also
    returns gitignored files. Measured on DevCouncil: 1347 files from
    `RepoMapper(".")` against 1151 from `RepoMapper(Path("."))`, so
    `map_is_stale` answered True or False for one map depending on how its
    caller spelled the root.

    The gitignored file is the discriminator: only the walk fallback lists it.
    """
    from pathlib import Path

    from devcouncil.indexing.repo_mapper import RepoMapper

    root = _tiny_git_repo(tmp_path)

    from_str = RepoMapper(str(root)).get_git_files()
    from_path = RepoMapper(Path(root)).get_git_files()

    assert from_str == from_path
    assert "ignored.py" not in from_str, "a gitignored file means the walk fallback answered"
    assert "pkg/a.py" in from_str


def test_fingerprints_do_not_depend_on_how_the_root_is_spelled(tmp_path):
    """`map_is_stale` is the consumer that made this matter."""
    from pathlib import Path

    from devcouncil.indexing.repo_mapper import RepoMapper

    root = _tiny_git_repo(tmp_path)
    as_str, as_path = RepoMapper(str(root)), RepoMapper(Path(root))

    files = as_path.get_git_files()
    stamped = {
        "generated_head": as_path._git_head(),
        "indexed_hash": as_path._files_fingerprint(files),
        "content_fingerprint": as_path._content_fingerprint(files),
    }
    assert as_path.map_is_stale(stamped) is False
    assert as_str.map_is_stale(stamped) is False, "same map, same answer, either spelling"


def test_a_programming_error_is_not_reported_as_a_missing_git(tmp_path, monkeypatch):
    """A fallback that cannot be told apart from the real path hides the bug.

    The walk fallback exists for "no git / not a repository". Catching every
    exception let an internal TypeError take that path and return a different
    file set under the same name, with nothing in the result saying which
    branch answered.
    """
    from devcouncil.indexing.repo_mapper import RepoMapper

    import devcouncil.utils.proc as proc

    root = _tiny_git_repo(tmp_path)
    mapper = RepoMapper(root)

    def _boom(*_args, **_kwargs):
        raise TypeError("injected: a bug in the git branch, not a missing git")

    # Injected where *only* the git branch reaches it. Patching something both
    # branches call would make the test pass on the unfixed code for the wrong
    # reason — the fallback would re-raise the same injection — which is what
    # the first version of this test did.
    monkeypatch.setattr(proc, "git_output", _boom)

    with pytest.raises(TypeError):
        mapper.get_git_files()


def test_a_directory_without_git_still_falls_back_to_the_walk(tmp_path):
    """Narrowing the except must not remove the fallback it was there for."""
    from devcouncil.indexing.repo_mapper import RepoMapper

    plain = tmp_path / "plain"
    (plain / "pkg").mkdir(parents=True)
    (plain / "pkg" / "a.py").write_text("x = 1\n")

    assert RepoMapper(plain).get_git_files() == ["pkg/a.py"]


def test_a_directory_without_git_does_not_log_to_the_console_channel(tmp_path, caplog):
    """The fallback must stay off the console: `--json` callers parse it.

    Logging the ordinary "not a repository" case at WARNING put a line on the
    console handler, which is configured at WARNING, and three `--json` CLI
    tests began failing with `JSONDecodeError: Extra data` — the log line landed
    in the middle of the envelope. Git being *absent* stays loud; git answering
    "not a repository" is the case this fallback exists for.
    """
    import logging

    from devcouncil.indexing.repo_mapper import RepoMapper

    plain = tmp_path / "plain"
    (plain / "pkg").mkdir(parents=True)
    (plain / "pkg" / "a.py").write_text("x = 1\n")

    with caplog.at_level(logging.DEBUG, logger="devcouncil.indexing.repo_mapper"):
        assert RepoMapper(plain).get_git_files() == ["pkg/a.py"]

    fallback = [r for r in caplog.records if "falling back to a directory walk" in r.message]
    assert fallback, "the degrade must still be recorded somewhere"
    assert all(r.levelno == logging.DEBUG for r in fallback), (
        "an ordinary non-git directory must not reach the console channel"
    )


# ── role_files: generic inference outside DevCouncil's own tree ──────────────
#
# `_SUBSYSTEM_ROLE_FILES` is keyed on DevCouncil's own source paths, so before
# this fix `_build_role_files` returned {} for every other repository — while
# the generated AGENTS.md told agents in every mapped project to use it, and
# test_resolver / wiki / map_viz all read it and silently got nothing.
# Measured on a 4,082-file polyglot repo: {} on all 10 subsystems.


def test_subsystem_carries_role_file_counts_field():
    from devcouncil.indexing.repo_mapper import RepoSubsystem

    sub = RepoSubsystem(area="a", summary="", entry_points=[], critical_files=[])
    assert sub.role_file_counts == {}


def test_get_git_files_drops_a_symlink_that_leaves_the_repository(tmp_path) -> None:
    """The inventory counts what the map covers, and a symlink out is not covered.

    The kernel refuses such a path at discovery
    (``DiscoverySkipReason::EscapesRoot``), because ``preview`` and the drain's
    ``classify_pending_entry`` have always refused a path that resolves outside
    the repository root. ``git ls-files`` lists the link — it is an ordinary
    mode-120000 entry — and ``_keep``'s ``is_file()`` follows it, so the
    inventory counted a file that is never indexed and
    ``_content_fingerprint`` hashed bytes the map does not describe. The map
    then reads stale on a change it can never absorb, which is the same failure
    ``_CacheDirectoryCache`` was added for, in a second shape.

    ``freshness_parity`` compares this list against the kernel's file for file,
    so this rule and ``devmap_extract::escapes_root`` have to agree.
    """
    outside = tmp_path / "elsewhere"
    outside.mkdir()
    (outside / "s.py").write_text("SECRET_V1 = 1\n")
    root = tmp_path / "repo"
    (root / "src").mkdir(parents=True)
    (root / "src" / "app.py").write_text("def app():\n    return 1\n")
    (root / "esc.py").symlink_to(outside / "s.py")
    (root / "inside.py").symlink_to(root / "src" / "app.py")

    for args in (
        ["init", "-q"],
        ["add", "-A"],
        ["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "x"],
    ):
        result = subprocess.run(
            ["git", *args], cwd=root, capture_output=True, text=True, check=False
        )
        if result.returncode != 0:
            pytest.skip(f"git unavailable: {result.stderr.strip()}")

    # The premise, asserted rather than assumed: git really does track the link.
    listed = subprocess.run(
        ["git", "ls-files"], cwd=root, capture_output=True, text=True, check=True
    ).stdout.split()
    assert "esc.py" in listed, listed

    files = RepoMapper(root).get_git_files()
    assert "esc.py" not in files, (
        "the inventory kept a symlink whose target is outside the repository, so "
        f"the content fingerprint hashes bytes the map never indexes: {files}"
    )
    # A symlink whose target is inside the tree is ordinary and stays: the bytes
    # it names are in the repository under their own path either way, and the
    # kernel's discovery walk keeps it too.
    assert "inside.py" in files, files
    assert "src/app.py" in files, files
