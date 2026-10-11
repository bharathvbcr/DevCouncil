//! TypeScript's core shapes, each pinned beside its adversarial twin: class and
//! object-literal members as symbols, lexical `this`, re-export forms, lazy and
//! dynamic imports, NodeNext specifiers, npm namesakes, and workspace packages.
//!
//! Every positive case has a negative one, because each rule here widens what
//! binds and the cost of a rule that over-reaches is a confident wrong edge.

use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, Extraction};
use devmap_extract::workspaces::WorkspacePackage;
use devmap_extract::ProjectManifests;
use devmap_resolve::{ResolutionResult, Resolver};

fn resolve_with(files: &[(&str, &str)], manifests: Option<ProjectManifests>) -> ResolutionResult {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, body)| extract_file(path, body))
        .collect();
    let mut resolver = Resolver::new();
    if let Some(manifests) = &manifests {
        resolver.index_project_manifests(manifests);
    }
    resolver.index_extractions(&extractions);
    resolver.resolve_all(&extractions).expect("resolution")
}

fn resolve(files: &[(&str, &str)]) -> ResolutionResult {
    resolve_with(files, None)
}

fn calls(result: &ResolutionResult, from: &str) -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = result
        .edges
        .iter()
        .filter(|edge| edge.source_symbol == from && edge.edge_kind == EdgeKind::Calls)
        .map(|edge| {
            (
                edge.target_symbol.clone(),
                edge.resolution
                    .as_ref()
                    .map(|resolution| resolution.kind().label())
                    .unwrap_or("none")
                    .to_string(),
            )
        })
        .collect();
    found.sort();
    found
}

fn targets(result: &ResolutionResult, from: &str) -> Vec<String> {
    calls(result, from).into_iter().map(|(t, _)| t).collect()
}

fn unresolved_class(result: &ResolutionResult, from: &str, name: &str) -> Vec<String> {
    result
        .unresolved
        .iter()
        .filter(|row| row.source_symbol == from && row.callee_name == name)
        .map(|row| row.class.label().to_string())
        .collect()
}

#[test]
fn a_class_field_arrow_is_a_method_and_attributes_its_own_calls() {
    let result = resolve(&[(
        "s.ts",
        "export class C {\n  get(k: string) { return k; }\n  handle = (k: string) => this.get(k);\n}\n\
         export function run() { return new C().handle('x'); }\n",
    )]);
    assert_eq!(targets(&result, "s.ts::C.handle"), ["s.ts::C.get"]);
    assert!(targets(&result, "s.ts::run").contains(&"s.ts::C.handle".to_string()));
}

#[test]
fn an_object_literal_member_is_qualified_by_its_object() {
    let result = resolve(&[(
        "s.ts",
        "export const svc = {\n  fetchOne(id: string) { return helper(id); },\n  remove: (id: string) => helper(id),\n};\n\
         function helper(id: string) { return id; }\n\
         export function use() { return svc.fetchOne('1') + svc.remove('2'); }\n",
    )]);
    assert_eq!(targets(&result, "s.ts::svc.fetchOne"), ["s.ts::helper"]);
    assert_eq!(targets(&result, "s.ts::svc.remove"), ["s.ts::helper"]);
    let from_use = targets(&result, "s.ts::use");
    assert!(
        from_use.contains(&"s.ts::svc.fetchOne".to_string()),
        "{from_use:?}"
    );
    assert!(
        from_use.contains(&"s.ts::svc.remove".to_string()),
        "{from_use:?}"
    );
}

#[test]
fn an_object_literal_that_is_not_a_module_const_names_no_owner() {
    // Passed as an argument, the object has no name a call could spell.
    let result = resolve(&[(
        "s.ts",
        "export function make() { return register({ run() { helper(); } }); }\nfunction helper() {}\n",
    )]);
    assert!(
        result
            .edges
            .iter()
            .all(|edge| !edge.target_symbol.contains("register.")),
        "no member symbol is invented for an anonymous object"
    );
}

