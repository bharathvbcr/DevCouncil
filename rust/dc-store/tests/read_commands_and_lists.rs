//! Tests for `dcstore` read commands (`runs`, `run-get`, `handoff-get`) and
//! filtered vs unfiltered list parity across `evidence`, `gaps`, `gap_history`,
//! and `verification_runs`.

mod support;

use dc_store::Store;
use dc_store::records::{AgentHandoff, GapRow, MAX_LIST_ROWS, VerificationRun};

#[test]
fn runs_run_get_and_handoff_get_cli_round_trip() {
    let db = support::seeded("read-cmds");
    let store = Store::open_existing(&db).unwrap();

    store
        .run_record(&VerificationRun {
            id: "RUN-1".into(),
            task_id: "TASK-A".into(),
            sandbox: "host".into(),
            environment_json: r#"{"os":"mac"}"#.into(),
            commands_json: r#"["go test"]"#.into(),
            status: "passed".into(),
            started_at: "2026-11-14T00:00:01Z".into(),
            finished_at: Some("2026-11-14T00:00:02Z".into()),
        })
        .unwrap();
    store
        .run_record(&VerificationRun {
            id: "RUN-2".into(),
            task_id: "TASK-B".into(),
            sandbox: "docker".into(),
            environment_json: "{}".into(),
            commands_json: "[]".into(),
            status: "blocked".into(),
            started_at: "2026-11-14T00:00:03Z".into(),
            finished_at: None,
        })
        .unwrap();
    store
        .handoff_record(&AgentHandoff {
            id: "HO-1".into(),
            task_id: "TASK-A".into(),
            from_agent: "builder".into(),
            to_agent: "verifier".into(),
            run_id: "RUN-1".into(),
            manifest_path: ".devcouncil/handoffs/ho-1.json".into(),
            status: "ready".into(),
            created_at: "2026-11-14T00:00:04Z".into(),
        })
        .unwrap();
    drop(store);

    let all_runs = support::dcstore(&db, &["runs"]);
    assert_eq!(all_runs.code, 0, "runs failed: {}", all_runs.stdout);
    assert!(
        all_runs.stdout.contains("\"ok\":true"),
        "{}",
        all_runs.stdout
    );
    assert!(
        all_runs.stdout.contains("\"shown\":2"),
        "{}",
        all_runs.stdout
    );
    assert!(
        all_runs.stdout.contains("\"truncated\":false"),
        "{}",
        all_runs.stdout
    );
    let r2 = all_runs.stdout.find("\"RUN-2\"").expect("RUN-2 present");
    let r1 = all_runs.stdout.find("\"RUN-1\"").expect("RUN-1 present");
    assert!(r2 < r1, "runs must be newest first: {}", all_runs.stdout);

    let task_runs = support::dcstore(&db, &["runs", "--task", "TASK-A"]);
    assert_eq!(task_runs.code, 0, "{}", task_runs.stdout);
    assert!(
        task_runs.stdout.contains("\"shown\":1") && task_runs.stdout.contains("\"RUN-1\""),
        "{}",
        task_runs.stdout
    );
    assert!(
        !task_runs.stdout.contains("\"RUN-2\""),
        "{}",
        task_runs.stdout
    );

    let got_run = support::dcstore(&db, &["run-get", "--id", "RUN-1"]);
    assert_eq!(got_run.code, 0, "{}", got_run.stdout);
    assert!(
        got_run.stdout.contains("\"found\":true")
            && got_run.stdout.contains("\"sandbox\":\"host\"")
            && got_run
                .stdout
                .contains("\"finished_at\":\"2026-11-14T00:00:02Z\""),
        "{}",
        got_run.stdout
    );

    let missing_run = support::dcstore(&db, &["run-get", "--id", "NOPE"]);
    assert_eq!(missing_run.code, 0, "{}", missing_run.stdout);
    assert!(
        missing_run.stdout.contains("\"ok\":true")
            && missing_run.stdout.contains("\"found\":false")
            && missing_run.stdout.contains("\"run\":null"),
        "{}",
        missing_run.stdout
    );

    let got_ho = support::dcstore(&db, &["handoff-get", "--id", "HO-1"]);
    assert_eq!(got_ho.code, 0, "{}", got_ho.stdout);
    assert!(
        got_ho.stdout.contains("\"found\":true")
            && got_ho.stdout.contains("\"from_agent\":\"builder\"")
            && got_ho.stdout.contains("\"to_agent\":\"verifier\""),
        "{}",
        got_ho.stdout
    );

    let missing_ho = support::dcstore(&db, &["handoff-get", "--id", "NOPE"]);
    assert_eq!(missing_ho.code, 0, "{}", missing_ho.stdout);
    assert!(
        missing_ho.stdout.contains("\"found\":false")
            && missing_ho.stdout.contains("\"handoff\":null"),
        "{}",
        missing_ho.stdout
    );

    let blank_id = support::dcstore(&db, &["run-get", "--id", "  "]);
    assert_eq!(blank_id.code, 2, "{}", blank_id.stdout);
    assert!(
        blank_id.stdout.contains("\"ok\":false"),
        "{}",
        blank_id.stdout
    );

    let absent_db = support::temp_dir("read-cmds-absent").join("missing.sqlite");
    for cmd in ["runs", "run-get", "handoff-get"] {
        let mut args = vec![cmd];
        if cmd != "runs" {
            args.extend(["--id", "X"]);
        }
        let reply = support::dcstore(&absent_db, &args);
        assert_eq!(reply.code, 2, "{cmd}: {}", reply.stdout);
        assert!(
            !absent_db.exists(),
            "{cmd} must not create a missing database"
        );
    }

    let _ = std::fs::remove_dir_all(db.parent().unwrap());
    let _ = std::fs::remove_dir_all(absent_db.parent().unwrap());
}

