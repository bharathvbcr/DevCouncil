//! A Rust call written as a module path names the module the path names.
//!
//! `crate::resolver::Resolver::resolve_all()` in `ledger/bindings.rs` is a
//! caller of `Resolver::resolve_all`. The receiver arrives as the path
//! `crate::resolver::Resolver`, not as an import binding and not as a bare
//! type, so the ladder classified it `module_path` and the callee kept no
//! inbound edge. The same hole drops `crate::work::helper()`, a file-level
//! `super::helper()` whose parent module declares `helper`, and a sibling
//! crate spelled by its `use` name.
//!
//! The rung binds only a path the module system can place on exactly one
//! indexed file. A popped prefix, a `mod.rs`/`name.rs` pair, a crate name two
//! directories claim, and a path that is also a child module all abstain.

use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, Extraction};
use devmap_resolve::model::ResolutionResult;
use devmap_resolve::Resolver;

fn resolve(files: &[(&str, &str)]) -> (Vec<Extraction>, ResolutionResult) {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    (extractions, resolution)
}

fn call_edges<'a>(
    result: &'a ResolutionResult,
    callee_suffix: &str,
) -> Vec<&'a devmap_resolve::model::ResolvedEdge> {
    result
        .edges
        .iter()
        .filter(|edge| {
            edge.edge_kind == EdgeKind::Calls && edge.target_symbol.ends_with(callee_suffix)
        })
        .collect()
}

fn classes_of(result: &ResolutionResult, name: &str) -> Vec<String> {
    let mut found: Vec<String> = result
        .unresolved
        .iter()
        .filter(|row| row.callee_name == name)
        .map(|row| row.class.label().to_string())
        .collect();
    found.sort();
    found.dedup();
    found
}

const BINDINGS: &str = "\
pub fn bind() {
    crate::resolver::Resolver::resolve_all();
}
";

const RESOLVER: &str = "\
pub struct Resolver;

impl Resolver {
    pub fn resolve_all() {}
}
";

#[test]
fn a_full_crate_path_to_an_associated_function_is_a_caller() {
    let (_, result) = resolve(&[
        (
            "crates/thing/src/lib.rs",
            "pub mod ledger;\npub mod resolver;\n",
        ),
        ("crates/thing/src/ledger/mod.rs", "pub mod bindings;\n"),
        ("crates/thing/src/ledger/bindings.rs", BINDINGS),
        ("crates/thing/src/resolver.rs", RESOLVER),
    ]);

    let edges = call_edges(&result, "resolve_all");
    assert_eq!(
        edges.len(),
        1,
        "bindings.rs calls Resolver::resolve_all by its crate path; got {edges:?}"
    );
    let edge = edges[0];
    assert_eq!(edge.source_file, "crates/thing/src/ledger/bindings.rs");
    assert_eq!(edge.target_file, "crates/thing/src/resolver.rs");
    assert!(
        classes_of(&result, "resolve_all").is_empty(),
        "a path that resolved is not also an unresolved module path: {:?}",
        classes_of(&result, "resolve_all")
    );
    assert_eq!(
        format!("{:?}", edge.evidence.expect("resolver evidence").kind),
        "ReceiverType",
        "the path's last segment is the type that declares the method"
    );
}

#[test]
fn a_full_crate_path_to_a_free_function_names_that_module() {
    let (_, result) = resolve(&[
        (
            "crates/thing/src/lib.rs",
            "pub mod ledger;\npub mod work;\n",
        ),
        (
            "crates/thing/src/ledger/bindings.rs",
            "pub fn bind() {\n    crate::work::helper();\n}\n",
        ),
        ("crates/thing/src/work.rs", "pub fn helper() {}\n"),
        ("src/work.rs", "pub fn helper() {}\n"),
    ]);

    let edges = call_edges(&result, "::helper");
    assert_eq!(
        edges.len(),
        1,
        "`crate::` is the workspace member's own crate, not a same-named file \
         at the repository root; got {edges:?}"
    );
    assert_eq!(edges[0].source_file, "crates/thing/src/ledger/bindings.rs");
    assert_eq!(edges[0].target_file, "crates/thing/src/work.rs");
}

