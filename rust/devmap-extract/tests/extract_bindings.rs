//! Extract-bindings hardening (schema v48+; identity includes attribution semantics).
//!
//! Declared types and simple initializers ride on `LocalBinding`; Go field
//! types become Type references with `assigned_to`; Python `isinstance` /
//! `except` keep Type/Name references; `self.field` stays a Name with
//! `receiver_expr` (no separate FieldAccess); Svelte runes are call sites.

use devmap_extract::extract_file;
use devmap_extract::model::{LocalBinding, ReferenceKind};

#[test]
fn extraction_schema_version_tracks_binding_and_ruleguard() {
    assert_eq!(
        devmap_extract::cache::EXTRACTION_SCHEMA_VERSION,
        "98",
        "v98 adds the `jvm_decls`, `package_scopes`, `local_extents`, `decl_gaps`, `recovered_decl_ranges` and `dynamic_dispatch` side tables (empty until each extractor lane fills one); v97 records a Swift member property (stored, computed, static, lazy, wrapped, protocol requirement) as a `Field` symbol and stops a Swift argument label or leading-dot member being a name reference; v96 adds `js_objects` (how a JS/TS module-scope const object is built and written); v95 stamps a Python `self.name = callee(..)` call reference with `assigned_to = \"self.name\"`; v94 marks a Python `from m import *` with the alias `*` and a JS/TS module-scope binding in `scope_locals` under the file's name; v93 integrates main's Swift receiver-typing payload (v90 on main: same-file `typealias`, parenthesised cast, tuple parameter member, own-method return and SwiftData fetch receivers, with scope-aware abstentions) on top of v92; v92 integrates the Rust, TypeScript/Go and Swift lanes (Swift type aliases are extractor output); v90 (ts/go lane) records a TypeScript interface method_signature as a Method symbol and a Go embedding as a Heritage reference; v90 (rust lane) records `macro_rules!` macros as Macro symbols their same-file invocations reference, blanket-impl type parameters, `?` From candidates and calls through a same-file const fn pointer; v89 types a Rust `dyn Trait` / `impl Trait` binding by its one named trait and records a literal Python `importlib.import_module` as a scoped module handle; v88 is the dead-policy lane's payload (function-local Rust impl keys, Kotlin callable-reference receivers on name references, Rust `fn main` outside `src/`, Rust restricted-visibility and SwiftPM executable-target scopes, indexed project.pbxproj, a qualified JS/TS base as one Heritage reference per segment); v87 records a Kotlin parameter, typed local and constructor property as a typed binding, a Kotlin/Java file's package header, Kotlin scope-function lambdas, callable references and written return types, Android manifests as config, and Ruby attributes and mixins as methods and heritage; v86 records Swift protocol requirements as symbols and scope-aware Swift locals; v85 records the implicit `__enter__` call of a Python `with Class(..)`; v84 binds Swift and Kotlin parameters and closure parameters as local bindings; v83 records the TypeScript workspace-package map and the member-qualified ids of class-field and object-literal members, and an `export default` as an export named `default`; v82 records a Go file's declarations, local scopes and call receivers as structured facts (`go_facts`), the input of the resolver's Go type evaluator; v81 records inline Rust modules, `Deref` targets, implicit operator/drop/from candidates, `generate_handler!` lists and `macro_rules!` expansions, and drops macro member names as references; v80 records which partial-parse error regions hold no name; v79 is both lanes' v78 payloads together, under a number neither stamped: the read-tools lane's v78 withholds a secret file's literal values, docstrings and string-made references; the dead-policy lane's v78 adds the dead-policy extractor fixes (cgo/linkname and Tauri entry points, TS ambient exemptions, destructuring defaults as uses, Swift custom-operator calls and standard-operator exemptions, Swift `Type.member` receivers); v77 adds macro-stamped C-family functions and EntryName references on top of v76; v76 is v73, v74 and the literal half of v75 together, because extraction-recall and metal-kernels both stamped 75 on different payloads; v75 records string literals and the uses of a Rust module-level const; v74 binds the names a CommonJS `require` declares; v73 references the type a Rust struct literal constructs (`Name {{ .. }}`, `Self {{ .. }}`); v72 adds the MCP-parity lane's v67 (route middleware and Go routes); v71 adds the resolver-precision lane's v67 (Rust function headers, Rust locals typed from their own binders, a Go `var w T`); v70 merges v68 and v69 under a number neither lane stamped; v69 records each callable's written return type and binds a single-name Go var/const spec's initializer; v68 stops a JSX tag, a C# member access, an annotation, an attribute or a preprocessor test from binding a local, and binds wrapped callbacks (`useCallback(() => …)`, `memo`, `forwardRef`) as functions; v66 emits every Python module-scope binding, multi-name Go specs, destructured TS/JS names and type aliases, reads import names off the tree, and stops a module binding shadowing its module-level reads; v65 references the scope of `T::m` and records the calls inside a function-like macro body; v64 stops filing a C++ template callee or template type use as a local binding and annotates a Python function decorated with a parameter of its enclosing callable; v63 records in-function Python `sys.path` inserts as scoped vetoes; v62 records Python `sys.path` entries; v61 reads Python path constants and parameterised path loaders; v60 records a Go file's unexported field and interface-method names, the veto for the unexported-selector rung; v59 records a Python module loaded by file path as a `path_load` import; v58 records a shell `source` as a glob import and a `build.rs`'s `cargo:rerun-if-changed=` paths as dynamic references, and stops emitting a Go struct-literal field key as a name reference, on top of v57's object-literal method exemption, v56's Rust `pub use` exports, v55's inline-module `super` receiver, v54's declared-field types, v53's receiver-prefix reduction and v52's duplicate-callee drop, while retaining v48 bindings, v49 ruleguard wiring, v50 attribution fixes and v51 captured-receiver classifications"
    );
}

