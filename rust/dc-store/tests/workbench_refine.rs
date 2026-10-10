//! Refinement proposals (schema 12): the enhancement lifecycle carrying
//! acceptance criteria, splits and `duplicate_of` links instead of text.

use dc_store::Store;

fn fixture() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.set_clock(|| 1_000);
    for (id, name) in [("r", "Manvi"), ("r2", "Other")] {
        call(
            &s,
            "repositories.put",
            &format!(
                r#"{{"id":"{id}","request_id":"repo-{id}","expected_revision":0,"name":"{name}","identity_key":"local:{id}"}}"#
            ),
        );
    }
    task(
        &s,
        "t",
        "r",
        r#","acceptance_criteria":["Original criterion"]"#,
    );
    task(&s, "twin", "r", "");
    task(&s, "alien", "r2", "");
    s
}
fn task(s: &Store, id: &str, repo: &str, extra: &str) {
    call(
        s,
        "items.put",
        &format!(
            r#"{{"id":"{id}","request_id":"task-{id}","expected_revision":0,"title":"{id} title","description":"Exact error: E42. Preserve all constraints.","repository_ids":["{repo}"],"primary_repository_id":"{repo}"{extra}}}"#
        ),
    );
}
fn call(s: &Store, method: &str, raw: &str) -> String {
    s.workbench_request(method, raw)
        .unwrap_or_else(|e| panic!("{method}: {e}"))
}
fn json<T: rusqlite::types::FromSql>(s: &Store, raw: &str, path: &str) -> T {
    s.connection()
        .query_row("SELECT json_extract(?1,?2)", [raw, path], |r| r.get(0))
        .unwrap()
}
fn refuse(s: &Store, method: &str, raw: &str, code: &str) {
    assert_eq!(
        s.workbench_request(method, raw).unwrap_err().code,
        code,
        "{method}: {raw}"
    );
}
fn create(s: &Store, id: &str, source_revision: i64) -> String {
    call(
        s,
        "enhancements.create",
        &format!(
            r#"{{"id":"{id}","request_id":"create-{id}","expected_revision":0,"task_id":"t","source_revision":{source_revision},"kind":"refine","fields":["acceptance_criteria","split_into","duplicate_of"],"provider":"local","model":"m"}}"#
        ),
    )
}
const PROPOSAL: &str = r#""acceptance_criteria":["E42 is reported once"],"split_into":[{"title":"Part A","description":"Keep E42."},{"title":"Part B","description":"Keep all constraints."}],"duplicate_of":[{"id":"twin","reason":"same ask"}]"#;
fn complete(s: &Store, id: &str, rev: i64) -> String {
    call(
        s,
        "enhancements.complete",
        &format!(
            r#"{{"id":"{id}","request_id":"complete-{id}","expected_revision":{rev},{PROPOSAL},"rationale":"why","grounding":{{"status":"available"}}}}"#
        ),
    )
}
fn ready(s: &Store, id: &str) {
    create(s, id, 1);
    complete(s, id, 1);
}
fn accept(s: &Store, id: &str, fields: &str, task_rev: i64, extra: &str) -> Result<String, String> {
    let tag: String = format!("{fields}{extra}")
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect();
    s.workbench_request(
        "enhancements.accept",
        &format!(
            r#"{{"id":"{id}","request_id":"accept-{id}-{task_rev}-{tag}","expected_revision":2,"expected_task_revision":{task_rev},"fields":{fields}{extra}}}"#
        ),
    )
    .map_err(|e| { if std::env::var("DBG").is_ok() { eprintln!("{}: {}", e.code, e.message); } e.code.to_string() })
}
const ALL: &str = r#"["acceptance_criteria","split_into","duplicate_of"]"#;