#[test]
fn a_file_level_super_call_names_the_parent_module() {
    let (_, result) = resolve(&[
        ("src/deep/mod.rs", "pub fn helper() {}\n"),
        (
            "src/deep/leaf.rs",
            "pub fn helper() {}\n\npub fn run() {\n    super::helper();\n}\n",
        ),
    ]);

    let edges = call_edges(&result, "::helper");
    assert_eq!(
        edges.len(),
        1,
        "file-level `super::helper()` names the parent module's helper, not \
         this file's; got {edges:?}"
    );
    assert_eq!(edges[0].source_file, "src/deep/leaf.rs");
    assert_eq!(edges[0].target_file, "src/deep/mod.rs");
}

#[test]
fn an_inline_super_path_to_a_type_names_this_file() {
    let (_, result) = resolve(&[(
        "crates/thing/src/work.rs",
        "pub struct Thing;\nimpl Thing {\n    pub fn build() -> Self { Self }\n}\n\n\
         #[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {\n        \
         super::Thing::build();\n    }\n}\n",
    )]);

    let edges = call_edges(&result, "build");
    assert_eq!(
        edges.len(),
        1,
        "`super::Thing::build()` inside `mod tests` spends one `super` on the \
         inline module and names this file's `Thing`; got {edges:?}"
    );
    assert_eq!(edges[0].target_file, "crates/thing/src/work.rs");
    assert_eq!(edges[0].source_file, "crates/thing/src/work.rs");
}

#[test]
fn a_sibling_crate_path_names_that_crates_root() {
    let (_, result) = resolve(&[
        (
            "crates/thing/src/ledger/bindings.rs",
            "pub fn bind() {\n    other_crate::Resolver::resolve_all();\n}\n",
        ),
        (
            "crates/other-crate/src/lib.rs",
            "pub struct Resolver;\nimpl Resolver {\n    pub fn resolve_all() {}\n}\n",
        ),
        (
            "crates/decoy/src/lib.rs",
            "pub struct Resolver;\nimpl Resolver {\n    pub fn resolve_all() {}\n}\n",
        ),
    ]);

    let edges = call_edges(&result, "resolve_all");
    assert_eq!(edges.len(), 1, "got {edges:?}");
    assert_eq!(edges[0].target_file, "crates/other-crate/src/lib.rs");
    assert_eq!(edges[0].source_file, "crates/thing/src/ledger/bindings.rs");
}

#[test]
fn a_child_module_path_from_the_crate_root_names_that_child() {
    let (_, result) = resolve(&[
        (
            "src/lib.rs",
            "pub mod work;\n\npub fn run() {\n    work::helper();\n}\n",
        ),
        ("src/work.rs", "pub fn helper() {}\n"),
        ("src/work/helper.rs", "pub fn helper() {}\n"),
    ]);

    let edges = call_edges(&result, "::helper");
    assert_eq!(
        edges.len(),
        1,
        "`work::helper()` in the crate root names `work.rs`, not a function \
         nested under that module; got {edges:?}"
    );
    assert_eq!(edges[0].target_file, "src/work.rs");
}

#[test]
fn a_nested_file_does_not_treat_a_sibling_as_its_child() {
    let (_, result) = resolve(&[
        ("src/lib.rs", "pub mod ledger;\n"),
        (
            "src/ledger/mod.rs",
            "pub mod bindings;\npub fn helper() {}\n",
        ),
        (
            "src/ledger/bindings.rs",
            "pub fn bind() {\n    helper::run();\n}\n",
        ),
        ("src/ledger/helper.rs", "pub fn run() {}\n"),
    ]);

    assert!(
        call_edges(&result, "::run").is_empty(),
        "`helper::run()` in `bindings.rs` is not the sibling `ledger/helper.rs`; \
         that path would be `super::helper`. got {:?}",
        call_edges(&result, "::run")
    );
}

#[test]
fn a_missing_crate_path_stays_a_module_path() {
    let (_, result) = resolve(&[(
        "crates/thing/src/ledger/bindings.rs",
        "pub fn bind() {\n    crate::missing::helper();\n}\n",
    )]);

    assert!(call_edges(&result, "::helper").is_empty());
    assert_eq!(
        classes_of(&result, "helper"),
        vec!["module_path".to_string()]
    );
}

