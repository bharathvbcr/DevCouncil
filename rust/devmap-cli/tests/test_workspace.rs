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
    let resolution = resolver.resolve_all(&extractions).unwrap();
    let analysis = devmap_analyze::analyze(&extractions, &resolution);

    let db = devmap_extract::paths::store_path(root);
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
                verify_every_row: false,
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
                // Resolved against the repository, exactly as `Workspace::add`
                // records it. A literal here would name a store the fixture
                // above did not build, and the search would then report every
                // repository as having no symbols — a confident zero.
                db: devmap_extract::paths::store_path(root)
                    .strip_prefix(root)
                    .expect("store is inside the repository")
                    .to_string_lossy()
                    .into_owned(),
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

/// A Python module loaded by file path names a file beside its loader. It is
/// not a module another repository provides, even when that repository has a
/// package whose name is the path's first directory.
#[test]
fn a_python_path_load_is_not_a_cross_repo_link() {
    let base = scratch("pathload");
    let a = base.join("svc-a");
    let b = base.join("lib-b");
    build_repo(
        &a,
        &[
            (
                "run.py",
                "import importlib.util\n\
                 spec = importlib.util.spec_from_file_location(\"x\", \"scripts/x.py\")\n\
                 mod = importlib.util.module_from_spec(spec)\n",
            ),
            ("scripts/x.py", "def go():\n    pass\n"),
        ],
    );
    build_repo(&b, &[("scripts/__init__.py", "")]);

    let workspace = registry(&[("svca", &a), ("libb", &b)]);
    let links = link_candidates(&workspace).unwrap();
    assert!(
        links.is_empty(),
        "a path load was reported as a cross-repo link: {links:?}"
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

/// A host that dispatches a sibling repository's kernel by name — ojas and
/// qd-metal with tessl's — is a cross-repository candidate naming both ends.
/// A string matching a shader helper is not, a name two libraries declare is
/// not, and a host's own kernel stays inside its own graph.
#[test]
fn a_kernel_name_another_repo_declares_is_a_link_candidate() {
    use devmap_query::workspace::LinkKind;
    let base = scratch("kernels");
    let kernels = base.join("tessl");
    let host = base.join("ojas");
    let other = base.join("mlx");
    build_repo(
        &kernels,
        &[(
            "kernels/rows.metal",
            "inline float twice(float x) { return x * 2.0f; }\n\
             kernel void shared_name(device float *o [[buffer(0)]]) { o[0] = 0.0f; }\n\
             #define ROWS(NAME, D) kernel void NAME(device float *o [[buffer(0)]]) { o[0] = twice(D); }\n\
             ROWS(rows_h256, 256.0f)\n",
        )],
    );
    build_repo(
        &other,
        &[(
            "kernels/other.metal",
            "kernel void shared_name(device float *o [[buffer(0)]]) { o[0] = 1.0f; }\n",
        )],
    );
    build_repo(
        &host,
        &[
            (
                "src/gpu.rs",
                "const ROWS: &str = \"rows_h256\";\n\
                 pub fn attn(rt: &Runtime) { let p = rt.pipeline(ROWS); }\n\
                 pub fn own(rt: &Runtime) { let p = rt.pipeline(\"host_own\"); }\n\
                 pub fn helper(rt: &Runtime) { let p = rt.pipeline(\"twice\"); }\n\
                 pub fn ambiguous(rt: &Runtime) { let p = rt.pipeline(\"shared_name\"); }\n",
            ),
            (
                "kernels/own.metal",
                "kernel void host_own(device float *o [[buffer(0)]]) { o[0] = 2.0f; }\n",
            ),
        ],
    );

    let workspace = registry(&[("tessl", &kernels), ("ojas", &host), ("mlx", &other)]);
    let links: Vec<_> = link_candidates(&workspace)
        .unwrap()
        .into_iter()
        .filter(|link| link.kind == LinkKind::EntryName)
        .collect();
    assert_eq!(links.len(), 1, "{links:?}");
    let link = &links[0];
    assert_eq!(link.from_repo, "ojas");
    assert_eq!(link.to_repo, "tessl");
    assert_eq!(link.module_specifier, "rows_h256");
    assert_eq!(link.from_symbol.as_deref(), Some("src/gpu.rs::ROWS"));
    assert_eq!(link.to_symbol.as_deref(), Some("kernels/rows.metal::rows_h256"));
    assert!(link.evidence.contains("kernels/rows.metal"), "{}", link.evidence);

    // An import candidate written before kinds existed still reads as one.
    let legacy: devmap_query::workspace::LinkCandidate = serde_json::from_value(serde_json::json!({
        "from_repo": "a", "from_file": "f", "module_specifier": "m", "to_repo": "b", "evidence": "e"
    }))
    .unwrap();
    assert_eq!(legacy.kind, LinkKind::Import);
    let _ = std::fs::remove_dir_all(&base);
}