#[test]
fn a_refinement_stores_the_full_source_snapshot_and_its_proposal() {
    let s = fixture();
    let pending = create(&s, "e", 1);
    assert_eq!(json::<String>(&s, &pending, "$.item.kind"), "refine");
    assert_eq!(json::<String>(&s, &pending, "$.item.state"), "pending");
    assert_eq!(
        json::<String>(&s, &pending, "$.item.source.acceptance_criteria[0]"),
        "Original criterion"
    );
    assert_eq!(
        json::<String>(&s, &pending, "$.item.source.description"),
        "Exact error: E42. Preserve all constraints."
    );
    let done = complete(&s, "e", 1);
    assert_eq!(json::<String>(&s, &done, "$.item.state"), "ready");
    let got = call(&s, "enhancements.get", r#"{"id":"e"}"#);
    assert_eq!(
        json::<String>(&s, &got, "$.item.proposed.split_into[1].title"),
        "Part B"
    );
    assert_eq!(
        json::<String>(&s, &got, "$.item.grounding.status"),
        "available"
    );
    // The history list carries neither the snapshot nor the proposal.
    let list = call(&s, "enhancements.list", r#"{"task_id":"t"}"#);
    assert!(!list.contains("Part B") && !list.contains("available"));
}

#[test]
fn accept_applies_through_the_revisioned_task_update() {
    let s = fixture();
    ready(&s, "e");
    let done = accept(&s, "e", ALL, 1, r#","split_ids":["s1","s2"]"#).unwrap();
    assert_eq!(json::<String>(&s, &done, "$.item.state"), "accepted");
    assert_eq!(json::<i64>(&s, &done, "$.item.applied_task_revision"), 2);
    let t = call(&s, "items.get", r#"{"id":"t"}"#);
    assert_eq!(json::<i64>(&s, &t, "$.item.revision"), 2);
    assert_eq!(
        json::<String>(&s, &t, "$.item.acceptance_criteria"),
        r#"["Original criterion","E42 is reported once"]"#
    );
    assert_eq!(json::<String>(&s, &t, "$.item.links[0].item_id"), "twin");
    let row: String = s
        .connection()
        .query_row(
            "SELECT kind FROM work_item_links WHERE item_id='t' AND target_id='twin'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(row, "duplicate_of");
    let s2 = call(&s, "items.get", r#"{"id":"s2"}"#);
    assert_eq!(json::<String>(&s, &s2, "$.item.title"), "Part B");
    assert_eq!(json::<String>(&s, &s2, "$.item.links[0].kind"), "parent");
    assert_eq!(json::<String>(&s, &s2, "$.item.links[0].item_id"), "t");
    assert_eq!(json::<String>(&s, &s2, "$.item.primary_repository_id"), "r");
    // The task revision history records the criteria and link change once.
    let hist = call(&s, "items.history", r#"{"id":"t"}"#);
    assert_eq!(json::<i64>(&s, &hist, "$.total"), 2);
    // Receipt replay applies nothing twice.
    let again = accept(&s, "e", ALL, 1, r#","split_ids":["s1","s2"]"#).unwrap();
    assert_eq!(again, done);
}

#[test]
fn accept_can_select_a_part_and_only_that_part_changes() {
    let s = fixture();
    ready(&s, "e");
    accept(&s, "e", r#"["duplicate_of"]"#, 1, "").unwrap();
    let t = call(&s, "items.get", r#"{"id":"t"}"#);
    assert_eq!(
        json::<String>(&s, &t, "$.item.acceptance_criteria"),
        r#"["Original criterion"]"#
    );
    assert_eq!(json::<i64>(&s, &t, "$.item.revision"), 2);
    let n: i64 = s
        .connection()
        .query_row("SELECT count(*) FROM work_items", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 3, "no split was selected, so no task was created");
}

#[test]
fn a_stale_source_revision_is_refused_at_create_and_a_stale_task_at_accept() {
    let s = fixture();
    refuse(
        &s,
        "enhancements.create",
        r#"{"id":"x","request_id":"cx","expected_revision":0,"task_id":"t","source_revision":9,"kind":"refine","fields":["acceptance_criteria"],"provider":"local","model":"m"}"#,
        "revision_conflict",
    );
    ready(&s, "e");
    call(
        &s,
        "items.put",
        r#"{"id":"t","request_id":"edit","expected_revision":1,"title":"t title","description":"Changed by a person.","repository_ids":["r"],"primary_repository_id":"r","acceptance_criteria":["Original criterion"]}"#,
    );
    assert_eq!(
        accept(&s, "e", ALL, 1, r#","split_ids":["s1","s2"]"#).unwrap_err(),
        "revision_conflict"
    );
    // Fresh task revision, but the description the split was drafted from moved.
    assert_eq!(
        accept(&s, "e", ALL, 2, r#","split_ids":["s1","s2"]"#).unwrap_err(),
        "field_conflict"
    );
    // The conflict consumed nothing: criteria-only accept of an unchanged field works.
    accept(&s, "e", r#"["acceptance_criteria"]"#, 2, "").unwrap();
}

#[test]
fn a_locked_acceptance_criteria_field_refuses_create_claim_and_accept() {
    let s = fixture();
    call(
        &s,
        "items.put",
        r#"{"id":"t","request_id":"lock","expected_revision":1,"title":"t title","description":"Exact error: E42. Preserve all constraints.","repository_ids":["r"],"primary_repository_id":"r","acceptance_criteria":["Original criterion"],"locked_fields":["acceptance_criteria"]}"#,
    );
    refuse(
        &s,
        "enhancements.create",
        r#"{"id":"x","request_id":"cx","expected_revision":0,"task_id":"t","source_revision":2,"kind":"refine","fields":["acceptance_criteria"],"provider":"local","model":"m"}"#,
        "field_locked",
    );
    // Locking after the proposal is ready blocks accepting that part only.
    let s = fixture();
    ready(&s, "e");
    call(
        &s,
        "items.put",
        r#"{"id":"t","request_id":"lock","expected_revision":1,"title":"t title","description":"Exact error: E42. Preserve all constraints.","repository_ids":["r"],"primary_repository_id":"r","acceptance_criteria":["Original criterion"],"locked_fields":["acceptance_criteria"]}"#,
    );
    assert_eq!(
        accept(&s, "e", r#"["acceptance_criteria"]"#, 2, "").unwrap_err(),
        "field_locked"
    );
    accept(&s, "e", r#"["duplicate_of"]"#, 2, "").unwrap();
}

#[test]
fn only_one_generation_holds_the_profile_slot_across_kinds() {
    let s = fixture();
    create(&s, "first", 1);
    refuse(
        &s,
        "enhancements.create",
        r#"{"id":"second","request_id":"c2","expected_revision":0,"task_id":"t","source_revision":1,"kind":"refine","fields":["duplicate_of"],"provider":"local","model":"m"}"#,
        "busy",
    );
    refuse(
        &s,
        "enhancements.create",
        r#"{"id":"text","request_id":"c3","expected_revision":0,"task_id":"t","source_revision":1,"fields":["title"],"provider":"local","model":"m"}"#,
        "busy",
    );
    // Dismissing releases it; so does expiry of an unclaimed pending one.
    call(
        &s,
        "enhancements.dismiss",
        r#"{"id":"first","request_id":"d1","expected_revision":1}"#,
    );
    create(&s, "second", 1);
}

#[test]
fn reject_and_expiry_leave_the_task_untouched() {
    let mut s = fixture();
    ready(&s, "e");
    call(
        &s,
        "enhancements.dismiss",
        r#"{"id":"e","request_id":"d","expected_revision":2}"#,
    );
    refuse(
        &s,
        "enhancements.accept",
        r#"{"id":"e","request_id":"late","expected_revision":3,"expected_task_revision":1,"fields":["duplicate_of"]}"#,
        "invalid_transition",
    );
    let t = call(&s, "items.get", r#"{"id":"t"}"#);
    assert_eq!(json::<i64>(&s, &t, "$.item.revision"), 1);
    create(&s, "late", 1);
    s.set_clock(|| 5_000);
    refuse(
        &s,
        "enhancements.complete",
        &format!(r#"{{"id":"late","request_id":"cl","expected_revision":1,{PROPOSAL}}}"#),
        "expired",
    );
}

#[test]
fn malformed_refinements_are_refused_whole() {
    let s = fixture();
    create(&s, "e", 1);
    let bad = |extra: &str, name: &str| {
        refuse(
            &s,
            "enhancements.complete",
            &format!(r#"{{"id":"e","request_id":"bad-{name}","expected_revision":1,{extra}}}"#),
            "invalid_input",
        );
    };
    let ac = r#""acceptance_criteria":["ok"]"#;
    let sp = r#""split_into":[]"#;
    let du = r#""duplicate_of":[]"#;
    bad(&format!("{ac},{sp}"), "missing-part");
    bad(&format!(r#"{ac},{sp},{du},"title":"x""#), "text-field");
    bad(&format!(r#"{ac},{sp},{du},"extra":1"#), "unknown");
    bad(
        &format!(r#""acceptance_criteria":["Original criterion"],{sp},{du}"#),
        "repeats-task",
    );
    bad(
        &format!(r#""acceptance_criteria":["a","a"],{sp},{du}"#),
        "repeats-self",
    );
    bad(
        &format!(r#""acceptance_criteria":[" "],{sp},{du}"#),
        "blank",
    );
    bad(
        &format!(r#"{ac},"split_into":[{{"title":" ","description":"d"}}],{du}"#),
        "blank-split",
    );
    bad(
        &format!(r#"{ac},"split_into":[{{"title":"a","oops":1}}],{du}"#),
        "split-key",
    );
    for target in ["t", "nope", "alien"] {
        bad(
            &format!(r#"{ac},{sp},"duplicate_of":[{{"id":"{target}","reason":"r"}}]"#),
            target,
        );
    }
    bad(
        &format!(
            r#"{ac},{sp},"duplicate_of":[{{"id":"twin","reason":"r"}},{{"id":"twin","reason":"r"}}]"#
        ),
        "dup-dup",
    );
    bad(
        &format!(r#"{ac},{sp},{du},"grounding":"text""#),
        "grounding",
    );
    let many: Vec<String> = (0..33).map(|i| format!(r#""c{i}""#)).collect();
    bad(
        &format!(r#""acceptance_criteria":[{}],{sp},{du}"#, many.join(",")),
        "oversized",
    );
    // A deleted twin is no live duplicate.
    call(
        &s,
        "items.delete",
        r#"{"id":"twin","request_id":"del","expected_revision":1}"#,
    );
    bad(
        &format!(r#"{ac},{sp},"duplicate_of":[{{"id":"twin","reason":"r"}}]"#),
        "deleted",
    );
    // After all that the proposal is still pending and completes cleanly.
    let ok = call(
        &s,
        "enhancements.complete",
        &format!(r#"{{"id":"e","request_id":"good","expected_revision":1,{ac},{sp},{du}}}"#),
    );
    assert_eq!(json::<String>(&s, &ok, "$.item.state"), "ready");
}

#[test]
fn accept_guards_the_new_task_identifiers_and_the_proposal_kind() {
    let s = fixture();
    ready(&s, "e");
    for (name, extra) in [
        ("none", ""),
        ("short", r#","split_ids":["s1"]"#),
        ("same", r#","split_ids":["s1","s1"]"#),
        ("bad-char", r#","split_ids":["s 1","s2"]"#),
    ] {
        assert_eq!(
            accept(&s, "e", ALL, 1, extra).unwrap_err(),
            "invalid_input",
            "{name}"
        );
    }
    assert_eq!(
        accept(&s, "e", ALL, 1, r#","split_ids":["twin","s2"]"#).unwrap_err(),
        "id_taken"
    );
    assert_eq!(
        accept(&s, "e", r#"["duplicate_of"]"#, 1, r#","split_ids":["s1"]"#).unwrap_err(),
        "invalid_input"
    );
    refuse(
        &s,
        "enhancements.revise",
        r#"{"id":"e","request_id":"rev","expected_revision":2,"title":"x"}"#,
        "invalid_transition",
    );
    // A refinement cannot be automatic.
    let s = fixture();
    refuse(
        &s,
        "enhancements.create",
        r#"{"id":"x","request_id":"cx","expected_revision":0,"task_id":"t","source_revision":1,"kind":"refine","fields":["split_into"],"provider":"local","model":"m","automatic":true}"#,
        "invalid_input",
    );
    refuse(
        &s,
        "enhancements.create",
        r#"{"id":"x","request_id":"cy","expected_revision":0,"task_id":"t","source_revision":1,"kind":"refine","fields":["title"],"provider":"local","model":"m"}"#,
        "invalid_input",
    );
}

#[test]
fn a_split_cannot_be_undone_and_text_proposals_still_work() {
    let s = fixture();
    ready(&s, "e");
    accept(&s, "e", r#"["duplicate_of"]"#, 1, "").unwrap();
    refuse(
        &s,
        "enhancements.undo",
        r#"{"id":"e","request_id":"u","expected_revision":3,"expected_task_revision":2}"#,
        "invalid_transition",
    );
    call(
        &s,
        "enhancements.create",
        r#"{"id":"txt","request_id":"ct","expected_revision":0,"task_id":"t","source_revision":2,"fields":["title"],"provider":"local","model":"m"}"#,
    );
    refuse(
        &s,
        "enhancements.complete",
        r#"{"id":"txt","request_id":"cc","expected_revision":1,"title":"x","split_into":[]}"#,
        "invalid_input",
    );
}

#[test]
fn schema_twelve_upgrades_eleven_and_refuses_what_is_newer() {
    let s = fixture();
    s.connection()
        .execute_batch("UPDATE work_meta SET version=11 WHERE id=1")
        .unwrap();
    // An upgrade spends no revision and keeps every existing proposal's shape.
    let t = call(&s, "items.get", r#"{"id":"t"}"#);
    assert_eq!(json::<i64>(&s, &t, "$.item.revision"), 1);
    let version = || -> i64 {
        s.connection()
            .query_row("SELECT version FROM work_meta WHERE id=1", [], |r| r.get(0))
            .unwrap()
    };
    // The rung is a hard fence: the stored version is the one a build that
    // knows only schema 11 compares with `!=` and refuses.
    assert_eq!(version(), 12);
    assert_eq!(dc_store::workbench::WORKBENCH_SCHEMA, 12);
    s.connection()
        .execute_batch("UPDATE work_meta SET version=13 WHERE id=1")
        .unwrap();
    refuse(&s, "enhancements.list", "{}", "schema_unsupported");
}

#[test]
fn a_claimed_refinement_is_refused_once_its_field_is_locked_and_three_locks_still_brief() {
    let s = fixture();
    create(&s, "e", 1);
    call(
        &s,
        "items.put",
        r#"{"id":"t","request_id":"lock3","expected_revision":1,"title":"t title","description":"d","repository_ids":["r"],"primary_repository_id":"r","acceptance_criteria":["Original criterion"],"locked_fields":["title","description","acceptance_criteria"]}"#,
    );
    refuse(
        &s,
        "enhancements.claim",
        r#"{"id":"e","request_id":"claim","expected_revision":1,"worker_id":"w"}"#,
        "field_locked",
    );
    let brief = call(&s, "items.brief.get", r#"{"id":"t","expected_revision":2}"#);
    assert!(
        brief.contains("title, description, acceptance_criteria"),
        "{brief}"
    );
}