#[test]
fn a_std_path_stays_external() {
    let (_, result) = resolve(&[(
        "crates/thing/src/ledger/bindings.rs",
        "pub fn bind(body: &str) {\n    std::fs::write(\"out.txt\", body);\n}\n",
    )]);

    assert!(call_edges(&result, "write").is_empty());
    assert_eq!(classes_of(&result, "write"), vec!["external".to_string()]);
}

#[test]
fn a_local_that_shares_a_crate_name_is_not_a_path() {
    let (_, result) = resolve(&[(
        "crates/thing/src/work.rs",
        "use serde_json::Value;\n\npub fn go() {\n    let serde_json = build();\n    \
         serde_json.take();\n}\n",
    )]);

    assert_eq!(
        classes_of(&result, "take"),
        vec!["uninferred_receiver".to_string()]
    );
}

#[test]
fn both_module_file_shapes_abstain() {
    let (_, result) = resolve(&[
        (
            "src/lib.rs",
            "pub fn run() {\n    crate::work::helper();\n}\n",
        ),
        ("src/work.rs", "pub fn helper() {}\n"),
        ("src/work/mod.rs", "pub fn helper() {}\n"),
    ]);

    assert!(
        call_edges(&result, "::helper").is_empty(),
        "a path that is both `work.rs` and `work/mod.rs` has no single target; \
         got {:?}",
        call_edges(&result, "::helper")
    );
    assert_eq!(
        classes_of(&result, "helper"),
        vec!["module_path".to_string()]
    );
}

#[test]
fn two_crates_with_one_name_abstain() {
    let (_, result) = resolve(&[
        (
            "crates/thing/src/ledger/bindings.rs",
            "pub fn bind() {\n    shared::Resolver::resolve_all();\n}\n",
        ),
        (
            "crates/a/shared/src/lib.rs",
            "pub struct Resolver;\nimpl Resolver {\n    pub fn resolve_all() {}\n}\n",
        ),
        (
            "vendor/shared/src/lib.rs",
            "pub struct Resolver;\nimpl Resolver {\n    pub fn resolve_all() {}\n}\n",
        ),
    ]);

    assert!(
        call_edges(&result, "resolve_all").is_empty(),
        "two indexed crates named `shared` are not a path to either; got {:?}",
        call_edges(&result, "resolve_all")
    );
}

#[test]
fn a_declared_child_module_wins_over_a_crate_of_the_same_name() {
    let (_, result) = resolve(&[
        (
            "src/lib.rs",
            "pub mod shared;\n\npub fn run() {\n    shared::helper();\n}\n",
        ),
        ("src/shared.rs", "pub fn helper() {}\n"),
        ("vendor/shared/src/lib.rs", "pub fn helper() {}\n"),
    ]);

    let edges = call_edges(&result, "::helper");
    assert_eq!(
        edges.len(),
        1,
        "`mod shared` puts the child module in scope ahead of the extern crate; \
         got {edges:?}"
    );
    assert_eq!(edges[0].target_file, "src/shared.rs");
}

#[test]
fn an_undeclared_child_file_and_a_crate_of_the_same_name_abstain() {
    let (_, result) = resolve(&[
        ("src/lib.rs", "pub fn run() {\n    shared::helper();\n}\n"),
        ("src/shared.rs", "pub fn helper() {}\n"),
        ("vendor/shared/src/lib.rs", "pub fn helper() {}\n"),
    ]);

    assert!(
        call_edges(&result, "::helper").is_empty(),
        "without `mod shared`, the child file and the crate are both plausible \
         and neither may be chosen; got {:?}",
        call_edges(&result, "::helper")
    );
}

#[test]
fn a_file_level_super_type_path_does_not_name_this_file() {
    let (_, result) = resolve(&[(
        "src/deep/leaf.rs",
        "pub struct Thing;\nimpl Thing {\n    pub fn build() {}\n}\n\n\
         pub fn run() {\n    super::Thing::build();\n}\n",
    )]);

    assert!(
        call_edges(&result, "build").is_empty(),
        "file-level `super::Thing` is the parent module's `Thing`, not this \
         file's; got {:?}",
        call_edges(&result, "build")
    );
}