#[test]
fn this_in_an_arrow_nested_in_a_method_is_the_class_but_not_in_a_function() {
    let result = resolve(&[(
        "g.ts",
        "export class G {\n  build(p: string) { return p; }\n  open() {\n    const arrow = () => { this.build('a'); };\n    function own() { this.build('b'); }\n    arrow(); own();\n  }\n}\n",
    )]);
    assert_eq!(targets(&result, "g.ts::G.open.arrow"), ["g.ts::G.build"]);
    // A `function` has a `this` of its own: nothing is claimed for it.
    assert!(targets(&result, "g.ts::G.open.own").is_empty());
    assert!(!unresolved_class(&result, "g.ts::G.open.own", "build").is_empty());
}

#[test]
fn an_import_then_export_list_republishes_the_imported_binding() {
    let result = resolve(&[
        ("store.ts", "export function useWorkspace() { return 1; }\n"),
        (
            "view.ts",
            "import { useWorkspace } from './store';\nexport { useWorkspace };\n",
        ),
        (
            "user.ts",
            "import { useWorkspace } from './view';\nexport function go() { return useWorkspace(); }\n",
        ),
    ]);
    assert_eq!(targets(&result, "user.ts::go"), ["store.ts::useWorkspace"]);
}

#[test]
fn a_default_import_binds_whatever_the_module_published_not_a_symbol_of_the_importers_name() {
    let result = resolve(&[
        (
            "card.tsx",
            "import { memo } from 'react';\nfunction Card() { return 1; }\nexport default memo(Card);\n",
        ),
        (
            "index.ts",
            "export { default as Tile } from './card';\nexport { default } from './card';\n",
        ),
        (
            "use.tsx",
            "import Anything, { Tile } from './index';\nimport Direct from './card';\n\
             export function Page() { return [Anything(), Tile(), Direct()]; }\n",
        ),
    ]);
    let found = targets(&result, "use.tsx::Page");
    assert_eq!(
        found.iter().filter(|t| *t == "card.tsx::Card").count(),
        3,
        "all three spellings reach Card: {found:?}"
    );
}

#[test]
fn a_default_import_of_an_unwrapped_hoc_is_not_looked_through() {
    let result = resolve(&[
        (
            "card.tsx",
            "function Card() { return 1; }\nexport default connect(Card);\n",
        ),
        (
            "use.tsx",
            "import Thing from './card';\nexport function Page() { return Thing(); }\n",
        ),
    ]);
    assert!(
        !targets(&result, "use.tsx::Page").contains(&"card.tsx::Card".to_string()),
        "connect(Card) returns something else; Card is not the default"
    );
}

#[test]
fn lazy_and_awaited_imports_bind_the_module_they_load() {
    let result = resolve(&[
        ("pages/about.tsx", "export default function AboutPage() { return 1; }\n"),
        ("heavy.ts", "export function compute() { return 2; }\n"),
        (
            "app.tsx",
            "import { lazy } from 'react';\nconst About = lazy(() => import('./pages/about'));\n\
             export async function run() { const m = await import('./heavy'); return m.compute(); }\n\
             export function App() { return <About />; }\n",
        ),
    ]);
    assert_eq!(
        targets(&result, "app.tsx::App"),
        ["pages/about.tsx::AboutPage"]
    );
    assert_eq!(targets(&result, "app.tsx::run"), ["heavy.ts::compute"]);
}

#[test]
fn a_dynamic_import_handle_does_not_bind_a_member_it_never_loaded() {
    let result = resolve(&[
        ("heavy.ts", "export function compute() { return 2; }\n"),
        (
            "app.ts",
            "export async function run() { const m = await import('./heavy'); return m.absent(); }\n",
        ),
    ]);
    assert!(targets(&result, "app.ts::run").is_empty());
}

#[test]
fn a_nodenext_js_specifier_names_the_ts_source_and_a_missing_one_names_nothing() {
    let result = resolve(&[
        ("a.ts", "export function f() { return 1; }\n"),
        (
            "b.ts",
            "import { f } from './a.js';\nimport { g } from './nope.js';\n\
             export function use() { f(); g(); }\n",
        ),
    ]);
    let found = calls(&result, "b.ts::use");
    assert!(
        found.contains(&("a.ts::f".to_string(), "ImportScoped".to_string())),
        "{found:?}"
    );
    assert!(found.iter().all(|(t, _)| t != "nope.ts::g"));
}

