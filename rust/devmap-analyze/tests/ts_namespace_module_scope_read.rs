//! ft-260158ce: a TS `namespace NS {}` / `module X {}` read only at file scope
//! (`console.log(NS.a)`) was published dead at 0.9.
//!
//! Root cause, in the extractor: `is_symbol_binding` has no `internal_module`
//! arm, so the namespace's own name is filed as a file-scope local and
//! `name_is_shadowed_by_local` drops every non-call read of it at file scope.
//! Extraction output cannot change in this lane (EXTRACTION_SCHEMA_VERSION
//! stays 97), so the analysis caps the row below the confident tier instead.
//! The neighbouring shapes (enum, static member, object constant, Python class
//! attribute) emit their head reference and must stay live.

use devmap_analyze::*;
use devmap_extract::extract_file;
use devmap_extract::model::*;
use devmap_resolve::*;

fn reports(path: &str, src: &str) -> Vec<DeadSymbolReport> {
    let extractions: Vec<Extraction> = vec![extract_file(path, src)];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    analyze_liveness(&extractions, &resolution)
}

/// Non-exempt rows in the confident branch, as `name conf=..`.
fn confident(path: &str, src: &str) -> Vec<String> {
    reports(path, src)
        .iter()
        .filter(|r| !r.is_exempt)
        .filter(|r| {
            dead_tier::in_confident_branch(f64::from(r.confidence), r.exemption_reason.as_deref())
        })
        .map(|r| format!("{} conf={:?}", r.symbol_name, r.confidence))
        .collect()
}

fn row<'a>(rows: &'a [DeadSymbolReport], name: &str) -> Option<&'a DeadSymbolReport> {
    rows.iter().find(|r| !r.is_exempt && r.symbol_name == name)
}

#[test]
fn a_namespace_read_at_file_scope_is_not_confidently_dead() {
    let src = "namespace NS { export const a = 1; }\nconsole.log(NS.a);\n";
    assert_eq!(confident("src/t.ts", src), Vec::<String>::new());
    let rows = reports("src/t.ts", src);
    let ns = row(&rows, "NS").expect("the namespace stays visible, unconfirmed");
    assert_eq!(
        ns.exemption_reason.as_deref(),
        Some(NAMESPACE_READ_UNEXTRACTED)
    );
    assert!(ns.confidence <= HIGHEST_DEGRADED_CONFIDENCE);
}

#[test]
fn the_module_keyword_is_capped_the_same_way() {
    let src = "module Legacy { export const a = 1; }\nconsole.log(Legacy.a);\n";
    assert_eq!(confident("src/t.ts", src), Vec::<String>::new());
}

#[test]
fn a_tsx_namespace_is_capped_the_same_way() {
    let src = "namespace NS { export const a = 1; }\nconsole.log(NS.a);\n";
    assert_eq!(confident("src/t.tsx", src), Vec::<String>::new());
}

#[test]
fn a_namespace_that_nothing_reads_is_still_reported() {
    // The facts cannot tell this from the file-scope read above, so it is
    // reported at the same unconfirmed tier: visible, with the reason, never
    // silently dropped.
    let src = "namespace Unused { export const a = 1; }\nexport const keep = 1;\n";
    let rows = reports("src/t.ts", src);
    let unused = row(&rows, "Unused").expect("an unused namespace is still reported");
    assert_eq!(
        unused.exemption_reason.as_deref(),
        Some(NAMESPACE_READ_UNEXTRACTED)
    );
}

#[test]
fn a_namespace_read_inside_a_function_is_live() {
    let src =
        "namespace Util { export const x = 1; }\nfunction main() { return Util.x; }\nmain();\n";
    assert_eq!(confident("src/t.ts", src), Vec::<String>::new());
    assert!(row(&reports("src/t.ts", src), "Util").is_none());
}

#[test]
fn a_namespace_function_called_at_file_scope_is_live() {
    let src = "namespace NS { export function f() { return 1; } }\nconsole.log(NS.f());\n";
    // The call is bound to the member, not to the namespace, so `NS` itself has
    // no inbound edge: pre-fix it was published at 0.9 as well.
    assert_eq!(confident("src/t.ts", src), Vec::<String>::new());
}

#[test]
fn an_enum_read_at_file_scope_stays_live() {
    let src = "enum E { A, B }\nconsole.log(E.A);\n";
    assert!(row(&reports("src/t.ts", src), "E").is_none());
}

#[test]
fn a_class_static_read_at_file_scope_stays_live() {
    let src = "class C { static x = 1; }\nconst y = C.x;\nconsole.log(y);\n";
    assert!(row(&reports("src/t.ts", src), "C").is_none());
}

#[test]
fn an_object_constant_member_read_at_file_scope_stays_live() {
    let src = "const O = { f() { return 1; }, k: 2 };\nconsole.log(O.k);\n";
    assert_eq!(confident("src/t.ts", src), Vec::<String>::new());
    assert!(row(&reports("src/t.ts", src), "O").is_none());
}

#[test]
fn a_python_class_attribute_read_at_module_scope_stays_live() {
    let src = "class Cls:\n    attr = 1\n\nprint(Cls.attr)\n";
    let rows = reports("lib/m.py", src);
    assert!(row(&rows, "Cls").is_none(), "{rows:?}");
}

fn multi(sources: &[(&str, &str)]) -> Vec<DeadSymbolReport> {
    let extractions: Vec<Extraction> = sources.iter().map(|(p, s)| extract_file(p, s)).collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    analyze_liveness(&extractions, &resolution)
}

#[test]
fn an_exported_namespace_read_at_its_own_file_scope_is_not_confidently_dead() {
    let own = "export namespace NS { export const a = 1; }\nconsole.log(NS.a);\n";
    let rows = multi(&[("src/a.ts", own)]);
    let bad: Vec<_> = rows
        .iter()
        .filter(|r| !r.is_exempt)
        .filter(|r| {
            dead_tier::in_confident_branch(f64::from(r.confidence), r.exemption_reason.as_deref())
        })
        .map(|r| format!("{} conf={:?}", r.symbol_name, r.confidence))
        .collect();
    assert!(bad.is_empty(), "{bad:?}");
}

#[test]
fn an_exported_namespace_read_from_another_file_is_live() {
    let a = "export namespace NS { export const a = 1; }\n";
    let b = "import { NS } from './a';\nconsole.log(NS.a);\n";
    let rows = multi(&[("src/a.ts", a), ("src/b.ts", b)]);
    let bad: Vec<_> = rows
        .iter()
        .filter(|r| !r.is_exempt)
        .filter(|r| {
            dead_tier::in_confident_branch(f64::from(r.confidence), r.exemption_reason.as_deref())
        })
        .map(|r| format!("{} conf={:?}", r.symbol_name, r.confidence))
        .collect();
    assert!(bad.is_empty(), "{bad:?}");
}

#[test]
fn a_python_class_never_read_is_still_reported() {
    let src = "class Cls:\n    attr = 1\n\nprint(1)\n";
    let rows = reports("lib/m.py", src);
    assert!(row(&rows, "Cls").is_some(), "{rows:?}");
}
