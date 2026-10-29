//! A class a file declares, or imports, is the one its own receivers name.
//!
//! `type_methods` is keyed by the bare type name, so a non-exported `class
//! Parser` in each of several files made `p.parse()` ambiguous in every one of
//! them (Orbit's `typescript/js_module_resolution`, `advanced/` and `ssa/`).
//! Every positive case sits beside one that must stay silent: a type no file
//! here declares or imports, and an import a nested class of the same name does
//! not displace.

use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, Extraction};
use devmap_resolve::{ResolutionResult, Resolver};

fn resolve(files: &[(&str, &str)]) -> ResolutionResult {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, body)| extract_file(path, body))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    resolver.resolve_all(&extractions).expect("resolution")
}

fn calls(result: &ResolutionResult, from: &str) -> Vec<String> {
    let mut found: Vec<String> = result
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls && edge.source_symbol == from)
        .map(|edge| edge.target_symbol.clone())
        .collect();
    found.sort();
    found.dedup();
    found
}

const PARSER: &str = "class Parser {\n  parse(i: string): string[] { return [i]; }\n}\n";

#[test]
fn a_class_declared_in_this_file_wins_over_a_namesake_in_another() {
    let result = resolve(&[
        (
            "a/one.ts",
            &format!(
                "{PARSER}export function viaNew(): string[] {{ const p = new Parser(); return p.parse('x'); }}\n\
                 export function viaParam(s: Parser): string[] {{ return s.parse('y'); }}\n"
            ),
        ),
        (
            "b/two.ts",
            &format!("{PARSER}export function other(): string[] {{ return new Parser().parse('x'); }}\n"),
        ),
    ]);
    for caller in ["viaNew", "viaParam"] {
        let found = calls(&result, &format!("a/one.ts::{caller}"));
        assert!(
            found.contains(&"a/one.ts::Parser.parse".to_string()),
            "{caller}: {found:?} / {:?}",
            result.unresolved
        );
        assert!(
            !found.iter().any(|target| target.starts_with("b/two.ts")),
            "{caller}: {found:?}"
        );
    }
}

#[test]
fn a_type_this_file_does_not_declare_stays_ambiguous_between_namesakes() {
    let result = resolve(&[
        ("a/one.ts", PARSER),
        ("b/two.ts", PARSER),
        (
            "c/user.ts",
            "export function run(s: Parser): string[] { return s.parse('y'); }\n",
        ),
    ]);
    assert!(
        calls(&result, "c/user.ts::run")
            .iter()
            .all(|target| !target.ends_with("Parser.parse")),
        "{:?}",
        calls(&result, "c/user.ts::run")
    );
}

#[test]
fn a_named_import_of_a_type_is_not_shadowed_by_a_namesake_elsewhere() {
    let result = resolve(&[
        ("a/one.ts", &format!("export {PARSER}")),
        ("b/two.ts", &format!("export {PARSER}")),
        (
            "c/user.ts",
            "import { Parser } from '../a/one';\n\
             export function run(s: Parser): string[] { return s.parse('y'); }\n",
        ),
    ]);
    assert_eq!(calls(&result, "c/user.ts::run"), ["a/one.ts::Parser.parse"]);
}

#[test]
fn a_nested_class_of_the_imported_name_does_not_displace_the_import() {
    let result = resolve(&[
        ("a/one.ts", &format!("export {PARSER}")),
        (
            "c/user.ts",
            &format!(
                "import {{ Parser }} from '../a/one';\n\
                 export function run(s: Parser): string[] {{ return s.parse('y'); }}\n\
                 export function inner() {{ {PARSER} return 1; }}\n"
            ),
        ),
    ]);
    assert_eq!(calls(&result, "c/user.ts::run"), ["a/one.ts::Parser.parse"]);
}
