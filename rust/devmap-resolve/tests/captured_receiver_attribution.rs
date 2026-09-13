//! Captured value bindings must remain evidence when dispatch is unknown.
use devmap_extract::extract_file;
use devmap_extract::model::{Extraction, ReferenceKind};
use devmap_resolve::model::{ResolutionResult, UnresolvedClass, UnresolvedKind};
use devmap_resolve::Resolver;

fn resolve(extraction: &Extraction) -> ResolutionResult {
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(extraction));
    resolver
        .resolve_all(std::slice::from_ref(extraction))
        .unwrap()
}

fn assert_class(result: &ResolutionResult, name: &str, expected: UnresolvedClass) {
    let rows: Vec<_> = result
        .unresolved
        .iter()
        .filter(|row| row.callee_name == name)
        .collect();
    assert!(!rows.is_empty(), "no evidence for {name}: {result:?}");
    assert!(
        rows.iter().all(|row| row.class == expected),
        "{name} must be {expected:?}: {rows:?}"
    );
}

const CAPTURE: &str = r#"
export function outer(Math: number) {
    function inner() {
        const value = Math.toFixed();
        const method = Math.toFixed;
        return [value, method];
    }
    return inner();
}
"#;

#[test]
fn primitive_capture_uses_existing_binding_facts_for_calls_and_references() {
    let extraction = extract_file("src/capture.ts", CAPTURE);
    let call = extraction
        .calls
        .iter()
        .find(|call| call.callee_name == "toFixed")
        .expect("method call was extracted");
    let binding = extraction
        .local_binding_at(call.span.start_byte, "Math")
        .expect("the existing extractor knows the captured binding");
    assert_eq!(binding.scope.as_deref(), Some("src/capture.ts::outer"));
    assert!(
        binding.declared_type.is_none(),
        "primitive annotation is not a named type fact"
    );
    let reference = extraction
        .references
        .iter()
        .find(|reference| reference.name == "toFixed" && reference.kind == ReferenceKind::Name)
        .expect("method value reference was extracted");
    assert!(extraction
        .local_binding_at(reference.span.start_byte, "Math")
        .is_some());
    let result = resolve(&extraction);
    for kind in [UnresolvedKind::Call, UnresolvedKind::Reference] {
        assert!(result
            .unresolved
            .iter()
            .any(|row| row.callee_name == "toFixed" && row.kind == kind));
    }
    assert_class(&result, "toFixed", UnresolvedClass::UninferredReceiver);
}

#[test]
fn captured_binding_survives_without_source_text() {
    let mut extraction = extract_file("src/capture.ts", CAPTURE);
    extraction.source_code = None;
    assert_class(
        &resolve(&extraction),
        "toFixed",
        UnresolvedClass::UninferredReceiver,
    );
}

#[test]
fn a_sibling_capture_does_not_hide_the_real_global() {
    let extraction = extract_file(
        "src/siblings.ts",
        &format!("{CAPTURE}\nexport function global() {{ return Math.max(1, 2); }}\n"),
    );
    let result = resolve(&extraction);
    assert_class(&result, "toFixed", UnresolvedClass::UninferredReceiver);
    assert!(result
        .unresolved
        .iter()
        .filter(|row| row.callee_name == "max")
        .all(|row| matches!(row.class, UnresolvedClass::HostGlobal { .. })));
    assert!(result.unresolved.iter().any(|row| row.callee_name == "max"));
}

#[test]
fn reassignment_does_not_turn_a_captured_value_into_a_global() {
    let extraction = extract_file(
        "src/reassigned.ts",
        r#"
export function outer(Math: number) {
    Math = 4;
    function inner() { Math = 8; return Math.toFixed(); }
    return inner();
}
"#,
    );
    assert_class(
        &resolve(&extraction),
        "toFixed",
        UnresolvedClass::UninferredReceiver,
    );
}

#[test]
fn missing_site_facts_do_not_erase_existing_scope_evidence() {
    let mut extraction = extract_file(
        "src/older.ts",
        "export function outer(Math: number) { return Math.toFixed(); }",
    );
    extraction.local_bindings.clear();
    extraction.source_code = None;
    assert_class(
        &resolve(&extraction),
        "toFixed",
        UnresolvedClass::UninferredReceiver,
    );
}

#[test]
fn a_captured_value_shadows_the_same_named_module_import() {
    let extraction = extract_file(
        "src/import_shadow.ts",
        &format!("import * as Math from 'outside';\n{CAPTURE}"),
    );
    assert_class(
        &resolve(&extraction),
        "toFixed",
        UnresolvedClass::UninferredReceiver,
    );
}

#[test]
fn a_primitive_capture_cannot_borrow_a_sibling_parameters_imported_type() {
    let extraction = extract_file("src/type_shadow.ts", &format!("import {{ Client }} from 'outside';\nexport function typed(Math: Client) {{ return Math.remote(); }}\n{CAPTURE}"));
    let result = resolve(&extraction);
    assert_class(&result, "toFixed", UnresolvedClass::UninferredReceiver);
    assert_class(
        &result,
        "remote",
        UnresolvedClass::External {
            module: "outside".into(),
        },
    );
}

#[test]
fn an_anonymous_primitive_parameter_cannot_borrow_its_graph_callers_type() {
    let extraction = extract_file(
        "src/anonymous.ts",
        r#"
import { Client } from 'outside';
export function run(Math: Client, items: number[]) {
    const values = items.map((Math: number) => Math.toFixed());
    return [Math.remote(), values];
}
"#,
    );
    let call = extraction
        .calls
        .iter()
        .find(|call| call.callee_name == "toFixed")
        .unwrap();
    let binding = extraction
        .local_binding_at(call.span.start_byte, "Math")
        .unwrap();
    assert!(
        binding.scope.is_none(),
        "anonymous scope has no graph identity"
    );
    assert!(binding.declared_type.is_none());
    let result = resolve(&extraction);
    assert_class(&result, "toFixed", UnresolvedClass::UninferredReceiver);
    assert_class(
        &result,
        "remote",
        UnresolvedClass::External {
            module: "outside".into(),
        },
    );
}

#[test]
fn typed_receivers_keep_their_import_and_prelude_explanations() {
    for (path, source, name, expected) in [
        ("src/imported.ts", "import { Client } from 'outside'; export function useClient(client: Client) { return client.get(); }", "get", UnresolvedClass::External { module: "outside".into() }),
        ("Sources/App/Value.swift", "func useValue(_ value: String) { _ = value.lowercased() }", "lowercased", UnresolvedClass::Builtin),
        ("Sources/App/URL.swift", "import Foundation\nfunc useURL(_ value: URL) { _ = value.appendingPathComponent(\"x\") }", "appendingPathComponent", UnresolvedClass::External { module: "Foundation".into() }),
    ] {
        assert_class(&resolve(&extract_file(path, source)), name, expected);
    }
}
