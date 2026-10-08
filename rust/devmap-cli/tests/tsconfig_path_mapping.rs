//! `tsconfig` `extends`, `paths`, `baseUrl` and project `references`, end to
//! end through `devmap build`.
//!
//! A non-relative specifier — `@core/util`, `@/lib/format` — names a file only
//! through the config that governs the importer. Before the kernel read those
//! configs, every such import was filed `External`, so a monorepo's own
//! packages read as third-party code and their callers vanished from impact.
//!
//! The fixture is the two shapes real repositories use:
//!
//! * a package whose `tsconfig.json` `extends` a root `tsconfig.base.json`
//!   (JSONC: comments and trailing commas) that declares `baseUrl` and
//!   `paths`, under a solution-style root that only lists `references`;
//! * a Vite-style app whose `tsconfig.json` is `"files": []` plus a reference
//!   to `tsconfig.app.json`, which holds `paths` and no `baseUrl` — so the
//!   targets resolve against the config that wrote them.

use std::path::{Path, PathBuf};
use std::process::Command;

fn devmap() -> String {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap").to_string_lossy().into_owned()
}

fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn fixture(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("devmap-tsconfig-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    write(
        &root,
        "tsconfig.base.json",
        r#"{
  // Shared options for every package.
  "compilerOptions": {
    "baseUrl": ".",
    "paths": {
      "@core/*": ["packages/core/src/*"], /* the core package */
    },
  },
}
"#,
    );
    write(
        &root,
        "tsconfig.json",
        r#"{ "files": [], "references": [{ "path": "./packages/app" }] }"#,
    );
    write(
        &root,
        "packages/app/tsconfig.json",
        r#"{ "extends": "../../tsconfig.base.json", "include": ["src"] }"#,
    );
    write(
        &root,
        "packages/core/src/util.ts",
        "export function coreHelper(): number { return 1; }\n",
    );
    // Namesakes. Without them the unique-global rung binds `coreHelper()` by
    // name alone and the fixture passes whether or not any config was read;
    // with them only the import — and so only the mapping — can choose.
    write(
        &root,
        "packages/legacy/src/util.ts",
        "export function coreHelper(): number { return 2; }\n",
    );
    write(
        &root,
        "web/src/other/format.ts",
        "export function formatName(name: string): string { return name + \"!\"; }\n",
    );
    write(
        &root,
        "packages/app/src/main.ts",
        "import { coreHelper } from \"@core/util\";\n\
         export function run(): number { return coreHelper(); }\n",
    );
    write(
        &root,
        "packages/app/src/base_url.ts",
        "import { coreHelper } from \"packages/core/src/util\";\n\
         export function viaBaseUrl(): number { return coreHelper(); }\n",
    );
    write(
        &root,
        "packages/app/src/outside.ts",
        "import { useState } from \"react\";\n\
         import { gone } from \"@core/missing\";\n\
         export function edges(): void { useState(0); gone(); }\n",
    );
    write(
        &root,
        "web/tsconfig.json",
        r#"{ "files": [], "references": [{ "path": "./tsconfig.app.json" }] }"#,
    );
    write(
        &root,
        "web/tsconfig.app.json",
        r#"{ "compilerOptions": { "paths": { "@/*": ["./src/*"] } } }"#,
    );
    write(
        &root,
        "web/src/lib/format.ts",
        "export function formatName(name: string): string { return name; }\n",
    );
    write(
        &root,
        "web/src/main.ts",
        "import { formatName } from \"@/lib/format\";\n\
         export function boot(): string { return formatName(\"x\"); }\n",
    );
    let out = Command::new(devmap())
        .args(["build", "."])
        .current_dir(&root)
        .output()
        .expect("devmap build");
    assert!(
        out.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    root
}

