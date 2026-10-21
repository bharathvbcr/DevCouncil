//! Kotlin receivers typed by a *written* type that `admissible_nominal_type`
//! used to refuse (ft-87b28bb6, resolver half; fixture row `kg.Z3`).
//!
//! * Z3: a local initialised from a function written to return `List<String>`
//!   is a `List`; the extension `List<String>.total()` is reachable.
//!
//! It binds only through a written, unqualified, unique type and abstains
//! otherwise. Z5 (`val repo = mock<Repo>()`) was reverted: soundness needs the
//! callee's type-parameter list, which the extraction does not carry (no
//! signature text for Kotlin), so `fun <T> f(): Logger` cannot be told from
//! `fun <T> f(): T`. Z1, Z2, Z4, Z5 and Z6 are blocked on extract-v98.

use devmap_extract::extract_file;
use devmap_extract::model::EdgeKind;
use devmap_resolve::Resolver;

fn calls(files: &[(&str, &str)]) -> Vec<(String, String)> {
    let extractions: Vec<_> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let result = resolver.resolve_all(&extractions).unwrap();
    result
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls)
        .map(|edge| (edge.source_symbol.clone(), edge.target_symbol.clone()))
        .collect()
}

fn reaches(found: &[(String, String)], from: &str, to: &str) -> bool {
    found.iter().any(|(f, t)| f == from && t == to)
}

const TOTAL: &str = "private fun List<String>.total(): Int = size\n\n";

fn z3(callee_return: &str, extra: &str) -> String {
    format!(
        "package p\n\n{TOTAL}fun names(): {callee_return} = listOf(\"a\")\n\n{extra}fun use(flag: Boolean): Int {{\n    val files = if (flag) names() else emptyList()\n    return files.total()\n}}\n"
    )
}

#[test]
fn z3_a_local_from_a_function_written_to_return_a_generic_list_is_a_list() {
    let source = z3("List<String>", "");
    let found = calls(&[("p/Z.kt", &source)]);
    assert!(
        reaches(&found, "p/Z.kt::use", "p/Z.kt::List.total"),
        "{found:?}"
    );
}

#[test]
fn z3_nested_generics_type_the_outer_constructor() {
    let source = "package p\n\nprivate fun Map<String, List<Int>>.total(): Int = size\n\nfun table(): Map<String, List<Int>> = emptyMap()\n\nfun use(): Int {\n    val t = table()\n    return t.total()\n}\n";
    let found = calls(&[("p/Z.kt", source)]);
    assert!(
        reaches(&found, "p/Z.kt::use", "p/Z.kt::Map.total"),
        "{found:?}"
    );
}

#[test]
fn z3_a_nullable_a_star_projection_and_a_platform_type_abstain() {
    for written in ["List<String>?", "List<*>", "List<String>!"] {
        let source = z3(written, "");
        let found = calls(&[("p/Z.kt", &source)]);
        assert!(
            !reaches(&found, "p/Z.kt::use", "p/Z.kt::List.total"),
            "{written} must abstain: {found:?}"
        );
    }
}

#[test]
fn z3_a_type_parameter_is_not_a_type() {
    let source = "package p\n\nprivate fun <T> T.total(): Int = 1\n\nfun <T> ident(x: T): T = x\n\nfun use(): Int {\n    val v = ident(1)\n    return v.total()\n}\n";
    let found = calls(&[("p/Z.kt", source)]);
    assert!(
        !reaches(&found, "p/Z.kt::use", "p/Z.kt::T.total"),
        "{found:?}"
    );
}

#[test]
fn z3_a_name_the_corpus_declares_twice_abstains() {
    let other = "package q\n\nclass List<T>\n";
    let again = "package r\n\nclass List<T>\n";
    let source = z3("List<String>", "");
    let found = calls(&[("p/Z.kt", &source), ("q/L.kt", other), ("r/L.kt", again)]);
    assert!(
        !reaches(&found, "p/Z.kt::use", "p/Z.kt::List.total"),
        "{found:?}"
    );
}

#[test]
fn z3_the_function_in_another_file_abstains() {
    let user = "package p\n\nprivate fun List<String>.total(): Int = size\n\nfun use(): Int {\n    val files = names()\n    return files.total()\n}\n";
    let provider = "package p\n\nfun names(): List<String> = listOf(\"a\")\n";
    let found = calls(&[("p/U.kt", user), ("p/N.kt", provider)]);
    assert!(
        !reaches(&found, "p/U.kt::use", "p/U.kt::List.total"),
        "{found:?}"
    );
}

#[test]
fn z3_a_callee_name_the_scope_binds_itself_abstains() {
    let source = "package p\n\nprivate fun List<String>.total(): Int = size\n\nfun names(): List<String> = listOf(\"a\")\n\nfun use(names: () -> Int): Int {\n    val files = names()\n    return files.total()\n}\n";
    let found = calls(&[("p/Z.kt", source)]);
    assert!(
        !reaches(&found, "p/Z.kt::use", "p/Z.kt::List.total"),
        "{found:?}"
    );
}
