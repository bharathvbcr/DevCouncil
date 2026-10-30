//! Where two lanes edit one resolver rung, each lane's case still resolves
//! when the other's cases share the run.
//!
//! The import-scoped rungs in `resolve_all.rs` were edited by two lanes:
//!
//! * the TS barrels lane rewrote a JS/TS whole-module handle (`*`) to the
//!   module's default export before the package lookup, so a CommonJS
//!   `const run = require('./util'); run()` binds `module.exports = main`;
//! * the compiler-cheap-fixes lane asks the Go build-variant set *before* the
//!   package lookup, and the TS barrels lane split that lookup into a module
//!   handle (follows re-exports) and a type (does not), for every family but
//!   Go.
//!
//! Each case here resolves in one `Resolver` run together with the others, so
//! an ordering that lets one lane's branch take another's input fails.

use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, Extraction};
use devmap_resolve::model::{ResolutionKind, ResolvedEdge};
use devmap_resolve::{ResolutionResult, Resolver};

const GOMOD: &str = "module example.com/app\n\ngo 1.22\n";

fn resolve(files: &[(&str, &str)]) -> ResolutionResult {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    let module = devmap_extract::parse_go_mod("go.mod", GOMOD).expect("go.mod parses");
    resolver.index_go_modules(std::slice::from_ref(&module));
    resolver.index_extractions(&extractions);
    resolver.resolve_all(&extractions).expect("resolution")
}

fn call_edges<'a>(result: &'a ResolutionResult, from: &str) -> Vec<&'a ResolvedEdge> {
    result
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls && edge.source_symbol == from)
        .collect()
}

fn targets(result: &ResolutionResult, from: &str) -> Vec<String> {
    let mut found: Vec<String> = call_edges(result, from)
        .iter()
        .map(|edge| edge.target_symbol.clone())
        .collect();
    found.sort();
    found.dedup();
    found
}

const UNIX: &str = "//go:build unix\n\npackage proc\n\nfunc ConfigureGroup() {}\n";
const OTHER: &str = "//go:build !unix\n\npackage proc\n\nfunc ConfigureGroup() {}\n";
const GO_CALLER: &str =
    "package components\n\nimport \"example.com/app/proc\"\n\nfunc Build() { proc.ConfigureGroup() }\n";
const GO_SIBLING_A: &str = "package tools\n\nfunc First() {}\n";
const GO_SIBLING_B: &str = "package tools\n\nfunc Second() {}\n";
const GO_SIBLING_CALLER: &str =
    "package components\n\nimport \"example.com/app/tools\"\n\nfunc Run() { tools.Second() }\n";

const JS_BARREL: &str = "export { buildTab } from './tabs';\n";
const JS_TABS: &str = "export function buildTab(name) { return name; }\n";
const JS_NAMESPACE: &str =
    "import * as ui from './index';\nexport function run(name) { return ui.buildTab(name); }\n";
const JS_UTIL: &str = "function main() { return 'ok'; }\nmodule.exports = main;\n";
const JS_MAIN: &str = "const runMain = require('./util');\nfunction boot() { return runMain(); }\n";

fn mixed() -> ResolutionResult {
    resolve(&[
        ("proc/procgroup_unix.go", UNIX),
        ("proc/procgroup_other.go", OTHER),
        ("components/build.go", GO_CALLER),
        ("tools/a.go", GO_SIBLING_A),
        ("tools/b.go", GO_SIBLING_B),
        ("components/run.go", GO_SIBLING_CALLER),
        ("ui/index.ts", JS_BARREL),
        ("ui/tabs.ts", JS_TABS),
        ("ui/consumer.ts", JS_NAMESPACE),
        ("cjs/util.js", JS_UTIL),
        ("cjs/main.js", JS_MAIN),
    ])
}

#[test]
fn a_go_package_member_declared_per_build_variant_stays_one_callee_set() {
    let result = mixed();
    let edges = call_edges(&result, "components/build.go::Build");
    let mut files: Vec<&str> = edges.iter().map(|edge| edge.target_file.as_str()).collect();
    files.sort();
    assert_eq!(
        files,
        ["proc/procgroup_other.go", "proc/procgroup_unix.go"],
        "{edges:?}"
    );
    for edge in &edges {
        assert_eq!(
            edge.resolution.as_ref().unwrap().kind(),
            ResolutionKind::Dispatch,
            "the variant set is asked before the module-member lookup: {edge:?}"
        );
    }
}

#[test]
fn a_go_package_member_in_a_sibling_file_binds_that_file() {
    let result = mixed();
    let edges = call_edges(&result, "components/run.go::Run");
    assert_eq!(edges.len(), 1, "{edges:?}");
    assert_eq!(edges[0].target_file, "tools/b.go");
    assert_eq!(edges[0].target_symbol, "tools/b.go::Second");
}

#[test]
fn a_js_namespace_over_a_barrel_resolves_beside_go() {
    let result = mixed();
    assert_eq!(
        targets(&result, "ui/consumer.ts::run"),
        ["ui/tabs.ts::buildTab"],
        "{:?}",
        result.unresolved
    );
}

#[test]
fn a_whole_module_handle_binds_the_default_export_at_the_named_import_rung() {
    let result = mixed();
    assert_eq!(
        targets(&result, "cjs/main.js::boot"),
        ["cjs/util.js::main"],
        "{:?}",
        result.unresolved
    );
}

/// ccf narrows a Go/Rust type call to a reference; the TS lane narrows a call
/// at a JS literal constant to a reference. Disjoint targets, so each edge
/// moves once and neither pass sees the other's edges.
#[test]
fn both_calls_to_references_narrowings_apply_once_in_one_run() {
    let result = resolve(&[
        ("pkg/args.go", "package pkg\n\ntype Args struct{ N int }\n"),
        (
            "pkg/use.go",
            "package pkg\n\nfunc Make() Args { return Args{N: 1} }\n",
        ),
        (
            "c.ts",
            "export const LIMIT = 200;\nexport function fmt() {}\n",
        ),
        (
            "m.ts",
            "import { LIMIT, fmt } from './c';\nexport function bad() { fmt(); return LIMIT(); }\n",
        ),
    ]);
    let kinds = |from: &str, target: &str| -> Vec<EdgeKind> {
        result
            .edges
            .iter()
            .filter(|edge| edge.source_symbol == from && edge.target_symbol.ends_with(target))
            .map(|edge| edge.edge_kind)
            .collect()
    };
    assert_eq!(
        kinds("m.ts::bad", "::LIMIT"),
        [EdgeKind::References],
        "{:?}",
        result.edges
    );
    assert_eq!(kinds("m.ts::bad", "::fmt"), [EdgeKind::Calls]);
    assert!(
        !kinds("pkg/use.go::Make", "Args").contains(&EdgeKind::Calls),
        "{:?}",
        kinds("pkg/use.go::Make", "Args")
    );
}