#[test]
fn an_npm_import_never_binds_to_a_repository_namesake() {
    let result = resolve(&[
        ("lib/format.ts", "export function formatTitle() { return 1; }\n"),
        (
            "use.ts",
            "import { formatTitle } from 'some-npm-lib';\nexport function f() { return formatTitle('y'); }\n",
        ),
    ]);
    assert!(
        targets(&result, "use.ts::f").is_empty(),
        "{:?}",
        calls(&result, "use.ts::f")
    );
    assert_eq!(
        unresolved_class(&result, "use.ts::f", "formatTitle"),
        ["external"]
    );
}

#[test]
fn an_import_that_resolves_to_no_file_is_unresolved_not_global() {
    let result = resolve(&[
        ("lib/casefile.ts", "export function caseHelper() { return 1; }\n"),
        (
            "use.ts",
            "import { caseHelper } from '../lib/CaseFile';\nexport function f() { return caseHelper(); }\n",
        ),
    ]);
    assert!(targets(&result, "use.ts::f").is_empty());
    assert_eq!(
        unresolved_class(&result, "use.ts::f", "caseHelper"),
        ["unresolved"]
    );
}

#[test]
fn a_name_the_file_does_not_import_still_resolves_at_the_global_tier() {
    // The gate is the import statement, not the name: a bare call with no
    // import evidence keeps the answer it always had.
    let result = resolve(&[
        (
            "lib/format.ts",
            "export function formatTitle() { return 1; }\n",
        ),
        (
            "use.ts",
            "export function f() { return formatTitle('y'); }\n",
        ),
    ]);
    assert_eq!(
        targets(&result, "use.ts::f"),
        ["lib/format.ts::formatTitle"]
    );
}

fn workspace(name: &str, dir: &str, entries: &[&str]) -> WorkspacePackage {
    WorkspacePackage {
        name: name.to_string(),
        dir: dir.to_string(),
        entries: entries.iter().map(|e| e.to_string()).collect(),
    }
}

#[test]
fn a_workspace_package_resolves_to_its_source_entry_and_subpaths() {
    let manifests = ProjectManifests {
        ts_workspaces: vec![
            workspace("@acme/shared", "packages/shared", &["dist/index.js"]),
            workspace("@acme/shared-ui", "packages/ui", &[]),
        ],
        ..ProjectManifests::default()
    };
    let result = resolve_with(
        &[
            ("packages/shared/src/index.ts", "export function sharedFormat() { return 1; }\n"),
            ("packages/shared/src/util.ts", "export function helper() { return 1; }\n"),
            ("packages/ui/src/index.ts", "export function button() { return 1; }\n"),
            (
                "apps/web/src/use.ts",
                "import { sharedFormat } from '@acme/shared';\nimport { helper } from '@acme/shared/util';\n\
                 import { button } from '@acme/shared-ui';\nimport { gone } from '@acme/missing';\n\
                 export function f() { sharedFormat(); helper(); button(); gone(); }\n",
            ),
        ],
        Some(manifests),
    );
    let found = calls(&result, "apps/web/src/use.ts::f");
    for expected in [
        "packages/shared/src/index.ts::sharedFormat",
        "packages/shared/src/util.ts::helper",
        "packages/ui/src/index.ts::button",
    ] {
        assert!(
            found.contains(&(expected.to_string(), "ImportScoped".to_string())),
            "{expected} not ImportScoped: {found:?}"
        );
    }
    // The longest name wins: `@acme/shared-ui` is not `@acme/shared` + `-ui`.
    assert!(found
        .iter()
        .all(|(t, _)| !t.contains("shared/src/index.ts::button")));
    // An unlisted package is external, and nothing binds `gone`.
    assert_eq!(
        unresolved_class(&result, "apps/web/src/use.ts::f", "gone"),
        ["external"]
    );
}
