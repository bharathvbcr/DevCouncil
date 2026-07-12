"""Phase 3 — optional tree-sitter import edges (Rust + file-level Go)."""

from __future__ import annotations

import subprocess

import pytest

from devcouncil.indexing.repo_mapper import RepoMapper
from devcouncil.indexing.ts_imports import (
    extract_go_import_specs,
    extract_rust_import_refs,
    tree_sitter_available,
)
from devcouncil.indexing.wiring import is_liveness_code_file


def _git(root, *args):
    subprocess.run(["git", *args], cwd=root, check=True, capture_output=True, text=True)


def _commit(root):
    _git(root, "init")
    _git(root, "-c", "user.email=t@t", "-c", "user.name=t", "add", "-A")
    _git(root, "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", "init")


def _write(tmp_path, files):
    for rel, content in files.items():
        p = tmp_path / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(content, encoding="utf-8")


@pytest.mark.skipif(not tree_sitter_available(), reason="tree-sitter optional extra not installed")
def test_tree_sitter_extracts_go_imports():
    src = (
        'package main\n\nimport (\n\t"fmt"\n\t"example.com/app/core"\n)\n'
        'import "example.com/app/util"\n'
    )
    specs = extract_go_import_specs(src)
    assert specs is not None
    assert "example.com/app/core" in specs
    assert "example.com/app/util" in specs
    assert "fmt" in specs


@pytest.mark.skipif(not tree_sitter_available(), reason="tree-sitter optional extra not installed")
def test_tree_sitter_extracts_rust_mod_use():
    src = (
        "mod foo;\n"
        "mod inline { fn x() {} }\n"
        "use crate::services::auth;\n"
        "use super::util;\n"
    )
    refs = extract_rust_import_refs(src)
    assert refs is not None
    mods = [r for r in refs if r.get("kind") == "mod"]
    assert {"name": "foo", "kind": "mod"} in mods or any(r.get("name") == "foo" for r in mods)
    # Inline mod with body must not produce a file-level mod ref.
    assert not any(r.get("name") == "inline" for r in mods)
    uses = [r for r in refs if r.get("kind") == "use"]
    assert any(r.get("segments", [])[:2] == ["crate", "services"] for r in uses)


def test_go_same_package_co_membership_edges(tmp_path):
    """Files in the same Go package wire each other (compile unit)."""
    _write(tmp_path, {
        "go.mod": "module example.com/app\n\ngo 1.21\n",
        "pkg/a.go": "package pkg\n\nfunc A() {}\n",
        "pkg/b.go": "package pkg\n\nfunc B() {}\n",
    })
    _commit(tmp_path)
    repo_map = RepoMapper(tmp_path).map_repo()
    assert "pkg/a.go" in repo_map.dependents.get("pkg/b.go", [])
    assert "pkg/b.go" in repo_map.dependents.get("pkg/a.go", [])
    assert "pkg/b.go" not in repo_map.unwired_candidates



@pytest.mark.skipif(not tree_sitter_available(), reason="tree-sitter optional extra not installed")
def test_rust_mod_and_use_edges(tmp_path):
    _write(tmp_path, {
        "src/lib.rs": (
            "mod foo;\n"
            "mod services;\n"
            "pub use crate::foo::Thing;\n"
        ),
        "src/foo.rs": "pub struct Thing;\n",
        "src/services/mod.rs": "pub mod auth;\n",
        "src/services/auth.rs": "pub fn login() {}\n",
    })
    _commit(tmp_path)
    repo_map = RepoMapper(tmp_path).map_repo()
    deps = repo_map.dependents
    assert "src/lib.rs" in deps.get("src/foo.rs", [])
    assert "src/lib.rs" in deps.get("src/services/mod.rs", [])


@pytest.mark.skipif(not tree_sitter_available(), reason="tree-sitter optional extra not installed")
def test_rust_use_crate_path_resolves_nested(tmp_path):
    _write(tmp_path, {
        "src/lib.rs": "pub mod services;\nuse crate::services::auth;\n",
        "src/services/mod.rs": "pub mod auth;\n",
        "src/services/auth.rs": "pub fn login() {}\n",
    })
    _commit(tmp_path)
    edges = RepoMapper(tmp_path)._all_import_edges(
        ["src/lib.rs", "src/services/mod.rs", "src/services/auth.rs"]
    )
    assert ("src/lib.rs", "src/services/mod.rs") in edges or (
        "src/lib.rs",
        "src/services/auth.rs",
    ) in edges


def test_rust_edges_absent_without_tree_sitter(tmp_path, monkeypatch):
    monkeypatch.setattr(
        "devcouncil.indexing.ts_imports.tree_sitter_available", lambda: False
    )
    _write(tmp_path, {
        "src/lib.rs": "mod foo;\n",
        "src/foo.rs": "pub struct Thing;\n",
    })
    _commit(tmp_path)
    edges = RepoMapper(tmp_path)._all_import_edges(["src/lib.rs", "src/foo.rs"])
    assert edges == []


def test_go_liveness_includes_go_files(tmp_path):
    assert is_liveness_code_file("core/a.go") is True
    assert is_liveness_code_file("pkg/mod.py") is True


def test_rust_liveness_gated_on_tree_sitter(monkeypatch):
    monkeypatch.setattr(
        "devcouncil.indexing.ts_imports.tree_sitter_available", lambda: False
    )
    assert is_liveness_code_file("src/lib.rs") is False
    monkeypatch.setattr(
        "devcouncil.indexing.ts_imports.tree_sitter_available", lambda: True
    )
    assert is_liveness_code_file("src/lib.rs") is True


@pytest.mark.skipif(not tree_sitter_available(), reason="tree-sitter optional extra not installed")
def test_go_map_liveness_parity_imported_package_not_unwired(tmp_path):
    """File-level Go edges clear unwired for every member of an imported package."""
    _write(tmp_path, {
        "go.mod": "module example.com/app\n\ngo 1.21\n",
        "main.go": (
            "package main\n\n"
            "import \"example.com/app/core\"\n\n"
            "func main() {}\n"
        ),
        "core/core.go": "package core\n\nfunc Hello() {}\n",
        "core/extra.go": "package core\n\nfunc Extra() {}\n",
    })
    _commit(tmp_path)
    repo_map = RepoMapper(tmp_path).map_repo()
    assert "core/core.go" not in repo_map.unwired_candidates
    assert "core/extra.go" not in repo_map.unwired_candidates