#[test]
fn rust_typed_let_local_binding_carries_declared_type() {
    let extraction = extract_file(
        "lib.rs",
        r#"
pub struct Engine;
impl Engine {
    pub fn tick(&self) {}
    pub fn new() -> Self { Self }
}
pub fn typed_let() {
    let x: Engine = Engine::new();
    x.tick();
}
"#,
    );
    let binding = extraction
        .local_bindings
        .iter()
        .find(|b| b.name == "x")
        .unwrap_or_else(|| {
            panic!(
                "expected LocalBinding for x: {:?}",
                extraction.local_bindings
            )
        });
    assert_eq!(
        binding.declared_type.as_deref(),
        Some("Engine"),
        "typed let must carry declared_type: {binding:?}"
    );
    assert_eq!(
        binding.initializer.as_deref(),
        Some("Engine::new"),
        "Engine::new() is a simple initializer shape: {binding:?}"
    );
}

#[test]
fn go_field_declaration_emits_type_reference_with_assigned_to() {
    let extraction = extract_file(
        "priority.go",
        r#"
package verify
type Priority int
func (p Priority) valid() bool { return p >= 0 }
type Worker struct {
	Priority Priority
}
func (w Worker) Check() bool {
	return w.Priority.valid()
}
"#,
    );
    let field_type = extraction.references.iter().find(|r| {
        r.kind == ReferenceKind::Type
            && r.name == "Priority"
            && r.assigned_to.as_deref() == Some("Priority")
    });
    assert!(
        field_type.is_some(),
        "Go struct field must emit Type with assigned_to=field name: {:?}",
        extraction
            .references
            .iter()
            .filter(|r| r.kind == ReferenceKind::Type)
            .map(|r| (&r.name, &r.assigned_to, &r.enclosing_symbol))
            .collect::<Vec<_>>()
    );
    // Use of `w` (receiver) should carry declared_type Worker when present.
    if let Some(w) = extraction.local_bindings.iter().find(|b| b.name == "w") {
        assert_eq!(
            w.declared_type.as_deref(),
            Some("Worker"),
            "method receiver binding should carry declared_type: {w:?}"
        );
    }
}

