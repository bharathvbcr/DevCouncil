//! `RESOLUTION_REUSE_EPOCH` is what keeps a store built before the CStyle split
//! from re-serving its cross-group edges.
//!
//! Per-file resolution blobs are keyed by content hash alone, so an unchanged
//! Java file would keep a pre-split `Cls.h -> c/lib.c::only_c` edge forever
//! unless the epoch moves the blobs to a directory the new build never reads.
//!
//! A "warm store built by the base resolver" is staged here rather than built:
//! a test cannot run the old resolver, so the store is built by this tree and
//! the one edge the old resolver would have written is added to the two places
//! it would have been (the committed generation and the file's reuse blob),
//! under the directory name the pre-split build used, `resolution-reuse-1`.
//! With the constant at "1" that directory is the live one and the edge
//! survives; at "2" it is never read.

use std::process::Command;
use std::sync::Arc;

use devmap_extract::model::EdgeKind;
use devmap_resolve::{LangFamily, Resolution, ResolvedEdge};

fn devmap() -> String {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap").to_string_lossy().into_owned()
}

fn build(root: &std::path::Path, extra: &[&str]) -> String {
    let mut args = vec!["build", "."];
    args.extend_from_slice(extra);
    let out = Command::new(devmap())
        .args(&args)
        .current_dir(root)
        .output()
        .expect("devmap build");
    assert!(
        out.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn scratch(label: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("devmap-cstyle-epoch-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("c")).unwrap();
    std::fs::create_dir_all(root.join("java")).unwrap();
    std::fs::create_dir_all(root.join("util")).unwrap();
    std::fs::write(root.join("c/lib.c"), "int only_c(int x) { return x; }\n").unwrap();
    std::fs::write(root.join("java/Cls.java"), "class Cls { void h() { only_c(1); } }\n").unwrap();
    std::fs::write(root.join("util/other.py"), "def f():\n    return 1\n").unwrap();
    root
}

fn db_path(root: &std::path::Path) -> std::path::PathBuf {
    devmap_extract::paths::store_path(root)
}

const STALE: &str = "java/Cls.java::Cls.h>c/lib.c::only_c:Calls:0.90000";

fn has_stale_edge(root: &std::path::Path) -> bool {
    let store = devmap_store::Store::open(db_path(root)).unwrap();
    store.latest_edges_for_test().unwrap().iter().any(|row| row == STALE)
}

/// A store the pre-split resolver would have left behind: the stale cross-group
/// edge is in the latest generation and in `Cls.java`'s reuse blob under
/// `resolution-reuse-1`.
fn warm_store_from_the_base_resolver(label: &str) -> std::path::PathBuf {
    let root = scratch(label);
    build(&root, &[]);
    assert!(
        !has_stale_edge(&root),
        "this tree must not itself produce a Java -> C edge"
    );

    // The blob.
    let state = devmap_extract::paths::state_dir(&root);
    let legacy = state.join("resolution-reuse-1");
    let current = state.join(format!(
        "resolution-reuse-{}",
        devmap_resolve::resolver::RESOLUTION_REUSE_EPOCH
    ));
    if current != legacy {
        std::fs::rename(&current, &legacy).unwrap();
    }
    let stale = serde_json::to_value(ResolvedEdge::resolved(
        "java/Cls.java".into(),
        "c/lib.c".into(),
        "java/Cls.java::Cls.h".into(),
        "c/lib.c::only_c".into(),
        EdgeKind::Calls,
        Arc::new(Resolution::UniqueGlobal {
            target_symbol: "c/lib.c::only_c".into(),
            target_file: "c/lib.c".into(),
            family: LangFamily::from_lang("c"),
        }),
        None,
    ))
    .unwrap();
    use sha2::{Digest, Sha256};
    let blob = legacy.join(format!("{:x}.json", Sha256::digest(b"java/Cls.java")));
    let mut value: serde_json::Value = serde_json::from_slice(&std::fs::read(&blob).unwrap()).unwrap();
    value["edges"].as_array_mut().unwrap().push(stale);
    std::fs::write(&blob, serde_json::to_vec(&value).unwrap()).unwrap();

    // The committed generation. `generation_edges` is a view over the
    // time-versioned `edge_rows`; an open row is one with no `valid_to`.
    let conn = rusqlite::Connection::open(db_path(&root)).unwrap();
    let added = conn
        .execute(
            "INSERT INTO edge_rows
               (source_file_id, target_file_id, source_symbol, target_symbol,
                edge_kind, confidence, resolution, candidate_total, valid_from, valid_to)
             SELECT (SELECT id FROM paths WHERE path = 'java/Cls.java'),
                    (SELECT id FROM paths WHERE path = 'c/lib.c'),
                    'java/Cls.java::Cls.h', 'c/lib.c::only_c', 'Calls', 0.9,
                    'UniqueGlobal', NULL, max(id), NULL
             FROM generations",
            [],
        )
        .unwrap();
    assert_eq!(added, 1, "the stale edge was staged");
    drop(conn);
    assert!(has_stale_edge(&root), "the staged store carries the stale edge");
    root
}

/// The resolving build removes every `resolution-reuse-<other epoch>` directory
/// (`remove_old_reuse_epochs`), so the pre-split `resolution-reuse-1` is gone.
fn assert_legacy_reuse_dir_removed(root: &std::path::Path) {
    let legacy = devmap_extract::paths::state_dir(root).join("resolution-reuse-1");
    assert!(
        devmap_resolve::resolver::RESOLUTION_REUSE_EPOCH == "1" || !legacy.exists(),
        "{} was not removed by the resolving build",
        legacy.display()
    );
}

fn edit_unrelated_file(root: &std::path::Path) {
    std::fs::write(root.join("util/other.py"), "def f():\n    return 2\n").unwrap();
}

#[test]
fn an_unrelated_edit_then_a_normal_build_drops_the_stale_cross_group_edge() {
    let root = warm_store_from_the_base_resolver("edit");
    edit_unrelated_file(&root);
    let out = build(&root, &[]);
    assert!(
        !has_stale_edge(&root),
        "a pre-split cross-group edge survived an unrelated edit; build said:\n{out}"
    );
    assert_legacy_reuse_dir_removed(&root);
}

#[test]
fn an_affected_build_drops_the_stale_cross_group_edge() {
    let root = warm_store_from_the_base_resolver("affected");
    edit_unrelated_file(&root);
    let out = build(&root, &["--affected", "util/other.py"]);
    assert!(
        !has_stale_edge(&root),
        "a pre-split cross-group edge survived --affected; build said:\n{out}"
    );
    assert_legacy_reuse_dir_removed(&root);
}

/// `--full` never reads reuse blobs, so it drops the edge at either epoch. This
/// is a guard that the full path keeps doing so, not evidence about the bump.
#[test]
fn a_full_build_drops_the_stale_cross_group_edge() {
    let root = warm_store_from_the_base_resolver("full");
    let out = build(&root, &["--full"]);
    assert!(
        !has_stale_edge(&root),
        "a pre-split cross-group edge survived --full; build said:\n{out}"
    );
}

/// Documents today's behaviour, which is NOT a defect of the split: a build with
/// no source change is the pre-existing no-change shortcut ("generation still
/// current"), accepted 2026-10-20, so it neither re-resolves nor reads the reuse
/// epoch. A store built before the split keeps its cross-group edges until any
/// file changes or `--full` runs.
#[test]
#[ignore = "pre-existing no-change shortcut has no resolver-version input; see ft-c1d8cb8e"]
fn a_zero_edit_build_keeps_the_stale_edge_by_the_no_change_shortcut() {
    let root = warm_store_from_the_base_resolver("zero");
    let out = build(&root, &[]);
    assert!(out.contains("No source changes"), "expected the shortcut; build said:\n{out}");
    assert!(has_stale_edge(&root));
}
