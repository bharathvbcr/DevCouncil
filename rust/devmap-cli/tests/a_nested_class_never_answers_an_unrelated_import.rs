//! The ChronosFlow `mini2` shape from the round-3 corpus diff: a file imports a
//! type the corpus does not declare (`...core.domain.model.Task`), while an
//! unrelated file declares a class *nested* in another type under the same bare
//! name (`UndoableAction.Task`). No import, reference or call from the first
//! file may reach the second.
//!
//! Run through the real binary so the whole build (import resolution, index and
//! ladder) is what is checked, not one rung.

use std::path::Path;
use std::process::Command;

fn devmap() -> String {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap").to_string_lossy().into_owned()
}

fn run(root: &Path, args: &[&str]) {
    let out = Command::new(devmap())
        .args(args)
        .current_dir(root)
        .output()
        .expect("devmap");
    assert!(
        out.status.success(),
        "devmap {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn scratch(name: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("devmap-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn write(root: &Path, path: &str, source: &str) {
    let full = root.join(path);
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::write(full, source).unwrap();
}

#[test]
fn an_import_of_a_type_the_corpus_lacks_never_binds_a_same_named_nested_class() {
    let root = scratch("nested-task");
    write(
        &root,
        "DayQuickItems.kt",
        "package com.ChronosFlow.VBCR.feature.daydial.model\n\nimport com.ChronosFlow.VBCR.core.domain.model.Task\n\ninternal data class QuickItem(val id: String)\n\nprivate fun Task.toDayQuickItem(): QuickItem? = null\n\ninternal fun build(tasks: List<Task>): List<QuickItem> =\n    tasks.mapNotNull { task -> task.toDayQuickItem() }\n",
    );
    write(
        &root,
        "w/W.kt",
        "package w\n\nsealed interface UndoableAction {\n    data class Task(val id: Int) : UndoableAction\n}\n",
    );
    run(&root, &["build", "."]);
    let store = devmap_store::Store::open(devmap_extract::paths::store_path(&root)).unwrap();
    let edges = store.latest_edges_for_test().unwrap();
    let crossing: Vec<&String> = edges
        .iter()
        .filter(|edge| edge.starts_with("DayQuickItems.kt") && edge.contains(">w/W.kt"))
        .collect();
    assert!(
        crossing.is_empty(),
        "DayQuickItems.kt must not reach w/W.kt: {crossing:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}