#[test]
fn python_isinstance_and_except_reference_the_type_name() {
    let extraction = extract_file(
        "hidden_test.py",
        r#"
from patch import PatchError, ProtocolError

def check(value):
    if isinstance(value, ProtocolError):
        return False
    try:
        raise PatchError("boom")
    except PatchError:
        return True
"#,
    );
    let names: Vec<&str> = extraction
        .references
        .iter()
        .map(|r| r.name.as_str())
        .collect();
    assert!(
        names.contains(&"ProtocolError"),
        "isinstance second arg must be a reference: {names:?}"
    );
    assert!(
        names.contains(&"PatchError"),
        "except type must be a reference: {names:?}"
    );
    let isinstance_type = extraction.references.iter().find(|r| {
        r.name == "ProtocolError" && matches!(r.kind, ReferenceKind::Type | ReferenceKind::Name)
    });
    assert!(
        isinstance_type.is_some(),
        "ProtocolError must be Type or Name: {:?}",
        extraction
            .references
            .iter()
            .filter(|r| r.name == "ProtocolError")
            .map(|r| r.kind)
            .collect::<Vec<_>>()
    );
}

#[test]
fn self_field_read_is_name_with_receiver_not_dropped() {
    let extraction = extract_file(
        "holder.rs",
        r#"
pub struct Engine;
impl Engine { pub fn tick(&self) {} }
pub struct Holder { engine: Engine }
impl Holder {
    pub fn go(&self) {
        let _ = &self.engine;
        self.engine.tick();
    }
}
"#,
    );
    let field_read = extraction.references.iter().find(|r| {
        r.name == "engine"
            && r.kind == ReferenceKind::Name
            && r.receiver_expr.as_deref() == Some("self")
    });
    assert!(
        field_read.is_some(),
        "self.engine must remain Name+receiver_expr (FieldAccess not required): {:?}",
        extraction
            .references
            .iter()
            .filter(|r| r.name == "engine")
            .map(|r| (&r.kind, &r.receiver_expr))
            .collect::<Vec<_>>()
    );
    assert!(
        !extraction
            .references
            .iter()
            .any(|r| matches!(format!("{:?}", r.kind).as_str(), "FieldAccess")),
        "FieldAccess is not part of the schema; Name+receiver_expr covers self.field"
    );
}