#[test]
fn repeated_inherent_impls_are_one_caller_not_a_fanout() {
    let (_, result) = resolve(&[
        (
            "src/ledger/bindings.rs",
            "pub fn bind() {\n    crate::resolver::Resolver::resolve_all();\n}\n",
        ),
        (
            "src/resolver.rs",
            "pub struct Resolver;\nimpl Resolver {\n    pub fn resolve_all() {}\n}\n\
             impl Resolver {\n    pub fn resolve_all() {}\n}\n",
        ),
    ]);

    let edges = call_edges(&result, "resolve_all");
    assert_eq!(
        edges.len(),
        1,
        "two impls of one method share one graph identity, so the path is one \
         caller of that identity and not a fan-out; got {edges:?}"
    );
    assert_eq!(edges[0].target_file, "src/resolver.rs");
    assert_eq!(
        edges[0].target_symbol,
        "src/resolver.rs::Resolver.resolve_all"
    );
}

#[test]
fn a_hostile_path_does_not_bind_and_does_not_panic() {
    let huge = format!("crate::{}", "seg::".repeat(10_000));
    let source = format!("pub fn bind() {{\n    {huge}helper();\n}}\n");
    let (_, result) = resolve(&[
        ("src/lib.rs", &source),
        ("src/seg.rs", "pub fn helper() {}\n"),
    ]);

    assert!(
        call_edges(&result, "::helper").is_empty(),
        "a path longer than the indexed tree is not a prefix of a real module"
    );
    let classes = classes_of(&result, "helper");
    assert!(
        classes.iter().all(|class| class == "module_path"),
        "when the call is extracted it stays a module path, never a caller; \
         got {classes:?}"
    );
}

#[test]
fn another_language_with_the_same_name_is_not_the_rust_target() {
    let (_, result) = resolve(&[
        (
            "src/lib.rs",
            "pub fn run() {\n    crate::work::helper();\n}\n",
        ),
        ("src/work.rs", "pub fn helper() {}\n"),
        (
            "pkg/work.cpp",
            "void helper() {}\nnamespace work { void helper() {} }\n",
        ),
    ]);

    let edges = call_edges(&result, "::helper");
    assert_eq!(edges.len(), 1, "got {edges:?}");
    assert_eq!(edges[0].target_file, "src/work.rs");

    let (_, cpp) = resolve(&[
        ("pkg/call.cpp", "void run() { work::helper(); }\n"),
        ("src/work.rs", "pub fn helper() {}\n"),
    ]);
    assert!(
        call_edges(&cpp, "helper").is_empty(),
        "a C++ `work::helper()` is not the Rust function of that name; got {:?}",
        call_edges(&cpp, "helper")
    );
}

#[test]
fn python_and_go_qualified_calls_still_follow_their_imports() {
    let (_, python) = resolve(&[
        (
            "app.py",
            "import helpers\n\ndef run():\n    return helpers.do()\n",
        ),
        ("helpers.py", "def do():\n    return 1\n"),
    ]);
    let py: Vec<_> = python
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls)
        .map(|edge| {
            format!(
                "{}::{}->{}",
                edge.source_file,
                edge.source_symbol.rsplit("::").next().unwrap_or(""),
                edge.target_symbol
            )
        })
        .collect();
    assert!(
        py.iter().any(|row| row.contains("helpers.py")),
        "python `helpers.do()` still resolves through its import; got {py:?}"
    );

    let (_, go) = resolve(&[
        (
            "app/main.go",
            "package main\nimport \"example.com/m/internal/svc\"\nfunc main() { svc.Do() }\n",
        ),
        ("internal/svc/s.go", "package svc\nfunc Do() {}\n"),
    ]);
    assert!(
        go.edges.iter().any(|edge| {
            edge.edge_kind == EdgeKind::Calls && edge.target_symbol.ends_with("::Do")
        }),
        "a Go package-qualified call still resolves into the imported package"
    );
}

