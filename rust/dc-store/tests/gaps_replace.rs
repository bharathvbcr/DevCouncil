//! Replacing a task's gaps is one transaction, and a replacement the table
//! cannot hold is refused rather than half-applied or left to wedge the task.

use dc_store::Store;
use dc_store::records::{GapRow, MAX_GAPS_PER_REPLACE, VerificationRun};

mod support;
use support::{dcstore, seeded};

fn gap(id: &str, task: Option<&str>) -> GapRow {
    GapRow {
        id: id.into(),
        severity: "high".into(),
        gap_type: "test_failed".into(),
        requirement_id: None,
        task_id: task.map(str::to_string),
        description: format!("gap {id}"),
        evidence_json: "[]".into(),
        recommended_fix: "fix".into(),
        blocking: true,
        file: None,
        line: None,
        suggested_command: None,
        acceptance_criterion_id: None,
        expected_verification_method: None,
        stdout_path: None,
        stderr_path: None,
    }
}

fn ids(store: &Store, task: &str) -> Vec<String> {
    let (rows, _) = store.gaps_list(Some(task)).expect("list");
    let mut ids: Vec<String> = rows.into_iter().map(|g| g.id).collect();
    ids.sort();
    ids
}

#[test]
fn a_row_naming_another_task_is_rewritten_to_the_key_and_cannot_wedge_it() {
    let store = Store::open_in_memory().expect("open");
    store
        .gaps_replace("T1", &[gap("G1", Some("OTHER")), gap("G2", None)])
        .expect("first replace");
    assert_eq!(
        ids(&store, "T1"),
        ["G1", "G2"],
        "rows belong to the key task"
    );
    assert!(ids(&store, "OTHER").is_empty());
    // Before the fix G1 was stored under OTHER, T1's DELETE missed it, and
    // this second replace failed UNIQUE constraint failed: gaps.id.
    store
        .gaps_replace("T1", &[gap("G1", Some("OTHER"))])
        .expect("a later replace of the same task still works");
    assert_eq!(ids(&store, "T1"), ["G1"]);
}

#[test]
fn a_duplicate_id_collapses_to_its_last_occurrence() {
    let store = Store::open_in_memory().expect("open");
    let mut second = gap("G1", Some("T1"));
    second.description = "the later one".into();
    store
        .gaps_replace(
            "T1",
            &[gap("G1", Some("T1")), gap("G2", Some("T1")), second],
        )
        .expect("duplicates must not abort the replacement");
    let (rows, _) = store.gaps_list(Some("T1")).expect("list");
    assert_eq!(rows.len(), 2);
    let g1 = rows.iter().find(|g| g.id == "G1").expect("G1");
    assert_eq!(g1.description, "the later one");
}

#[test]
fn an_over_limit_set_is_refused_and_changes_nothing() {
    let store = Store::open_in_memory().expect("open");
    store
        .gaps_replace("T1", &[gap("KEEP", Some("T1"))])
        .expect("seed");
    let many: Vec<GapRow> = (0..=MAX_GAPS_PER_REPLACE)
        .map(|i| gap(&format!("G{i}"), Some("T1")))
        .collect();
    let err = store.gaps_replace("T1", &many).unwrap_err();
    assert!(err.to_string().contains("limit"), "{err}");
    assert_eq!(ids(&store, "T1"), ["KEEP"]);
}

#[test]
fn a_write_that_fails_part_way_leaves_the_previous_set_intact() {
    let store = Store::open_in_memory().expect("open");
    store
        .gaps_replace("OTHER", &[gap("TAKEN", Some("OTHER"))])
        .expect("other");
    store
        .gaps_replace("T1", &[gap("OLD-1", Some("T1")), gap("OLD-2", Some("T1"))])
        .expect("seed");
    // The third row collides with another task's gap: the Nth write fails.
    let err = store
        .gaps_replace(
            "T1",
            &[
                gap("NEW-1", Some("T1")),
                gap("NEW-2", Some("T1")),
                gap("TAKEN", Some("T1")),
            ],
        )
        .unwrap_err();
    assert!(err.to_string().contains("UNIQUE"), "{err}");
    assert_eq!(
        ids(&store, "T1"),
        ["OLD-1", "OLD-2"],
        "the previous set survives"
    );
}

#[test]
fn a_run_and_its_gaps_commit_together_or_not_at_all() {
    let store = Store::open_in_memory().expect("open");
    store
        .gaps_replace("OTHER", &[gap("TAKEN", Some("OTHER"))])
        .expect("other");
    let run = VerificationRun {
        id: "run-1".into(),
        task_id: "T1".into(),
        sandbox: "local".into(),
        environment_json: "{}".into(),
        commands_json: "[]".into(),
        status: "blocked".into(),
        started_at: "2026-01-01T00:00:00Z".into(),
        finished_at: None,
    };
    store
        .verification_commit(Some(&run), "T1", &[gap("TAKEN", Some("T1"))])
        .unwrap_err();
    assert!(
        store.run_get("run-1").expect("get").is_none(),
        "no run without its gaps"
    );
    store
        .verification_commit(Some(&run), "T1", &[gap("G1", Some("T1"))])
        .expect("commit");
    assert!(store.run_get("run-1").expect("get").is_some());
    assert_eq!(ids(&store, "T1"), ["G1"]);
    let mut foreign = run.clone();
    foreign.task_id = "ELSE".into();
    assert!(
        store
            .verification_commit(Some(&foreign), "T1", &[])
            .is_err()
    );
}

fn f(s: &str) -> String {
    format!("{}:{s}", s.len())
}

#[test]
fn the_gaps_replace_command_commits_a_run_and_its_gaps() {
    let db = seeded("gaps-replace-cmd");
    let mut w = String::new();
    for p in [
        "DCVC1",
        "R",
        "run-9",
        "TASK-1",
        "local",
        "{}",
        "[{\"command\":\"false\"}]",
        "blocked",
        "t0",
    ] {
        w += &f(p);
    }
    w += "-";
    w += &f("1");
    for p in [
        "G-A",
        "high",
        "test_failed",
        "failed\nbadly",
        "[\"exit code 1\"]",
        "fix",
        "1",
    ] {
        w += &f(p);
    }
    w += "---";
    w += &f("go test ./...");
    w += "----";
    let reply = dcstore(&db, &["gaps-replace", "--task", "TASK-1", "--wire", &w]);
    assert_eq!(reply.code, 0, "{}", reply.stdout);
    let gaps = dcstore(&db, &["gaps", "--task", "TASK-1"]);
    assert!(
        gaps.stdout.contains("\"id\":\"G-A\"") && gaps.stdout.contains("exit code 1"),
        "{}",
        gaps.stdout
    );
    assert!(gaps.stdout.contains("go test ./..."), "{}", gaps.stdout);

    let bad = dcstore(
        &db,
        &["gaps-replace", "--task", "TASK-1", "--wire", "garbage"],
    );
    assert_ne!(bad.code, 0, "{}", bad.stdout);
    let after = dcstore(&db, &["gaps", "--task", "TASK-1"]);
    assert!(
        after.stdout.contains("\"id\":\"G-A\""),
        "a refused wire left the set intact: {}",
        after.stdout
    );
}