#[test]
fn svelte_runes_are_extracted_as_calls() {
    let extraction = extract_file(
        "Counter.svelte",
        r#"
<script>
  let count = $state(0);
  let doubled = $derived(count * 2);
  function bump() {
    count += 1;
  }
  $effect(() => {
    console.log(count);
  });
</script>
<button onclick={bump}>{doubled}</button>
"#,
    );
    let callees: Vec<&str> = extraction
        .calls
        .iter()
        .map(|c| c.callee_name.as_str())
        .collect();
    for rune in ["$state", "$derived", "$effect"] {
        assert!(
            callees.contains(&rune)
                || extraction.references.iter().any(|r| r.name == rune
                    && matches!(
                        r.kind,
                        ReferenceKind::Call | ReferenceKind::Name | ReferenceKind::Constructor
                    )),
            "{rune} must appear as a Call/Name reference for HostGlobal classification: \
             calls={callees:?} refs={:?}",
            extraction
                .references
                .iter()
                .map(|r| (&r.name, r.kind))
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn typescript_typed_param_and_factory_initializer() {
    let extraction = extract_file(
        "f.ts",
        r#"
class Client { go() {} }
function factory() { return new Client(); }
function use() {
  const x = factory();
  x.go();
}
function typed(p: Client) {
  p.go();
}
"#,
    );
    let x = extraction
        .local_bindings
        .iter()
        .find(|b| b.name == "x")
        .unwrap_or_else(|| panic!("binding for x: {:?}", extraction.local_bindings));
    assert_eq!(
        x.initializer.as_deref(),
        Some("factory"),
        "const x = factory() should record factory as initializer: {x:?}"
    );
    let p = extraction
        .local_bindings
        .iter()
        .find(|b| b.name == "p")
        .unwrap_or_else(|| panic!("binding for p: {:?}", extraction.local_bindings));
    assert_eq!(
        p.declared_type.as_deref(),
        Some("Client"),
        "typed param should carry declared_type: {p:?}"
    );
}

/// Compile-time shape check so Ord/Eq stay coherent with the new optional fields.
#[test]
fn local_binding_optional_fields_serialize_when_set() {
    let binding = LocalBinding {
        start_byte: 10,
        name: "x".into(),
        scope: Some("f.rs::typed_let".into()),
        declared_type: Some("Engine".into()),
        initializer: Some("Engine::new".into()),
    };
    let json = serde_json::to_value(&binding).expect("serialize");
    assert_eq!(json["declared_type"], "Engine");
    assert_eq!(json["initializer"], "Engine::new");
    let sparse = LocalBinding {
        start_byte: 1,
        name: "y".into(),
        scope: None,
        declared_type: None,
        initializer: None,
    };
    let sparse_json = serde_json::to_value(&sparse).expect("serialize");
    assert!(sparse_json.get("declared_type").is_none());
    assert!(sparse_json.get("initializer").is_none());
}

#[test]
fn rust_trait_object_and_opaque_parameters_are_typed_by_their_one_trait() {
    let extraction = extract_file(
        "lib.rs",
        r#"
pub trait Render { fn render(&self) -> u8; }
pub fn by_ref(a: &dyn Render) -> u8 { a.render() }
pub fn boxed(b: Box<dyn Render>) -> u8 { b.render() }
pub fn opaque(c: impl Render) -> u8 { c.render() }
pub fn send(d: &(dyn Render + Send)) -> u8 { d.render() }
pub fn two(e: impl Render + Clone) -> u8 { e.render() }
"#,
    );
    let typed = |param: &str| -> Vec<String> {
        extraction
            .references
            .iter()
            .filter(|r| r.kind == ReferenceKind::Type && r.assigned_to.as_deref() == Some(param))
            .map(|r| r.name.clone())
            .collect()
    };
    assert_eq!(typed("a"), ["Render"], "&dyn Render");
    assert_eq!(typed("b"), ["Render"], "Box<dyn Render>");
    assert_eq!(typed("c"), ["Render"], "impl Render");
    // A bound list is not one trait: neither `+ Send` nor `+ Clone` is read
    // as naming its first member.
    assert!(typed("d").is_empty(), "{:?}", typed("d"));
    assert!(typed("e").is_empty(), "{:?}", typed("e"));
}

/// A closure parameter is bound whether or not it is written `mut`: in
/// tree-sitter-rust `mut s` is a `mut_pattern` whose first named child is the
/// `mutable_specifier`, not the identifier, so a binder test that reads only
/// the first named child never sees the name. The control (`|s|`) proves the
/// fixture reaches the closure-parameter hint at all.
#[test]
fn rust_mut_closure_parameter_is_bound_like_a_plain_one() {
    let source = r#"
pub struct Engine;
impl Engine { pub fn tick(&self) {} }
pub fn apply(f: impl Fn(Engine)) {}
pub fn caller() {
    apply(|s| s.tick());
    apply(|mut s| s.tick());
}
"#;
    let extraction = extract_file("lib.rs", source);
    let hint_at = |needle: &str| -> Option<String> {
        let closure = source.find(needle).expect("fixture spells the closure");
        let mut bindings: Vec<&LocalBinding> = extraction
            .local_bindings
            .iter()
            .filter(|b| b.name == "s" && b.start_byte > closure)
            .collect();
        bindings.sort_by_key(|b| b.start_byte);
        bindings.first().and_then(|b| b.initializer.clone())
    };
    assert_eq!(
        hint_at("|s|").as_deref(),
        Some("|apply|0|0"),
        "control: {:?}",
        extraction.local_bindings
    );
    assert_eq!(
        hint_at("|mut s|").as_deref(),
        Some("|apply|0|0"),
        "`mut s` must bind like `s`: {:?}",
        extraction.local_bindings
    );
}

/// A Swift `if let task = task(from: i)` binds `task` after its initializer, so
/// the call inside the initializer names the method, not the new local; the
/// uses after it are the local's. `guard let session = s, session > 0` binds
/// `session` for the condition after the comma.
#[test]
fn a_swift_binding_is_not_a_local_in_its_own_initializer() {
    let source = "final class D {\n    func task(from i: Int) -> Int? { i }\n    func go(_ i: Int) {\n        if let task = task(from: i) { _ = task }\n    }\n    func check(_ s: Int?) {\n        guard let session = s, session > 0 else { return }\n    }\n}\n";
    let extraction = extract_file("a.swift", source);
    let at = |name: &str, start: usize| {
        extraction
            .local_bindings
            .iter()
            .any(|b| b.name == name && b.start_byte == start)
    };
    let call = source
        .find("task(from: i) {")
        .expect("the initializer's call");
    let later_use = source.find("_ = task").expect("the use") + "_ = ".len();
    assert!(
        !at("task", call),
        "the call in the initializer names the method: {:?}",
        extraction.local_bindings
    );
    assert!(
        at("task", later_use),
        "a use after the binding is the local: {:?}",
        extraction.local_bindings
    );
    let test = source
        .find("session > 0")
        .expect("the condition after the binding");
    assert!(
        at("session", test),
        "a plain test after a binding sees the new local: {:?}",
        extraction.local_bindings
    );
}

/// `self.engine = Engine()` assigns a field, not a local, and the extractor
/// says so with the `self.` spelling no local can have. Only a call that is the
/// whole right-hand side is stamped: a wrapped, nested or conditional value is
/// not what the call returns.
#[test]
fn a_python_self_field_initializer_is_stamped_with_the_field_spelling() {
    let source = "class H:\n    def __init__(self):\n        self.a = Engine()\n        self.b = mod.make()\n        self.c = [Engine()]\n        self.d = wrap(Engine())\n        self.e = Engine() if x else None\n        local = Engine()\n        other.f = Engine()\n";
    let extraction = extract_file("h.py", source);
    let stamped = |line: &str| -> Vec<(String, Option<String>)> {
        let start = source.find(line).expect("line");
        let end = start + line.len();
        extraction
            .references
            .iter()
            .filter(|r| r.kind == ReferenceKind::Call)
            .filter(|r| r.span.start_byte >= start && r.span.end_byte <= end)
            .map(|r| (r.name.clone(), r.assigned_to.clone()))
            .collect()
    };
    assert_eq!(
        stamped("self.a = Engine()"),
        vec![("Engine".to_string(), Some("self.a".to_string()))]
    );
    assert_eq!(
        stamped("self.b = mod.make()"),
        vec![("make".to_string(), Some("self.b".to_string()))]
    );
    // The outer call is the value; the argument is not.
    assert_eq!(
        stamped("self.d = wrap(Engine())"),
        vec![
            ("wrap".to_string(), Some("self.d".to_string())),
            ("Engine".to_string(), None)
        ]
    );
    for wrapped in ["self.c = [Engine()]", "self.e = Engine() if x else None"] {
        assert!(
            stamped(wrapped)
                .iter()
                .all(|(_, assigned)| assigned.as_deref().is_none_or(|a| !a.starts_with("self."))),
            "{wrapped}: {:?}",
            stamped(wrapped)
        );
    }
    assert_eq!(
        stamped("local = Engine()"),
        vec![("Engine".to_string(), Some("local".to_string()))]
    );
    assert!(
        stamped("other.f = Engine()")
            .iter()
            .all(|(_, assigned)| assigned.is_none()),
        "only `self` names a field of the enclosing class"
    );
}

/// What the callee names and which function writes it decide whether a
/// `self.x = ...()` types a field: a call of a call, a lambda, a subscript or a
/// static method's `self` parameter is not an instance's constructor call, and
/// a closure inside a method still writes the method's `self`.
#[test]
fn a_python_self_field_stamp_needs_a_named_callee_and_an_instance_method() {
    let source = "class H:\n    def a(self):\n        self.a = A()()\n    def b(self):\n        self.b = (lambda: A())()\n    def c(self):\n        self.c = fns[0]()\n    @staticmethod\n    def d(self):\n        self.d = A()\n    @classmethod\n    def e(self):\n        self.e = A()\n    def f(self):\n        def later():\n            self.f = A()\n    def g(self):\n        def later(self):\n            self.g = A()\n    def h(self):\n        self.h = pkg.mod.make()\n";
    let extraction = extract_file("h.py", source);
    let stamp = |line: &str| -> Vec<Option<String>> {
        let start = source.find(line).expect("line");
        let end = start + line.len();
        extraction
            .references
            .iter()
            .filter(|r| r.kind == ReferenceKind::Call)
            .filter(|r| r.span.start_byte >= start && r.span.end_byte <= end)
            .map(|r| r.assigned_to.clone())
            .collect()
    };
    let typed = |line: &str| {
        stamp(line)
            .iter()
            .any(|assigned| assigned.as_deref().is_some_and(|a| a.starts_with("self.")))
    };
    assert!(!typed("self.a = A()()"), "call of a call");
    assert!(!typed("self.b = (lambda: A())()"), "lambda");
    assert!(!typed("self.c = fns[0]()"), "subscript callee");
    assert!(
        !typed("self.d = A()"),
        "static method's `self` is a parameter"
    );
    assert!(
        !typed("self.e = A()"),
        "classmethod's first parameter is the class"
    );
    assert!(typed("self.f = A()"), "a closure writes the method's self");
    assert!(
        !typed("self.g = A()"),
        "a nested function with its own `self`"
    );
    assert!(typed("self.h = pkg.mod.make()"), "a dotted callee");
}

/// A class header's keyword arguments configure the statement and are not
/// bases; a subscripted base is the base named by its value.
#[test]
fn a_python_class_header_names_only_its_positional_bases() {
    let source = "class C(Base, metaclass=Meta, flag=Other):\n    pass\n\nclass D(Gen[int], pkg.Mixin):\n    pass\n";
    let extraction = extract_file("c.py", source);
    let bases = |class: &str| -> Vec<String> {
        extraction
            .references
            .iter()
            .filter(|r| r.kind == ReferenceKind::Heritage)
            .filter(|r| {
                r.enclosing_symbol
                    .as_deref()
                    .is_some_and(|s| s.ends_with(&format!("::{class}")))
            })
            .map(|r| r.name.clone())
            .collect()
    };
    assert_eq!(bases("C"), ["Base"]);
    // A dotted base is one reference per segment over one span: the member
    // first (the name the resolver reads the base's own name from), then the
    // qualifier from the root down.
    assert_eq!(bases("D"), ["Gen", "Mixin", "pkg"]);
    let deep = extract_file("e.py", "class E(a.b.C):\n    pass\n");
    let segments: Vec<String> = deep
        .references
        .iter()
        .filter(|r| r.kind == ReferenceKind::Heritage)
        .map(|r| r.name.clone())
        .collect();
    assert_eq!(segments, ["C", "a", "b"]);
}

/// No linked grammar emits an `async_function_definition` node: Python spells
/// `async def` as a `function_definition` with an anonymous `async` token, and
/// the other grammars' async forms are modifiers on their own function nodes.
/// This pins that an `async def` extracts exactly like a plain `def` at every
/// position (top level, method, nested, decorated), symbols and calls alike.
#[test]
fn a_python_async_def_extracts_like_a_def_at_every_position() {
    let shape = |source: &str| {
        let extraction = extract_file("a.py", source);
        let symbols: Vec<(String, String, Option<String>)> = extraction
            .symbols
            .iter()
            .map(|s| {
                (
                    s.qualified_name.clone(),
                    format!("{:?}", s.kind),
                    s.parent_symbol.clone(),
                )
            })
            .collect();
        let calls: Vec<(Option<String>, String)> = extraction
            .calls
            .iter()
            .map(|c| (c.caller_symbol.clone(), c.callee_name.clone()))
            .collect();
        (symbols, calls)
    };
    let template = [
        "import functools",
        "@functools.cache",
        "KW top():",
        "    helper()",
        "class Svc:",
        "    KW method(self):",
        "        self.step()",
        "    @staticmethod",
        "    KW decorated():",
        "        build()",
        "KW outer():",
        "    KW inner():",
        "        leaf()",
        "    inner()",
        "",
    ]
    .join("\n");
    let sync = template.replace("KW", "def");
    let asynchronous = template.replace("KW", "async def");
    let (sync_symbols, sync_calls) = shape(&sync);
    let (async_symbols, async_calls) = shape(&asynchronous);

    let names: Vec<&str> = async_symbols.iter().map(|s| s.0.as_str()).collect();
    assert_eq!(
        names,
        [
            "a.py",
            "a.py::top",
            "a.py::Svc",
            "a.py::Svc.method",
            "a.py::Svc.decorated",
            "a.py::outer",
            "a.py::outer.inner",
        ]
    );
    for callee in ["helper", "step", "build", "leaf", "inner"] {
        assert!(
            async_calls.iter().any(|c| c.1 == callee && c.0.is_some()),
            "{callee} lost its caller: {async_calls:?}"
        );
    }
    assert_eq!(sync_symbols, async_symbols, "async def must extract as def");
    assert_eq!(
        sync_calls, async_calls,
        "async def calls must attribute as def"
    );
}
