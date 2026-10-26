//! Adversarial probes for the scope-only `@mod` qualifier read
//! (`qualifier_declared_in`). Each probe asserts what the *correct* behaviour
//! is; a failing probe is a BREAK. Probe numbers follow the attack brief.

use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
use devmap_resolve::model::{ResolutionResult, UnresolvedClass};
use devmap_resolve::Resolver;

fn resolve(path: &str, source: &str) -> ResolutionResult {
    let extractions: Vec<Extraction> = vec![extract_file(path, source)];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    resolver.resolve_all(&extractions).unwrap()
}

fn dump(path: &str, source: &str) -> String {
    let ex = extract_file(path, source);
    let mut out = String::new();
    for r in &ex.references {
        out.push_str(&format!(
            "REF {:?} {} enc={:?} as={:?}\n",
            r.kind, r.name, r.enclosing_symbol, r.assigned_to
        ));
    }
    let res = resolve(path, source);
    for e in &res.edges {
        out.push_str(&format!(
            "EDGE {} -> {}\n",
            e.source_symbol, e.target_symbol
        ));
    }
    for u in &res.unresolved {
        out.push_str(&format!("UNRES {} {:?}\n", u.callee_name, u.class));
    }
    out
}

fn has_edge(res: &ResolutionResult, src: &str, tgt: &str) -> bool {
    res.edges.iter().any(|e| {
        e.source_symbol.rsplit(['.', ':']).next() == Some(src)
            && e.target_symbol.rsplit(['.', ':']).next() == Some(tgt)
    })
}

fn classes(res: &ResolutionResult, callee: &str) -> Vec<UnresolvedClass> {
    res.unresolved
        .iter()
        .filter(|r| r.callee_name == callee)
        .map(|r| r.class.clone())
        .collect()
}

fn all_external(cs: &[UnresolvedClass]) -> bool {
    !cs.is_empty()
        && cs
            .iter()
            .all(|c| matches!(c, UnresolvedClass::External { .. }))
}

// ---------------------------------------------------------------- probe 1
const GO_SHAPES: &str = "package p\n\nimport \"encoding/json\"\n\ntype t struct{}\ntype Res struct{}\n\nvar pkgv json.RawMessage\n\nfunc F(params json.RawMessage) (out json.RawMessage, err error) {\n\tfn := func() { params.X() }\n\tfn()\n\tif true {\n\t\tvar blk json.RawMessage\n\t\t_ = blk\n\t}\n\tfor i := 0; i < 1; i++ {\n\t\tparams.Z()\n\t}\n\treturn\n}\n\nfunc (r *Res) M(p json.RawMessage) { p.Q() }\n\nfunc shadow() {\n\tvar pkgv t\n\t_ = pkgv\n}\n";

// A qualified local used inside a func literal / nested block of its function
// must still classify External (it did via the file-wide slot at base).
#[test]
fn p1_go_closure_use_of_qualified_param_is_external() {
    let res = resolve("p/p.go", GO_SHAPES);
    assert!(
        all_external(&classes(&res, "RawMessage")),
        "RawMessage classes: {:?}\n{}",
        classes(&res, "RawMessage"),
        dump("p/p.go", GO_SHAPES)
    );
}

const GO_NAMED_RET: &str = "package p\n\nimport \"encoding/json\"\n\ntype t struct{}\n\nfunc G() (out json.RawMessage, err error) { return }\n\nfunc H() { var out t; _ = out }\n";

#[test]
fn p1_go_named_return_does_not_qualify_a_bare_sibling_local() {
    let res = resolve("p/p.go", GO_NAMED_RET);
    assert!(has_edge(&res, "H", "t"), "{}", dump("p/p.go", GO_NAMED_RET));
}

const GO_RECV: &str = "package p\n\nimport \"encoding/json\"\n\ntype Res struct{}\n\nfunc (r *Res) M(p json.RawMessage) { _ = p }\n";

#[test]
fn p1_go_method_param_qualified_is_external() {
    let res = resolve("p/p.go", GO_RECV);
    assert!(
        all_external(&classes(&res, "RawMessage")),
        "{:?}\n{}",
        classes(&res, "RawMessage"),
        dump("p/p.go", GO_RECV)
    );
}

const GO_SHADOW: &str = "package p\n\nimport \"encoding/json\"\n\ntype t struct{}\n\nvar pkgv json.RawMessage\n\nfunc shadow() {\n\tvar pkgv t\n\t_ = pkgv\n}\n";

#[test]
fn p1_go_local_shadowing_qualified_package_var_binds_bare_type() {
    let res = resolve("p/p.go", GO_SHADOW);
    assert!(
        has_edge(&res, "shadow", "t"),
        "{}",
        dump("p/p.go", GO_SHADOW)
    );
}

// ---------------------------------------------------------------- probe 3
const GO_PKG_LEVEL: &str =
    "package p\n\nimport \"encoding/json\"\n\nvar a json.RawMessage\nvar b = json.RawMessage{}\n";