// A path whose root is a crate name or a `use`-imported module, with no `::`
// left in the receiver once the callee is split off. `dc_grep::build_index()`
// reduces to the receiver `dc_grep`, and the rung above fired only for a
// receiver that still contained `::` — so every call into a sibling crate's
// root, and every `module::f()` through `use other_crate::module`, was filed
// as an uninferred receiver with the crate name as its "value".

const GLOB_LIB: &str = "\
pub fn matches(pattern: &str, name: &str) -> bool {
    pattern == name
}
";

#[test]
fn a_lib_calls_a_sibling_crate_root_function_by_crate_name() {
    let (_, result) = resolve(&[
        (
            "rust/dc-verify/src/lib.rs",
            "pub fn classify_scope(p: &str, c: &str) -> bool {\n    \
             dc_glob::matches(p, c)\n}\n",
        ),
        ("rust/dc-glob/src/lib.rs", GLOB_LIB),
        ("rust/other/src/lib.rs", GLOB_LIB),
    ]);

    let edges = call_edges(&result, "::matches");
    assert_eq!(
        edges.len(),
        1,
        "`dc_glob::matches` names the crate `dc-glob`'s root; got {edges:?}"
    );
    assert_eq!(edges[0].source_file, "rust/dc-verify/src/lib.rs");
    assert_eq!(edges[0].target_file, "rust/dc-glob/src/lib.rs");
    assert!(
        classes_of(&result, "matches").is_empty(),
        "a bound call is not also an uninferred receiver: {:?}",
        classes_of(&result, "matches")
    );
}

#[test]
fn a_bin_calls_its_own_lib_by_crate_name_through_a_reexport() {
    let (_, result) = resolve(&[
        (
            "rust/dc-grep/src/bin/dcgrep.rs",
            "use dc_grep::IndexRequest;\n\nfn index(r: &IndexRequest) {\n    \
             dc_grep::build_index(r);\n}\n",
        ),
        (
            "rust/dc-grep/src/lib.rs",
            "mod index;\npub use index::{IndexRequest, build_index};\n",
        ),
        (
            "rust/dc-grep/src/index.rs",
            "pub struct IndexRequest;\npub fn build_index(r: &IndexRequest) {}\n",
        ),
        ("rust/elsewhere/src/index.rs", "pub fn build_index() {}\n"),
    ]);

    let edges = call_edges(&result, "::build_index");
    assert_eq!(
        edges.len(),
        1,
        "the bin's `dc_grep::build_index` reaches the declaration its crate root \
         re-exports; got {edges:?}"
    );
    assert_eq!(edges[0].source_file, "rust/dc-grep/src/bin/dcgrep.rs");
    assert_eq!(edges[0].target_file, "rust/dc-grep/src/index.rs");
}

#[test]
fn a_lib_calls_through_a_use_imported_sibling_crate_module() {
    let (_, result) = resolve(&[
        (
            "rust/devmap-cli/src/session.rs",
            "use devmap_serve::session_log;\n\nfn build_report(db: &str) {\n    \
             let live = session_log::read_live(db);\n}\n",
        ),
        ("rust/devmap-serve/src/lib.rs", "pub mod session_log;\n"),
        (
            "rust/devmap-serve/src/session_log.rs",
            "pub fn read_live(db: &str) {}\n",
        ),
        (
            "rust/devmap-cli/src/session_log.rs",
            "pub fn read_live(db: &str) {}\n",
        ),
    ]);

    let edges = call_edges(&result, "::read_live");
    assert_eq!(
        edges.len(),
        1,
        "`use devmap_serve::session_log; session_log::read_live()` names the \
         sibling crate's module, not a same-named file in this crate; got {edges:?}"
    );
    assert_eq!(edges[0].source_file, "rust/devmap-cli/src/session.rs");
    assert_eq!(edges[0].target_file, "rust/devmap-serve/src/session_log.rs");
}

