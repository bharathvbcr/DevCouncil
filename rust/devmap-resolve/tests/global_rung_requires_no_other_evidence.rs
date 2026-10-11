#![cfg(feature = "parse")]
//! S2. The bare-name rung answers only when nothing else is known about the
//! name: no import of it, no builtin or host global of that spelling, no
//! enclosing parameter or closure parameter, no visibility that hides the
//! declaration from the caller. Each case pairs a call that the rung used to
//! bind to a same-spelled repository declaration with the evidence that says
//! what it really is.

use devmap_extract::extract_file;
use devmap_extract::model::EdgeKind;
use devmap_resolve::model::{Resolution, ResolutionResult, UnresolvedClass, UnresolvedReference};
use devmap_resolve::Resolver;

fn resolve(files: &[(&str, &str)]) -> ResolutionResult {
    let extractions: Vec<_> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    resolver.resolve_all(&extractions).unwrap()
}

fn call_edges_to<'a>(result: &'a ResolutionResult, target_suffix: &str) -> Vec<&'a str> {
    result
        .edges
        .iter()
        .filter(|edge| {
            edge.edge_kind == EdgeKind::Calls && edge.target_symbol.ends_with(target_suffix)
        })
        .map(|edge| edge.source_symbol.as_str())
        .collect()
}

fn row<'a>(result: &'a ResolutionResult, callee: &str) -> &'a UnresolvedReference {
    result
        .unresolved
        .iter()
        .find(|row| row.callee_name == callee)
        .unwrap_or_else(|| panic!("no ledger row for {callee}: {:?}", result.unresolved))
}

#[test]
fn a_package_import_is_external_not_a_repository_namesake() {
    let result = resolve(&[
        (
            "apps/web/src/v/npmcollide.ts",
            "import { formatTitle } from 'some-npm-lib';\nexport function callsNpm(): string { return formatTitle('y'); }\n",
        ),
        (
            "apps/web/src/lib/format.ts",
            "export function formatTitle(s: string): string { return s; }\n",
        ),
    ]);
    assert!(
        call_edges_to(&result, "::formatTitle").is_empty(),
        "an npm import must not bind to the repository's formatTitle"
    );
    assert!(
        matches!(&row(&result, "formatTitle").class, UnresolvedClass::External { module } if module == "some-npm-lib"),
        "{:?}",
        row(&result, "formatTitle").class
    );
}

#[test]
fn an_unindexed_relative_import_is_an_index_gap_never_a_global_bind() {
    let result = resolve(&[
        (
            "src/user.ts",
            "import { exportedArrow } from './gone.js';\nexport function useArrow() { return exportedArrow(); }\n",
        ),
        (
            "src/arrows.ts",
            "export const exportedArrow = () => 1;\nexport function exportedFn() { return 2; }\n",
        ),
    ]);
    assert!(call_edges_to(&result, "::exportedArrow").is_empty());
    assert_eq!(
        row(&result, "exportedArrow").class,
        UnresolvedClass::Unresolved
    );
}

/// The same import with a specifier that does name an indexed file — a TypeScript
/// ESM `.js` specifier for the `.ts` source — is proven by the import, so the
/// edge is the right one and not a namesake bind. (This test named the
/// specifier `./arrows.js` as an index gap before the TypeScript lane taught
/// import resolution the `.js` -> `.ts` mapping.)
#[test]
fn a_js_specifier_for_an_indexed_ts_file_binds_through_the_import() {
    let result = resolve(&[
        (
            "src/user.ts",
            "import { exportedArrow } from './arrows.js';\nexport function useArrow() { return exportedArrow(); }\n",
        ),
        (
            "src/arrows.ts",
            "export const exportedArrow = () => 1;\nexport function exportedFn() { return 2; }\n",
        ),
    ]);
    assert!(!call_edges_to(&result, "::exportedArrow").is_empty());
}

#[test]
fn a_family_builtin_is_not_bound_to_a_repository_function_of_that_name() {
    let result = resolve(&[
        ("app.py", "def run(xs):\n    return len(xs)\n"),
        ("helpers.py", "def len(xs):\n    return 0\n"),
    ]);
    assert!(call_edges_to(&result, "::len").is_empty());
    assert_eq!(row(&result, "len").class, UnresolvedClass::Builtin);
}

