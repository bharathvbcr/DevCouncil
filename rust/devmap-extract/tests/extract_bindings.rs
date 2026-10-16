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
        "90",
        "v90 records `macro_rules!` macros as Macro symbols their same-file invocations reference, blanket-impl type parameters, `?` From candidates and calls through a same-file const fn pointer; v89 types a Rust `dyn Trait` / `impl Trait` binding by its one named trait and records a literal Python `importlib.import_module` as a scoped module handle; v88 is the dead-policy lane's payload (function-local Rust impl keys, Kotlin callable-reference receivers on name references, Rust `fn main` outside `src/`, Rust restricted-visibility and SwiftPM executable-target scopes, indexed project.pbxproj, a qualified JS/TS base as one Heritage reference per segment); v87 records a Kotlin parameter, typed local and constructor property as a typed binding, a Kotlin/Java file's package header, Kotlin scope-function lambdas, callable references and written return types, Android manifests as config, and Ruby attributes and mixins as methods and heritage; v86 records Swift protocol requirements as symbols and scope-aware Swift locals; v85 records the implicit `__enter__` call of a Python `with Class(..)`; v84 binds Swift and Kotlin parameters and closure parameters as local bindings; v83 records the TypeScript workspace-package map and the member-qualified ids of class-field and object-literal members, and an `export default` as an export named `default`; v82 records a Go file's declarations, local scopes and call receivers as structured facts (`go_facts`), the input of the resolver's Go type evaluator; v81 records inline Rust modules, `Deref` targets, implicit operator/drop/from candidates, `generate_handler!` lists and `macro_rules!` expansions, and drops macro member names as references; v80 records which partial-parse error regions hold no name; v79 is both lanes' v78 payloads together, under a number neither stamped: the read-tools lane's v78 withholds a secret file's literal values, docstrings and string-made references; the dead-policy lane's v78 adds the dead-policy extractor fixes (cgo/linkname and Tauri entry points, TS ambient exemptions, destructuring defaults as uses, Swift custom-operator calls and standard-operator exemptions, Swift `Type.member` receivers); v77 adds macro-stamped C-family functions and EntryName references on top of v76; v76 is v73, v74 and the literal half of v75 together, because extraction-recall and metal-kernels both stamped 75 on different payloads; v75 records string literals and the uses of a Rust module-level const; v74 binds the names a CommonJS `require` declares; v73 references the type a Rust struct literal constructs (`Name {{ .. }}`, `Self {{ .. }}`); v72 adds the MCP-parity lane's v67 (route middleware and Go routes); v71 adds the resolver-precision lane's v67 (Rust function headers, Rust locals typed from their own binders, a Go `var w T`); v70 merges v68 and v69 under a number neither lane stamped; v69 records each callable's written return type and binds a single-name Go var/const spec's initializer; v68 stops a JSX tag, a C# member access, an annotation, an attribute or a preprocessor test from binding a local, and binds wrapped callbacks (`useCallback(() => …)`, `memo`, `forwardRef`) as functions; v66 emits every Python module-scope binding, multi-name Go specs, destructured TS/JS names and type aliases, reads import names off the tree, and stops a module binding shadowing its module-level reads; v65 references the scope of `T::m` and records the calls inside a function-like macro body; v64 stops filing a C++ template callee or template type use as a local binding and annotates a Python function decorated with a parameter of its enclosing callable; v63 records in-function Python `sys.path` inserts as scoped vetoes; v62 records Python `sys.path` entries; v61 reads Python path constants and parameterised path loaders; v60 records a Go file's unexported field and interface-method names, the veto for the unexported-selector rung; v59 records a Python module loaded by file path as a `path_load` import; v58 records a shell `source` as a glob import and a `build.rs`'s `cargo:rerun-if-changed=` paths as dynamic references, and stops emitting a Go struct-literal field key as a name reference, on top of v57's object-literal method exemption, v56's Rust `pub use` exports, v55's inline-module `super` receiver, v54's declared-field types, v53's receiver-prefix reduction and v52's duplicate-callee drop, while retaining v48 bindings, v49 ruleguard wiring, v50 attribution fixes and v51 captured-receiver classifications"
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
