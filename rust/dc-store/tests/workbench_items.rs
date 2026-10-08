//! Schema 11 task fields: the archive flag, completion time, checklists, task
//! links and restoring a deleted task — and the migration from schema 10.

use dc_store::Store;
use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};

fn fixture() -> (Store, Arc<AtomicI64>) {
    let mut s = Store::open_in_memory().unwrap();
    let clock = Arc::new(AtomicI64::new(1000));
    let time = clock.clone();
    s.set_clock(move || time.load(Ordering::SeqCst));
    call(
        &s,
        "repositories.put",
        r#"{"id":"r","request_id":"repo","expected_revision":0,"name":"Repository","identity_key":"local:/checkout/.git"}"#,
    );
    (s, clock)
}
fn call(s: &Store, method: &str, input: &str) -> String {
    s.workbench_request(method, input)
        .unwrap_or_else(|e| panic!("{method} {input}: {e}"))
}
fn refuse(s: &Store, method: &str, input: &str, code: &str) -> String {
    match s.workbench_request(method, input) {
        Ok(ok) => panic!("expected {method} {input} to fail, got {ok}"),
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
fn version(s: &Store) -> i64 {
    s.connection()
        .query_row("SELECT version FROM work_meta WHERE id=1", [], |r| r.get(0))
        .unwrap()
}
/// Save task `id` at `revision` with `extra` request fields.
fn put(s: &Store, id: &str, revision: i64, extra: &str) -> String {
    call(
        s,
        "items.put",
        &format!(
            r#"{{"id":"{id}","request_id":"{id}-{revision}-{}","expected_revision":{revision},"title":"Task {id}","repository_ids":["r"],"primary_repository_id":"r"{extra}}}"#,
            extra.len()
        ),
    )
}
fn get(s: &Store, id: &str) -> String {
    call(s, "items.get", &format!(r#"{{"id":"{id}"}}"#))
}
fn brief(s: &Store, id: &str) -> String {
    let revision: i64 = json(s, &get(s, id), "$.item.revision");
    let raw = call(
        s,
        "items.brief.get",
        &format!(r#"{{"id":"{id}","expected_revision":{revision}}}"#),
    );
    json(s, &raw, "$.item.markdown")
}
fn ids(s: &Store, page: &str) -> Vec<String> {
    let mut stmt = s
        .connection()
        .prepare("SELECT json_extract(value,'$.id') FROM json_each(?1,'$.items')")
        .unwrap();
    stmt.query_map([page], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

/// Undo schema 11 on a store, leaving the shape a schema 10 host wrote:
/// no columns, no link table, and no new keys in any body or revision.
fn downgrade_to_v10(s: &Store) {
    s.connection()
        .execute_batch(
            "DROP TABLE work_item_links; DROP INDEX work_items_archive; DROP INDEX work_items_completed;
             ALTER TABLE work_items DROP COLUMN archived; ALTER TABLE work_items DROP COLUMN completed_at;
             UPDATE work_items SET body=json_remove(body,'$.archived','$.completed_at','$.checklist','$.links');
             UPDATE work_revisions SET body=json_remove(body,'$.archived','$.completed_at','$.checklist','$.links') WHERE entity_type='item';
             UPDATE work_meta SET version=10 WHERE id=1;",
        )
        .unwrap();
}

#[test]
fn schema_10_profiles_migrate_with_done_archived_and_completion_recovered_from_history() {
    let (s, clock) = fixture();
    // `streak`: Done at 2000, reopened at 3000, Done again at 4000 and edited
    // at 5000 while Done. Its completion is 4000: the start of the current
    // streak, not the first Done and not the last edit.
    put(&s, "streak", 0, "");
    clock.store(2000, Ordering::SeqCst);
    put(&s, "streak", 1, r#","status":"done""#);
    clock.store(3000, Ordering::SeqCst);
    put(&s, "streak", 2, r#","status":"ready""#);
    clock.store(4000, Ordering::SeqCst);
    put(&s, "streak", 3, r#","status":"done""#);
    clock.store(5000, Ordering::SeqCst);
    call(
        &s,
        "items.put",
        r#"{"id":"streak","request_id":"edit","expected_revision":4,"title":"Edited while done","status":"done","repository_ids":["r"],"primary_repository_id":"r"}"#,
    );
    // Created Done: completed when it was created.
    clock.store(6000, Ordering::SeqCst);
    put(&s, "born", 0, r#","status":"done""#);
    put(&s, "open", 0, r#","status":"ready""#);
    put(&s, "gone", 0, r#","status":"done""#);
    call(
        &s,
        "items.delete",
        r#"{"id":"gone","request_id":"del","expected_revision":1}"#,
    );
    downgrade_to_v10(&s);
    assert_eq!(version(&s), 10);
    let revisions: i64 = s
        .connection()
        .query_row("SELECT sum(revision) FROM work_items", [], |r| r.get(0))
        .unwrap();

    clock.store(9000, Ordering::SeqCst);
    let streak = get(&s, "streak");
    assert_eq!(version(&s), 11);
    assert_eq!(json::<i64>(&s, &streak, "$.item.archived"), 1);
    assert_eq!(json::<i64>(&s, &streak, "$.item.completed_at"), 4000);
    let born = get(&s, "born");
    assert_eq!(json::<i64>(&s, &born, "$.item.archived"), 1);
    assert_eq!(json::<i64>(&s, &born, "$.item.completed_at"), 6000);
    let open = get(&s, "open");
    assert_eq!(json::<Option<i64>>(&s, &open, "$.item.archived"), None);
    assert_eq!(json::<Option<i64>>(&s, &open, "$.item.completed_at"), None);
    // The migration restates what each row meant; it spends no revision, so
    // a host holding a revision from before the upgrade can still save.
    let after: i64 = s
        .connection()
        .query_row("SELECT sum(revision) FROM work_items", [], |r| r.get(0))
        .unwrap();
    assert_eq!(after, revisions);
    // A deleted Done task is archived too, so restoring it lands in the archive.
    let deleted: (i64, i64) = s
        .connection()
        .query_row(
            "SELECT archived,deleted FROM work_items WHERE id='gone'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(deleted, (1, 1));

    // The archive reads every migrated Done task; the board's Done column none.
    let archive = call(&s, "items.list", r#"{"archived":true}"#);
    assert_eq!(json::<i64>(&s, &archive, "$.total"), 2);
    let done_column = call(&s, "items.list", r#"{"status":"done","archived":false}"#);
    assert_eq!(json::<i64>(&s, &done_column, "$.total"), 0);

    // An existing write path keeps working against the migrated row, and a
    // host that omits `archived` does not unarchive it.
    let saved = call(
        &s,
        "items.put",
        r#"{"id":"born","request_id":"after","expected_revision":1,"title":"Saved after upgrade","status":"done","repository_ids":["r"],"primary_repository_id":"r"}"#,
    );
    assert_eq!(json::<i64>(&s, &saved, "$.item.archived"), 1);
    assert_eq!(json::<i64>(&s, &saved, "$.item.completed_at"), 6000);
}

#[test]
fn a_profile_newer_than_this_build_is_refused() {
    let (s, _) = fixture();
    s.connection()
        .execute_batch("UPDATE work_meta SET version=12 WHERE id=1")
        .unwrap();
    refuse(&s, "items.list", "{}", "schema_unsupported");
}

#[test]
fn archived_is_independent_of_status_and_survives_edits_that_omit_it() {
    let (s, _) = fixture();
    put(&s, "t", 0, r#","status":"done""#);
    // New work is not archived by finishing it: Done stays on the board.
    assert_eq!(json::<i64>(&s, &get(&s, "t"), "$.item.archived"), 0);
    put(&s, "t", 1, r#","status":"done","archived":true"#);
    // An edit that does not name the flag keeps it — a status move by an agent
    // must not pull a task out of the archive.
    put(&s, "t", 2, r#","status":"review""#);
    let task = get(&s, "t");
    assert_eq!(json::<i64>(&s, &task, "$.item.archived"), 1);
    assert_eq!(json::<String>(&s, &task, "$.item.status"), "review");
    // Restoring from the archive keeps the status the task has.
    put(&s, "t", 3, r#","status":"review","archived":false"#);
    let task = get(&s, "t");
    assert_eq!(json::<i64>(&s, &task, "$.item.archived"), 0);
    assert_eq!(json::<String>(&s, &task, "$.item.status"), "review");
    // Archived straight out of Ready, without completing it.
    put(&s, "u", 0, r#","status":"ready","archived":true"#);

    let archive = call(&s, "items.list", r#"{"archived":true}"#);
    assert_eq!(ids(&s, &archive), ["u"]);
    let board = call(&s, "items.list", r#"{"archived":false}"#);
    assert_eq!(ids(&s, &board), ["t"]);
    let both = call(&s, "items.list", "{}");
    assert_eq!(json::<i64>(&s, &both, "$.total"), 2);
    let ready_archived = call(&s, "items.list", r#"{"status":"ready","archived":true}"#);
    assert_eq!(json::<i64>(&s, &ready_archived, "$.total"), 1);
    // Scoped and searched lists apply the same filter.
    call(
        &s,
        "workspaces.put",
        r#"{"id":"w","request_id":"w","expected_revision":0,"name":"W","repository_ids":["r"]}"#,
    );
    let scoped = call(&s, "items.list", r#"{"workspace_id":"w","archived":true}"#);
    assert_eq!(ids(&s, &scoped), ["u"]);
    let searched = call(&s, "items.list", r#"{"query":"Task","archived":false}"#);
    assert_eq!(json::<i64>(&s, &searched, "$.total"), 1);
    assert_eq!(ids(&s, &searched), ["t"]);

    refuse(
        &s,
        "items.put",
        r#"{"id":"t","request_id":"bad","expected_revision":4,"title":"T","repository_ids":["r"],"primary_repository_id":"r","archived":"yes"}"#,
        "invalid_input",
    );
    refuse(&s, "items.list", r#"{"archived":1}"#, "invalid_input");
}

#[test]
fn completion_time_is_the_stores_and_follows_the_done_status() {
    let (s, clock) = fixture();
    put(&s, "t", 0, "");
    assert_eq!(
        json::<Option<i64>>(&s, &get(&s, "t"), "$.item.completed_at"),
        None
    );
    clock.store(2000, Ordering::SeqCst);
    put(&s, "t", 1, r#","status":"done""#);
    assert_eq!(json::<i64>(&s, &get(&s, "t"), "$.item.completed_at"), 2000);
    // Editing a Done task does not move its completion.
    clock.store(3000, Ordering::SeqCst);
    put(&s, "t", 2, r#","status":"done","description":"more""#);
    assert_eq!(json::<i64>(&s, &get(&s, "t"), "$.item.completed_at"), 2000);
    // Leaving Done clears it; finishing again records the new time.
    put(&s, "t", 3, r#","status":"in_progress""#);
    assert_eq!(
        json::<Option<i64>>(&s, &get(&s, "t"), "$.item.completed_at"),
        None
    );
    clock.store(4000, Ordering::SeqCst);
    put(&s, "t", 4, r#","status":"done""#);
    assert_eq!(json::<i64>(&s, &get(&s, "t"), "$.item.completed_at"), 4000);
    // A request cannot set it: a host clock never back-dates a completion.
    let message = refuse(
        &s,
        "items.put",
        r#"{"id":"t","request_id":"forge","expected_revision":5,"title":"T","status":"done","repository_ids":["r"],"primary_repository_id":"r","completed_at":1}"#,
        "invalid_input",
    );
    assert!(message.contains("completed_at"), "{message}");
}

#[test]
fn the_archive_reads_most_recently_completed_first_and_pages_resume_in_that_order() {
    let (s, clock) = fixture();
    for (id, at) in [("a", 3000), ("b", 1000), ("c", 2000), ("d", 2000)] {
        clock.store(at, Ordering::SeqCst);
        put(&s, id, 0, r#","status":"done","archived":true"#);
    }
    // Archived without ever finishing: completion 0, so it reads last.
    put(&s, "e", 0, r#","status":"ready","archived":true"#);
    let all = call(&s, "items.list", r#"{"archived":true,"order":"completed"}"#);
    // Ties on time fall back to id, descending like the time.
    assert_eq!(ids(&s, &all), ["a", "d", "c", "b", "e"]);

    // Page by one and collect: no task is skipped or repeated, and every
    // cursor is the completion order's own.
    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let input = match &cursor {
            Some(c) => {
                format!(r#"{{"archived":true,"order":"completed","limit":2,"cursor":"{c}"}}"#)
            }
            None => r#"{"archived":true,"order":"completed","limit":2}"#.to_string(),
        };
        let page = call(&s, "items.list", &input);
        assert_eq!(json::<i64>(&s, &page, "$.total"), 5);
        seen.extend(ids(&s, &page));
        cursor = json(&s, &page, "$.next_cursor");
        match &cursor {
            Some(c) => assert!(c.starts_with("2:"), "{c}"),
            None => break,
        }
    }
    assert_eq!(seen, ["a", "d", "c", "b", "e"]);

    // Re-reading a page from the cursor it was fetched with returns that page
    // as it is now — the in-place refresh the archive needs.
    let first = call(
        &s,
        "items.list",
        r#"{"archived":true,"order":"completed","limit":2}"#,
    );
    let resume: String = json(&s, &first, "$.next_cursor");
    put(
        &s,
        "c",
        1,
        r#","status":"done","archived":true,"description":"edited""#,
    );
    let again = call(
        &s,
        "items.list",
        &format!(r#"{{"archived":true,"order":"completed","limit":2,"cursor":"{resume}"}}"#),
    );
    assert_eq!(ids(&s, &again), ["c", "b"]);

    // Workspace scope goes through the scoped query and orders the same way.
    call(
        &s,
        "workspaces.put",
        r#"{"id":"w","request_id":"w","expected_revision":0,"name":"W","repository_ids":["r"]}"#,
    );
    let scoped = call(
        &s,
        "items.list",
        r#"{"workspace_id":"w","archived":true,"order":"completed","limit":3}"#,
    );
    assert_eq!(ids(&s, &scoped), ["a", "d", "c"]);
    assert_eq!(json::<String>(&s, &scoped, "$.next_cursor"), "2:2000:c");

    // A cursor belongs to its order.
    refuse(
        &s,
        "items.list",
        r#"{"archived":true,"order":"completed","cursor":"1:0:a"}"#,
        "invalid_input",
    );
    refuse(&s, "items.list", r#"{"cursor":"2:0:a"}"#, "invalid_input");
    refuse(&s, "items.list", r#"{"order":"newest"}"#, "invalid_input");
}

#[test]
fn checklists_round_trip_survive_omission_and_reach_the_brief_but_not_cards() {
    let (s, _) = fixture();
    put(
        &s,
        "t",
        0,
        r#","checklist":[{"text":"Write the migration","done":true},{"text":"Re-vendor","done":false}]"#,
    );
    let task = get(&s, "t");
    assert_eq!(
        json::<String>(&s, &task, "$.item.checklist[1].text"),
        "Re-vendor"
    );
    assert_eq!(json::<i64>(&s, &task, "$.item.checklist[0].done"), 1);
    // A save that does not name the checklist keeps it.
    put(&s, "t", 1, r#","status":"ready""#);
    assert_eq!(
        json::<i64>(&s, &get(&s, "t"), "$.item.checklist[0].done"),
        1
    );
    let markdown = brief(&s, "t");
    assert!(
        markdown
            .contains("## Checklist\n1 of 2 done.\n- [x] Write the migration\n- [ ] Re-vendor\n"),
        "{markdown}"
    );
    // Cards stay small; the sheet reads the task.
    let cards = call(&s, "items.list", "{}");
    assert_eq!(
        json::<Option<String>>(&s, &cards, "$.items[0].checklist"),
        None
    );
    // An explicit empty list clears it, and the brief drops the section.
    put(&s, "t", 2, r#","checklist":[]"#);
    assert!(!brief(&s, "t").contains("## Checklist"));

    for bad in [
        r#","checklist":"one""#,
        r#","checklist":["one"]"#,
        r#","checklist":[{"text":"x"}]"#,
        r#","checklist":[{"text":"x","done":1}]"#,
        r#","checklist":[{"text":"  ","done":false}]"#,
        r#","checklist":[{"text":"x","done":false,"owner":"me"}]"#,
    ] {
        refuse(
            &s,
            "items.put",
            &format!(
                r#"{{"id":"t","request_id":"bad","expected_revision":3,"title":"T","repository_ids":["r"],"primary_repository_id":"r"{bad}}}"#
            ),
            "invalid_input",
        );
    }
    let many = (0..129)
        .map(|n| format!(r#"{{"text":"step {n}","done":false}}"#))
        .collect::<Vec<_>>()
        .join(",");
    refuse(
        &s,
        "items.put",
        &format!(
            r#"{{"id":"t","request_id":"many","expected_revision":3,"title":"T","repository_ids":["r"],"primary_repository_id":"r","checklist":[{many}]}}"#
        ),
        "invalid_input",
    );
}

#[test]
fn links_are_validated_kept_on_omission_and_read_from_both_ends() {
    let (s, _) = fixture();
    for id in ["epic", "a", "b", "dup"] {
        put(&s, id, 0, "");
    }
    put(
        &s,
        "a",
        1,
        r#","links":[{"kind":"parent","item_id":"epic"},{"kind":"blocks","item_id":"b"},{"kind":"related","item_id":"dup"}]"#,
    );
    put(
        &s,
        "dup",
        1,
        r#","links":[{"kind":"duplicate_of","item_id":"a"}]"#,
    );
    let task = get(&s, "a");
    assert_eq!(json::<String>(&s, &task, "$.item.links[1].kind"), "blocks");
    // Omitted links are kept, in the body and in the index the other end reads.
    put(&s, "a", 2, r#","status":"ready""#);
    assert_eq!(
        json::<String>(&s, &get(&s, "a"), "$.item.links[0].item_id"),
        "epic"
    );

    let markdown = brief(&s, "a");
    assert!(markdown.contains("## Linked tasks\n"), "{markdown}");
    assert!(
        markdown.contains("- Parent: Task epic [epic] (inbox)\n"),
        "{markdown}"
    );
    assert!(
        markdown.contains("- Blocks: Task b [b] (inbox)\n"),
        "{markdown}"
    );
    assert!(
        markdown.contains("- Duplicated by: Task dup [dup] (inbox)\n"),
        "{markdown}"
    );
    let blocked = brief(&s, "b");
    assert!(
        blocked.contains("- Blocked by: Task a [a] (ready)\n"),
        "{blocked}"
    );
    let epic = brief(&s, "epic");
    assert!(epic.contains("- Subtask: Task a [a] (ready)\n"), "{epic}");

    // A parent may not be its own descendant, at any depth.
    let message = refuse(
        &s,
        "items.put",
        r#"{"id":"epic","request_id":"cycle","expected_revision":1,"title":"Epic","repository_ids":["r"],"primary_repository_id":"r","links":[{"kind":"parent","item_id":"a"}]}"#,
        "invalid_input",
    );
    assert!(message.contains("subtask"), "{message}");
    put(&s, "b", 1, r#","links":[{"kind":"parent","item_id":"a"}]"#);
    refuse(
        &s,
        "items.put",
        r#"{"id":"epic","request_id":"cycle2","expected_revision":1,"title":"Epic","repository_ids":["r"],"primary_repository_id":"r","links":[{"kind":"parent","item_id":"b"}]}"#,
        "invalid_input",
    );

    for bad in [
        r#"[{"kind":"parent","item_id":"a"}]"#,
        r#"[{"kind":"child","item_id":"b"}]"#,
        r#"[{"kind":"related","item_id":"missing"}]"#,
        r#"[{"kind":"related","item_id":"b"},{"kind":"related","item_id":"b"}]"#,
        r#"[{"kind":"parent","item_id":"b"},{"kind":"parent","item_id":"epic"}]"#,
        r#"[{"kind":"related","item_id":"b","note":"x"}]"#,
        r#"[{"kind":"related","item_id":"b c"}]"#,
        r#"{"kind":"related","item_id":"b"}"#,
    ] {
        refuse(
            &s,
            "items.put",
            &format!(
                r#"{{"id":"a","request_id":"bad","expected_revision":3,"title":"T","repository_ids":["r"],"primary_repository_id":"r","links":{bad}}}"#
            ),
            "invalid_input",
        );
    }

    // A deleted target cannot be newly linked, and an existing link to it
    // reads as deleted rather than vanishing from the brief.
    call(
        &s,
        "items.delete",
        r#"{"id":"b","request_id":"del-b","expected_revision":2}"#,
    );
    assert!(brief(&s, "a").contains("- Blocks: Task b [b] (deleted)\n"));
    refuse(
        &s,
        "items.put",
        r#"{"id":"dup","request_id":"to-deleted","expected_revision":2,"title":"T","repository_ids":["r"],"primary_repository_id":"r","links":[{"kind":"related","item_id":"b"}]}"#,
        "invalid_input",
    );
    // An explicit empty list unlinks, from both ends.
    put(&s, "a", 3, r#","links":[]"#);
    assert!(!brief(&s, "epic").contains("Subtask"));
    let rows: i64 = s
        .connection()
        .query_row(
            "SELECT count(*) FROM work_item_links WHERE item_id='a'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(rows, 0);
}

#[test]
fn a_deleted_task_is_restored_with_its_fields_and_history() {
    let (s, clock) = fixture();
    put(
        &s,
        "t",
        0,
        r#","status":"done","archived":true,"checklist":[{"text":"x","done":true}]"#,
    );
    clock.store(2000, Ordering::SeqCst);
    call(
        &s,
        "items.delete",
        r#"{"id":"t","request_id":"del","expected_revision":1}"#,
    );
    refuse(&s, "items.get", r#"{"id":"t"}"#, "not_found");
    let reused = refuse(
        &s,
        "items.put",
        r#"{"id":"t","request_id":"reuse","expected_revision":0,"title":"T","repository_ids":["r"],"primary_repository_id":"r"}"#,
        "invalid_input",
    );
    assert!(reused.contains("restore"), "{reused}");
    // Restore is checked against the deleted revision.
    refuse(
        &s,
        "items.restore",
        r#"{"id":"t","request_id":"stale","expected_revision":1}"#,
        "revision_conflict",
    );
    clock.store(3000, Ordering::SeqCst);
    let receipt = call(
        &s,
        "items.restore",
        r#"{"id":"t","request_id":"restore","expected_revision":2}"#,
    );
    assert_eq!(json::<i64>(&s, &receipt, "$.item.revision"), 3);
    assert_eq!(json::<Option<i64>>(&s, &receipt, "$.item.deleted"), None);
    let task = get(&s, "t");
    assert_eq!(json::<i64>(&s, &task, "$.item.archived"), 1);
    assert_eq!(json::<i64>(&s, &task, "$.item.completed_at"), 1000);
    assert_eq!(json::<i64>(&s, &task, "$.item.updated_at"), 3000);
    assert_eq!(json::<i64>(&s, &task, "$.item.checklist[0].done"), 1);
    // The same request replays its receipt; the event and history are recorded.
    assert_eq!(
        call(
            &s,
            "items.restore",
            r#"{"id":"t","request_id":"restore","expected_revision":2}"#
        ),
        receipt
    );
    let history = call(&s, "items.history", r#"{"id":"t"}"#);
    assert_eq!(json::<i64>(&s, &history, "$.total"), 3);
    let events = call(&s, "events.list", "{}");
    let kinds: String = s
        .connection()
        .query_row(
            "SELECT group_concat(json_extract(value,'$.kind')) FROM json_each(?1,'$.items')",
            [&events],
            |r| r.get(0),
        )
        .unwrap();
    assert!(kinds.ends_with("items.delete,items.restore"), "{kinds}");
    // It counts and searches as live again.
    let found = call(&s, "items.list", r#"{"query":"Task"}"#);
    assert_eq!(json::<i64>(&s, &found, "$.total"), 1);
    // Ordinary edits resume at the restored revision.
    put(&s, "t", 3, r#","status":"done""#);

    refuse(
        &s,
        "items.restore",
        r#"{"id":"t","request_id":"live","expected_revision":4}"#,
        "invalid_state",
    );
    refuse(
        &s,
        "items.restore",
        r#"{"id":"missing","request_id":"missing","expected_revision":0}"#,
        "not_found",
    );
    refuse(
        &s,
        "items.restore",
        r#"{"id":"t","request_id":"extra","expected_revision":4,"title":"x"}"#,
        "invalid_state",
    );
}