#[test]
fn a_module_imported_from_super_is_not_the_parent_file() {
    let (_, result) = resolve(&[
        (
            "rust/store/src/workbench/mod.rs",
            "mod automation;\nmod enhancements;\n\nfn mutate() {}\n",
        ),
        (
            "rust/store/src/workbench/automation.rs",
            "use super::{enhancements};\n\npub fn prepare() {\n    enhancements::mutate();\n}\n",
        ),
        ("rust/store/src/workbench/enhancements.rs", "pub fn mutate() {}\n"),
    ]);

    let edges = call_edges(&result, "::mutate");
    assert_eq!(
        edges.len(),
        1,
        "`use super::{{enhancements}}` binds the module, so `enhancements::mutate` \
         is not the parent's own `mutate`; got {edges:?}"
    );
    assert_eq!(edges[0].target_file, "rust/store/src/workbench/enhancements.rs");
}

#[test]
fn a_local_named_like_a_crate_is_still_a_value() {
    let (_, result) = resolve(&[
        (
            "rust/app/src/lib.rs",
            "pub fn run() {\n    let dc_glob = make();\n    dc_glob.matches(\"a\", \"b\");\n}\n",
        ),
        ("rust/dc-glob/src/lib.rs", GLOB_LIB),
    ]);

    assert!(
        call_edges(&result, "::matches").is_empty(),
        "a local `dc_glob` is a value, not the crate; got {:?}",
        call_edges(&result, "::matches")
    );
}

#[test]
fn a_type_the_crate_root_reexports_names_its_declaring_file() {
    let (_, result) = resolve(&[
        (
            "rust/app/src/lib.rs",
            "pub fn run() {\n    other_crate::Resolver::resolve_all();\n}\n",
        ),
        (
            "rust/other-crate/src/lib.rs",
            "mod resolver;\npub use resolver::Resolver;\n",
        ),
        (
            "rust/other-crate/src/resolver.rs",
            "pub struct Resolver;\nimpl Resolver {\n    pub fn resolve_all() {}\n}\n",
        ),
        (
            "rust/decoy/src/resolver.rs",
            "pub struct Resolver;\nimpl Resolver {\n    pub fn resolve_all() {}\n}\n",
        ),
    ]);

    let edges = call_edges(&result, "resolve_all");
    assert_eq!(edges.len(), 1, "got {edges:?}");
    assert_eq!(edges[0].target_file, "rust/other-crate/src/resolver.rs");
}

#[test]
fn a_crate_name_is_its_library_when_it_also_has_a_main() {
    let (_, result) = resolve(&[
        (
            "rust/app/src/lib.rs",
            "pub fn run() {\n    dc_glob::matches(\"a\", \"b\");\n}\n",
        ),
        ("rust/dc-glob/src/lib.rs", GLOB_LIB),
        ("rust/dc-glob/src/main.rs", "fn main() {}\n"),
    ]);

    let edges = call_edges(&result, "::matches");
    assert_eq!(
        edges.len(),
        1,
        "a binary is not addressable by crate name, so lib.rs + main.rs is one \
         target; got {edges:?}"
    );
    assert_eq!(edges[0].target_file, "rust/dc-glob/src/lib.rs");
}

#[test]
fn an_import_alias_shadows_a_crate_of_the_same_name() {
    let (_, result) = resolve(&[
        (
            "rust/app/src/lib.rs",
            "use crate::shim as dc_glob;\n\npub fn run() {\n    dc_glob::matches(\"a\", \"b\");\n}\n",
        ),
        ("rust/app/src/shim.rs", GLOB_LIB),
        ("rust/dc-glob/src/lib.rs", GLOB_LIB),
    ]);

    let edges = call_edges(&result, "::matches");
    assert!(
        edges.iter().all(|edge| edge.target_file != "rust/dc-glob/src/lib.rs"),
        "`use crate::shim as dc_glob` makes `dc_glob` the alias, not the crate; \
         got {edges:?}"
    );
}

#[test]
fn a_bare_crate_root_two_directories_claim_abstains() {
    let (_, result) = resolve(&[
        (
            "rust/app/src/lib.rs",
            "pub fn run() {\n    shared::matches(\"a\", \"b\");\n}\n",
        ),
        ("crates/shared/src/lib.rs", GLOB_LIB),
        ("vendor/shared/src/lib.rs", GLOB_LIB),
    ]);

    assert!(
        call_edges(&result, "::matches").is_empty(),
        "two crates named `shared` are not a path to either; got {:?}",
        call_edges(&result, "::matches")
    );
}
