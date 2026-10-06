use dc_store::Store;
use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};

fn fixture() -> (Store, Arc<AtomicI64>) {
    let mut store = Store::open_in_memory().unwrap();
    let clock = Arc::new(AtomicI64::new(1000));
    let time = clock.clone();
    store.set_clock(move || time.load(Ordering::SeqCst));
    call(
        &store,
        "repositories.put",
        r#"{"id":"r","request_id":"r","expected_revision":0,"name":"Manvi","identity_key":"local:/checkout/.git"}"#,
    );
    call(
        &store,
        "items.put",
        r#"{"id":"t","request_id":"t","expected_revision":0,"title":"Keep E42","description":"Preserve exact evidence","repository_ids":["r"],"primary_repository_id":"r"}"#,
    );
    (store, clock)
}
fn call(store: &Store, method: &str, input: &str) -> String {
    request(store, method, input).unwrap_or_else(|e| panic!("{method}: {e}"))
}
fn request(store: &Store, method: &str, input: &str) -> Result<String, dc_store::workbench::Error> {
    // Forward slashes are accepted in absolute Windows paths and avoid adding
    // fixture-specific JSON escaping to every protocol case.
    let raw = if cfg!(windows) {
        input.replace("/checkout", "C:/checkout")
    } else {
        input.to_owned()
    };
    store.workbench_request(method, &raw)
}
fn text(store: &Store, raw: &str, path: &str) -> String {
    store
        .connection()
        .query_row("SELECT json_extract(?1,?2)", [raw, path], |r| r.get(0))
        .unwrap()
}
fn number(store: &Store, raw: &str, path: &str) -> i64 {
    store
        .connection()
        .query_row("SELECT json_extract(?1,?2)", [raw, path], |r| r.get(0))
        .unwrap()
}
const PREPARE: &str = r#"{"id":"run","request_id":"prepare","expected_revision":0,"task_id":"t","source_revision":1,"repository_id":"r","repository_revision":1,"provider":"codex","permission_mode":"ask","cwd":"/checkout","git_dir":"/checkout/.git","git_common_dir":"/checkout/.git","head_oid":null}"#;
const CLAIM: &str = r#"{"id":"run","request_id":"claim","expected_revision":1,"owner_id":"host-one","session_id":"terminal-one"}"#;
/// The profile-wide bound in `runs.rs`. Transcribed on purpose: the refusal
/// message is asserted to name it, so a change there fails here.
const CAPACITY: usize = 8;

/// A preparation for the same repository in a linked worktree `name`.
fn in_worktree(name: &str) -> String {
    PREPARE
        .replace("\"run\"", &format!("\"run-{name}\""))
        .replace("\"prepare\"", &format!("\"prepare-{name}\""))
        .replace(
            "\"cwd\":\"/checkout\"",
            &format!("\"cwd\":\"/worktrees/{name}\""),
        )
        .replace(
            "\"git_dir\":\"/checkout/.git\"",
            &format!("\"git_dir\":\"/checkout/.git/worktrees/{name}\""),
        )
}
fn reconcile(id: &str, revision: i64, reconciler: &str, prior: &str, reason: &str) -> String {
    format!(
        r#"{{"id":"{id}","request_id":"reconcile-{id}-{reconciler}-{revision}","expected_revision":{revision},"owner_id":"{reconciler}","prior_owner_id":"{prior}","reason":"{reason}"}}"#
    )
}

#[test]
fn worktrees_of_one_repository_run_concurrently_but_one_checkout_holds_one_run() {
    let (store, _) = fixture();
    call(&store, "runs.prepare", PREPARE);
    // Same repository, its own working tree: admitted. This is the case the
    // per-repository slot refused, and the reason only one task could run.
    call(&store, "runs.prepare", &in_worktree("a"));
    call(&store, "runs.prepare", &in_worktree("b"));
    let again = in_worktree("a")
        .replace("\"run-a\"", "\"run-a2\"")
        .replace("\"prepare-a\"", "\"prepare-a2\"");
    let refused = request(&store, "runs.prepare", &again).unwrap_err();
    assert_eq!(refused.code, "checkout_busy");
    assert!(refused.message.contains("another worktree"));
    let main_again = PREPARE
        .replace("\"run\"", "\"run-main2\"")
        .replace("\"prepare\"", "\"prepare-main2\"");
    assert_eq!(
        request(&store, "runs.prepare", &main_again)
            .unwrap_err()
            .code,
        "checkout_busy"
    );
    assert_eq!(
        number(&store, &call(&store, "runs.list", "{}"), "$.total"),
        3
    );
    // Releasing one checkout frees exactly that checkout.
    call(
        &store,
        "runs.cancel",
        r#"{"id":"run-a","request_id":"cancel-a","expected_revision":1}"#,
    );
    call(&store, "runs.prepare", &again);
    assert_eq!(
        request(&store, "runs.prepare", &main_again)
            .unwrap_err()
            .code,
        "checkout_busy"
    );
}

