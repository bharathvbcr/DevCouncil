"""Regex-fallback + dispatch coverage for graph.extract_ts.

The tree-sitter happy paths are covered elsewhere; here we force the regex
fallbacks (by neutralizing ``_parse_lang``) so the JS/Go/Rust regex extractors
and the ``extract_file`` dispatcher are exercised directly.
"""

from __future__ import annotations

import pytest

from devcouncil.indexing.graph import extract_ts
from devcouncil.indexing.graph.extract_ts import (
    extract_file,
    extract_go,
    extract_rust,
    extract_ts_js,
)


@pytest.fixture
def force_regex(monkeypatch):
    """Make tree-sitter parsing return nothing so the regex fallbacks run."""
    monkeypatch.setattr(extract_ts, "_parse_lang", lambda language, source: (None, b""))


def test_js_regex_fallback_symbols_and_calls(force_regex):
    src = (
        "import { dep } from './dep';\n"
        "export function foo() { bar(); obj.method(); }\n"
        "export class Widget {}\n"
        "function helper() { return 1; }\n"
        "export { helper };\n"
    )
    ext = extract_ts_js("src/app.ts", src)
    assert ext.language == "typescript"
    assert "./dep" in ext.imports
    by_name = {s.name: s for s in ext.symbols if s.kind != "rationale"}
    assert by_name["foo"].kind == "function" and by_name["foo"].exported
    assert by_name["Widget"].kind == "class" and by_name["Widget"].exported
    assert by_name["helper"].exported is True  # via export list
    assert "helper" in ext.all_exports
    call_names = {c.name for c in ext.calls}
    assert "bar" in call_names
    assert any(c.name == "method" and c.receiver == "obj" for c in ext.calls)


def test_js_regex_skips_keyword_calls(force_regex):
    src = "function run() { if (x) {} return y(); }\n"
    ext = extract_ts_js("a.js", src)
    assert ext.language == "javascript"
    call_names = {c.name for c in ext.calls}
    assert "if" not in call_names  # keyword filtered
    assert "y" in call_names


def test_js_regex_rationale_comment(force_regex):
    src = "// WHY: keep this tiny\nexport function f() {}\n"
    ext = extract_ts_js("a.ts", src)
    rats = [s for s in ext.symbols if s.kind == "rationale"]
    assert rats and "WHY" in rats[0].name


def test_go_regex_fallback(force_regex):
    src = (
        "package svc\n"
        'import "fmt"\n'
        "func Helper() { fmt.Println() }\n"
        "func (s *Server) Start() {}\n"
        "func private() {}\n"
    )
    ext = extract_go("svc/server.go", src)
    assert ext.language == "go"
    by_q = {s.qualname: s for s in ext.symbols}
    assert by_q["Helper"].kind == "function" and by_q["Helper"].exported
    assert by_q["Server.Start"].kind == "method"
    assert by_q["private"].exported is False
    call_names = {c.name for c in ext.calls}
    assert "Println" in call_names


def test_rust_regex_fallback(force_regex):
    src = (
        "use crate::bar::Baz;\n"
        "mod m;\n"
        "pub struct Foo {}\n"
        "pub enum E { A }\n"
        "pub trait T {}\n"
        "pub fn free() { helper(); }\n"
    )
    ext = extract_rust("src/lib.rs", src)
    assert ext.language == "rust"
    by_q = {s.qualname: s for s in ext.symbols if s.kind != "rationale"}
    assert by_q["Foo"].kind == "struct" and by_q["Foo"].exported
    assert by_q["E"].kind == "enum"
    assert by_q["T"].kind == "trait"
    assert by_q["free"].kind == "function" and by_q["free"].exported
    assert "crate::bar::Baz" in ext.imports
    assert "mod:m" in ext.imports
    assert any(c.name == "helper" for c in ext.calls)


def test_extract_file_dispatch_python():
    ext = extract_file("pkg/mod.py", "def hello():\n    return 1\n")
    assert ext.language == "python"
    assert any(s.name == "hello" for s in ext.symbols)


