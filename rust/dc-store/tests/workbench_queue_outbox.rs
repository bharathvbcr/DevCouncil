//! Schema 17: cross-repository execution queue, two-way GitHub issue sync,
//! opt-in confirmed durable outbox, and task dependency/readiness gating.

use dc_store::Store;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicI64, AtomicU64, Ordering},
};

static NEXT_DIR: AtomicU64 = AtomicU64::new(1);

mod support;

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let path = std::env::temp_dir().join(format!(
            "dc-store-queue-outbox-{}-{}-{nanos}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn fixture() -> (Store, Arc<AtomicI64>) {
    let mut s = Store::open_in_memory().unwrap();
    let clock = Arc::new(AtomicI64::new(10_000));
    let time = clock.clone();
    s.set_clock(move || time.load(Ordering::SeqCst));
    for (id, path) in [
        ("repo-a", "/tmp/git-a/.git"),
        ("repo-b", "/tmp/git-b/.git"),
        ("repo-c", "/tmp/git-c/.git"),
        ("repo-z", "/tmp/git-z/.git"),
    ] {
        call(
            &s,
            "repositories.put",
            &format!(
                r#"{{"id":"{id}","request_id":"req-{id}","expected_revision":0,"name":"{id}","identity_key":"local:{path}"}}"#
            ),
        );
    }
    (s, clock)
}

fn call(s: &Store, method: &str, input: &str) -> String {
    s.workbench_request(method, &support::real_checkouts(input))
        .unwrap_or_else(|e| panic!("{method} {input}: {e}"))
}

fn refuse(s: &Store, method: &str, input: &str, code: &str) -> String {
    match s.workbench_request(method, &support::real_checkouts(input)) {
        Ok(ok) => panic!("expected {method} {input} to fail ({code}), got {ok}"),
        Err(e) => {
            assert_eq!(e.code, code, "{method} {input}: {}", e.message);
            e.message
        }
    }
}

fn json<T: rusqlite::types::FromSql>(s: &Store, raw: &str, path: &str) -> T {
    s.connection()
        .query_row("SELECT json_extract(?1,?2)", [raw, path], |r| r.get(0))
        .unwrap()
}

fn put_task(s: &Store, id: &str, revision: i64, repo: &str, extra: &str) -> String {
    call(
        s,
        "items.put",
        &format!(
            r#"{{"id":"{id}","request_id":"{id}-rev-{revision}-{}","expected_revision":{revision},"title":"Task {id}","repository_ids":["{repo}"],"primary_repository_id":"{repo}"{extra}}}"#,
            extra.len()
        ),
    )
}

const OID_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const OID_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const OID_C: &str = "cccccccccccccccccccccccccccccccccccccccc";

#[test]
fn cross_repo_queue_acquires_leases_in_canonical_repo_order_all_or_none_and_verifies() {
    let (s, clock) = fixture();
    put_task(&s, "t-a", 0, "repo-a", r#","status":"ready""#);
    put_task(&s, "t-b", 0, "repo-b", r#","status":"ready""#);
    put_task(&s, "t-z", 0, "repo-z", r#","status":"ready""#);

    // Out-of-bounds concurrency limits are refused.
    refuse(
        &s,
        "queue.enqueue",
        &format!(
            r#"{{"id":"q-bad-1","request_id":"qb1","expected_revision":0,"title":"Too small","concurrency_limit":1,"members":[{{"member_id":"m1","repository_id":"repo-a","git_dir":"/tmp/git-a/.git","base_oid":"{OID_A}"}}]}}"#
        ),
        "invalid_input",
    );
    refuse(
        &s,
        "queue.enqueue",
        &format!(
            r#"{{"id":"q-bad-5","request_id":"qb5","expected_revision":0,"title":"Too big","concurrency_limit":5,"members":[{{"member_id":"m1","repository_id":"repo-a","git_dir":"/tmp/git-a/.git","base_oid":"{OID_A}"}}]}}"#
        ),
        "invalid_input",
    );

    // Enqueue out of alphabetical repository order (repo-z, repo-b, repo-a) with concurrency_limit=2.
    let enq = call(
        &s,
        "queue.enqueue",
        &format!(
            r#"{{"id":"q1","request_id":"q1-enq","expected_revision":0,"title":"Cross-repo release","concurrency_limit":2,"members":[
                {{"member_id":"m-z","repository_id":"repo-z","git_dir":"/tmp/git-z/.git","base_oid":"{OID_A}","task_id":"t-a","depends_on":["m-a"]}},
                {{"member_id":"m-b","repository_id":"repo-b","git_dir":"/tmp/git-b/.git","base_oid":"{OID_B}","task_id":"t-b"}},
                {{"member_id":"m-a","repository_id":"repo-a","git_dir":"/tmp/git-a/.git","base_oid":"{OID_C}"}}
            ]}}"#
        ),
    );
    assert_eq!(json::<String>(&s, &enq, "$.item.lease_order[0]"), "repo-a");
    assert_eq!(json::<String>(&s, &enq, "$.item.lease_order[1]"), "repo-b");
    assert_eq!(json::<String>(&s, &enq, "$.item.lease_order[2]"), "repo-z");
    assert_eq!(
        json::<String>(&s, &enq, "$.item.publication"),
        "manual_per_repository"
    );

    // While repo-b already holds an external run, acquiring q1 fails all-or-none (zero partial leases held).
    call(
        &s,
        "runs.prepare",
        &format!(
            r#"{{"id":"run-b-ext","request_id":"rbext","expected_revision":0,"task_id":"t-b","source_revision":1,"repository_id":"repo-b","repository_revision":1,"provider":"codex","permission_mode":"edit","cwd":"/tmp/git-b","git_dir":"/tmp/git-b/.git","git_common_dir":"/tmp/git-b/.git","head_oid":"{OID_B}","head_ref":"refs/heads/main"}}"#
        ),
    );
    refuse(
        &s,
        "queue.acquire",
        r#"{"id":"q1","request_id":"q1-acq-blocked","expected_revision":1}"#,
        "lease_conflict",
    );
    let held_after_conflict: i64 = s
        .connection()
        .query_row(
            "SELECT count(*) FROM work_queue_members WHERE queue_id='q1' AND lease_held=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        held_after_conflict, 0,
        "conflict must not leave partial leases"
    );

    // Cancel external run and acquire q1 cleanly.
    call(
        &s,
        "runs.cancel",
        r#"{"id":"run-b-ext","request_id":"rbext-cancel","expected_revision":1}"#,
    );
    let acq = call(
        &s,
        "queue.acquire",
        r#"{"id":"q1","request_id":"q1-acq","expected_revision":1}"#,
    );
    assert_eq!(json::<String>(&s, &acq, "$.item.state"), "leased");
    // m-a and m-b leased (2 concurrent slots); m-z stays queued on depends_on=["m-a"].
    assert_eq!(
        json::<String>(&s, &acq, "$.item.members[0].state"),
        "queued"
    );
    assert_eq!(json::<i64>(&s, &acq, "$.item.members[0].lease_held"), 0);
    assert_eq!(
        json::<String>(&s, &acq, "$.item.members[1].state"),
        "leased"
    );
    assert_eq!(json::<i64>(&s, &acq, "$.item.members[1].lease_held"), 1);
    assert_eq!(
        json::<String>(&s, &acq, "$.item.members[2].state"),
        "leased"
    );
    assert_eq!(json::<i64>(&s, &acq, "$.item.members[2].lease_held"), 1);

    // An unrelated run cannot hijack repo-a's leased checkout.
    refuse(
        &s,
        "runs.prepare",
        &format!(
            r#"{{"id":"run-steal","request_id":"rsteal","expected_revision":0,"task_id":"t-a","source_revision":1,"repository_id":"repo-a","repository_revision":1,"provider":"codex","permission_mode":"edit","cwd":"/tmp/git-a","git_dir":"/tmp/git-a/.git","git_common_dir":"/tmp/git-a/.git","head_oid":"{OID_C}","head_ref":"refs/heads/main"}}"#
        ),
        "checkout_busy",
    );

    // Move m-a running -> passed; m-z automatically gets promoted and leased.
    clock.store(10_100, Ordering::SeqCst);
    call(
        &s,
        "queue.member.update",
        r#"{"id":"q1","request_id":"q1-ma-run","expected_revision":2,"member_id":"m-a","state":"running"}"#,
    );
    let passed_ma = call(
        &s,
        "queue.member.update",
        r#"{"id":"q1","request_id":"q1-ma-pass","expected_revision":3,"member_id":"m-a","state":"passed"}"#,
    );
    assert_eq!(
        json::<String>(&s, &passed_ma, "$.item.members[2].state"),
        "passed"
    );
    assert_eq!(
        json::<i64>(&s, &passed_ma, "$.item.members[2].lease_held"),
        0
    );
    assert_eq!(
        json::<String>(&s, &passed_ma, "$.item.members[0].state"),
        "leased"
    );
    assert_eq!(
        json::<i64>(&s, &passed_ma, "$.item.members[0].lease_held"),
        1
    );

    // Finish m-b and m-z.
    call(
        &s,
        "queue.member.update",
        r#"{"id":"q1","request_id":"q1-mb-pass","expected_revision":4,"member_id":"m-b","state":"passed"}"#,
    );
    let ready_verify = call(
        &s,
        "queue.member.update",
        r#"{"id":"q1","request_id":"q1-mz-pass","expected_revision":5,"member_id":"m-z","state":"passed"}"#,
    );
    assert_eq!(
        json::<String>(&s, &ready_verify, "$.item.state"),
        "verifying"
    );

    // Verification refuses non-manual publication and refuses skipping passed members.
    refuse(
        &s,
        "queue.verify",
        r#"{"id":"q1","request_id":"q1-bad-pub","expected_revision":6,"publication":"atomic","results":[{"member_id":"m-a","ok":true},{"member_id":"m-b","ok":true},{"member_id":"m-z","ok":true}]}"#,
        "invalid_input",
    );
    refuse(
        &s,
        "queue.verify",
        r#"{"id":"q1","request_id":"q1-missing-member","expected_revision":6,"results":[{"member_id":"m-a","ok":true},{"member_id":"m-b","ok":true}]}"#,
        "invalid_input",
    );

    let verified = call(
        &s,
        "queue.verify",
        &format!(
            r#"{{"id":"q1","request_id":"q1-verify","expected_revision":6,"results":[
                {{"member_id":"m-a","ok":true,"head_oid":"{OID_A}","command":"cargo test","exit_code":0,"summary":"ok"}},
                {{"member_id":"m-b","ok":true,"head_oid":"{OID_B}","command":"bun test","exit_code":0,"summary":"ok"}},
                {{"member_id":"m-z","ok":true,"head_oid":"{OID_C}","command":"go test","exit_code":0,"summary":"ok"}}
            ]}}"#
        ),
    );
    assert_eq!(json::<String>(&s, &verified, "$.item.state"), "passed");
    assert_eq!(
        json::<String>(&s, &verified, "$.item.verification.publication"),
        "manual_per_repository"
    );
    assert_eq!(json::<i64>(&s, &verified, "$.item.verification.ok"), 1);
}

