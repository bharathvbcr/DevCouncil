//! Measurement for the workbench scale claims: 5,000 repositories and one task
//! with 5,000 revisions. Run it on purpose, in release:
//!
//!   cargo test -p dc-store --release --test workbench_scale_bench -- --ignored --nocapture
//!
//! It prints the before (the paged walks a host had to make) and after (one
//! request) side by side, min of five runs each, plus the receipt bytes.

use dc_store::Store;
use std::time::{Duration, Instant};

#[allow(dead_code)]
mod support;

fn call(s: &Store, method: &str, raw: &str) -> String {
    s.workbench_request(method, raw)
        .unwrap_or_else(|e| panic!("{method}: {e}"))
}
fn field<T: rusqlite::types::FromSql>(s: &Store, raw: &str, path: &str) -> T {
    s.connection()
        .query_row("SELECT json_extract(?1,?2)", [raw, path], |r| r.get(0))
        .unwrap()
}
fn min_of_five(mut f: impl FnMut()) -> Duration {
    (0..5)
        .map(|_| {
            let t = Instant::now();
            f();
            t.elapsed()
        })
        .min()
        .unwrap()
}

#[test]
#[ignore = "benchmark: run with --release --ignored --nocapture"]
fn registry_and_history_lookups_at_5k_by_5k() {
    let db = support::temp_dir("scale-bench").join("state.sqlite");
    let s = Store::open(&db).unwrap();
    for n in 0..5_000 {
        call(
            &s,
            "repositories.put",
            &format!(
                r#"{{"id":"r{n}","request_id":"r{n}","expected_revision":0,"name":"Repo {n}","identity_key":"local:/checkout-{n}/.git"}}"#
            ),
        );
    }
    let body = "z".repeat(4_000);
    for n in 0..5_000 {
        call(
            &s,
            "items.put",
            &format!(
                r#"{{"id":"t","request_id":"t{n}","expected_revision":{n},"title":"Task {n}","description":"{body}","repository_ids":["r0"],"primary_repository_id":"r0"}}"#
            ),
        );
    }
    let identity = "local:/checkout-4999/.git";

    // Before: page repositories.list (200 a page) until the identity matches.
    let mut requests_before = 0;
    let find_before = min_of_five(|| {
        requests_before = 0;
        let mut cursor = String::new();
        loop {
            requests_before += 1;
            let input = if cursor.is_empty() {
                r#"{"limit":200}"#.to_string()
            } else {
                format!(r#"{{"limit":200,"cursor":"{cursor}"}}"#)
            };
            let page = call(&s, "repositories.list", &input);
            let found: i64 = s
                .connection()
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM json_each(?1,'$.items') WHERE json_extract(value,'$.identity_key')=?2)",
                    [&page, identity],
                    |r| r.get(0),
                )
                .unwrap();
            if found == 1 || field::<i64>(&s, &page, "$.has_more") == 0 {
                break;
            }
            cursor = field(&s, &page, "$.next_cursor");
        }
    });
    let find_after = min_of_five(|| {
        call(
            &s,
            "repositories.get",
            &format!(r#"{{"identity_key":"{identity}"}}"#),
        );
    });

    // Before: history ascending from revision 0 until the last page.
    let mut history_requests = 0;
    let last_before = min_of_five(|| {
        history_requests = 0;
        let mut after = 0_i64;
        loop {
            history_requests += 1;
            let page = call(
                &s,
                "items.history",
                &format!(r#"{{"id":"t","limit":200,"after_revision":{after}}}"#),
            );
            if field::<i64>(&s, &page, "$.has_more") == 0 {
                break;
            }
            after = field(&s, &page, "$.next_cursor");
        }
    });
    let last_after = min_of_five(|| {
        call(
            &s,
            "items.history",
            r#"{"id":"t","limit":1,"newest_first":true}"#,
        );
    });

    let receipt_bytes: i64 = s
        .connection()
        .query_row(
            "SELECT CAST(avg(length(response)) AS INTEGER) FROM work_requests WHERE id LIKE 't%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let events_page = min_of_five(|| {
        call(&s, "events.list", r#"{"limit":200}"#);
    });
    let size = std::fs::metadata(&db).unwrap().len();
    println!(
        "find by identity   (5k repos):     before {find_before:?} in {requests_before} requests, after {find_after:?} in 1"
    );
    println!(
        "last revision      (5k revisions): before {last_before:?} in {history_requests} requests, after {last_after:?} in 1"
    );
    println!("events.list page of 200 (10k events, capped count): {events_page:?}");
    println!(
        "average receipt response after: {receipt_bytes} bytes (a 4000-byte body plus envelope before)"
    );
    println!("profile file: {} MiB", size / (1024 * 1024));
}
