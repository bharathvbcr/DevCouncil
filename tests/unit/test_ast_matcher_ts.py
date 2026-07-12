"""Phase 3 — AstMatcher tree-sitter parsing for TS/JS/Go/Rust."""

from __future__ import annotations

import pytest

from devcouncil.indexing.ast_matcher import AstMatcher
from devcouncil.indexing.ts_imports import tree_sitter_available


@pytest.mark.skipif(not tree_sitter_available(), reason="tree-sitter not available")
def test_ast_matcher_uses_tree_sitter_for_typescript(tmp_path):
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "app.ts").write_text(
        "export function handleRequest() {}\n"
        "export class Service {}\n"
        "export interface Opts {}\n"
        "export type Id = string;\n"
        "export const targetSymbol = () => 2;\n",
        encoding="utf-8",
    )
    matcher = AstMatcher(tmp_path)
    assert matcher.tree_sitter_available is True
    matches = matcher.match(query="handle", language="typescript")
    assert matches
    assert matches[0].name == "handleRequest"
    assert matches[0].engine == "tree-sitter"

    const_matches = matcher.match(query="target", language="typescript")
    assert any(m.name == "targetSymbol" for m in const_matches)
    assert all(m.engine == "tree-sitter" for m in const_matches)


@pytest.mark.skipif(not tree_sitter_available(), reason="tree-sitter not available")
def test_ast_matcher_tree_sitter_go_and_rust(tmp_path):
    (tmp_path / "main.go").write_text(
        "package main\n\nfunc Hello() string { return \"hi\" }\n",
        encoding="utf-8",
    )
    (tmp_path / "lib.rs").write_text(
        "pub fn greet() {}\npub struct Thing;\npub enum Kind { A }\npub trait Run {}\n",
        encoding="utf-8",
    )
    matcher = AstMatcher(tmp_path)
    go = matcher.match(query="Hello", language="go")
    assert go and go[0].engine == "tree-sitter" and go[0].kind == "function"
    rust_fn = matcher.match(query="greet", language="rust")
    assert rust_fn and rust_fn[0].engine == "tree-sitter"
    rust_struct = matcher.match(query="Thing", language="rust", kind="struct")
    assert rust_struct and rust_struct[0].name == "Thing"


def test_ast_matcher_falls_back_without_tree_sitter(tmp_path, monkeypatch):
    monkeypatch.setattr(
        "devcouncil.indexing.ts_imports.tree_sitter_available", lambda: False
    )
    (tmp_path / "app.ts").write_text(
        "export function handleRequest() {}\n",
        encoding="utf-8",
    )
    matcher = AstMatcher(tmp_path)
    assert matcher.tree_sitter_available is False
    matches = matcher.match(query="handle", language="typescript")
    assert matches[0].name == "handleRequest"
    assert matches[0].engine == "fallback-ast"
