//! Kotlin receivers typed by a *written* type that `admissible_nominal_type`
//! used to refuse (ft-87b28bb6, resolver half; fixture rows `kg.Z3`, `kg.Z5`).
//!
//! * Z3: a local initialised from a function written to return `List<String>`
//!   is a `List`; the extension `List<String>.total()` is reachable.
//! * Z5: `val repo = mock<Repo>()` writes its type argument; `Repo` may be an
//!   interface.
//!
//! Each binds only through a written, unique type and abstains otherwise. Z1,
//! Z2, Z4 and Z6 need extractor facts (extract-v98) and are not touched.

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

const Z5_TYPES: &str = "package p\n\ninterface Repo {\n    fun find(id: Int): String\n}\n\nfun <reified T> mock(): T = throw IllegalStateException()\n\n";

#[test]
fn z5_an_explicit_type_argument_naming_an_interface_types_the_local() {
    let source = format!(
        "{Z5_TYPES}fun use(): String {{\n    val repo = mock<Repo>()\n    return repo.find(1)\n}}\n"
    );
    let found = calls(&[("p/Z.kt", &source)]);
    assert!(
        reaches(&found, "p/Z.kt::use", "p/Z.kt::Repo.find"),
        "{found:?}"
    );
}

#[test]
fn z5_abstains_when_the_argument_is_not_one_declared_type() {
    let cases = [
        // Two declarations of the name: no unique type.
        ("class Repo2\n", "Repo", true),
        // A nullable argument and a nested one are not a bare type name.
        ("", "Repo?", false),
        ("", "List<Repo>", false),
        // A name nothing declares.
        ("", "Nothing2", false),
    ];
    for (extra, argument, _) in cases {
        let source = format!(
            "{Z5_TYPES}{extra}class Repo\n\nfun use(): String {{\n    val repo = mock<{argument}>()\n    return repo.find(1)\n}}\n"
        );
        let found = calls(&[("p/Z.kt", &source)]);
        assert!(
            !reaches(&found, "p/Z.kt::use", "p/Z.kt::Repo.find"),
            "{argument}: {found:?}"
        );
    }
}

#[test]
fn z5_a_concrete_return_type_is_the_callees_word_not_the_argument() {
    let source = "package p\n\ninterface Repo {\n    fun find(id: Int): String\n}\n\nclass Box {\n    fun find(id: Int): String = \"b\"\n}\n\nfun <T> make(): Box = Box()\n\nfun use(): String {\n    val repo = make<Repo>()\n    return repo.find(1)\n}\n";
    let found = calls(&[("p/Z.kt", source)]);
    assert!(
        !reaches(&found, "p/Z.kt::use", "p/Z.kt::Repo.find"),
        "{found:?}"
    );
}

#[test]
fn z5_a_value_argument_is_not_a_type_argument() {
    let source = format!(
        "{Z5_TYPES}fun <T> pick(x: Any): T = throw IllegalStateException()\n\nfun use(): String {{\n    val repo = pick(Repo)\n    return repo.find(1)\n}}\n"
    );
    let found = calls(&[("p/Z.kt", &source)]);
    assert!(
        !reaches(&found, "p/Z.kt::use", "p/Z.kt::Repo.find"),
        "{found:?}"
    );
}

#[test]
fn z5_a_local_bound_twice_in_one_scope_is_withdrawn() {
    let source = format!(
        "{Z5_TYPES}class Other {{\n    fun find(id: Int): String = \"o\"\n}}\n\nfun use(flag: Boolean): String {{\n    if (flag) {{\n        val repo = mock<Repo>()\n        return repo.find(1)\n    }} else {{\n        val repo = mock<Other>()\n        return repo.find(2)\n    }}\n}}\n"
    );
    let found = calls(&[("p/Z.kt", &source)]);
    assert!(
        !reaches(&found, "p/Z.kt::use", "p/Z.kt::Repo.find"),
        "{found:?}"
    );
    assert!(
        !reaches(&found, "p/Z.kt::use", "p/Z.kt::Other.find"),
        "{found:?}"
    );
}
