//! Cross-repository queries over a registry of independently indexed repos.

use devmap_extract::*;
use devmap_query::workspace::{Workspace, WorkspaceRepo};
use devmap_query::*;
use devmap_resolve::*;
use devmap_store::{GenerationWriteOpts, Store};
use std::path::{Path, PathBuf};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("devmap-wsit-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Write a repo on disk and index it into its own store.
fn build_repo(root: &Path, files: &[(&str, &str)]) {
    for (rel, contents) in files {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, contents).unwrap();
    }
    let extractions: Vec<_> = files
        .iter()
        .map(|(rel, contents)| extract_file(rel, contents))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let analysis = devmap_analyze::analyze(&extractions, &resolution);

    let db = root.join(".devcouncil/codeintel/devmap.sqlite");
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();
    let store = Store::open(&db).unwrap();
    store
        .save_generation_with_opts(
            &extractions,
            &resolution,
            &analysis,
            GenerationWriteOpts {
                affected_paths: Vec::new(),
                deleted_paths: Vec::new(),
                build_started: None,
                repo_root: Some(root.to_string_lossy().into_owned()),
                discovery_refusals: None,
            },
        )
        .unwrap();
}

fn registry(repos: &[(&str, &Path)]) -> Workspace {
    Workspace {
        version: 1,
        repos: repos
            .iter()
            .map(|(name, root)| WorkspaceRepo {
                name: (*name).to_string(),
                root: root.to_path_buf(),
                db: ".devcouncil/codeintel/devmap.sqlite".to_string(),
            })
            .collect(),
    }
}

#[test]
fn a_search_spans_every_registered_repository() {
    let base = scratch("search");
    let a = base.join("svc-a");
    let b = base.join("lib-b");
    build_repo(
        &a,
        &[(
            "main.go",
            "package main\n\nfunc RunService() error {\n\treturn nil\n}\n",
        )],
    );
    build_repo(
        &b,
        &[(
            "store.go",
            "package store\n\nfunc RunStore() error {\n\treturn nil\n}\n",
        )],
    );

    let workspace = registry(&[("svca", &a), ("libb", &b)]);
    let result = workspace_search(&workspace, "Run", 4000, false).unwrap();

    assert_eq!(result.repos_queried, 2);
    assert!(result.unavailable.is_empty(), "{:?}", result.unavailable);
    let repos: std::collections::BTreeSet<&str> =
        result.items.iter().map(|i| i.repo.as_str()).collect();
    assert!(
        repos.contains("svca") && repos.contains("libb"),
        "a federated search returned only one repository: {repos:?}"
    );
    let _ = std::fs::remove_dir_all(&base);
}

/// A workspace answer assembled from a subset of its repositories is not a
/// workspace answer. The missing ones must be named.
#[test]
fn a_repository_with_no_store_is_reported_not_skipped() {
    let base = scratch("unavailable");
    let a = base.join("svc-a");
    let never_built = base.join("svc-c");
    build_repo(&a, &[("main.go", "package main\n\nfunc RunService() {}\n")]);
    std::fs::create_dir_all(&never_built).unwrap();

    let workspace = registry(&[("svca", &a), ("svcc", &never_built)]);
    let result = workspace_search(&workspace, "Run", 4000, false).unwrap();

    assert_eq!(result.repos_queried, 1);
    assert_eq!(result.unavailable.len(), 1, "{:?}", result.unavailable);
    assert_eq!(result.unavailable[0].repo, "svcc");
    assert!(
        result.unavailable[0].reason.contains("no store"),
        "the reason must say what was wrong: {}",
        result.unavailable[0].reason
    );
    let _ = std::fs::remove_dir_all(&base);
}

/// The one cross-repository relation this asserts, and the evidence for it.
#[test]
fn an_import_of_another_repos_go_module_is_a_link_candidate() {
    let base = scratch("links");
    let a = base.join("svc-a");
    let b = base.join("lib-b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    std::fs::write(a.join("go.mod"), "module example.com/svca\n\ngo 1.21\n").unwrap();
    std::fs::write(b.join("go.mod"), "module example.com/libb\n\ngo 1.21\n").unwrap();
    build_repo(
        &a,
        &[(
            "main.go",
            "package main\n\nimport (\n\t\"example.com/libb/store\"\n)\n\nfunc main() {\n\t_ = store.Open(\"x\")\n}\n",
        )],
    );
    build_repo(
        &b,
        &[(
            "store/store.go",
            "package store\n\nfunc Open(p string) error {\n\treturn nil\n}\n",
        )],
    );

    let workspace = registry(&[("svca", &a), ("libb", &b)]);
    let links = link_candidates(&workspace).unwrap();

    let link = links
        .iter()
        .find(|l| l.from_repo == "svca" && l.to_repo == "libb")
        .unwrap_or_else(|| panic!("no cross-repo link found: {links:?}"));
    assert_eq!(link.module_specifier, "example.com/libb/store");
    assert!(
        link.evidence.contains("module example.com/libb"),
        "the evidence must name what matched: {}",
        link.evidence
    );

    // A repository importing its own module is not a cross-repository link.
    assert!(
        !links.iter().any(|l| l.from_repo == l.to_repo),
        "a repository was linked to itself: {links:?}"
    );
    let _ = std::fs::remove_dir_all(&base);
}

/// Symbol names shared between repositories must not become links. Two repos
/// both declaring `New` is the normal case, not a dependency.
#[test]
fn shared_symbol_names_alone_do_not_make_a_link() {
    let base = scratch("nolink");
    let a = base.join("svc-a");
    let b = base.join("lib-b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    std::fs::write(a.join("go.mod"), "module example.com/svca\n\ngo 1.21\n").unwrap();
    std::fs::write(b.join("go.mod"), "module example.com/libb\n\ngo 1.21\n").unwrap();
    build_repo(
        &a,
        &[(
            "a.go",
            "package main\n\nfunc New() error {\n\treturn nil\n}\n",
        )],
    );
    build_repo(
        &b,
        &[(
            "b.go",
            "package store\n\nfunc New() error {\n\treturn nil\n}\n",
        )],
    );

    let workspace = registry(&[("svca", &a), ("libb", &b)]);
    let links = link_candidates(&workspace).unwrap();
    assert!(
        links.is_empty(),
        "a shared symbol name was reported as a cross-repo link: {links:?}"
    );
    let _ = std::fs::remove_dir_all(&base);
}
