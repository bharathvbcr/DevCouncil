//! A federated search must report what it did not show.
//!
//! `workspace_search` unioned each repository's *returned items* and recomputed
//! `total` from that union, discarding every per-repository `total`/`hidden`/
//! `truncated`. Each repository had already applied the token budget before
//! answering, so a repository with 500 matches that could only fit four in the
//! budget contributed four — and the federated answer reported `total: 4,
//! truncated: false`. A capped sample presented as the complete answer is the
//! failure class this pins.

use devmap_extract::extract_file;
use devmap_query::workspace::{Workspace, WorkspaceRepo};
use devmap_query::workspace_search;
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};
use std::path::PathBuf;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("devmap-ws-search-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

/// A repository rooted at `root` whose store holds `matches` symbols all
/// sharing one name prefix.
fn repo_with_matches(root: &std::path::Path, matches: usize) -> WorkspaceRepo {
    let mut source = String::new();
    for index in 0..matches {
        source.push_str(&format!("def widget{index:04}():\n    return {index}\n"));
    }
    std::fs::write(root.join("things.py"), &source).unwrap();

    let extractions = vec![extract_file("things.py", &source)];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let analysis = devmap_analyze::analyze(&extractions, &resolution);

    let repo = WorkspaceRepo {
        name: "alpha".to_string(),
        root: root.to_path_buf(),
        db: ".devcouncil/codeintel/devmap.sqlite".to_string(),
    };
    let db = repo.db_path();
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();
    let store = Store::open(&db).unwrap();
    store
        .save_generation_with_opts(
            &extractions,
            &resolution,
            &analysis,
            GenerationWriteOpts {
                repo_root: Some(root.to_string_lossy().into_owned()),
                ..GenerationWriteOpts::default()
            },
        )
        .unwrap();
    repo
}

/// 500 matches behind a budget that admits a handful must be reported as 500
/// matches, truncated.
#[test]
fn a_budgeted_repository_contributes_its_whole_total_not_its_page() {
    let dir = scratch("total");
    let root = dir.join("alpha");
    std::fs::create_dir_all(&root).unwrap();
    let repo = repo_with_matches(&root, 500);
    let workspace = Workspace {
        version: 1,
        repos: vec![repo],
    };

    let found = workspace_search(&workspace, "widget", 120, false).expect("federated search");

    assert_eq!(found.repos_queried, 1, "the repository must be queried");
    assert!(
        found.shown > 0 && (found.shown as usize) < 500,
        "the budget must admit some hits and withhold most: shown={}",
        found.shown
    );
    assert_eq!(
        found.total, 500,
        "the federated total must aggregate the repository's own total, not the \
         page it could afford to return"
    );
    assert!(
        found.truncated,
        "495 withheld matches were reported as a complete answer"
    );
    assert_eq!(
        found.hidden,
        found.total - found.shown,
        "hidden must account for exactly the matches not shown"
    );
    assert_eq!(found.shown as usize, found.items.len());
    let _ = std::fs::remove_dir_all(&dir);
}

/// The aggregate must still say "complete" when it is: a budget that fits
/// everything must not report truncation.
#[test]
fn a_budget_that_fits_everything_is_not_reported_as_truncated() {
    let dir = scratch("complete");
    let root = dir.join("alpha");
    std::fs::create_dir_all(&root).unwrap();
    let repo = repo_with_matches(&root, 3);
    let workspace = Workspace {
        version: 1,
        repos: vec![repo],
    };

    let found = workspace_search(&workspace, "widget", 100_000, false).expect("federated search");
    assert_eq!(found.total, 3);
    assert_eq!(found.shown, 3);
    assert_eq!(found.hidden, 0);
    assert!(!found.truncated);
    let _ = std::fs::remove_dir_all(&dir);
}
