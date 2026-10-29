//! A namespace handle over a barrel reaches what the barrel re-exports.
//!
//! `import * as ui from './index'` then `ui.buildTab()` asks the barrel for a
//! name it declares nowhere: `export { buildTab } from './tabs'`,
//! `export { default as App } from './app'` and `export * from './filters'` all
//! publish it. A *named* import already followed that chain; the member of a
//! namespace handle asked the barrel's own declarations and found none, so every
//! call through a namespace over a barrel was lost (Orbit's
//! `typescript/gitlab_monolith_patterns`, `runProjectsNamespace`: 0 of 3).

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

const APP: &str = "export default function renderProjects(name) { return name; }\n";
const TABS: &str = "export function buildTab(name) { return name; }\n";
const FILTERS: &str = "export function normalizeSearch(term) { return term; }\n";
const BARREL: &str = "export { default as ProjectsApp } from './app';\n\
export { buildTab } from './tabs';\n\
export * from './filters';\n";
const CONSUMER: &str = "import * as ui from './components/index';\n\
export function run(term) {\n\
  return [ui.ProjectsApp(term), ui.buildTab(term), ui.normalizeSearch(term)];\n\
}\n";

#[test]
fn a_namespace_over_a_barrel_reaches_an_explicit_default_and_a_glob_re_export() {
    let result = resolve(&[
        ("components/app.ts", APP),
        ("components/tabs.ts", TABS),
        ("components/filters.ts", FILTERS),
        ("components/index.ts", BARREL),
        ("consumer.ts", CONSUMER),
    ]);
    assert_eq!(
        calls(&result, "consumer.ts::run"),
        [
            "components/app.ts::renderProjects",
            "components/filters.ts::normalizeSearch",
            "components/tabs.ts::buildTab",
        ],
        "{:?}",
        result.unresolved
    );
}

#[test]
fn a_name_the_barrel_does_not_publish_binds_nothing() {
    let result = resolve(&[
        ("components/tabs.ts", TABS),
        ("components/other.ts", "export function hidden() { return 1; }\n"),
        ("components/index.ts", "export { buildTab } from './tabs';\n"),
        (
            "consumer.ts",
            "import * as ui from './components/index';\nexport function run() { return ui.hidden(); }\n",
        ),
    ]);
    assert!(calls(&result, "consumer.ts::run").is_empty());
}

#[test]
fn a_name_two_globs_publish_from_different_files_is_ambiguous_not_arbitrary() {
    let result = resolve(&[
        ("c/a.ts", "export function dup() { return 'a'; }\n"),
        ("c/b.ts", "export function dup() { return 'b'; }\n"),
        ("c/index.ts", "export * from './a';\nexport * from './b';\n"),
        (
            "consumer.ts",
            "import * as ui from './c/index';\nexport function run() { return ui.dup(); }\n",
        ),
    ]);
    let found = calls(&result, "consumer.ts::run");
    assert!(
        found.len() != 1,
        "a name two modules publish has no single source, got {found:?}"
    );
}

#[test]
fn a_cycle_of_barrels_terminates_without_an_edge() {
    let result = resolve(&[
        ("c/a.ts", "export * from './b';\n"),
        ("c/b.ts", "export * from './a';\n"),
        (
            "consumer.ts",
            "import * as ui from './c/a';\nexport function run() { return ui.ghost(); }\n",
        ),
    ]);
    assert!(calls(&result, "consumer.ts::run").is_empty());
}

#[test]
fn a_renaming_re_export_binds_the_declared_name() {
    let result = resolve(&[
        ("c/impl.ts", "export function original() { return 1; }\n"),
        ("c/index.ts", "export { original as renamed } from './impl';\n"),
        (
            "consumer.ts",
            "import * as ui from './c/index';\nexport function run() { return ui.renamed(); }\n",
        ),
    ]);
    assert_eq!(
        calls(&result, "consumer.ts::run"),
        ["c/impl.ts::original"],
        "{:?}",
        result.unresolved
    );
}

#[test]
fn a_two_hundred_deep_chain_of_barrels_terminates_and_never_binds_a_wrong_target() {
    // `b0` re-exports from `b1`, … , `b199` declares `leaf`. Whatever the hop
    // cap does, it must not bind another file's `leaf`, and must not hang.
    let mut files: Vec<(String, String)> = (0..199)
        .map(|n| (format!("deep/b{n}.ts"), format!("export * from './b{}';\n", n + 1)))
        .collect();
    files.push((
        "deep/b199.ts".to_string(),
        "export function leaf() { return 1; }\n".to_string(),
    ));
    files.push(("deep/decoy.ts".to_string(), "export function leaf() { return 2; }\n".to_string()));
    files.push((
        "consumer.ts".to_string(),
        "import * as ui from './deep/b0';\nexport function run() { return ui.leaf(); }\n".to_string(),
    ));
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, body)| (path.as_str(), body.as_str()))
        .collect();
    let result = resolve(&borrowed);
    let found = calls(&result, "consumer.ts::run");
    assert!(
        found.is_empty() || found == ["deep/b199.ts::leaf"],
        "a deep chain binds the declaring file or nothing, got {found:?}"
    );
}