#[test]
fn cross_repo_queue_enforces_active_and_elapsed_timeouts_and_failure_blocks_dependents() {
    let (s, clock) = fixture();

    // Queue 1: active timeout hits 2700s (45 min).
    call(
        &s,
        "queue.enqueue",
        &format!(
            r#"{{"id":"q-timeout","request_id":"qt-enq","expected_revision":0,"title":"Timeout test","members":[
                {{"member_id":"m-a","repository_id":"repo-a","git_dir":"/tmp/git-a/.git","base_oid":"{OID_A}"}},
                {{"member_id":"m-b","repository_id":"repo-b","git_dir":"/tmp/git-b/.git","base_oid":"{OID_B}"}}
            ]}}"#
        ),
    );
    call(
        &s,
        "queue.acquire",
        r#"{"id":"q-timeout","request_id":"qt-acq","expected_revision":1}"#,
    );
    refuse(
        &s,
        "queue.sweep",
        r#"{"id":"q-timeout","request_id":"qt-early","expected_revision":2}"#,
        "not_expired",
    );

    // Advance 45 active minutes (2700s) -> sweep times out and releases leases.
    clock.store(12_700, Ordering::SeqCst);
    let swept = call(
        &s,
        "queue.sweep",
        r#"{"id":"q-timeout","request_id":"qt-sweep","expected_revision":2}"#,
    );
    assert_eq!(json::<String>(&s, &swept, "$.item.state"), "timed_out");
    assert_eq!(json::<i64>(&s, &swept, "$.item.members[0].lease_held"), 0);
    assert_eq!(json::<i64>(&s, &swept, "$.item.members[1].lease_held"), 0);

    // Queue 2: failure of m-a blocks downstream m-b instead of launching it, and elapsed timeout (7200s) works too.
    call(
        &s,
        "queue.enqueue",
        &format!(
            r#"{{"id":"q-dep","request_id":"qd-enq","expected_revision":0,"title":"Dep propagation","members":[
                {{"member_id":"m-a","repository_id":"repo-a","git_dir":"/tmp/git-a/.git","base_oid":"{OID_A}"}},
                {{"member_id":"m-b","repository_id":"repo-b","git_dir":"/tmp/git-b/.git","base_oid":"{OID_B}","depends_on":["m-a"]}}
            ]}}"#
        ),
    );
    call(
        &s,
        "queue.acquire",
        r#"{"id":"q-dep","request_id":"qd-acq","expected_revision":1}"#,
    );
    let failed = call(
        &s,
        "queue.member.update",
        r#"{"id":"q-dep","request_id":"qd-fail","expected_revision":2,"member_id":"m-a","state":"failed"}"#,
    );
    assert_eq!(json::<String>(&s, &failed, "$.item.state"), "failed");
    assert_eq!(
        json::<String>(&s, &failed, "$.item.members[1].state"),
        "blocked"
    );

    // Queue 3: concurrency cap keeps 3rd member queued while 2 are active.
    call(
        &s,
        "queue.enqueue",
        &format!(
            r#"{{"id":"q-cap","request_id":"qc-enq","expected_revision":0,"title":"Cap 2","concurrency_limit":2,"members":[
                {{"member_id":"m-a","repository_id":"repo-a","git_dir":"/tmp/git-a/.git","base_oid":"{OID_A}"}},
                {{"member_id":"m-b","repository_id":"repo-b","git_dir":"/tmp/git-b/.git","base_oid":"{OID_B}"}},
                {{"member_id":"m-c","repository_id":"repo-c","git_dir":"/tmp/git-c/.git","base_oid":"{OID_C}"}}
            ]}}"#
        ),
    );
    let cap_acq = call(
        &s,
        "queue.acquire",
        r#"{"id":"q-cap","request_id":"qc-acq","expected_revision":1}"#,
    );
    assert_eq!(
        json::<String>(&s, &cap_acq, "$.item.members[0].state"),
        "leased"
    );
    assert_eq!(
        json::<String>(&s, &cap_acq, "$.item.members[1].state"),
        "leased"
    );
    assert_eq!(
        json::<String>(&s, &cap_acq, "$.item.members[2].state"),
        "queued"
    );
    // Advance past 2 elapsed hours (7200s); next sweep expires q-cap.
    clock.store(12_700 + 7_200, Ordering::SeqCst);
    let elapsed_swept = call(
        &s,
        "queue.sweep",
        r#"{"id":"q-cap","request_id":"qc-sweep","expected_revision":2}"#,
    );
    assert_eq!(
        json::<String>(&s, &elapsed_swept, "$.item.state"),
        "timed_out"
    );
}

