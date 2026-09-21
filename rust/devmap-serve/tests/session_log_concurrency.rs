#![cfg(unix)]
//! The live query log is appended by every `devmap mcp` process at once.
//!
//! On this machine eleven daemons were live against one store, and the log
//! they share had torn lines in it: one record's body followed immediately by
//! the next record's body with no newline between them. A ledger that tears
//! under its own normal load is not a ledger, so the guarantee is pinned here.

use devmap_serve::session_log::{append_query, live_log_path, parse_jsonl, read_live};
use serde_json::{json, Map, Value};
use std::fs;
use std::path::PathBuf;

fn scratch(name: &str) -> PathBuf {
    // pid, not a clock: nanosecond stamps collide when these run in parallel.
    let root = std::env::temp_dir().join(format!("devmap-session-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
}

/// Every record survives concurrent appenders intact.
///
/// Against the pre-fix writer this fails: `writeln!` sends the body and the
/// newline as two separate `write` calls, and `O_APPEND` only makes each one
/// atomic on its own, so a second writer lands in the gap between them.
#[test]
fn concurrent_appends_never_tear_a_record() {
    let root = scratch("concurrency");
    let db = root.join("store.sqlite");
    fs::write(&db, "store").unwrap();

    const WRITERS: usize = 8;
    const PER_WRITER: usize = 250;

    std::thread::scope(|scope| {
        for writer in 0..WRITERS {
            let db = db.clone();
            scope.spawn(move || {
                for n in 0..PER_WRITER {
                    append_query(
                        &db,
                        "devmap_search",
                        Some(&json!({"writer": writer, "n": n, "pad": "x".repeat(96)})),
                        Some(&json!({"items": [], "truncated": false})),
                        None,
                        n as u64,
                    );
                }
            });
        }
    });

    let raw = fs::read_to_string(live_log_path(&db)).unwrap();
    let torn: Vec<String> = raw
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .filter(|(_, line)| serde_json::from_str::<Value>(line).is_err())
        .map(|(i, line)| {
            let head: String = line.chars().take(90).collect();
            format!("line {}: {head}…", i + 1)
        })
        .collect();

    let parsed = raw
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter(|line| serde_json::from_str::<Value>(line).is_ok())
        .count();

    assert!(
        torn.is_empty(),
        "{} of {} lines are torn by concurrent appends:\n  {}",
        torn.len(),
        raw.lines().filter(|l| !l.trim().is_empty()).count(),
        torn.join("\n  ")
    );
    assert_eq!(
        parsed,
        WRITERS * PER_WRITER,
        "every append must produce exactly one readable record"
    );

    fs::remove_dir_all(root).unwrap();
}

/// A line that cannot be parsed costs that record and nothing else.
///
/// The pre-fix reader collected into `io::Result`, so the first bad line threw
/// away every good record with it and `session-report` exited 1. Because the
/// rotation that retires the bad line runs *after* the read, the ledger could
/// never recover on its own: it grew until logging stopped at the size cap.
#[test]
fn a_torn_line_is_counted_not_fatal() {
    let root = scratch("torn");
    let db = root.join("store.sqlite");
    fs::write(&db, "store").unwrap();
    let log = live_log_path(&db);
    fs::create_dir_all(log.parent().unwrap()).unwrap();

    // The exact shape found in the field: one record's body followed straight
    // by the next record's, the newline between them lost to a second writer.
    let first = json!({"tool": "devmap_status", "ok": true}).to_string();
    let second = json!({"tool": "devmap_search", "ok": true}).to_string();
    let third = json!({"tool": "devmap_explore", "ok": true}).to_string();
    let fourth = json!({"tool": "devmap_impact", "ok": true}).to_string();
    fs::write(&log, format!("{first}\n{second}{third}\n{fourth}\n")).unwrap();

    let ledger = read_live(&db).unwrap();
    assert_eq!(
        ledger.records.len(),
        2,
        "the two intact records must survive the torn one"
    );
    assert_eq!(ledger.malformed, 1, "the torn line must be counted");
    assert_eq!(ledger.records[0]["tool"], "devmap_status");
    assert_eq!(ledger.records[1]["tool"], "devmap_impact");

    fs::remove_dir_all(root).unwrap();
}

/// Reading the ledger while a daemon appends to it must not fail.
///
/// An append-only log that several daemons write is *expected* to grow under
/// the reader. Treating growth as tampering makes `session-report` fail on a
/// busy repository — and because the rotation that retires the log runs after
/// the read, every such failure leaves the log to grow further, which widens
/// the window for the next one. The check that matters is that the file was
/// not *replaced* or truncated, not that it stayed the same size.
#[test]
fn reading_while_a_writer_appends_is_not_tampering() {
    let root = scratch("read-during-append");
    let db = root.join("store.sqlite");
    fs::write(&db, "store").unwrap();
    append_query(&db, "devmap_status", None, None, None, 1);

    let stop = std::sync::atomic::AtomicBool::new(false);
    let failures = std::sync::atomic::AtomicUsize::new(0);
    let reads = std::sync::atomic::AtomicUsize::new(0);

    std::thread::scope(|scope| {
        let writer_db = db.clone();
        let writer_stop = &stop;
        scope.spawn(move || {
            while !writer_stop.load(std::sync::atomic::Ordering::Relaxed) {
                append_query(
                    &writer_db,
                    "devmap_search",
                    Some(&json!({"pad": "z".repeat(256)})),
                    None,
                    None,
                    1,
                );
            }
        });

        for _ in 0..400 {
            reads.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if read_live(&db).is_err() {
                failures.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
    });

    let failed = failures.load(std::sync::atomic::Ordering::Relaxed);
    assert_eq!(
        failed,
        0,
        "{failed} of {} reads were refused while a writer appended",
        reads.load(std::sync::atomic::Ordering::Relaxed)
    );

    fs::remove_dir_all(root).unwrap();
}

/// Retiring the log while daemons write to it loses nothing.
///
/// Rotation renames the file out from under live appenders. A writer that
/// opened the old inode a moment before the rename goes on writing into the
/// rotated file; one that opens a moment after creates a fresh live log. Both
/// are fine, but only if every record lands in *some* file exactly once — so
/// the union across every rotation is checked against what was written.
#[test]
fn rotation_under_concurrent_appends_loses_no_record() {
    use devmap_serve::session_log::{rotate_live, sessions_dir};
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    let root = scratch("rotate-race");
    let db = root.join("store.sqlite");
    fs::write(&db, "store").unwrap();

    const WRITERS: usize = 6;
    const PER_WRITER: usize = 300;

    let stop = AtomicBool::new(false);
    let rotations = AtomicUsize::new(0);

    std::thread::scope(|scope| {
        let writers: Vec<_> = (0..WRITERS)
            .map(|writer| {
                let db = db.clone();
                scope.spawn(move || {
                    for n in 0..PER_WRITER {
                        append_query(
                            &db,
                            "devmap_search",
                            Some(&json!({"w": writer, "n": n})),
                            None,
                            None,
                            1,
                        );
                    }
                })
            })
            .collect();
        let rotate_db = db.clone();
        let rotate_stop = &stop;
        let rotate_count = &rotations;
        scope.spawn(move || {
            let mut k = 0;
            while !rotate_stop.load(Ordering::Relaxed) {
                k += 1;
                if rotate_live(&rotate_db, &format!("rot-{k}")).unwrap_or(false) {
                    rotate_count.fetch_add(1, Ordering::Relaxed);
                }
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        });
        // The rotator loops until told to stop, and the scope will not return
        // until it does, so the writers are joined here rather than by falling
        // out of the scope with the flag still unset.
        for writer in writers {
            writer.join().unwrap();
        }
        stop.store(true, Ordering::Relaxed);
    });
    let _ = rotate_live(&db, "rot-final");

    let mut seen: HashSet<(u64, u64)> = HashSet::new();
    let mut duplicates = Vec::new();
    let mut torn = 0usize;
    for entry in fs::read_dir(sessions_dir(&db)).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        for line in fs::read_to_string(&path).unwrap().lines() {
            if line.trim().is_empty() {
                continue;
            }
            let Ok(value) = serde_json::from_str::<Value>(line) else {
                torn += 1;
                continue;
            };
            let w = value["args"]["w"].as_u64().unwrap();
            let n = value["args"]["n"].as_u64().unwrap();
            if !seen.insert((w, n)) {
                duplicates.push((w, n));
            }
        }
    }

    assert_eq!(torn, 0, "rotation must not tear records");
    assert!(duplicates.is_empty(), "records duplicated: {duplicates:?}");
    assert_eq!(
        seen.len(),
        WRITERS * PER_WRITER,
        "records lost across {} rotations",
        rotations.load(Ordering::Relaxed)
    );

    fs::remove_dir_all(root).unwrap();
}

/// Blank and whitespace-only lines are not records and are not damage.
#[test]
fn blank_lines_are_neither_records_nor_malformed() {
    let ledger: devmap_serve::session_log::Ledger<Map<String, Value>> =
        parse_jsonl("{\"a\":1}\n\n   \n\t\n{\"b\":2}\n");
    assert_eq!(ledger.records.len(), 2);
    assert_eq!(ledger.malformed, 0);
}

/// A ledger whose every line is damaged reads as damage, never as silence.
///
/// This is the case the counter exists for: without it the report says "no
/// queries logged", which is what a session that genuinely asked nothing says.
#[test]
fn a_wholly_unreadable_ledger_is_not_an_empty_one() {
    let ledger: devmap_serve::session_log::Ledger<Map<String, Value>> =
        parse_jsonl("{\"a\":1}{\"b\":2}\nnot json at all\n{\"c\":\n");
    assert!(ledger.records.is_empty());
    assert_eq!(ledger.malformed, 3);
}
