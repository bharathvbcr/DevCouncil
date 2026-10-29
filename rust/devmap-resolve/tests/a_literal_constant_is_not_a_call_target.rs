//! A call is not an edge to a constant nothing can run.
//!
//! `export const DEFAULT_SCOPE = 'project'` imported and then written
//! `DEFAULT_SCOPE()` is a bug in the caller, not a call: the import rung bound
//! it to the constant at DETERMINISTIC confidence. Orbit's fixtures assert the
//! absence ("imported constants do not create false call edges"), and a bundler
//! alias or a barrel makes the same shape reachable through more specifiers.
//!
//! Only a declaration that holds nothing callable is inert. A value built by a
//! call, named from elsewhere or destructured stays a target, because what it
//! holds is not known.

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

#[test]
fn a_called_imported_literal_constant_is_no_edge_but_a_called_function_is() {
    let result = resolve(&[
        (
            "c.ts",
            "export const HTTP_STATUS_OK = 200;\nexport function formatter(v: string) { return v; }\n",
        ),
        (
            "m.ts",
            "import { HTTP_STATUS_OK, formatter } from './c';\n\
             export function run(v: string) { return formatter(v); }\n\
             export function bad() { return HTTP_STATUS_OK(); }\n",
        ),
    ]);
    assert_eq!(calls(&result, "m.ts::run"), ["c.ts::formatter"]);
    assert!(
        calls(&result, "m.ts::bad").is_empty(),
        "{:?}",
        calls(&result, "m.ts::bad")
    );
}

#[test]
fn string_array_object_and_regex_constants_are_inert() {
    for (name, declaration) in [
        ("S", "export const S = 'project';"),
        ("A", "export const A = ['a', 'b'];"),
        ("O", "export const O = { a: 1, b: 'two' };"),
        ("R", "export const R = /ab+c/;"),
        ("N", "export const N = -1 * 24;"),
    ] {
        let result = resolve(&[
            ("c.ts", declaration),
            (
                "m.ts",
                &format!("import {{ {name} }} from './c';\nexport function bad() {{ return {name}(); }}\n"),
            ),
        ]);
        assert!(calls(&result, "m.ts::bad").is_empty(), "{name}");
    }
}

#[test]
fn a_value_that_may_be_callable_stays_a_target() {
    // Built by a call, named from elsewhere, destructured: all unknown, all kept.
    for (name, declaration) in [
        ("hook", "export const hook = createHook();"),
        (
            "alias",
            "import { other } from './o';\nexport const alias = other;",
        ),
        (
            "picked",
            "const bag = makeBag();\nexport const { picked } = bag;",
        ),
    ] {
        let result = resolve(&[
            ("c.ts", declaration),
            ("o.ts", "export function other() { return 1; }\n"),
            (
                "m.ts",
                &format!("import {{ {name} }} from './c';\nexport function use() {{ return {name}(); }}\n"),
            ),
        ]);
        assert!(
            calls(&result, "m.ts::use")
                .iter()
                .any(|target| target.starts_with("c.ts::")),
            "{name}: {:?}",
            calls(&result, "m.ts::use")
        );
    }
}

#[test]
fn a_function_valued_constant_is_a_function_not_a_constant() {
    let result = resolve(&[
        ("c.ts", "export const run = () => 1;\n"),
        (
            "m.ts",
            "import { run } from './c';\nexport function go() { return run(); }\n",
        ),
    ]);
    assert_eq!(calls(&result, "m.ts::go"), ["c.ts::run"]);
}

#[test]
fn a_same_file_call_of_a_literal_constant_is_no_edge() {
    let result = resolve(&[(
        "s.ts",
        "const LIMIT = 3;\nexport function f() { return LIMIT(); }\n",
    )]);
    assert!(calls(&result, "s.ts::f").is_empty());
}