#[test]
fn task_blocks_and_parent_dependencies_gate_launch_and_reject_cycles() {
    let (s, _) = fixture();
    put_task(&s, "blocker", 0, "repo-a", r#","status":"ready""#);
    put_task(&s, "downstream", 0, "repo-a", r#","status":"ready""#);
    // blocker -> blocks -> downstream
    put_task(
        &s,
        "blocker",
        1,
        "repo-a",
        r#","status":"ready","links":[{"kind":"blocks","item_id":"downstream"}]"#,
    );

    // Creating a blocks cycle (downstream -> blocks -> blocker) is refused.
    refuse(
        &s,
        "items.put",
        r#"{"id":"downstream","request_id":"cyc","expected_revision":1,"title":"Task downstream","repository_ids":["repo-a"],"primary_repository_id":"repo-a","status":"ready","links":[{"kind":"blocks","item_id":"blocker"}]}"#,
        "invalid_input",
    );

    let r = call(&s, "items.readiness", r#"{"id":"downstream"}"#);
    assert_eq!(json::<i64>(&s, &r, "$.item.ready"), 0);
    assert_eq!(json::<String>(&s, &r, "$.item.blocked_by[0].id"), "blocker");

    // runs.prepare refuses blocked task with task_blocked.
    refuse(
        &s,
        "runs.prepare",
        &format!(
            r#"{{"id":"run-blocked","request_id":"rb1","expected_revision":0,"task_id":"downstream","source_revision":1,"repository_id":"repo-a","repository_revision":1,"provider":"codex","permission_mode":"edit","cwd":"/tmp/git-a","git_dir":"/tmp/git-a/.git","git_common_dir":"/tmp/git-a/.git","head_oid":"{OID_A}","head_ref":"refs/heads/main"}}"#
        ),
        "task_blocked",
    );

    // Completing blocker unblocks downstream.
    put_task(
        &s,
        "blocker",
        2,
        "repo-a",
        r#","status":"done","links":[{"kind":"blocks","item_id":"downstream"}]"#,
    );
    let r2 = call(&s, "items.readiness", r#"{"id":"downstream"}"#);
    assert_eq!(json::<i64>(&s, &r2, "$.item.ready"), 1);

    // Parent task with incomplete child is refused only when require_subtasks_done=true.
    put_task(
        &s,
        "child",
        0,
        "repo-a",
        r#","status":"ready","links":[{"kind":"parent","item_id":"downstream"}]"#,
    );
    let r3 = call(&s, "items.readiness", r#"{"id":"downstream"}"#);
    assert_eq!(json::<i64>(&s, &r3, "$.item.ready"), 1);
    assert_eq!(json::<i64>(&s, &r3, "$.item.ready_with_subtasks"), 0);
    refuse(
        &s,
        "runs.prepare",
        &format!(
            r#"{{"id":"run-sub","request_id":"rsub1","expected_revision":0,"task_id":"downstream","source_revision":1,"repository_id":"repo-a","repository_revision":1,"provider":"codex","permission_mode":"edit","cwd":"/tmp/git-a","git_dir":"/tmp/git-a/.git","git_common_dir":"/tmp/git-a/.git","head_oid":"{OID_A}","head_ref":"refs/heads/main","require_subtasks_done":true}}"#
        ),
        "subtasks_incomplete",
    );
}

#[test]
fn github_sync_and_confirmed_durable_outbox_survive_restart_and_reconcile() {
    let dir = Scratch::new();
    let db = dir.path().join("workbench.sqlite");
    let store = Store::open(&db).unwrap();
    store
        .workbench_request(
            "repositories.put",
            r#"{"id":"repo-a","request_id":"ra","expected_revision":0,"name":"repo-a","identity_key":"local:/tmp/git-a/.git"}"#,
        )
        .unwrap();
    store
        .workbench_request(
            "items.put",
            r#"{"id":"t1","request_id":"t1-0","expected_revision":0,"title":"Original local title","description":"Local body","status":"ready","labels":["bug"],"locked_fields":["title"],"repository_ids":["repo-a"],"primary_repository_id":"repo-a"}"#,
        )
        .unwrap();

    // Link t1 with outbound_opt_in=false initially.
    call(
        &store,
        "github_sync.link",
        r#"{"id":"s1","request_id":"s1-link","expected_revision":0,"task_id":"t1","repo_slug":"acme/widgets","issue_number":42,"outbound_opt_in":false,"last_seen":{"title":"Original local title","body":"Local body","state":"open","labels":["bug"]}}"#,
    );

    // Inbound observe records proposal WITHOUT modifying task t1, and notes locked_skipped=["title"].
    let obs = call(
        &store,
        "github_sync.observe",
        r#"{"id":"s1","request_id":"s1-obs","expected_revision":1,"remote":{"title":"Remote renamed title","body":"Remote updated body","state":"closed","labels":["bug","urgent"]}}"#,
    );
    assert_eq!(
        json::<String>(&store, &obs, "$.item.pending_proposal.locked_skipped[0]"),
        "title"
    );
    let pending_before = call(&store, "github_sync.list", r#"{"only_pending":true}"#);
    assert_eq!(json::<i64>(&store, &pending_before, "$.total"), 1);
    let task_unchanged = call(&store, "items.get", r#"{"id":"t1"}"#);
    assert_eq!(
        json::<String>(&store, &task_unchanged, "$.item.title"),
        "Original local title"
    );
    assert_eq!(
        json::<String>(&store, &task_unchanged, "$.item.description"),
        "Local body"
    );

    // Explicitly accepting proposal applies unlocked fields and preserves locked title.
    refuse(
        &store,
        "github_sync.accept",
        r#"{"id":"s1","request_id":"s1-acc-lock","expected_revision":2,"fields":["title"]}"#,
        "field_locked",
    );
    call(
        &store,
        "github_sync.accept",
        r#"{"id":"s1","request_id":"s1-acc","expected_revision":2}"#,
    );
    let pending_after = call(&store, "github_sync.list", r#"{"only_pending":true}"#);
    assert_eq!(json::<i64>(&store, &pending_after, "$.total"), 0);
    let task_accepted = call(&store, "items.get", r#"{"id":"t1"}"#);
    assert_eq!(
        json::<String>(&store, &task_accepted, "$.item.title"),
        "Original local title"
    );
    assert_eq!(
        json::<String>(&store, &task_accepted, "$.item.description"),
        "Remote updated body"
    );
    assert_eq!(
        json::<String>(&store, &task_accepted, "$.item.status"),
        "done"
    );

    // Outbound writes fail without confirmation AND fail without opt-in.
    refuse(
        &store,
        "outbox.enqueue",
        r#"{"id":"o1","request_id":"o1-noconf","expected_revision":0,"task_id":"t1","op":"update_issue","confirmed":false,"idempotency_key":"op-key-1234"}"#,
        "confirmation_required",
    );
    refuse(
        &store,
        "outbox.enqueue",
        r#"{"id":"o1","request_id":"o1-noopt","expected_revision":0,"task_id":"t1","op":"update_issue","confirmed":true,"idempotency_key":"op-key-1234"}"#,
        "outbound_disabled",
    );

    // Enable outbound_opt_in, edit local description, preview & enqueue with confirmed=true.
    call(
        &store,
        "github_sync.link",
        r#"{"id":"s1","request_id":"s1-optin","expected_revision":3,"task_id":"t1","repo_slug":"acme/widgets","issue_number":42,"outbound_opt_in":true}"#,
    );
    call(
        &store,
        "items.put",
        r#"{"id":"t1","request_id":"t1-local-edit","expected_revision":2,"title":"Original local title","description":"Edited locally for push","status":"done","labels":["bug","urgent"],"repository_ids":["repo-a"],"primary_repository_id":"repo-a"}"#,
    );
    let preview = call(
        &store,
        "outbox.preview",
        r#"{"task_id":"t1","op":"update_issue"}"#,
    );
    let digest: String = json(&store, &preview, "$.preview.payload_digest");
    assert_eq!(json::<i64>(&store, &preview, "$.preview.can_enqueue"), 1);

    // Stale digest refused.
    refuse(
        &store,
        "outbox.enqueue",
        r#"{"id":"o1","request_id":"o1-bad-dig","expected_revision":0,"task_id":"t1","op":"update_issue","confirmed":true,"confirm_payload_digest":"0000000000000000000000000000000000000000000000000000000000000000","idempotency_key":"op-key-1234"}"#,
        "payload_changed",
    );

    call(
        &store,
        "outbox.enqueue",
        &format!(
            r#"{{"id":"o1","request_id":"o1-enq","expected_revision":0,"task_id":"t1","op":"update_issue","confirmed":true,"confirm_payload_digest":"{digest}","idempotency_key":"op-key-1234"}}"#
        ),
    );
    // Second entry with same idempotency_key is refused idempotently.
    refuse(
        &store,
        "outbox.enqueue",
        r#"{"id":"o2","request_id":"o2-dup","expected_revision":0,"task_id":"t1","op":"update_issue","confirmed":true,"idempotency_key":"op-key-1234"}"#,
        "duplicate_outbox_entry",
    );

    // Claim o1, then simulate crash & reopen from disk!
    call(
        &store,
        "outbox.claim",
        r#"{"id":"o1","request_id":"o1-claim","expected_revision":1}"#,
    );
    drop(store);

    let reopened = Store::open(&db).unwrap();
    let restarted = call(
        &reopened,
        "outbox.sweep_restart",
        r#"{"id":"o1","request_id":"o1-sweep","expected_revision":2,"reason":"process restarted"}"#,
    );
    assert_eq!(
        json::<String>(&reopened, &restarted, "$.item.state"),
        "outcome_unknown"
    );
    // Cannot blindly reclaim while outcome_unknown.
    refuse(
        &reopened,
        "outbox.claim",
        r#"{"id":"o1","request_id":"o1-reclaim","expected_revision":3}"#,
        "invalid_state",
    );

    // Case A: remote preflight finds remote diverged since last_seen -> transitions to conflict!
    let conflict = call(
        &reopened,
        "outbox.preflight",
        r#"{"id":"o1","request_id":"o1-pre-conflict","expected_revision":3,"remote":{"title":"Original local title","body":"Someone else edited remotely","state":"closed","labels":["bug","urgent"]}}"#,
    );
    assert_eq!(
        json::<String>(&reopened, &conflict, "$.item.state"),
        "conflict"
    );
    assert_eq!(
        json::<String>(&reopened, &conflict, "$.item.conflict.fields[0]"),
        "body"
    );

    // Case B: preflight sees remote ALREADY received the exact payload -> marks delivered cleanly!
    let reconciled = call(
        &reopened,
        "outbox.preflight",
        r#"{"id":"o1","request_id":"o1-pre-done","expected_revision":4,"remote":{"title":"Original local title","body":"Edited locally for push","state":"closed","labels":["bug","urgent"]}}"#,
    );
    assert_eq!(
        json::<String>(&reopened, &reconciled, "$.item.state"),
        "pending"
    );
    // Move to delivering then test retry backoff + outcome_unknown already-applied reconcile.
    call(
        &reopened,
        "outbox.claim",
        r#"{"id":"o1","request_id":"o1-claim2","expected_revision":5}"#,
    );
    call(
        &reopened,
        "outbox.mark_unknown",
        r#"{"id":"o1","request_id":"o1-unk2","expected_revision":6,"reason":"timeout"}"#,
    );
    let already_applied = call(
        &reopened,
        "outbox.preflight",
        r#"{"id":"o1","request_id":"o1-pre-landed","expected_revision":7,"remote":{"title":"Original local title","body":"Edited locally for push","state":"closed","labels":["bug","urgent"]}}"#,
    );
    assert_eq!(
        json::<String>(&reopened, &already_applied, "$.item.state"),
        "delivered"
    );
}

#[test]
fn skip_checkout_occupancy_never_blocks_queue_acquire_but_still_obeys_queue_leases() {
    let (s, _) = fixture();
    put_task(&s, "t-a", 0, "repo-a", r#","status":"ready""#);
    put_task(&s, "t-b", 0, "repo-b", r#","status":"ready""#);

    let remote_b = |id: &str, req: &str, extra: &str| {
        format!(
            r#"{{"id":"{id}","request_id":"{req}","expected_revision":0,"task_id":"t-b","source_revision":1,"repository_id":"repo-b","repository_revision":1,"provider":"codex","permission_mode":"edit","cwd":"/tmp/git-b","git_dir":"/tmp/git-b/.git","git_common_dir":"/tmp/git-b/.git","head_oid":"{OID_B}","head_ref":"refs/heads/main","skip_checkout_occupancy":true{extra}}}"#
        )
    };

    // 1. While a `skip_checkout_occupancy: true` run is `prepared`, `queue.acquire` on repo-b succeeds.
    call(&s, "runs.prepare", &remote_b("run-b-prep", "rbp-1", ""));
    call(
        &s,
        "queue.enqueue",
        &format!(
            r#"{{"id":"q1","request_id":"q1-enq","expected_revision":0,"title":"Q1","members":[{{"member_id":"m-b","repository_id":"repo-b","git_dir":"/tmp/git-b/.git","base_oid":"{OID_B}","task_id":"t-b"}}]}}"#
        ),
    );
    let acq1 = call(
        &s,
        "queue.acquire",
        r#"{"id":"q1","request_id":"q1-acq","expected_revision":1}"#,
    );
    assert_eq!(json::<String>(&s, &acq1, "$.item.state"), "leased");
    // Release q1 so we can test `running` and `unresolved` states.
    call(
        &s,
        "queue.member.update",
        r#"{"id":"q1","request_id":"q1-pass","expected_revision":2,"member_id":"m-b","state":"passed"}"#,
    );
    call(
        &s,
        "queue.verify",
        &format!(
            r#"{{"id":"q1","request_id":"q1-ver","expected_revision":3,"results":[{{"member_id":"m-b","ok":true,"head_oid":"{OID_B}","command":"test","exit_code":0,"summary":"ok"}}]}}"#
        ),
    );

    // 2. While `skip_checkout_occupancy: true` run is `running`, `queue.acquire` on repo-b succeeds.
    call(&s, "runs.prepare", &remote_b("run-b-live", "rbl-1", ""));
    call(
        &s,
        "runs.claim",
        r#"{"id":"run-b-live","request_id":"rbl-claim","expected_revision":1,"owner_id":"host-1","session_id":"sess-1"}"#,
    );
    call(
        &s,
        "runs.started",
        r#"{"id":"run-b-live","request_id":"rbl-start","expected_revision":2,"owner_id":"host-1","session_id":"sess-1","process_id":12,"process_start":"boot:12"}"#,
    );
    call(
        &s,
        "queue.enqueue",
        &format!(
            r#"{{"id":"q2","request_id":"q2-enq","expected_revision":0,"title":"Q2","members":[{{"member_id":"m-b","repository_id":"repo-b","git_dir":"/tmp/git-b/.git","base_oid":"{OID_B}","task_id":"t-b"}}]}}"#
        ),
    );
    let acq2 = call(
        &s,
        "queue.acquire",
        r#"{"id":"q2","request_id":"q2-acq","expected_revision":1}"#,
    );
    assert_eq!(json::<String>(&s, &acq2, "$.item.state"), "leased");
    call(
        &s,
        "queue.member.update",
        r#"{"id":"q2","request_id":"q2-pass","expected_revision":2,"member_id":"m-b","state":"passed"}"#,
    );
    call(
        &s,
        "queue.verify",
        &format!(
            r#"{{"id":"q2","request_id":"q2-ver","expected_revision":3,"results":[{{"member_id":"m-b","ok":true,"head_oid":"{OID_B}","command":"test","exit_code":0,"summary":"ok"}}]}}"#
        ),
    );

    // 3. While `skip_checkout_occupancy: true` run is `unresolved`, `queue.acquire` on repo-b succeeds.
    call(
        &s,
        "runs.finish",
        r#"{"id":"run-b-live","request_id":"rbl-unres","expected_revision":3,"owner_id":"host-1","session_id":"sess-1","outcome":"unresolved","reason":"lost ssh"}"#,
    );
    call(
        &s,
        "queue.enqueue",
        &format!(
            r#"{{"id":"q3","request_id":"q3-enq","expected_revision":0,"title":"Q3","members":[{{"member_id":"m-b","repository_id":"repo-b","git_dir":"/tmp/git-b/.git","base_oid":"{OID_B}","task_id":"t-b"}}]}}"#
        ),
    );
    let acq3 = call(
        &s,
        "queue.acquire",
        r#"{"id":"q3","request_id":"q3-acq","expected_revision":1}"#,
    );
    assert_eq!(json::<String>(&s, &acq3, "$.item.state"), "leased");

    // 4. While q3 holds an active queue lease on repo-b, `runs.prepare` with
    // `skip_checkout_occupancy: true` still obeys `queue::verify_run_lease`.
    refuse(
        &s,
        "runs.prepare",
        &remote_b("run-b-nolease", "rbnl-1", ""),
        "checkout_busy",
    );
    refuse(
        &s,
        "runs.prepare",
        &remote_b(
            "run-b-badlease",
            "rbbl-1",
            r#","queue_id":"q3","member_id":"m-missing""#,
        ),
        "queue_lease_required",
    );
    let leased = call(
        &s,
        "runs.prepare",
        &remote_b(
            "run-b-goodlease",
            "rbgl-1",
            r#","queue_id":"q3","member_id":"m-b""#,
        ),
    );
    assert_eq!(
        json::<i64>(&s, &leased, "$.item.skip_checkout_occupancy"),
        1
    );
}

fn member(id: &str, repo: &str, git_dir: &str) -> String {
    format!(
        r#"{{"member_id":"{id}","repository_id":"{repo}","git_dir":"{git_dir}","base_oid":"{OID_A}"}}"#
    )
}

/// Removes the fixture checkout `name` (created by `real_checkouts`) so the
/// path a stored member names no longer resolves.
fn delete_checkout(name: &str) {
    std::fs::remove_dir_all(format!("{}/checkout/{name}", support::checkout_root())).unwrap();
}

#[test]
fn a_member_whose_directory_was_deleted_does_not_break_finishing_another_member() {
    let (s, _) = fixture();
    call(
        &s,
        "queue.enqueue",
        &format!(
            r#"{{"id":"q-stale","request_id":"qs-enq","expected_revision":0,"title":"Stale","concurrency_limit":2,"members":[{},{},{}]}}"#,
            member("m-a", "repo-a", "/checkout/stale-a/.git"),
            member("m-c", "repo-c", "/checkout/stale-c/.git"),
            member("m-z", "repo-z", "/checkout/stale-z/.git"),
        ),
    );
    delete_checkout("stale-z");
    let acquired = call(
        &s,
        "queue.acquire",
        r#"{"id":"q-stale","request_id":"qs-acq","expected_revision":1}"#,
    );
    assert_eq!(
        json::<String>(&s, &acquired, "$.item.members[0].state"),
        "leased"
    );
    let passed = call(
        &s,
        "queue.member.update",
        r#"{"id":"q-stale","request_id":"qs-pass","expected_revision":2,"member_id":"m-a","state":"passed"}"#,
    );
    assert_eq!(
        json::<String>(&s, &passed, "$.item.members[2].state"),
        "leased"
    );
}

#[test]
fn a_member_whose_directory_was_deleted_does_not_fail_acquire_for_another_member() {
    let (s, _) = fixture();
    call(
        &s,
        "queue.enqueue",
        &format!(
            r#"{{"id":"q-acq","request_id":"qa-enq","expected_revision":0,"title":"Stale acquire","members":[{},{}]}}"#,
            member("m-a", "repo-a", "/checkout/acq-a/.git"),
            member("m-b", "repo-b", "/checkout/acq-b/.git"),
        ),
    );
    delete_checkout("acq-b");
    let acquired = call(
        &s,
        "queue.acquire",
        r#"{"id":"q-acq","request_id":"qa-acq","expected_revision":1}"#,
    );
    assert_eq!(
        json::<String>(&s, &acquired, "$.item.members[0].state"),
        "leased"
    );
    assert_eq!(
        json::<String>(&s, &acquired, "$.item.members[1].state"),
        "leased"
    );
}

#[test]
fn a_member_enqueued_with_a_respelled_git_dir_is_stored_resolved() {
    let (s, _) = fixture();
    let enqueued = call(
        &s,
        "queue.enqueue",
        &format!(
            r#"{{"id":"q-resolved","request_id":"qr-enq","expected_revision":0,"title":"Resolved","members":[{}]}}"#,
            member("m-a", "repo-a", "/checkout/resolved/x/../.git/"),
        ),
    );
    let expected = std::fs::canonicalize(format!(
        "{}/checkout/resolved/.git",
        support::checkout_root()
    ))
    .unwrap();
    assert_eq!(
        json::<String>(&s, &enqueued, "$.item.members[0].git_dir"),
        expected.to_str().unwrap()
    );
}

#[test]
fn a_leased_member_stored_in_the_old_spelling_still_passes_a_claimed_prepare() {
    let (s, _) = fixture();
    put_task(&s, "t-a", 0, "repo-a", r#","status":"ready""#);
    call(
        &s,
        "queue.enqueue",
        &format!(
            r#"{{"id":"q-old","request_id":"qo-enq","expected_revision":0,"title":"Old spelling","members":[{}]}}"#,
            member("m-a", "repo-a", "/checkout/old/.git")
                .replace("\"base_oid\"", "\"task_id\":\"t-a\",\"base_oid\""),
        ),
    );
    call(
        &s,
        "queue.acquire",
        r#"{"id":"q-old","request_id":"qo-acq","expected_revision":1}"#,
    );
    // A member enqueued before checkout paths were resolved holds the host's
    // spelling, whatever it was.
    let git = format!("{}/checkout/old/.git", support::checkout_root());
    let legacy = format!("{}/checkout/old/x/../.git/", support::checkout_root());
    std::fs::create_dir_all(format!("{}/checkout/old/x", support::checkout_root())).unwrap();
    let changed = s
        .connection()
        .execute(
            "UPDATE work_queue_members SET git_dir=?1 WHERE queue_id='q-old'",
            [&legacy],
        )
        .unwrap();
    assert_eq!(changed, 1);
    let prepared = call(
        &s,
        "runs.prepare",
        &format!(
            r#"{{"id":"run-old","request_id":"ro-prep","expected_revision":0,"task_id":"t-a","source_revision":1,"repository_id":"repo-a","repository_revision":1,"provider":"codex","permission_mode":"edit","cwd":"/checkout/old","git_dir":"{git}","git_common_dir":"/tmp/git-a/.git","head_oid":"{OID_A}","head_ref":"refs/heads/main","queue_id":"q-old","member_id":"m-a"}}"#
        ),
    );
    assert_eq!(json::<String>(&s, &prepared, "$.item.id"), "run-old");
}