#[test]
fn a_swift_private_declaration_is_invisible_to_another_file() {
    let result = resolve(&[
        (
            "Sources/M/A.swift",
            "private func forwardSpan() -> Int { 1 }\nfunc keep() -> Int { forwardSpan() }\n",
        ),
        ("Sources/M/B.swift", "func use() -> Int { forwardSpan() }\n"),
    ]);
    let callers = call_edges_to(&result, "::forwardSpan");
    assert!(
        callers
            .iter()
            .all(|caller| caller.starts_with("Sources/M/A.swift")),
        "only the declaring file may call its private function: {callers:?}"
    );
}

#[test]
fn a_ts_module_binding_that_is_not_exported_is_invisible_to_another_file() {
    let result = resolve(&[
        (
            "a.ts",
            "function hidden(): number { return 1; }\nexport function shown(): number { return hidden(); }\n",
        ),
        ("b.ts", "export function use(): number { return hidden(); }\n"),
    ]);
    let callers = call_edges_to(&result, "::hidden");
    assert!(
        callers.iter().all(|caller| caller.starts_with("a.ts")),
        "a non-exported binding is private to its module: {callers:?}"
    );
}

#[test]
fn a_rust_item_that_is_not_pub_is_invisible_to_another_crate() {
    let result = resolve(&[
        (
            "a/src/lib.rs",
            "fn secret() -> i32 { 1 }\npub fn open_api() -> i32 { 2 }\n",
        ),
        (
            "b/src/lib.rs",
            "pub fn go() -> i32 {\n    secret() + open_api()\n}\n",
        ),
    ]);
    assert!(
        call_edges_to(&result, "::secret").is_empty(),
        "a private item of another crate cannot be named"
    );
    assert_eq!(
        call_edges_to(&result, "::open_api"),
        ["b/src/lib.rs::go"],
        "a pub item of another crate still binds"
    );
}

#[test]
fn a_swift_parameter_and_a_closure_parameter_shadow_a_project_function() {
    let result = resolve(&[
        (
            "Sources/M/A.swift",
            "func run(completion: () -> Void) { completion() }\nfunc each(_ xs: [Int]) { xs.forEach { handler in handler() } }\n",
        ),
        ("Sources/M/B.swift", "func completion() {}\nfunc handler() {}\n"),
    ]);
    assert!(call_edges_to(&result, "::completion").is_empty());
    assert!(call_edges_to(&result, "::handler").is_empty());
}

#[test]
fn a_kotlin_function_typed_parameter_shadows_a_project_method() {
    let result = resolve(&[(
        "gaps/Events.kt",
        "class Widget {\n    fun onStart(n: Int) {}\n}\n\nfun runWith(onStart: (Int) -> Unit) {\n    onStart(1)\n}\n",
    )]);
    assert!(call_edges_to(&result, "Widget.onStart").is_empty());
}

#[test]
fn a_kotlin_method_is_not_a_bare_callee_outside_every_type() {
    let result = resolve(&[
        (
            "gaps/Prefs.kt",
            "class Prefs {\n    fun putBoolean(key: String, value: Boolean) {}\n}\n",
        ),
        (
            "gaps/Save.kt",
            "fun save(editor: Editor) {\n    editor.apply { putBoolean(\"k\", true) }\n}\n",
        ),
    ]);
    assert!(
        call_edges_to(&result, "Prefs.putBoolean").is_empty(),
        "a top-level function has no implicit receiver to dispatch a method on"
    );
}

#[test]
fn an_unambiguous_bare_name_with_no_other_evidence_still_binds() {
    let result = resolve(&[
        ("a.py", "def only_one():\n    return 1\n"),
        ("b.py", "def run():\n    return only_one()\n"),
    ]);
    let edge = result
        .edges
        .iter()
        .find(|edge| edge.edge_kind == EdgeKind::Calls && edge.source_symbol == "b.py::run")
        .expect("the unique bare name binds");
    assert!(matches!(
        edge.resolution.as_deref(),
        Some(Resolution::UniqueGlobal { .. })
    ));
}
