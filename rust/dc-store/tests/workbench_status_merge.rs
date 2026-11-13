//! The Inbox status is Backlog (schema 16). A request that still says `inbox`
//! is read as `backlog`; nothing the store keeps or returns says it again; and a
//! profile written by a schema 15 host is rewritten on upgrade.

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
fn refuse(s: &Store, method: &str, input: &str, code: &str) {
    match s.workbench_request(method, input) {
        Ok(ok) => panic!("expected {method} {input} to fail, got {ok}"),
        Err(e) => assert_eq!(e.code, code, "{method} {input}: {}", e.message),
    }
}
fn text(s: &Store, raw: &str, path: &str) -> String {
    s.connection()
        .query_row("SELECT json_extract(?1,?2)", [raw, path], |r| r.get(0))
        .unwrap()
}
fn number(s: &Store, raw: &str, path: &str) -> i64 {
    s.connection()
        .query_row("SELECT json_extract(?1,?2)", [raw, path], |r| r.get(0))
        .unwrap()
}
fn version(s: &Store) -> i64 {
    s.connection()
        .query_row("SELECT version FROM work_meta WHERE id=1", [], |r| r.get(0))
        .unwrap()
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
fn put(s: &Store, id: &str, revision: i64, extra: &str) -> String {
    call(
        s,
        "items.put",
        &format!(
            r#"{{"id":"{id}","request_id":"{id}-{revision}","expected_revision":{revision},"title":"Task {id}","repository_ids":["r"],"primary_repository_id":"r"{extra}}}"#
        ),
    )
}

#[test]
fn a_put_that_says_inbox_stores_and_returns_backlog() {
    let (s, _) = fixture();
    let saved = put(&s, "a", 0, r#","status":"inbox""#);
    assert_eq!(text(&s, &saved, "$.item.status"), "backlog");
    let got = call(&s, "items.get", r#"{"id":"a"}"#);
    assert_eq!(text(&s, &got, "$.item.status"), "backlog");
    let stored: String = s
        .connection()
        .query_row("SELECT status FROM work_items WHERE id='a'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(stored, "backlog");
    // The event and the revision hold the item as it was stored.
    let events = call(&s, "events.list", "{}");
    assert!(!events.contains("inbox"), "{events}");
    let history: String = s
        .connection()
        .query_row(
            "SELECT group_concat(body) FROM work_revisions WHERE entity_id='a'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!history.contains("inbox"), "{history}");
}

#[test]
fn a_retry_of_a_put_that_said_inbox_replays_the_stored_answer() {
    let (s, _) = fixture();
    let first = put(&s, "a", 0, r#","status":"inbox""#);
    let again = put(&s, "a", 0, r#","status":"inbox""#);
    assert_eq!(first, again);
    assert_eq!(text(&s, &again, "$.item.status"), "backlog");
}

#[test]
fn a_put_without_a_status_defaults_to_backlog() {
    let (s, _) = fixture();
    let saved = put(&s, "a", 0, "");
    assert_eq!(text(&s, &saved, "$.item.status"), "backlog");
}

#[test]
fn listing_by_inbox_returns_the_backlog_items() {
    let (s, _) = fixture();
    put(&s, "a", 0, r#","status":"inbox","position":1"#);
    put(&s, "b", 0, r#","status":"backlog","position":2"#);
    put(&s, "c", 0, r#","status":"ready","position":3"#);
    let by_alias = call(&s, "items.list", r#"{"status":"inbox"}"#);
    let by_name = call(&s, "items.list", r#"{"status":"backlog"}"#);
    assert_eq!(ids(&s, &by_alias), ["a", "b"]);
    assert_eq!(by_alias, by_name);
    assert!(!by_alias.contains("inbox"), "{by_alias}");
}

#[test]
fn an_unknown_status_is_still_refused() {
    let (s, _) = fixture();
    // The alias is the exact lowercase word: no trimming, no case folding.
    for status in ["triage", "Inbox", "INBOX", " inbox", "inbox "] {
        refuse(
            &s,
            "items.put",
            &format!(
                r#"{{"id":"x","request_id":"x","expected_revision":0,"title":"x","status":"{status}","repository_ids":["r"],"primary_repository_id":"r"}}"#
            ),
            "invalid_input",
        );
        refuse(
            &s,
            "items.list",
            &format!(r#"{{"status":"{status}"}}"#),
            "invalid_input",
        );
    }
}

/// Rewrites what a schema 15 host would have stored for the tasks in `inbox`:
/// the status in the body, in every revision, in the events that carry the
/// body, and in a receipt written before schema 13 (which keeps its whole
/// response); then rolls the version back to 15.
fn downgrade_to_v15(s: &Store, inbox: &[&str]) {
    let list = inbox
        .iter()
        .map(|id| format!("'{id}'"))
        .collect::<Vec<_>>()
        .join(",");
    s.connection()
        .execute_batch(&format!(
            "UPDATE work_items SET body=json_set(body,'$.status','inbox') WHERE id IN ({list});
             UPDATE work_revisions SET body=json_set(body,'$.status','inbox')
              WHERE entity_type='item' AND entity_id IN ({list});
             UPDATE work_events SET payload=json_set(payload,'$.item.status','inbox')
              WHERE kind LIKE 'items.%' AND entity_id IN ({list});
             UPDATE work_revisions SET body=json_set(body,'$.source.status','inbox')
              WHERE entity_type='enhancement' AND json_extract(body,'$.task_id') IN ({list});
             UPDATE work_enhancements SET body=json_set(body,'$.source.status','inbox')
              WHERE task_id IN ({list});
             UPDATE work_events SET payload=json_set(payload,'$.item.source.status','inbox')
              WHERE kind LIKE 'enhancements.%' AND json_extract(payload,'$.item.task_id') IN ({list});
             UPDATE work_requests SET response=(SELECT json_set(e.payload,'$.ok',json('true'),'$.sequence',e.sequence)
                                                  FROM work_events e WHERE e.sequence=work_requests.sequence)
              WHERE id IN ('full-receipt');
             UPDATE work_meta SET version=15 WHERE id=1;"
        ))
        .unwrap();
}

fn status_everywhere(s: &Store) -> Vec<(String, String)> {
    // Every text value in the profile that is exactly the word, outside the
    // request inputs and the immutable run briefs.
    let mut found = Vec::new();
    for (table, column) in [
        ("work_items", "body"),
        ("work_revisions", "body"),
        ("work_events", "payload"),
        ("work_requests", "response"),
        ("work_enhancements", "body"),
    ] {
        let mut stmt = s
            .connection()
            .prepare(&format!(
                "SELECT rowid FROM {table} WHERE {column} LIKE '%inbox%'"
            ))
            .unwrap();
        for row in stmt.query_map([], |r| r.get::<_, i64>(0)).unwrap() {
            found.push((table.to_string(), row.unwrap().to_string()));
        }
    }
    found
}

#[test]
fn a_schema_15_profile_with_inbox_tasks_is_rewritten_to_backlog() {
    let (s, clock) = fixture();
    // Backlog first, then the board's other columns, so the order is not an
    // accident of insertion: position is global, not per column.
    put(&s, "back", 0, r#","status":"backlog","position":5"#);
    put(&s, "ready", 0, r#","status":"ready","position":3"#);
    put(&s, "live", 0, r#","status":"backlog","position":1"#);
    put(
        &s,
        "arch",
        0,
        r#","status":"backlog","position":2,"archived":true"#,
    );
    put(&s, "gone", 0, r#","status":"backlog","position":4"#);
    call(
        &s,
        "items.delete",
        r#"{"id":"gone","request_id":"del","expected_revision":1}"#,
    );
    // Several revisions of history for one task.
    put(&s, "many", 0, r#","status":"backlog","position":6"#);
    for revision in 1..=3 {
        clock.fetch_add(10, Ordering::SeqCst);
        put(
            &s,
            "many",
            revision,
            &format!(r#","status":"backlog","position":6,"description":"edit {revision}""#),
        );
    }
    // A receipt written before schema 13 keeps its whole response.
    s.workbench_request(
        "items.put",
        r#"{"id":"live","request_id":"full-receipt","expected_revision":1,"title":"Task live","repository_ids":["r"],"primary_repository_id":"r","status":"backlog","position":1,"description":"d"}"#,
    )
    .unwrap();
    // An enhancement proposal snapshots the whole task as its `source`.
    call(
        &s,
        "enhancements.create",
        r#"{"id":"proposal","request_id":"create-proposal","expected_revision":0,"task_id":"many","source_revision":4,"fields":["title"],"provider":"local","model":"m"}"#,
    );
    let before_rows: Vec<(String, i64, String)> = {
        let mut stmt = s
            .connection()
            .prepare(
                "SELECT id,revision,json_extract(body,'$.updated_at') FROM work_items ORDER BY id",
            )
            .unwrap();
        stmt.query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)?.to_string()))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
    };
    let positions = |s: &Store| -> Vec<(String, i64)> {
        let mut stmt = s
            .connection()
            .prepare("SELECT id,position FROM work_items ORDER BY id")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    let before_positions = positions(&s);

    downgrade_to_v15(&s, &["live", "arch", "gone", "many"]);
    assert_eq!(version(&s), 15);
    assert!(
        !status_everywhere(&s).is_empty(),
        "the fixture must really hold inbox tasks"
    );
    let planted: i64 = s
        .connection()
        .query_row(
            "SELECT count(*) FROM work_items WHERE status='inbox'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(planted, 4, "live, archived, deleted and the long history");

    clock.store(9000, Ordering::SeqCst);
    s.upgrade_workbench_profile().unwrap();

    assert_eq!(version(&s), 16);
    assert_eq!(version(&s), dc_store::workbench::WORKBENCH_SCHEMA);
    assert_eq!(status_everywhere(&s), Vec::<(String, String)>::new());
    for (id, want) in [
        ("live", "backlog"),
        ("arch", "backlog"),
        ("gone", "backlog"),
        ("many", "backlog"),
        ("back", "backlog"),
        ("ready", "ready"),
    ] {
        let got: String = s
            .connection()
            .query_row("SELECT status FROM work_items WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(got, want, "{id}");
    }
    // Revision numbers, update times and positions are what they were: the
    // rewrite restates the status, it is not an edit.
    let after_rows: Vec<(String, i64, String)> = {
        let mut stmt = s
            .connection()
            .prepare(
                "SELECT id,revision,json_extract(body,'$.updated_at') FROM work_items ORDER BY id",
            )
            .unwrap();
        stmt.query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)?.to_string()))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
    };
    assert_eq!(after_rows, before_rows);
    assert_eq!(positions(&s), before_positions);
    // The board's column holds the former inbox tasks and the backlog ones in
    // the order their global positions give.
    let column = call(&s, "items.list", r#"{"status":"backlog","archived":false}"#);
    assert_eq!(ids(&s, &column), ["live", "back", "many"]);
    // History is still readable, one revision per save, all of it backlog.
    let history: i64 = s
        .connection()
        .query_row(
            "SELECT count(*) FROM work_revisions WHERE entity_type='item' AND entity_id='many'
               AND json_extract(body,'$.status')='backlog'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(history, 4);
    let series = call(&s, "items.history", r#"{"id":"many"}"#);
    assert!(!series.contains("inbox"), "{series}");
    let revisions: Vec<i64> = {
        let mut stmt = s
            .connection()
            .prepare("SELECT json_extract(value,'$.revision') FROM json_each(?1,'$.items')")
            .unwrap();
        stmt.query_map([&series], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    assert_eq!(revisions, [1, 2, 3, 4], "{series}");
    let events = call(&s, "events.list", "{}");
    assert!(!events.contains("inbox"), "{events}");
    // The proposal's snapshot, the receipt's whole response and a retry of the
    // earlier request all read backlog.
    let proposal = call(&s, "enhancements.get", r#"{"id":"proposal"}"#);
    assert_eq!(text(&s, &proposal, "$.item.source.status"), "backlog");
    let receipt = call(
        &s,
        "items.put",
        r#"{"id":"live","request_id":"full-receipt","expected_revision":1,"title":"Task live","repository_ids":["r"],"primary_repository_id":"r","status":"backlog","position":1,"description":"d"}"#,
    );
    assert_eq!(text(&s, &receipt, "$.item.status"), "backlog");
    assert_eq!(number(&s, &receipt, "$.item.revision"), 2);
    // A write through the upgraded profile is an ordinary one.
    let next = put(
        &s,
        "many",
        4,
        r#","status":"ready","position":6,"description":"after""#,
    );
    assert_eq!(text(&s, &next, "$.item.status"), "ready");
}

#[test]
fn an_idle_profile_without_inbox_tasks_upgrades_untouched() {
    let (s, _) = fixture();
    put(&s, "a", 0, r#","status":"ready""#);
    let before: String = s
        .connection()
        .query_row("SELECT body FROM work_items WHERE id='a'", [], |r| r.get(0))
        .unwrap();
    s.connection()
        .execute_batch("UPDATE work_meta SET version=15 WHERE id=1")
        .unwrap();
    s.upgrade_workbench_profile().unwrap();
    assert_eq!(version(&s), 16);
    let after: String = s
        .connection()
        .query_row("SELECT body FROM work_items WHERE id='a'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(before, after);
}
