"""Coverage for ts_imports symbol extraction + graceful degradation."""

from __future__ import annotations

import pytest

from devcouncil.indexing import ts_imports
from devcouncil.indexing.ts_imports import (
    extract_go_import_specs,
    extract_rust_import_refs,
    extract_symbols,
    language_for_suffix,
    tree_sitter_available,
)

ts_available = pytest.mark.skipif(
    not tree_sitter_available(), reason="tree-sitter required"
)


def test_language_for_suffix():
    assert language_for_suffix(".rs") == "rust"
    assert language_for_suffix(".TS") == "typescript"
    assert language_for_suffix(".tsx") == "tsx"
    assert language_for_suffix(".py") is None


def test_functions_return_none_without_tree_sitter(monkeypatch):
    monkeypatch.setattr(ts_imports, "tree_sitter_available", lambda: False)
    assert extract_go_import_specs("package p\n") is None
    assert extract_rust_import_refs("mod m;\n") is None
    assert extract_symbols("go", "package p\n") is None


def test_extract_symbols_unknown_language_returns_none():
    assert extract_symbols("cobol", "IDENTIFICATION DIVISION.\n") is None


@ts_available
def test_extract_symbols_js():
    src = (
        "export function alpha() {}\n"
        "class Beta {}\n"
        "interface Gamma {}\n"
        "type Delta = string;\n"
        "const eps = () => 1;\n"
    )
    hits = extract_symbols("typescript", src)
    assert hits is not None
    kinds = {(name, kind) for kind, name, _line, _text in hits}
    names = {name for _kind, name, _l, _t in hits}
    assert "alpha" in names
    assert ("Beta", "class") in kinds
    assert ("Gamma", "interface") in kinds
    assert ("Delta", "type") in kinds
    assert "eps" in names


@ts_available
def test_extract_symbols_go():
    src = (
        "package svc\n"
        "type Server struct{}\n"
        "type Reader interface{}\n"
        "func (s *Server) Start() {}\n"
        "func Helper() {}\n"
    )
    hits = extract_symbols("go", src)
    assert hits is not None
    by_name = {name: kind for kind, name, _l, _t in hits}
    assert by_name["Server"] == "struct"
    assert by_name["Reader"] == "interface"
    assert by_name["Start"] == "function"  # method recorded as function hit
    assert by_name["Helper"] == "function"


@ts_available
def test_extract_symbols_rust():
    src = (
        "pub fn free() {}\n"
        "pub struct Foo {}\n"
        "pub enum E { A }\n"
        "pub trait T {}\n"
        "type Alias = u32;\n"
    )
    hits = extract_symbols("rust", src)
    assert hits is not None
    by_name = {name: kind for kind, name, _l, _t in hits}
    assert by_name["free"] == "function"
    assert by_name["Foo"] == "struct"
    assert by_name["E"] == "enum"
    assert by_name["T"] == "trait"
    assert by_name["Alias"] == "type"


@ts_available
def test_extract_go_import_specs_only_import_specs():
    src = 'package main\n\nimport (\n\t"fmt"\n\t"example.com/app/core"\n)\n'
    specs = extract_go_import_specs(src)
    assert specs is not None
    assert "fmt" in specs
    assert "example.com/app/core" in specs


@ts_available
def test_extract_rust_import_refs_mod_and_use():
    src = "mod foo;\nmod inline { fn x() {} }\nuse crate::services::auth;\n"
    refs = extract_rust_import_refs(src)
    assert refs is not None
    mods = [r for r in refs if r.get("kind") == "mod"]
    assert any(r.get("name") == "foo" for r in mods)
    assert not any(r.get("name") == "inline" for r in mods)
    uses = [r for r in refs if r.get("kind") == "use"]
    assert any(r.get("segments", [])[:2] == ["crate", "services"] for r in uses)


# ----------------------------------------------------------------------
# graceful degradation of the low-level parser helpers
# ----------------------------------------------------------------------


def test_tree_sitter_available_false_when_import_fails(monkeypatch):
    import builtins

    real_import = builtins.__import__

    def fake_import(name, *args, **kwargs):
        if name == "tree_sitter" or name == "tree_sitter_language_pack":
            raise ImportError("missing")
        return real_import(name, *args, **kwargs)

    monkeypatch.setattr(builtins, "__import__", fake_import)
    assert ts_imports.tree_sitter_available() is False


def test_parser_for_returns_none_without_tree_sitter(monkeypatch):
    monkeypatch.setattr(ts_imports, "tree_sitter_available", lambda: False)
    ts_imports._parser_for.cache_clear()
    try:
        assert ts_imports._parser_for("rust") is None
    finally:
        ts_imports._parser_for.cache_clear()


@ts_available
def test_parser_for_handles_get_parser_error(monkeypatch):
    import tree_sitter_language_pack

    def boom(name):
        raise RuntimeError("no parser")

    monkeypatch.setattr(tree_sitter_language_pack, "get_parser", boom)
    ts_imports._parser_for.cache_clear()
    try:
        assert ts_imports._parser_for("go") is None
    finally:
        ts_imports._parser_for.cache_clear()


def test_parse_returns_none_when_parser_missing(monkeypatch):
    monkeypatch.setattr(ts_imports, "_parser_for", lambda language: None)
    assert ts_imports._parse("go", b"package p\n") is None


def test_parse_returns_none_on_parse_exception(monkeypatch):
    class _BadParser:
        def parse(self, raw):
            raise RuntimeError("boom")

    monkeypatch.setattr(ts_imports, "_parser_for", lambda language: _BadParser())
    assert ts_imports._parse("go", b"package p\n") is None


@ts_available
def test_extract_go_import_specs_tree_none(monkeypatch):
    monkeypatch.setattr(ts_imports, "_parse", lambda language, raw: None)
    assert extract_go_import_specs("package p\n") is None


@ts_available
def test_extract_rust_import_refs_tree_none(monkeypatch):
    monkeypatch.setattr(ts_imports, "_parse", lambda language, raw: None)
    assert extract_rust_import_refs("mod m;\n") is None


@ts_available
def test_extract_symbols_tree_none(monkeypatch):
    monkeypatch.setattr(ts_imports, "_parse", lambda language, raw: None)
    assert extract_symbols("go", "package p\n") is None
    # typescript path also attempts the tsx fallback before giving up.
    assert extract_symbols("typescript", "export const x = 1;\n") is None


@ts_available
def test_extract_go_import_specs_accepts_bytes():
    specs = extract_go_import_specs(b'package main\nimport "fmt"\n')
    assert specs is not None
    assert "fmt" in specs


@ts_available
def test_extract_rust_use_wildcard_list_and_keywords():
    src = (
        "use crate::a::*;\n"
        "use crate::b::{C, D};\n"
        "use super::x;\n"
        "use self::y;\n"
        "use foo as bar;\n"
    )
    refs = extract_rust_import_refs(src)
    assert refs is not None
    uses = [r for r in refs if r.get("kind") == "use"]
    segs = [r.get("segments") for r in uses]
    assert ["crate", "a"] in segs  # wildcard star dropped, path kept
    assert any(s[:2] == ["crate", "b"] and "C" in s and "D" in s for s in segs)
    assert any(s[0] == "super" for s in segs)
    assert any(s[0] == "self" for s in segs)


@ts_available
def test_extract_symbols_go_edgecases():
    # nameless / receiver-only forms exercise the continue branches.
    src = (
        "package svc\n"
        "type Alias = int\n"
        "func (s *Server) Do() {}\n"
    )
    hits = extract_symbols("go", src)
    assert hits is not None
    names = {name for _kind, name, _l, _t in hits}
    assert "Do" in names