#[test]
fn a_reconciled_attempt_releases_its_checkout_as_an_uncertain_exit_with_a_notice() {
    let (store, _) = fixture();
    call(&store, "runs.prepare", PREPARE);
    call(&store, "runs.claim", CLAIM);
    call(
        &store,
        "runs.started",
        r#"{"id":"run","request_id":"started","expected_revision":2,"owner_id":"host-one","session_id":"terminal-one","process_id":42,"process_start":"native-birth"}"#,
    );
    let second = PREPARE
        .replace("\"run\"", "\"second\"")
        .replace("\"prepare\"", "\"second-prepare\"");
    assert_eq!(
        request(&store, "runs.prepare", &second).unwrap_err().code,
        "checkout_busy"
    );
    let saved = call(
        &store,
        "runs.reconcile",
        &reconcile(
            "run",
            3,
            "host-two",
            "host-one",
            "owner process 7 is gone; child 42 is gone",
        ),
    );
    assert_eq!(text(&store, &saved, "$.item.state"), "exited");
    assert_eq!(number(&store, &saved, "$.item.outcome_uncertain"), 1);
    let exit: Option<i64> = store
        .connection()
        .query_row(
            "SELECT json_extract(?1,'$.item.exit_code')",
            [&saved],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(exit, None, "a reconciler never observed an exit status");
    assert!(text(&store, &saved, "$.item.reason").starts_with("Reconciled: owner process 7"));
    // The original owner and session are kept as the record of who ran it.
    assert_eq!(text(&store, &saved, "$.item.owner_id"), "host-one");
    call(&store, "runs.prepare", &second);
    let inbox = call(&store, "attention.list", r#"{"filter":"all"}"#);
    assert_eq!(text(&store, &inbox, "$.items[0].kind"), "run_unresolved");
    assert_eq!(text(&store, &inbox, "$.items[0].target_id"), "run");
    // An ended attempt cannot be reconciled again, nor finished by its owner.
    assert_eq!(
        request(
            &store,
            "runs.reconcile",
            &reconcile("run", 4, "host-three", "host-one", "again")
        )
        .unwrap_err()
        .code,
        "invalid_state"
    );
}

#[test]
fn reconciliation_refuses_the_owner_a_misnamed_owner_an_unstarted_attempt_and_no_evidence() {
    let (store, _) = fixture();
    call(&store, "runs.prepare", PREPARE);
    // Prepared: nothing was ever claimed, and `cancel` is how that ends.
    assert_eq!(
        request(
            &store,
            "runs.reconcile",
            &reconcile("run", 1, "host-two", "host-one", "gone")
        )
        .unwrap_err()
        .code,
        "invalid_state"
    );
    call(&store, "runs.claim", CLAIM);
    for (input, code) in [
        // The live owner reports its own outcome; reconcile is for the others.
        (
            reconcile("run", 2, "host-one", "host-one", "gone"),
            "invalid_state",
        ),
        // A reconciler must name the owner it actually judged.
        (
            reconcile("run", 2, "host-two", "host-zero", "gone"),
            "owner_mismatch",
        ),
        (
            reconcile("run", 2, "host-two", "host-one", "   "),
            "invalid_input",
        ),
        (
            reconcile("run", 1, "host-two", "host-one", "gone"),
            "revision_conflict",
        ),
    ] {
        assert_eq!(
            request(&store, "runs.reconcile", &input).unwrap_err().code,
            code,
            "{input}"
        );
    }
    let extra =
        reconcile("run", 2, "host-two", "host-one", "gone").replacen('{', r#"{"exit_code":0,"#, 1);
    assert_eq!(
        request(&store, "runs.reconcile", &extra).unwrap_err().code,
        "invalid_input",
        "a reconciliation cannot claim an exit status"
    );
    assert_eq!(
        text(
            &store,
            &call(&store, "runs.get", r#"{"id":"run"}"#),
            "$.item.state"
        ),
        "starting"
    );
}

#[test]
fn concurrent_preparations_admit_one_per_checkout_and_never_exceed_capacity() {
    let (store, _) = fixture();
    let dir = std::env::temp_dir().join(format!(
        "dc-run-slots-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("profile.sqlite");
    store
        .connection()
        .execute("VACUUM INTO ?1", [path.to_str().unwrap()])
        .unwrap();
    let race = |inputs: Vec<String>| -> Vec<Result<String, dc_store::workbench::Error>> {
        let barrier = Arc::new(std::sync::Barrier::new(inputs.len()));
        std::thread::scope(|scope| {
            let handles: Vec<_> = inputs
                .into_iter()
                .map(|input| {
                    let barrier = barrier.clone();
                    let path = path.clone();
                    scope.spawn(move || {
                        let mut store = Store::open(&path).unwrap();
                        store.set_clock(|| 1000);
                        barrier.wait();
                        request(&store, "runs.prepare", &input)
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        })
    };
    // Twelve writers for one checkout: exactly one wins, every loser is told
    // the checkout is taken — never a store error, never two runs.
    let same: Vec<String> = (0..12)
        .map(|i| {
            PREPARE
                .replace("\"run\"", &format!("\"same-{i}\""))
                .replace("\"prepare\"", &format!("\"same-prepare-{i}\""))
        })
        .collect();
    let outcomes = race(same);
    assert_eq!(outcomes.iter().filter(|r| r.is_ok()).count(), 1);
    for refusal in outcomes.iter().filter_map(|r| r.as_ref().err()) {
        assert_eq!(refusal.code, "checkout_busy", "{}", refusal.message);
    }
    // Then sixteen distinct worktrees: the remaining capacity is admitted
    // and the rest are refused for capacity, not for a busy checkout.
    let outcomes = race((0..16).map(|i| in_worktree(&format!("w{i}"))).collect());
    assert_eq!(outcomes.iter().filter(|r| r.is_ok()).count(), CAPACITY - 1);
    for refusal in outcomes.iter().filter_map(|r| r.as_ref().err()) {
        assert_eq!(refusal.code, "capacity_reached", "{}", refusal.message);
    }
    let reopened = Store::open(&path).unwrap();
    assert_eq!(
        number(&reopened, &call(&reopened, "runs.list", "{}"), "$.total"),
        CAPACITY as i64
    );
    drop(reopened);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn failed_managed_initialization_keeps_process_evidence_and_never_forges_a_thread() {
    let (store, _) = fixture();
    call(
        &store,
        "runs.prepare",
        &PREPARE.replace("\"provider\":", "\"kind\":\"managed\",\"provider\":"),
    );
    call(
        &store,
        "runs.claim",
        &CLAIM.replace("\"owner_id\":", "\"kind\":\"managed\",\"owner_id\":"),
    );
    call(
        &store,
        "runs.started",
        r#"{"id":"run","request_id":"started","expected_revision":2,"owner_id":"host-one","session_id":"terminal-one","process_id":42,"process_start":"native-birth"}"#,
    );
    let finish = r#"{"id":"run","request_id":"finish","expected_revision":3,"owner_id":"host-one","session_id":"terminal-one","outcome":"exited","exit_code":0,"reason":"Handshake rejected; process reaped","provider_state":"failed"}"#;
    for status in ["ready", "running", "completed"] {
        assert!(
            request(
                &store,
                "runs.finish",
                &finish.replace("\"failed\"", &format!("\"{status}\""))
            )
            .is_err()
        );
    }
    let result = call(&store, "runs.finish", finish);
    assert_eq!(text(&store, &result, "$.item.provider_state"), "failed");
    assert_eq!(
        text(&store, &result, "$.item.process_start"),
        "native-birth"
    );
    assert!(!result.contains("provider_thread_id"));
    let notices = call(&store, "attention.list", r#"{"task_id":"t"}"#);
    assert_eq!(text(&store, &notices, "$.items[0].kind"), "run_exit_failed");
    let task = call(&store, "items.get", r#"{"id":"t"}"#);
    assert_eq!(number(&store, &task, "$.item.revision"), 1);
    call(
        &store,
        "runs.prepare",
        &PREPARE
            .replace("\"run\"", "\"retry\"")
            .replace("\"prepare\"", "\"prepare-retry\""),
    );
}

#[test]
fn terminal_finish_cannot_introduce_managed_provider_evidence() {
    let (store, _) = fixture();
    call(&store, "runs.prepare", PREPARE);
    call(&store, "runs.claim", CLAIM);
    assert!(request(&store, "runs.finish", r#"{"id":"run","request_id":"finish","expected_revision":2,"owner_id":"host-one","session_id":"terminal-one","outcome":"failed","reason":"Not started","provider_state":"failed"}"#).is_err());
}

#[test]
fn managed_kind_cannot_be_claimed_as_a_terminal_and_protocol_receipts_are_owned() {
    let (store, _) = fixture();
    let prepared = call(
        &store,
        "runs.prepare",
        &PREPARE.replace("\"provider\":", "\"kind\":\"managed\",\"provider\":"),
    );
    assert_eq!(text(&store, &prepared, "$.item.kind"), "managed");
    assert_eq!(
        request(&store, "runs.claim", CLAIM).unwrap_err().code,
        "run_kind_mismatch"
    );
    call(
        &store,
        "runs.claim",
        &CLAIM.replace("\"owner_id\":", "\"kind\":\"managed\",\"owner_id\":"),
    );
    let protocol = r#"{"id":"run","request_id":"protocol","expected_revision":2,"owner_id":"host-one","session_id":"terminal-one","provider_thread_id":"thread","provider_turn_id":null,"provider_state":"ready","effective_configuration":"{\"cwd\":\"/checkout\",\"sandbox\":{\"type\":\"readOnly\",\"networkAccess\":false}}","output":"","output_truncated":false}"#;
    assert!(
        request(
            &store,
            "runs.protocol",
            &protocol.replace("host-one", "other")
        )
        .is_err()
    );
    assert_eq!(
        request(&store, "runs.protocol", protocol).unwrap_err().code,
        "invalid_state"
    );
    call(
        &store,
        "runs.started",
        r#"{"id":"run","request_id":"started","expected_revision":2,"owner_id":"host-one","session_id":"terminal-one","process_id":42,"process_start":"native-birth"}"#,
    );
    let protocol = protocol.replace("\"expected_revision\":2", "\"expected_revision\":3");
    call(&store, "runs.protocol", &protocol);
    let page = call(&store, "runs.list", "{}");
    assert!(
        !page.contains("effective_configuration"),
        "history transferred full provider settings"
    );
    assert!(
        !page.contains("\"output\":"),
        "history transferred full provider output"
    );
    assert!(call(&store, "runs.get", r#"{"id":"run"}"#).contains("effective_configuration"));
    assert_eq!(
        number(
            &store,
            &call(&store, "items.get", r#"{"id":"t"}"#),
            "$.item.revision"
        ),
        1
    );
    let changed = protocol
        .replace("\"protocol\"", "\"different\"")
        .replace("\"expected_revision\":3", "\"expected_revision\":4")
        .replace("\"thread\"", "\"other-thread\"");
    assert!(request(&store, "runs.protocol", &changed).is_err());
}

#[test]
fn newest_run_pages_keep_equal_timestamp_ties_and_exact_totals() {
    let (store, _) = fixture();
    for name in ["a", "b", "c"] {
        call(
            &store,
            "runs.prepare",
            &PREPARE
                .replace("\"run\"", &format!("\"{name}\""))
                .replace("\"prepare\"", &format!("\"prepare-{name}\"")),
        );
        call(
            &store,
            "runs.cancel",
            &format!(r#"{{"id":"{name}","request_id":"cancel-{name}","expected_revision":1}}"#),
        );
    }
    let first = call(
        &store,
        "runs.list",
        r#"{"task_id":"t","newest":true,"limit":1}"#,
    );
    assert_eq!(text(&store, &first, "$.items[0].id"), "c");
    assert_eq!(number(&store, &first, "$.total"), 3);
    let cursor = text(&store, &first, "$.next_cursor");
    let next = call(
        &store,
        "runs.list",
        &format!(r#"{{"task_id":"t","newest":true,"limit":2,"cursor":"{cursor}"}}"#),
    );
    assert_eq!(text(&store, &next, "$.items[0].id"), "b");
    assert_eq!(text(&store, &next, "$.items[1].id"), "a");
    assert_eq!(number(&store, &next, "$.total"), 3);
    assert_eq!(number(&store, &next, "$.has_more"), 0);
    assert_eq!(
        text(
            &store,
            &call(&store, "runs.list", r#"{"limit":1}"#),
            "$.items[0].id"
        ),
        "a"
    );
}

#[test]
fn run_snapshots_preserve_branch_identity_and_refuse_malformed_refs() {
    let (store, _) = fixture();
    let input = PREPARE.replace(
        "\"head_oid\":null",
        "\"head_oid\":null,\"head_ref\":\"refs/heads/main\"",
    );
    let prepared = call(&store, "runs.prepare", &input);
    assert_eq!(
        text(&store, &prepared, "$.item.head_ref"),
        "refs/heads/main"
    );
    assert_eq!(call(&store, "runs.prepare", &input), prepared);
    for branch in [
        "main",
        "refs/tags/main",
        "refs/heads/",
        "refs/heads/a..b",
        "refs/heads/with space",
    ] {
        let (store, _) = fixture();
        assert_eq!(
            request(
                &store,
                "runs.prepare",
                &input.replace("refs/heads/main", branch)
            )
            .unwrap_err()
            .code,
            "invalid_input"
        );
        assert_eq!(
            number(&store, &call(&store, "runs.list", "{}"), "$.total"),
            0
        );
    }
}

#[test]
fn a_launch_snapshot_is_durable_and_claim_receipts_never_authorize_a_second_spawn() {
    let (store, _) = fixture();
    let prepared = call(&store, "runs.prepare", PREPARE);
    assert_eq!(text(&store, &prepared, "$.item.state"), "prepared");
    assert_eq!(call(&store, "runs.prepare", PREPARE), prepared);
    let captured = call(&store, "runs.get", r#"{"id":"run"}"#);
    assert_eq!(
        text(&store, &captured, "$.item.brief.task.description"),
        "Preserve exact evidence"
    );
    // The attempt records the guidance it was given, so what an agent was told
    // does not depend on which host or harness binary launched it.
    assert!(
        text(&store, &captured, "$.item.brief.markdown").starts_with(&format!(
            "# Task brief v1\n\n{}\n{}\n\n## Title\n",
            dc_store::workbench::AGENT_GUIDANCE_HEADING,
            dc_store::workbench::AGENT_GUIDANCE.trim()
        ))
    );
    let claimed = call(&store, "runs.claim", CLAIM);
    assert_eq!(text(&store, &claimed, "$.item.state"), "starting");
    assert_eq!(
        request(&store, "runs.claim", CLAIM).unwrap_err().code,
        "claim_consumed"
    );
    assert_eq!(
        text(
            &store,
            &call(&store, "runs.get", r#"{"id":"run"}"#),
            "$.item.session_id"
        ),
        "terminal-one"
    );
    assert_eq!(
        number(
            &store,
            &call(&store, "items.get", r#"{"id":"t"}"#),
            "$.item.revision"
        ),
        1
    );
}

#[test]
fn launch_claims_refuse_changed_task_or_repository_and_expired_preparation() {
    for change in [0, 1, 2] {
        let (store, time) = fixture();
        call(&store, "runs.prepare", PREPARE);
        match change {
            0 => {
                call(
                    &store,
                    "items.put",
                    r#"{"id":"t","request_id":"edit","expected_revision":1,"title":"Changed task","repository_ids":["r"],"primary_repository_id":"r"}"#,
                );
            }
            1 => {
                call(
                    &store,
                    "repositories.put",
                    r#"{"id":"r","request_id":"rename","expected_revision":1,"name":"Renamed repo","identity_key":"local:/checkout/.git"}"#,
                );
            }
            _ => time.store(1300, Ordering::SeqCst),
        }
        let error = request(&store, "runs.claim", CLAIM).unwrap_err();
        assert_eq!(
            error.code,
            if change == 2 {
                "expired"
            } else {
                "revision_conflict"
            }
        );
        let stored = call(&store, "runs.get", r#"{"id":"run"}"#);
        assert_eq!(text(&store, &stored, "$.item.state"), "prepared");
        assert_eq!(text(&store, &stored, "$.item.brief.task.title"), "Keep E42");
    }
}

#[test]
fn only_the_claim_owner_can_report_process_lifecycle_and_exit_never_accepts_a_task() {
    let (store, _) = fixture();
    call(&store, "runs.prepare", PREPARE);
    call(&store, "runs.claim", CLAIM);
    let start = r#"{"id":"run","request_id":"started","expected_revision":2,"owner_id":"host-one","session_id":"terminal-one","process_id":42,"process_start":"boot-one:birth-two"}"#;
    assert_eq!(
        request(
            &store,
            "runs.started",
            &start.replace("host-one", "host-two")
        )
        .unwrap_err()
        .code,
        "owner_mismatch"
    );
    let started = call(&store, "runs.started", start);
    let events = call(&store, "events.list", "{}");
    assert_eq!(call(&store, "runs.started", start), started);
    assert_eq!(call(&store, "events.list", "{}"), events);
    call(
        &store,
        "runs.finish",
        r#"{"id":"run","request_id":"exited","expected_revision":3,"owner_id":"host-one","session_id":"terminal-one","outcome":"exited","exit_code":0,"reason":"Process exited normally"}"#,
    );
    let result = call(&store, "runs.get", r#"{"id":"run"}"#);
    assert_eq!(text(&store, &result, "$.item.state"), "exited");
    assert_eq!(number(&store, &result, "$.item.exit_code"), 0);
    let task = call(&store, "items.get", r#"{"id":"t"}"#);
    assert_eq!(text(&store, &task, "$.item.status"), "inbox");
    assert_eq!(number(&store, &task, "$.item.revision"), 1);
}

#[test]
fn deleting_a_task_keeps_its_run_history_and_snapshots_available() {
    let (store, _) = fixture();
    call(&store, "runs.prepare", PREPARE);
    call(
        &store,
        "items.delete",
        r#"{"id":"t","request_id":"delete","expected_revision":1}"#,
    );
    let history = call(&store, "runs.list", r#"{"task_id":"t","limit":1}"#);
    assert_eq!(number(&store, &history, "$.total"), 1);
    assert_eq!(text(&store, &history, "$.items[0].task_title"), "Keep E42");
    assert!(
        !history.contains("Preserve exact evidence"),
        "run listing fetched the full source brief"
    );
    let saved = call(&store, "runs.get", r#"{"id":"run"}"#);
    assert_eq!(
        text(&store, &saved, "$.item.brief.task.description"),
        "Preserve exact evidence"
    );
    assert_eq!(
        request(&store, "runs.claim", CLAIM).unwrap_err().code,
        "not_found"
    );
}

#[test]
fn claim_event_failure_rolls_back_the_execution_claim_and_its_receipt() {
    let (store, _) = fixture();
    call(&store, "runs.prepare", PREPARE);
    store.connection().execute_batch("CREATE TRIGGER fail_claim BEFORE INSERT ON work_events WHEN new.kind='runs.claim' BEGIN SELECT RAISE(ABORT,'fixture claim event failure'); END").unwrap();
    assert_eq!(
        request(&store, "runs.claim", CLAIM).unwrap_err().code,
        "store_error"
    );
    assert_eq!(
        text(
            &store,
            &call(&store, "runs.get", r#"{"id":"run"}"#),
            "$.item.state"
        ),
        "prepared"
    );
    store
        .connection()
        .execute_batch("DROP TRIGGER fail_claim")
        .unwrap();
    call(&store, "runs.claim", CLAIM);
    assert_eq!(
        request(&store, "runs.claim", CLAIM).unwrap_err().code,
        "claim_consumed"
    );
}

#[test]
fn uncertain_processes_keep_the_repository_reserved_and_cannot_turn_into_success() {
    let (store, clock) = fixture();
    call(&store, "runs.prepare", PREPARE);
    call(&store, "runs.claim", CLAIM);
    call(
        &store,
        "runs.finish",
        r#"{"id":"run","request_id":"uncertain","expected_revision":2,"owner_id":"host-one","session_id":"terminal-one","outcome":"unresolved","reason":"Host connection lost before spawn confirmation"}"#,
    );
    clock.store(100_000, Ordering::SeqCst);
    let other = PREPARE
        .replace("\"run\"", "\"second\"")
        .replace("\"prepare\"", "\"second-prepare\"");
    assert_eq!(
        request(&store, "runs.prepare", &other).unwrap_err().code,
        "checkout_busy"
    );
    let false_success = r#"{"id":"run","request_id":"false-success","expected_revision":3,"owner_id":"host-one","session_id":"terminal-one","outcome":"exited","exit_code":0,"reason":"Time passed"}"#;
    assert_eq!(
        request(&store, "runs.finish", false_success)
            .unwrap_err()
            .code,
        "invalid_state"
    );
    assert_eq!(
        number(
            &store,
            &call(&store, "runs.get", r#"{"id":"run"}"#),
            "$.item.outcome_uncertain"
        ),
        1
    );
}

#[test]
fn schema_four_upgrade_rolls_back_on_conflict_and_preserves_existing_tasks() {
    let (store, _) = fixture();
    let task = call(&store, "items.get", r#"{"id":"t"}"#);
    // Retain an empty conflicting input table. The migration must roll back
    // the run table and indexes created before discovering that conflict.
    store
        .connection()
        .execute_batch("DROP TABLE work_decisions; DROP TABLE work_notification_deliveries; DROP TABLE work_notification_settings; DROP TABLE work_attention; DROP TABLE work_runs; UPDATE work_meta SET version=4 WHERE id=1")
        .unwrap();
    assert_eq!(
        request(&store, "runs.list", "{}").unwrap_err().code,
        "store_error"
    );
    let state: (i64,i64) = store.connection().query_row("SELECT version,(SELECT count(*) FROM sqlite_master WHERE name='work_runs') FROM work_meta WHERE id=1",[],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
    assert_eq!(state, (4, 0));
    store
        .connection()
        .execute_batch("DROP TABLE work_run_inputs")
        .unwrap();
    assert_eq!(call(&store, "items.get", r#"{"id":"t"}"#), task);
    assert_eq!(
        number(&store, &call(&store, "runs.list", "{}"), "$.total"),
        0
    );
    assert_eq!(
        store
            .connection()
            .query_row("SELECT version FROM work_meta WHERE id=1", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        10
    );
}

#[test]
fn independent_connections_share_one_consumable_claim_and_reopening_cannot_reissue_it() {
    let (store, _) = fixture();
    call(&store, "runs.prepare", PREPARE);
    let dir = std::env::temp_dir().join(format!(
        "manvi-run-claim-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("profile.sqlite");
    store
        .connection()
        .execute("VACUUM INTO ?1", [path.to_str().unwrap()])
        .unwrap();
    let a = Store::open(&path).unwrap();
    let b = Store::open(&path).unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let outcomes = std::thread::scope(|scope| {
        let first = barrier.clone();
        let a = scope.spawn(move || {
            let mut a = a;
            a.set_clock(|| 1000);
            first.wait();
            request(&a, "runs.claim", CLAIM)
        });
        let b = scope.spawn(move || {
            let mut b = b;
            b.set_clock(|| 1000);
            barrier.wait();
            request(&b, "runs.claim", CLAIM)
        });
        [a.join().unwrap(), b.join().unwrap()]
    });
    assert_eq!(outcomes.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        outcomes
            .iter()
            .filter_map(|r| r.as_ref().err())
            .next()
            .unwrap()
            .code,
        "claim_consumed"
    );
    {
        let reopened = Store::open(&path).unwrap();
        assert_eq!(
            request(&reopened, "runs.claim", CLAIM).unwrap_err().code,
            "claim_consumed"
        );
        assert_eq!(
            text(
                &reopened,
                &call(&reopened, "runs.get", r#"{"id":"run"}"#),
                "$.item.state"
            ),
            "starting"
        );
        assert_eq!(
            reopened
                .connection()
                .query_row(
                    "SELECT count(*) FROM work_events WHERE kind='runs.claim'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn preparation_limits_and_pagination_preserve_counts_without_copying_source_bodies() {
    let (store, _) = fixture();
    for i in 0..=CAPACITY {
        call(
            &store,
            "repositories.put",
            &format!(
                r#"{{"id":"r{i}","request_id":"repo-{i}","expected_revision":0,"name":"Repository {i}","identity_key":"local:/checkout/{i}/.git"}}"#
            ),
        );
        call(
            &store,
            "items.put",
            &format!(
                r#"{{"id":"t{i}","request_id":"task-{i}","expected_revision":0,"title":"Task {i}","description":"Large source text must stay outside lifecycle events","repository_ids":["r{i}"],"primary_repository_id":"r{i}"}}"#
            ),
        );
    }
    let prepare = |i: usize| {
        PREPARE
            .replace("\"run\"", &format!("\"run-{i}\""))
            .replace("\"prepare\"", &format!("\"prepare-{i}\""))
            .replace("\"t\"", &format!("\"t{i}\""))
            .replace("\"r\"", &format!("\"r{i}\""))
            .replace("/checkout", &format!("/checkout/{i}"))
    };
    for i in 0..CAPACITY {
        call(&store, "runs.prepare", &prepare(i));
    }
    let full = request(&store, "runs.prepare", &prepare(CAPACITY)).unwrap_err();
    assert_eq!(full.code, "capacity_reached");
    assert!(
        full.message
            .starts_with(&format!("{CAPACITY} runs are already")),
        "the refusal must state the bound it enforces: {}",
        full.message
    );
    let first = call(&store, "runs.list", r#"{"limit":1}"#);
    assert_eq!(number(&store, &first, "$.total"), CAPACITY as i64);
    assert_eq!(number(&store, &first, "$.shown"), 1);
    assert!(!first.contains("Large source text"));
    let cursor = text(&store, &first, "$.next_cursor");
    let second = call(
        &store,
        "runs.list",
        &format!(r#"{{"limit":1,"cursor":"{cursor}"}}"#),
    );
    assert_eq!(number(&store, &second, "$.total"), CAPACITY as i64);
    assert_eq!(text(&store, &second, "$.items[0].id"), "run-1");
    assert_eq!(number(&store, &second, "$.has_more"), 1);
    call(
        &store,
        "runs.cancel",
        r#"{"id":"run-0","request_id":"cancel-zero","expected_revision":1}"#,
    );
    call(&store, "runs.prepare", &prepare(CAPACITY));
    assert_eq!(
        number(
            &store,
            &call(&store, "runs.list", r#"{"state":"cancelled"}"#),
            "$.total"
        ),
        1
    );
    assert_eq!(store.connection().query_row("SELECT count(*) FROM work_events WHERE kind LIKE 'runs.%' AND instr(payload,'Large source text')>0",[],|r|r.get::<_,i64>(0)).unwrap(),0);
    assert_eq!(
        store
            .connection()
            .query_row("SELECT count(*) FROM work_run_inputs", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        CAPACITY as i64 + 1
    );
    assert!(
        store
            .connection()
            .execute(
                "UPDATE work_run_inputs SET brief='{}' WHERE run_id='run-0'",
                []
            )
            .is_err()
    );
}

#[test]
fn grok_and_antigravity_are_supported_terminal_providers() {
    let (store, _) = fixture();
    for provider in ["grok", "agy"] {
        let input = PREPARE
            .replace(r#""id":"run""#, &format!(r#""id":"run-{provider}""#))
            .replace(
                r#""request_id":"prepare""#,
                &format!(r#""request_id":"prepare-{provider}""#),
            )
            .replace(
                r#""provider":"codex""#,
                &format!(r#""provider":"{provider}""#),
            );
        let raw = call(&store, "runs.prepare", &input);
        assert_eq!(text(&store, &raw, "$.item.provider"), provider);
        assert_eq!(text(&store, &raw, "$.item.kind"), "external_terminal");
        call(
            &store,
            "runs.cancel",
            &format!(
                r#"{{"id":"run-{provider}","request_id":"cancel-{provider}","expected_revision":1}}"#
            ),
        );
    }
}

#[test]
fn malformed_or_unrelated_launch_inputs_never_reserve_a_slot() {
    let (store, _) = fixture();
    for input in [
        PREPARE.replace("\"ask\"", "\"unknown\""),
        PREPARE.replace("\"codex\"", "\"other\""),
        PREPARE.replace("\"repository_revision\":1", "\"repository_revision\":2"),
        PREPARE.replace("\"cwd\":\"/checkout\"", "\"cwd\":\"relative\""),
        PREPARE.replace("\"head_oid\":null", "\"head_oid\":\"short\""),
        PREPARE.replace(
            "\"git_common_dir\":\"/checkout/.git\"",
            "\"git_common_dir\":\"/other/.git\"",
        ),
        PREPARE.trim_end_matches('}').to_owned() + ",\"acknowledge_bypass\":true}",
        PREPARE.trim_end_matches('}').to_owned() + ",\"skip_permissions\":true}",
    ] {
        assert!(
            request(&store, "runs.prepare", &input).is_err(),
            "accepted {input}"
        );
        assert_eq!(
            number(&store, &call(&store, "runs.list", "{}"), "$.total"),
            0
        );
    }
}

#[test]
fn bypass_is_explicit_for_each_new_attempt_and_claimed_runs_cannot_be_cancelled_as_unstarted() {
    let (store, _) = fixture();
    let bypass = PREPARE.replace("\"ask\"", "\"bypass\"");
    assert_eq!(
        request(&store, "runs.prepare", &bypass).unwrap_err().code,
        "invalid_input"
    );
    let acknowledged = bypass.trim_end_matches('}').to_owned() + ",\"acknowledge_bypass\":true}";
    call(&store, "runs.prepare", &acknowledged);
    call(&store, "runs.claim", CLAIM);
    assert_eq!(
        request(
            &store,
            "runs.cancel",
            r#"{"id":"run","request_id":"cancel","expected_revision":2}"#
        )
        .unwrap_err()
        .code,
        "invalid_state"
    );
    assert_eq!(
        request(
            &store,
            "runs.prepare",
            &bypass
                .replace("\"run\"", "\"retry\"")
                .replace("\"prepare\"", "\"retry-prepare\"")
        )
        .unwrap_err()
        .code,
        "invalid_input"
    );
}

/// The host names how many attempts may be live at once; the store enforces
/// exactly that number, refuses a malformed one as input, and never lets a
/// host raise it past the shared ceiling.
#[test]
fn a_host_named_run_limit_is_enforced_exactly_and_bounded() {
    use dc_store::workbench::{DEFAULT_ACTIVE_RUNS, MAX_ACTIVE_RUNS_CEILING};
    assert_eq!(DEFAULT_ACTIVE_RUNS, CAPACITY as i64);
    let with_limit = |name: &str, limit: &str| {
        in_worktree(name).replacen('{', &format!(r#"{{"max_active_runs":{limit},"#), 1)
    };
    let (store, _) = fixture();
    for bad in [
        "0",
        "-1",
        "65",
        "1.5",
        "\"8\"",
        "true",
        "[8]",
        "9007199254740991",
    ] {
        let refused = request(&store, "runs.prepare", &with_limit("bad", bad)).unwrap_err();
        assert_eq!(
            refused.code, "invalid_input",
            "max_active_runs={bad}: {}",
            refused.message
        );
    }
    assert_eq!(
        number(&store, &call(&store, "runs.list", "{}"), "$.total"),
        0,
        "a refused limit must not leave a run behind"
    );
    // A limit of one admits one, and the refusal names the limit in force.
    call(&store, "runs.prepare", &with_limit("one", "1"));
    let full = request(&store, "runs.prepare", &with_limit("two", "1")).unwrap_err();
    assert_eq!(full.code, "capacity_reached");
    assert!(
        full.message.starts_with("1 runs are already"),
        "{}",
        full.message
    );
    // Raising it admits more at once; lowering it below what is live refuses
    // new work without touching the attempts already admitted.
    for i in 0..11 {
        call(
            &store,
            "runs.prepare",
            &with_limit(&format!("raised-{i}"), "12"),
        );
    }
    let full = request(&store, "runs.prepare", &with_limit("raised-12", "12")).unwrap_err();
    assert_eq!(full.code, "capacity_reached");
    let lowered = request(&store, "runs.prepare", &with_limit("lowered", "2")).unwrap_err();
    assert_eq!(lowered.code, "capacity_reached");
    assert!(
        lowered.message.starts_with("2 runs are already"),
        "{}",
        lowered.message
    );
    assert_eq!(
        number(&store, &call(&store, "runs.list", "{}"), "$.total"),
        12
    );
    // Omitted means the default, so a client that never names a limit keeps
    // the bound it always had.
    let (store, _) = fixture();
    for i in 0..CAPACITY {
        call(
            &store,
            "runs.prepare",
            &in_worktree(&format!("default-{i}")),
        );
    }
    assert_eq!(
        request(&store, "runs.prepare", &in_worktree("default-over"))
            .unwrap_err()
            .code,
        "capacity_reached"
    );
    // And the ceiling itself is reachable, and is the last admission.
    let (store, _) = fixture();
    let ceiling = MAX_ACTIVE_RUNS_CEILING.to_string();
    for i in 0..MAX_ACTIVE_RUNS_CEILING {
        call(
            &store,
            "runs.prepare",
            &with_limit(&format!("c{i}"), &ceiling),
        );
    }
    assert_eq!(
        request(&store, "runs.prepare", &with_limit("c-over", &ceiling))
            .unwrap_err()
            .code,
        "capacity_reached"
    );
}

/// Eighty writers race a limit of sixty-four, each in its own worktree: the
/// admission count is exactly the limit — the check and the insert are one
/// transaction, so concurrency can never overshoot a raised limit.
#[test]
fn racing_preparations_never_exceed_a_raised_limit() {
    use dc_store::workbench::MAX_ACTIVE_RUNS_CEILING;
    let (store, _) = fixture();
    let dir = std::env::temp_dir().join(format!(
        "dc-run-limit-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("profile.sqlite");
    store
        .connection()
        .execute("VACUUM INTO ?1", [path.to_str().unwrap()])
        .unwrap();
    let inputs: Vec<String> = (0..80)
        .map(|i| {
            in_worktree(&format!("race-{i}")).replacen(
                '{',
                &format!(r#"{{"max_active_runs":{MAX_ACTIVE_RUNS_CEILING},"#),
                1,
            )
        })
        .collect();
    let barrier = Arc::new(std::sync::Barrier::new(inputs.len()));
    let outcomes: Vec<_> = std::thread::scope(|scope| {
        let handles: Vec<_> = inputs
            .into_iter()
            .map(|input| {
                let barrier = barrier.clone();
                let path = path.clone();
                scope.spawn(move || {
                    let mut store = Store::open(&path).unwrap();
                    store.set_clock(|| 1000);
                    barrier.wait();
                    request(&store, "runs.prepare", &input)
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert_eq!(
        outcomes.iter().filter(|r| r.is_ok()).count() as i64,
        MAX_ACTIVE_RUNS_CEILING
    );
    for refusal in outcomes.iter().filter_map(|r| r.as_ref().err()) {
        assert_eq!(refusal.code, "capacity_reached", "{}", refusal.message);
    }
    let reopened = Store::open(&path).unwrap();
    assert_eq!(
        number(&reopened, &call(&reopened, "runs.list", "{}"), "$.total"),
        MAX_ACTIVE_RUNS_CEILING
    );
    drop(reopened);
    std::fs::remove_dir_all(&dir).unwrap();
}