fn call_edges(root: &Path) -> Vec<(String, String)> {
    let conn = rusqlite::Connection::open(devmap_extract::paths::store_path(root)).unwrap();
    let mut statement = conn
        .prepare(
            "SELECT source_symbol, target_symbol FROM generation_edges
             WHERE generation_id = (SELECT max(id) FROM generations)
               AND lower(edge_kind) LIKE '%call%'
             ORDER BY 1, 2",
        )
        .unwrap();
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn classes(root: &Path, callee: &str) -> Vec<String> {
    let conn = rusqlite::Connection::open(devmap_extract::paths::store_path(root)).unwrap();
    let mut statement = conn
        .prepare(
            "SELECT classification FROM generation_unresolved
             WHERE generation_id = (SELECT max(id) FROM generations)
               AND callee_name = ?1",
        )
        .unwrap();
    statement
        .query_map([callee], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// `from` calls `to`, and no namesake of it: the edge is the import's bind,
/// not the global rung's fan-out across every `coreHelper` in the tree.
fn binds_only(edges: &[(String, String)], from: &str, to: &str) -> bool {
    let name = to.rsplit("::").next().unwrap_or(to);
    let reached: Vec<&str> = edges
        .iter()
        .filter(|(source, target)| source == from && target.rsplit("::").next() == Some(name))
        .map(|(_, target)| target.as_str())
        .collect();
    reached == [to]
}

#[test]
fn paths_inherited_through_extends_and_a_solution_root_bind_the_import() {
    let root = fixture("extends");
    let edges = call_edges(&root);
    assert!(
        binds_only(
            &edges,
            "packages/app/src/main.ts::run",
            "packages/core/src/util.ts::coreHelper"
        ),
        "`@core/util` is mapped by the base config the package extends: {edges:?}"
    );
    assert!(
        binds_only(
            &edges,
            "packages/app/src/base_url.ts::viaBaseUrl",
            "packages/core/src/util.ts::coreHelper"
        ),
        "a bare specifier resolves against the inherited `baseUrl`: {edges:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// An edit to a config alone re-resolves on an ordinary incremental build.
///
/// Every `tsconfig*.json` is an indexed, content-hashed file, so changing one
/// defeats the no-change early return and the build re-collects the configs.
/// Pinned rather than assumed: if a config edit ever stopped moving the
/// importer's edge, the graph would keep the old mapping with nothing saying
/// it is stale.
#[test]
fn a_config_only_edit_moves_the_edge_on_an_incremental_build() {
    let root = fixture("remap");
    assert!(binds_only(
        &call_edges(&root),
        "packages/app/src/main.ts::run",
        "packages/core/src/util.ts::coreHelper"
    ));
    write(
        &root,
        "tsconfig.base.json",
        r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "@core/*": ["packages/legacy/src/*"] } } }"#,
    );
    let out = Command::new(devmap())
        .args(["build", "."])
        .current_dir(&root)
        .output()
        .expect("devmap build");
    assert!(
        out.status.success(),
        "incremental build failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let edges = call_edges(&root);
    assert!(
        binds_only(
            &edges,
            "packages/app/src/main.ts::run",
            "packages/legacy/src/util.ts::coreHelper"
        ),
        "the remapped `@core/*` must move the edge without `--full`: {edges:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_referenced_app_config_maps_paths_against_the_config_that_wrote_them() {
    let root = fixture("references");
    let edges = call_edges(&root);
    assert!(
        binds_only(
            &edges,
            "web/src/main.ts::boot",
            "web/src/lib/format.ts::formatName"
        ),
        "`@/lib/format` is mapped by `tsconfig.app.json`, reached through \
         `references`: {edges:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_unmapped_package_stays_external_and_a_mapped_miss_is_an_index_gap() {
    let root = fixture("negative");
    assert_eq!(
        classes(&root, "useState"),
        vec!["external".to_string()],
        "no mapping names `react`, so it comes from outside the corpus"
    );
    assert_eq!(
        classes(&root, "gone"),
        vec!["unresolved".to_string()],
        "`@core/missing` matched a `paths` pattern into this tree, so a miss \
         is a repository file the index lacks, not an outside module"
    );
    let _ = std::fs::remove_dir_all(&root);
}
