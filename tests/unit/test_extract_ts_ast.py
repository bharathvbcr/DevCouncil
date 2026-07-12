"""Phase 2 — tree-sitter AST extraction for TS/JS, Go receivers, Rust."""

from __future__ import annotations

import pytest

from devcouncil.indexing.graph.extract_ts import extract_go, extract_rust, extract_ts_js
from devcouncil.indexing.ts_imports import tree_sitter_available

pytestmark = pytest.mark.skipif(
    not tree_sitter_available(),
    reason="tree-sitter required for AST extraction tests",
)


def test_ts_methods_qualnames_extends_implements_exports():
    src = """
export interface IFoo { x: number }
export type Id = string
export class Animal extends Base implements IFoo {
  greet(name: string): void {
    helper();
    this.speak();
  }
  speak() { return 1 }
}
function helper() { return 2 }
export { helper }
export default function main() { helper() }
"""
    ext = extract_ts_js("src/animal.ts", src)
    by_q = {s.qualname: s for s in ext.symbols if s.kind != "rationale"}

    assert by_q["IFoo"].kind == "interface" and by_q["IFoo"].exported
    assert by_q["Id"].kind == "type" and by_q["Id"].exported
    animal = by_q["Animal"]
    assert animal.kind == "class"
    assert animal.bases == ["Base"]
    assert animal.implements == ["IFoo"]
    assert animal.exported is True
    assert animal.end_line > animal.line

    greet = by_q["Animal.greet"]
    assert greet.kind == "method"
    assert greet.end_line >= greet.line
    assert by_q["Animal.speak"].kind == "method"

    assert by_q["helper"].exported is True
    assert by_q["main"].exported is True

    assert any(c.name == "helper" and not c.receiver for c in ext.calls)
    assert any(c.name == "speak" and c.receiver == "this" for c in ext.calls)


def test_ts_export_list_and_reexport():
    src = """
class Local {}
function util() { return 1 }
export { Local, util }
export { util as u } from './other'
"""
    ext = extract_ts_js("src/barrel.ts", src)
    by_q = {s.qualname: s for s in ext.symbols if s.kind != "rationale"}
    assert by_q["Local"].exported is True
    assert by_q["util"].exported is True
    assert "util" in ext.reexports
    assert "./other" in ext.imports


def test_ts_rationale_comment():
    src = """
// NOTE: keep Animal small
export class Animal {
  speak() { return 1 }
}
"""
    ext = extract_ts_js("a.ts", src)
    rats = [s for s in ext.symbols if s.kind == "rationale"]
    assert rats
    assert "NOTE" in rats[0].name


def test_go_methods_with_receiver_types():
    src = """
package svc

type Server struct{}

func (s *Server) Start() {}
func (s Server) Stop() {}
func Helper() {}
"""
    ext = extract_go("svc/server.go", src)
    by_q = {s.qualname: s for s in ext.symbols}
    assert by_q["Server"].kind == "struct"
    assert by_q["Server.Start"].kind == "method"
    assert by_q["Server.Stop"].kind == "method"
    assert by_q["Helper"].kind == "function"
    assert by_q["Helper"].exported is True


def test_ts_imports_exports_reexports_and_calls():
    src = """
import def_export, { named, aliased as renamed } from './dep'
import './side-effect'
export { foo, bar } from './reexp'
export default main
export const arrow = () => { helper(); obj.method(); this.run(); a.b.c(); }
export function fn() {}
export interface IThing extends Other { x: number }
export type Alias = string
let plain = 1
const fnLike = function () { return 1 }
"""
    ext = extract_ts_js("src/mixed.ts", src)
    assert "./dep" in ext.imports
    assert "./side-effect" in ext.imports
    assert "./reexp" in ext.imports
    # re-export names captured
    assert "foo" in ext.reexports or "foo" in ext.all_exports
    by_q = {s.qualname: s for s in ext.symbols if s.kind != "rationale"}
    assert "arrow" in by_q
    assert by_q["fn"].kind == "function"
    assert by_q["IThing"].kind == "interface"
    assert "Other" in by_q["IThing"].bases
    assert by_q["Alias"].kind == "type"
    # import detail alias map records default + named + aliased
    detail = next((d for d in ext.import_details if d.module == "./dep"), None)
    assert detail is not None
    call_names = {c.name for c in ext.calls}
    assert "helper" in call_names
    assert any(c.name == "run" and c.receiver == "this" for c in ext.calls)
    # nested member a.b.c() → property identifier extracted
    assert "c" in call_names


def test_ts_lexical_declaration_non_function_unexported_skipped():
    src = """
const exported = 1
export const shown = 2
const helper = () => 1
"""
    ext = extract_ts_js("src/vals.ts", src)
    by_q = {s.qualname: s for s in ext.symbols if s.kind != "rationale"}
    # exported const and arrow-function const are recorded; plain unexported const is not.
    assert "shown" in by_q
    assert "helper" in by_q
    assert "exported" not in by_q


def test_ts_class_methods_and_constructor():
    src = """
export class Service {
  constructor() {}
  run() { return this.value }
}
"""
    ext = extract_ts_js("src/svc.ts", src)
    by_q = {s.qualname: s for s in ext.symbols if s.kind != "rationale"}
    assert by_q["Service"].kind == "class"
    assert by_q["Service.run"].kind == "method"
    assert by_q["Service.constructor"].kind == "method"