#[test]
fn p3_go_package_level_qualified_type_is_external() {
    let res = resolve("p/p.go", GO_PKG_LEVEL);
    assert!(
        all_external(&classes(&res, "RawMessage")),
        "{:?}\n{}",
        classes(&res, "RawMessage"),
        dump("p/p.go", GO_PKG_LEVEL)
    );
}

const GO_FIELD: &str = "package p\n\nimport \"encoding/json\"\n\ntype S struct {\n\tf json.RawMessage\n}\n\ntype T2 struct{}\n\nfunc use() { var f T2; _ = f }\n";

#[test]
fn p3_go_struct_field_qualified_type_does_not_leak_into_function() {
    let res = resolve("p/p.go", GO_FIELD);
    assert!(
        all_external(&classes(&res, "RawMessage")),
        "{:?}\n{}",
        classes(&res, "RawMessage"),
        dump("p/p.go", GO_FIELD)
    );
    assert!(has_edge(&res, "use", "T2"), "{}", dump("p/p.go", GO_FIELD));
}

// ---------------------------------------------------------------- probe 2
// Typed locals (not parameters) in other families.
fn bare_binds(path: &str, src: &str, user: &str, target: &str) {
    let res = resolve(path, src);
    assert!(
        has_edge(&res, user, target),
        "{path}: `{user}` has a bare `{target}` local\n{}",
        dump(path, src)
    );
}

#[test]
fn p2_rust_typed_local() {
    bare_binds(
        "lib.rs",
        "use serde_json;\n\npub struct Value;\n\npub fn first() { let p: serde_json::Value = todo!(); }\n\npub fn second() { let p: Value = Value; }\n",
        "second",
        "Value",
    );
}

#[test]
fn p2_typescript_typed_local() {
    bare_binds(
        "m.ts",
        "import * as ns from 'lib';\n\nexport class T {}\n\nexport function first() { const p: ns.T = null as any; }\n\nexport function second() { const p: T = new T(); }\n",
        "second",
        "T",
    );
}

#[test]
fn p2_python_annotated_local() {
    bare_binds(
        "m.py",
        "import mod\n\nclass T:\n    pass\n\ndef first():\n    p: mod.T = None\n\ndef second():\n    p: T = T()\n",
        "second",
        "T",
    );
}

#[test]
fn p2_kotlin_typed_val() {
    bare_binds(
        "A.kt",
        "import lib.Raw\n\nclass T\n\nfun first() { val p: lib.T? = null }\n\nfun second() { val p: T = T() }\n",
        "second",
        "T",
    );
}

#[test]
fn p2_swift_typed_let() {
    bare_binds(
        "A.swift",
        "import Foundation\n\nstruct T {}\n\nfunc first() { let p: Foundation.T? = nil }\n\nfunc second() { let p: T = T() }\n",
        "second",
        "T",
    );
}

// ---------------------------------------------------------------- probe 3
#[test]
fn p3_python_module_level_annotation_stays_external() {
    let src = "import mod\n\np: mod.Thing = None\n";
    let res = resolve("m.py", src);
    assert!(
        all_external(&classes(&res, "Thing")),
        "{:?}\n{}",
        classes(&res, "Thing"),
        dump("m.py", src)
    );
}

// ---------------------------------------------------------------- probe 3b
// A reference whose `enclosing_symbol` is `None` is classified with
// `source_symbol = ext.file_path` (resolve_all.rs), which `classify_unresolved`
// hands to `qualifier_declared_in` as `Some(file_path)`: the scope key
// `{file}:{file}:{typed}@mod` is never written, so the file-wide slot that the
// helper's own doc says answers unscoped references is unreachable here.
const GO_BODILESS: &str =
    "package p\n\nimport \"encoding/json\"\n\nfunc native(params json.RawMessage) int\n";

const GO_IFACE: &str = "package p\n\nimport \"encoding/json\"\n\ntype I interface {\n\tM(params json.RawMessage) int\n}\n\nfunc native2(a, b json.RawMessage, c *json.RawMessage) int\n";

// Same shape as the bodiless probe: every function-with-no-body declaration
// (assembly-backed Go functions) keeps `enclosing_symbol = None`.
#[test]
fn p3_go_bodiless_func_every_param_form_is_external() {
    let res = resolve("p/p.go", GO_IFACE);
    let cs = classes(&res, "RawMessage");
    assert!(all_external(&cs), "{cs:?}\n{}", dump("p/p.go", GO_IFACE));
}

#[test]
fn p3_go_bodiless_func_qualified_param_is_external() {
    let res = resolve("p/p.go", GO_BODILESS);
    assert!(
        all_external(&classes(&res, "RawMessage")),
        "{:?}\n{}",
        classes(&res, "RawMessage"),
        dump("p/p.go", GO_BODILESS)
    );
}