def test_extract_file_dispatch_js_go_rust():
    assert extract_file("x.ts", "export function a() {}\n").language == "typescript"
    assert extract_file("x.go", "package p\nfunc A() {}\n").language == "go"
    assert extract_file("x.rs", "pub fn a() {}\n").language == "rust"


def test_extract_file_unknown_suffix_empty_language():
    ext = extract_file("notes.txt", "hello world\n")
    assert ext.language == ""
    assert ext.symbols == []


def test_js_import_specs_require_and_bare(force_regex):
    src = (
        "import './side-effect';\n"
        "const y = require('./y');\n"
        "import { z } from './z';\n"
    )
    ext = extract_ts_js("a.js", src)
    assert "./side-effect" in ext.imports
    assert "./y" in ext.imports
    assert "./z" in ext.imports


# ----------------------------------------------------------------------
# _parse_lang graceful failure paths
# ----------------------------------------------------------------------


def test_parse_lang_import_error(monkeypatch):
    import builtins

    real_import = builtins.__import__

    def fake_import(name, *args, **kwargs):
        if name == "tree_sitter_language_pack":
            raise ImportError("missing")
        return real_import(name, *args, **kwargs)

    monkeypatch.setattr(builtins, "__import__", fake_import)
    tree, raw = extract_ts._parse_lang("go", "package p\n")
    assert tree is None and raw == b""


def test_parse_lang_parse_exception(monkeypatch):
    import tree_sitter_language_pack

    class _BadParser:
        def parse(self, raw):
            raise RuntimeError("boom")

    monkeypatch.setattr(tree_sitter_language_pack, "get_parser", lambda name: _BadParser())
    tree, raw = extract_ts._parse_lang("go", "package p\n")
    assert tree is None and raw == b""


# ----------------------------------------------------------------------
# JS export-name regex branches (as / default / empty part)
# ----------------------------------------------------------------------


def test_js_regex_export_list_forms(force_regex):
    src = (
        "function a() {}\n"
        "function b() {}\n"
        "export { a as aa, b, };\n"          # alias + trailing empty part
        "export default function main() {}\n"  # default function name
    )
    ext = extract_ts_js("m.ts", src)
    exports = set(ext.all_exports)
    assert "a" in exports            # `a as aa` records local name a
    assert "b" in exports
    assert "main" in exports         # export default function name


def test_js_regex_adr_rationale(force_regex):
    src = "// ADR-42 keep this\nexport function f() {}\n"
    ext = extract_ts_js("a.ts", src)
    rats = [s for s in ext.symbols if s.kind == "rationale"]
    assert any(s.name.startswith("ADR") for s in rats)


def test_js_regex_exported_class_dedup(force_regex):
    # `export class` is matched by both the export RE and the class RE; the second
    # match must be de-duplicated via the `seen` set.
    src = "export class Widget {}\n"
    ext = extract_ts_js("a.ts", src)
    widgets = [s for s in ext.symbols if s.name == "Widget"]
    assert len(widgets) == 1
    assert widgets[0].kind == "class"


# ----------------------------------------------------------------------
# Go regex import single-line + call skip
# ----------------------------------------------------------------------


def test_go_regex_single_import_and_call_skip(force_regex):
    src = (
        "package svc\n"
        'import "fmt"\n'
        "func Run() { make([]int, 0); fmt.Println(); helper() }\n"
    )
    ext = extract_go("svc/x.go", src)
    assert "fmt" in ext.imports
    call_names = {c.name for c in ext.calls}
    assert "Println" in call_names
    assert "helper" in call_names
    assert "make" not in call_names  # builtin skipped


# ----------------------------------------------------------------------
# Rust regex call skip + rationale
# ----------------------------------------------------------------------


def test_rust_regex_call_skip_and_rationale(force_regex):
    src = (
        "// NOTE: rust regex path\n"
        "pub fn run() { match x {}; helper(); obj.method(); }\n"
    )
    ext = extract_rust("src/lib.rs", src)
    call_names = {c.name for c in ext.calls}
    assert "helper" in call_names
    assert any(c.name == "method" and c.receiver == "obj" for c in ext.calls)
    assert "match" not in call_names  # keyword skipped
    assert any(s.kind == "rationale" and "NOTE" in s.name for s in ext.symbols)