def test_js_calls_keyword_skip_tree_sitter():
    # `new` inside a call target; `require` is not skipped but `super()` is.
    src = """
class A extends B {
  constructor() { super(); realCall(); }
}
"""
    ext = extract_ts_js("a.ts", src)
    call_names = {c.name for c in ext.calls}
    assert "realCall" in call_names
    assert "super" not in call_names


def test_go_tree_sitter_pointer_value_receivers_and_calls():
    src = """
package svc

import (
	"fmt"
	m "example.com/x"
)

type Server struct{}
type Reader interface{ Read() }

func (s *Server) Start() {
	fmt.Println()
	s.helper()
	make([]int, 0)
	bareCall()
}

func (s Server) Stop() {}

func Helper() {}
"""
    ext = extract_go("svc/server.go", src)
    by_q = {s.qualname: s for s in ext.symbols}
    assert by_q["Server"].kind == "struct"
    assert by_q["Reader"].kind == "interface"
    assert by_q["Server.Start"].kind == "method"  # pointer receiver
    assert by_q["Server.Stop"].kind == "method"   # value receiver
    assert by_q["Helper"].exported is True
    assert "fmt" in ext.imports
    assert "example.com/x" in ext.imports
    call_names = {c.name for c in ext.calls}
    assert "Println" in call_names       # selector expression
    assert "helper" in call_names        # selector on receiver
    assert "bareCall" in call_names      # identifier call
    assert "make" not in call_names      # builtin skipped


def test_rust_use_import_details_and_trait_methods():
    src = """
mod submod;
use crate::a::Widget;
use std::collections::HashMap;
pub trait Greeter {
    fn greet(&self);
    fn hello(&self) {}
}
pub fn top() { free(); }
"""
    ext = extract_rust("src/lib.rs", src)
    assert "mod:submod" in ext.imports
    assert any("Widget" in i for i in ext.imports)
    # use produces import_details with a names entry
    assert any(d.names for d in ext.import_details)
    by_q = {s.qualname: s for s in ext.symbols if s.kind != "rationale"}
    assert by_q["Greeter"].kind == "trait"
    assert by_q["Greeter.greet"].kind == "method"   # signature item
    assert by_q["Greeter.hello"].kind == "method"   # provided (function_item)


def test_rust_impl_trait_for_new_type_and_calls():
    # impl Trait for a type NOT previously defined → synthetic struct symbol.
    src = """
impl MyTrait for Newtype {
    fn act(&self) { self.field.call_it(); direct(); }
}
"""
    ext = extract_rust("src/impl.rs", src)
    by_q = {s.qualname: s for s in ext.symbols if s.kind != "rationale"}
    assert "Newtype" in by_q
    assert "MyTrait" in by_q["Newtype"].implements
    assert by_q["Newtype.act"].kind == "method"
    call_names = {c.name for c in ext.calls}
    assert "direct" in call_names
    assert "call_it" in call_names  # field_expression call


def test_go_tree_sitter_import_exception_falls_back_to_regex(monkeypatch):
    import devcouncil.indexing.ts_imports as ti

    monkeypatch.setattr(
        ti, "extract_go_import_specs",
        lambda source: (_ for _ in ()).throw(RuntimeError("boom")),
    )
    src = 'package svc\nimport "fmt"\nfunc A() {}\n'
    ext = extract_go("svc/a.go", src)
    assert "fmt" in ext.imports  # regex fallback still resolved the import


def test_rust_tree_sitter_import_exception_swallowed(monkeypatch):
    import devcouncil.indexing.ts_imports as ti

    monkeypatch.setattr(
        ti, "extract_rust_import_refs",
        lambda source: (_ for _ in ()).throw(RuntimeError("boom")),
    )
    src = "pub fn go() {}\n"
    ext = extract_rust("src/x.rs", src)
    # symbols still extracted even though import extraction raised.
    assert any(s.name == "go" for s in ext.symbols)


def test_rust_impl_trait_for_primitive_single_type_id():
    # `impl Trait for i32` — i32 is a primitive_type, so only one type_identifier
    # is present, exercising the single-type-id branch.
    src = "impl MyMarker for i32 {\n    fn tag(&self) {}\n}\n"
    ext = extract_rust("src/prim.rs", src)
    assert any(s.kind == "method" for s in ext.symbols)


def test_rust_rationale_comment_tree_sitter():
    src = """
// NOTE: keep this crate lean
pub fn go() {}
// ADR-7 recorded here
pub fn other() {}
"""
    ext = extract_rust("src/r.rs", src)
    rats = [s for s in ext.symbols if s.kind == "rationale"]
    names = " ".join(s.name for s in rats)
    assert "NOTE" in names
    assert "ADR" in names


def test_rust_struct_trait_impl_use_mod():
    src = """
pub struct Foo {}
pub enum E { A }
pub trait T { fn f(&self); }
impl Foo {
    pub fn method(&self) { free(); }
}
impl T for Foo {
    fn f(&self) {}
}
use crate::bar::Baz;
mod m;
pub fn free() {}
"""
    ext = extract_rust("src/lib.rs", src)
    by_q = {s.qualname: s for s in ext.symbols if s.kind != "rationale"}
    assert by_q["Foo"].kind == "struct"
    assert "T" in by_q["Foo"].implements
    assert by_q["E"].kind == "enum"
    assert by_q["T"].kind == "trait"
    assert by_q["Foo.method"].kind == "method"
    assert by_q["Foo.f"].kind == "method"
    assert by_q["free"].kind == "function" and by_q["free"].exported
    assert any("Baz" in i for i in ext.imports)
    assert "mod:m" in ext.imports
    assert any(c.name == "free" for c in ext.calls)
