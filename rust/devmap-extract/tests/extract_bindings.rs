//! Extract-bindings hardening (schema v48+; current schema tracks ruleguard too).
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
        "49",
        "v49 adds dsl.Matcher RuntimeEntryPoint wiring on top of v48 bindings"
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