#[test]
fn filtered_and_unfiltered_list_queries_preserve_ordering_and_cap() {
    let db = support::seeded("collapsed-lists");
    let store = Store::open_existing(&db).unwrap();

    store
        .evidence_append("check", Some("TASK-1"), None, None, r#"{"n":1}"#)
        .unwrap();
    store
        .evidence_append("check", Some("TASK-2"), None, None, r#"{"n":2}"#)
        .unwrap();
    store
        .evidence_append("check", None, None, None, r#"{"n":3}"#)
        .unwrap();

    let (all_ev, trunc) = store.evidence_list(None).unwrap();
    assert!(!trunc);
    assert_eq!(all_ev.len(), 3);
    let (t1_ev, trunc) = store.evidence_list(Some("TASK-1")).unwrap();
    assert!(!trunc);
    assert_eq!(t1_ev.len(), 1);
    assert_eq!(t1_ev[0].data_json, r#"{"n":1}"#);

    store
        .gaps_replace(
            "TASK-1",
            &[GapRow {
                id: "G-1".into(),
                severity: "high".into(),
                gap_type: "test".into(),
                requirement_id: None,
                task_id: Some("TASK-1".into()),
                description: "first".into(),
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
            }],
        )
        .unwrap();
    store
        .gaps_replace(
            "TASK-2",
            &[GapRow {
                id: "G-2".into(),
                severity: "low".into(),
                gap_type: "lint".into(),
                requirement_id: None,
                task_id: Some("TASK-2".into()),
                description: "second".into(),
                evidence_json: "[]".into(),
                recommended_fix: "fix".into(),
                blocking: false,
                file: None,
                line: None,
                suggested_command: None,
                acceptance_criterion_id: None,
                expected_verification_method: None,
                stdout_path: None,
                stderr_path: None,
            }],
        )
        .unwrap();

    assert_eq!(store.gaps_list(None).unwrap().0.len(), 2);
    assert_eq!(store.gaps_list(Some("TASK-2")).unwrap().0.len(), 1);
    assert_eq!(store.gap_history_list(None).unwrap().0.len(), 2);
    assert_eq!(store.gap_history_list(Some("TASK-1")).unwrap().0.len(), 1);

    // Seed MAX_LIST_ROWS + 1 runs in a single transaction to check cap + truncation.
    let conn = store.connection();
    let tx = conn.unchecked_transaction().unwrap();
    for i in 0..=MAX_LIST_ROWS {
        tx.execute(
            "INSERT INTO verification_runs
                (id, task_id, sandbox, environment_json, commands_json, status, started_at)
             VALUES (?1, ?2, 'host', '{}', '[]', 'passed', '2026-11-14T00:00:00Z')",
            rusqlite::params![
                format!("R-{i:05}"),
                if i == 0 { "TASK-UNIQUE" } else { "TASK-BULK" }
            ],
        )
        .unwrap();
    }
    tx.commit().unwrap();

    let (capped, truncated) = store.runs_list(None).unwrap();
    assert!(truncated);
    assert_eq!(capped.len(), MAX_LIST_ROWS);

    let (filtered, f_trunc) = store.runs_list(Some("TASK-UNIQUE")).unwrap();
    assert!(!f_trunc);
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].id, "R-00000");

    drop(store);
    let _ = std::fs::remove_dir_all(db.parent().unwrap());
}
