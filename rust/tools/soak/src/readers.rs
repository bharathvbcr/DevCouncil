//! Readers mode: one writer cycling edit -> incremental build while N reader
//! threads keep querying the same store, and a count of what each reader saw.
//!
//! The readers drive the shipped read path: `devmap --json --db <store>
//! search|impact|explore` as a subprocess per query, not `StoreQueryEngine`
//! in-process. Reasons: that is the process an agent host actually spawns, it
//! opens the store through `open_for_read` (`Store::open_read_only`) exactly as
//! production does, its error text is what an operator would see, and no
//! workspace dependency on `devmap-query` (whose sources other lanes are
//! editing) is needed. The cost is process start-up per query, which lowers the
//! query rate; every query still opens a connection and reads the WAL while the
//! writer commits, which is the contention under test.
//!
//! Every subprocess has a timeout longer than the store's 5 s `busy_timeout`
//! (a real SQLITE_BUSY surfaces only after that wait, so a shorter timeout
//! would relabel every busy error as a timeout). A timeout is counted as an
//! other error, never dropped.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::Value;

/// Per-query bound. Above the 5 s busy timeout, below anything a human would
/// call a live read.
pub const QUERY_TIMEOUT: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(5);

/// What one query came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    /// SQLITE_BUSY / SQLITE_LOCKED reached the caller; carries the first line
    /// of the error text.
    Busy(String),
    Other(String),
}

/// Whether error text is a SQLite busy or locked report.
///
/// The strings are the ones this build can print, each verified in source:
/// `sqlite3.c`'s `sqlite3ErrStr` table gives `database is locked`
/// (SQLITE_BUSY, the message `sqlite3_errmsg` carries and rusqlite prints as
/// the error's message) and `database table is locked` (SQLITE_LOCKED);
/// `libsqlite3-sys/src/error.rs` `code_to_str` gives the text used when a
/// failure has no message (`The database file is locked`, `A table in the
/// database is locked`) and for the extended codes (`SQLITE_BUSY_RECOVERY`,
/// `SQLITE_BUSY_SNAPSHOT`, `SQLITE_BUSY_TIMEOUT`); and its `ErrorCode`
/// variants `DatabaseBusy` / `DatabaseLocked` are what a `{:?}` of the error
/// prints. There is no SQLite string `database is busy`.
pub fn is_busy(text: &str) -> bool {
    const MARKERS: [&str; 11] = [
        "database is locked",
        "database table is locked",
        "the database file is locked",
        "a table in the database is locked",
        "another process is recovering a wal mode database file",
        "cannot promote read transaction to write transaction because of writes by another connection",
        "sqlite_busy",
        "sqlite_locked",
        "databasebusy",
        "databaselocked",
        "database schema is locked",
    ];
    let lower = text.to_ascii_lowercase();
    MARKERS.iter().any(|m| lower.contains(m))
}

fn first_line(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("(no output)");
    line.chars().take(240).collect()
}

/// Classify one finished query. An exit 0 with a JSON object on stdout is a
/// good read; anything else is an error, busy when its text says so.
pub fn classify(exit_ok: bool, stdout: &str, stderr: &str) -> Outcome {
    if exit_ok {
        return match serde_json::from_str::<Value>(stdout) {
            Ok(Value::Object(_)) => Outcome::Ok,
            _ => Outcome::Other(format!(
                "exit 0 but stdout is not a JSON object: {}",
                first_line(stdout)
            )),
        };
    }
    // The CLI prints `Error: <chain>; DevMap context: ...` on stderr; look in
    // both streams, since `--json` errors can be framed on stdout.
    let combined = format!("{stderr}\n{stdout}");
    let line = first_line(if stderr.trim().is_empty() { stdout } else { stderr });
    if is_busy(&combined) {
        Outcome::Busy(line)
    } else {
        Outcome::Other(line)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Search,
    Impact,
    Explore,
}

const SEARCH_TERMS: [&str; 6] = ["build", "parse", "store", "session", "config", "handler"];

/// The argv (after the program) of reader `id`'s `seq`-th query. The kind
/// rotates and starts at a different point per reader, so every kind is under
/// way at once; the search term rotates too, so the page cache is not the only
/// thing being read.
pub fn query_args(id: usize, seq: u64, db: &Path, impact_target: &str) -> (Kind, Vec<String>) {
    let kind = match (id as u64 + seq) % 3 {
        0 => Kind::Search,
        1 => Kind::Impact,
        _ => Kind::Explore,
    };
    let term = SEARCH_TERMS[((id as u64 + seq / 3) % SEARCH_TERMS.len() as u64) as usize];
    let mut args = vec![
        "--json".to_owned(),
        "--db".to_owned(),
        db.to_string_lossy().into_owned(),
    ];
    match kind {
        Kind::Search => args.extend(["search".into(), term.into()]),
        Kind::Impact => args.extend(["impact".into(), impact_target.to_owned()]),
        Kind::Explore => args.extend(["explore".into(), term.into(), "--limit".into(), "5".into()]),
    }
    (kind, args)
}

/// Pick `file::symbol` targets out of a `search` reply: rows whose
/// (file, symbol) pair is unique in the page, since the kernel refuses an
/// ambiguous target. In reply order.
pub fn targets_from_search(reply: &str) -> Vec<String> {
    let Ok(Value::Object(root)) = serde_json::from_str::<Value>(reply) else {
        return Vec::new();
    };
    let Some(Value::Array(items)) = root.get("items") else {
        return Vec::new();
    };
    let pairs: Vec<(String, String)> = items
        .iter()
        .filter_map(|item| {
            Some((
                item.get("file_path")?.as_str()?.to_owned(),
                item.get("symbol_name")?.as_str()?.to_owned(),
            ))
        })
        .collect();
    pairs
        .iter()
        .filter(|p| pairs.iter().filter(|q| q == p).count() == 1)
        .map(|(file, symbol)| format!("{file}::{symbol}"))
        .collect()
}

/// One bounded query. `out` and `err` are scratch files the caller owns; the
/// child is the process this function spawned, and the only one it kills.
pub fn run_query(devmap: &Path, args: &[String], out: &Path, err: &Path) -> (Outcome, Duration) {
    let started = Instant::now();
    let files = fs::File::create(out).and_then(|o| fs::File::create(err).map(|e| (o, e)));
    let (stdout, stderr) = match files {
        Ok(pair) => pair,
        Err(e) => return (Outcome::Other(format!("cannot create scratch file: {e}")), started.elapsed()),
    };
    let mut child = match Command::new(devmap)
        .args(args)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            return (
                Outcome::Other(format!("cannot spawn {}: {e}", devmap.display())),
                started.elapsed(),
            )
        }
    };
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if started.elapsed() >= QUERY_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return (
                    Outcome::Other(format!("timeout: killed after {QUERY_TIMEOUT:?}")),
                    started.elapsed(),
                );
            }
            Ok(None) => std::thread::sleep(POLL),
            Err(e) => break Err(e),
        }
    };
    let elapsed = started.elapsed();
    let status = match status {
        Ok(status) => status,
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            return (Outcome::Other(format!("cannot wait for the query: {e}")), elapsed);
        }
    };
    let read = |p: &Path| String::from_utf8_lossy(&fs::read(p).unwrap_or_default()).into_owned();
    (classify(status.success(), &read(out), &read(err)), elapsed)
}

/// What one reader saw.
#[derive(Debug, Default, Clone)]
pub struct ReaderStats {
    pub ok: u64,
    pub busy: u64,
    pub other: u64,
    /// Queries that were in flight while a build was running.
    pub overlapped: u64,
    pub timeouts: u64,
    pub latencies_ms: Vec<u64>,
    pub first_busy: Option<String>,
    pub first_other: Option<String>,
}

impl ReaderStats {
    pub fn total(&self) -> u64 {
        self.ok + self.busy + self.other
    }

    pub fn record(&mut self, outcome: Outcome, elapsed: Duration, overlapped: bool) {
        self.latencies_ms.push(elapsed.as_millis() as u64);
        if overlapped {
            self.overlapped += 1;
        }
        match outcome {
            Outcome::Ok => self.ok += 1,
            Outcome::Busy(text) => {
                self.busy += 1;
                self.first_busy.get_or_insert(text);
            }
            Outcome::Other(text) => {
                self.other += 1;
                if text.starts_with("timeout:") {
                    self.timeouts += 1;
                }
                self.first_other.get_or_insert(text);
            }
        }
    }
}

/// Removes a reader's scratch files however the thread ends.
struct Scratch(PathBuf, PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
        let _ = fs::remove_file(&self.1);
    }
}

pub struct Setup<'a> {
    pub devmap: &'a Path,
    pub db: &'a Path,
    pub impact_target: &'a str,
    /// Where each reader's stdout/stderr scratch files go: outside the corpus,
    /// so a reader never changes the tree the writer is indexing.
    pub scratch_dir: &'a Path,
    pub tag: u32,
}

/// One reader's loop: issue queries back to back until `stop`. `building` is
/// set by the writer around each build, so a query can say whether it ran
/// against a live write.
pub fn reader_loop(id: usize, setup: &Setup<'_>, stop: &AtomicBool, building: &AtomicBool) -> ReaderStats {
    let out = setup.scratch_dir.join(format!("devmap-soak-{}-r{id}.out", setup.tag));
    let err = setup.scratch_dir.join(format!("devmap-soak-{}-r{id}.err", setup.tag));
    let _scratch = Scratch(out.clone(), err.clone());
    let mut stats = ReaderStats::default();
    let mut seq = 0u64;
    while !stop.load(Ordering::Acquire) {
        let (_, args) = query_args(id, seq, setup.db, setup.impact_target);
        let at_start = building.load(Ordering::Acquire);
        let (outcome, elapsed) = run_query(setup.devmap, &args, &out, &err);
        let overlapped = at_start || building.load(Ordering::Acquire);
        stats.record(outcome, elapsed, overlapped);
        seq += 1;
    }
    stats
}

/// What the writer did while the readers ran.
#[derive(Debug, Default, Clone)]
pub struct WriterTally {
    /// Completed edit -> build -> restore -> build cycles.
    pub cycles: u64,
    pub builds_ms: Vec<u64>,
    /// Writer builds whose error text was a SQLite busy/locked report.
    pub busy: u64,
}

/// The reasons a readers run failed; empty when it passed. A run in which no
/// query overlapped a build proved nothing about contention, so it fails as
/// "could not run" rather than passing with zero errors.
pub fn failures(readers: &[ReaderStats], writer: &WriterTally) -> Vec<String> {
    let mut fails = Vec::new();
    for (i, r) in readers.iter().enumerate() {
        if r.total() == 0 {
            fails.push(format!("reader {i} never ran a query"));
        }
        if r.busy > 0 {
            fails.push(format!(
                "reader {i} hit {} SQLITE_BUSY/locked error(s): {}",
                r.busy,
                r.first_busy.as_deref().unwrap_or("?")
            ));
        }
        if r.other > 0 {
            fails.push(format!(
                "reader {i} hit {} other error(s) ({} timeouts): {}",
                r.other,
                r.timeouts,
                r.first_other.as_deref().unwrap_or("?")
            ));
        }
    }
    if writer.busy > 0 {
        fails.push(format!(
            "the writer hit {} SQLITE_BUSY/locked build error(s)",
            writer.busy
        ));
    }
    if writer.cycles == 0 {
        fails.push("the writer completed no edit -> build cycle, so there was no write to contend with".into());
    }
    if readers.iter().map(|r| r.overlapped).sum::<u64>() == 0 {
        fails.push("no query overlapped a build: the contention check could not run".into());
    }
    fails
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The CLI's own error framing (main.rs: `Error: {chain}; DevMap context:
    /// {context}`) around SQLite's real message.
    const CLI_BUSY: &str =
        "Error: database is locked; DevMap context: command=search db=/x/devmap.sqlite\n";

    #[test]
    fn the_real_sqlite_and_rusqlite_busy_texts_classify_as_busy() {
        for text in [
            CLI_BUSY,
            "Error: Query returned no rows: database table is locked",
            "Error: The database file is locked",
            "Error: A table in the database is locked",
            "Error: Another process is recovering a WAL mode database file",
            "Error: Cannot promote read transaction to write transaction because of writes by another connection",
            "Error: SqliteFailure(Error { code: DatabaseBusy, extended_code: 5 }, Some(\"x\"))",
            "error: SQLITE_BUSY_TIMEOUT",
        ] {
            assert!(is_busy(text), "{text}");
            assert!(
                matches!(classify(false, "", text), Outcome::Busy(_)),
                "{text}"
            );
        }
    }

    #[test]
    fn other_failures_are_not_busy() {
        for text in [
            "Error: no devmap store at /x — run `devmap build` first",
            "Error: another devmap writer holds \"/x.lock\" (pid 9); waited 30s",
            "Error: ambiguous target `main`: 29 symbols",
            "Error: database disk image is malformed",
            "",
        ] {
            assert!(!is_busy(text), "{text}");
            assert!(matches!(classify(false, "", text), Outcome::Other(_)), "{text}");
        }
        // There is no such SQLite string; the task text's wording is not a marker.
        assert!(!is_busy("database is busy"));
    }

    #[test]
    fn a_good_read_is_exit_zero_with_a_json_object() {
        assert_eq!(classify(true, "{\"items\":[]}", ""), Outcome::Ok);
        assert!(matches!(classify(true, "", ""), Outcome::Other(_)));
        assert!(matches!(classify(true, "[1]", ""), Outcome::Other(_)));
        assert!(matches!(classify(true, "not json", ""), Outcome::Other(_)));
        // A busy message on stderr does not make an exit-0 read an error, and a
        // failure on stdout is still seen.
        assert_eq!(classify(true, "{}", "warning: database is locked, retried"), Outcome::Ok);
        assert!(matches!(
            classify(false, "{\"error\":\"database is locked\"}", ""),
            Outcome::Busy(_)
        ));
    }

    #[test]
    fn the_first_line_of_the_error_is_kept_and_bounded() {
        let Outcome::Busy(line) = classify(false, "", &format!("\n  {CLI_BUSY}second line\n")) else {
            panic!("busy");
        };
        assert!(line.starts_with("Error: database is locked"), "{line}");
        assert!(!line.contains("second line"));
        let long = format!("Error: {} database is locked", "x".repeat(1000));
        let Outcome::Busy(line) = classify(false, "", &long) else {
            panic!("busy");
        };
        assert_eq!(line.chars().count(), 240);
    }

    #[test]
    fn queries_rotate_kind_and_every_kind_is_in_flight_across_readers() {
        let db = Path::new("/x/devmap.sqlite");
        let kinds: Vec<Kind> = (0..3).map(|id| query_args(id, 0, db, "a.rs::f").0).collect();
        assert!(kinds.contains(&Kind::Search));
        assert!(kinds.contains(&Kind::Impact));
        assert!(kinds.contains(&Kind::Explore));
        let (_, args) = query_args(0, 1, db, "a.rs::f");
        assert_eq!(args[..3], ["--json", "--db", "/x/devmap.sqlite"]);
        assert_eq!(args[3..], ["impact", "a.rs::f"]);
        let (kind, args) = query_args(0, 2, db, "a.rs::f");
        assert_eq!(kind, Kind::Explore);
        assert!(args.contains(&"--limit".to_owned()));
    }

    #[test]
    fn impact_targets_come_only_from_unambiguous_search_rows() {
        let reply = r#"{"items":[
            {"symbol_name":"new","file_path":"a.rs"},
            {"symbol_name":"new","file_path":"a.rs"},
            {"symbol_name":"run","file_path":"a.rs"},
            {"symbol_name":"new","file_path":"b.rs"}]}"#;
        assert_eq!(targets_from_search(reply), ["a.rs::run", "b.rs::new"]);
        assert!(targets_from_search("{}").is_empty());
        assert!(targets_from_search("nope").is_empty());
    }

    #[test]
    fn stats_count_each_outcome_separately_and_keep_the_first_text() {
        let mut s = ReaderStats::default();
        let t = Duration::from_millis(12);
        s.record(Outcome::Ok, t, true);
        s.record(Outcome::Busy("b1".into()), t, false);
        s.record(Outcome::Busy("b2".into()), t, false);
        s.record(Outcome::Other("timeout: killed after 30s".into()), t, false);
        s.record(Outcome::Other("o2".into()), t, false);
        assert_eq!((s.ok, s.busy, s.other, s.timeouts), (1, 2, 2, 1));
        assert_eq!(s.overlapped, 1);
        assert_eq!(s.total(), 5);
        assert_eq!(s.first_busy.as_deref(), Some("b1"));
        assert_eq!(s.first_other.as_deref(), Some("timeout: killed after 30s"));
        assert_eq!(s.latencies_ms, vec![12; 5]);
    }

    fn healthy() -> (Vec<ReaderStats>, WriterTally) {
        let r = ReaderStats {
            ok: 10,
            overlapped: 4,
            ..ReaderStats::default()
        };
        (
            vec![r.clone(), r],
            WriterTally {
                cycles: 3,
                builds_ms: vec![1, 2],
                busy: 0,
            },
        )
    }

    #[test]
    fn a_clean_run_has_no_failures() {
        let (r, w) = healthy();
        assert!(failures(&r, &w).is_empty(), "{:?}", failures(&r, &w));
    }

    #[test]
    fn one_busy_error_fails_the_run_and_names_the_reader() {
        let (mut r, w) = healthy();
        r[1].busy = 1;
        r[1].first_busy = Some("Error: database is locked".into());
        let f = failures(&r, &w);
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].contains("reader 1") && f[0].contains("database is locked"), "{f:?}");
    }

    #[test]
    fn any_other_error_a_timeout_or_a_silent_reader_fails() {
        let (mut r, w) = healthy();
        r[0].other = 1;
        r[0].timeouts = 1;
        r[0].first_other = Some("timeout: killed after 30s".into());
        assert!(failures(&r, &w)[0].contains("other error"));
        let (mut r, w) = healthy();
        r[1] = ReaderStats::default();
        let f = failures(&r, &w);
        assert!(f.iter().any(|l| l == "reader 1 never ran a query"), "{f:?}");
    }

    #[test]
    fn a_run_with_no_write_or_no_overlap_could_not_run_its_check() {
        let (r, mut w) = healthy();
        w.cycles = 0;
        assert!(failures(&r, &w).iter().any(|l| l.contains("no edit -> build cycle")));
        let (mut r, w) = healthy();
        for x in &mut r {
            x.overlapped = 0;
        }
        assert!(failures(&r, &w).iter().any(|l| l.contains("could not run")));
        let (r, mut w) = healthy();
        w.busy = 2;
        assert!(failures(&r, &w).iter().any(|l| l.contains("the writer hit 2")));
    }

    #[cfg(unix)]
    #[test]
    fn run_query_bounds_and_classifies_real_subprocesses() {
        let dir = std::env::temp_dir().join(format!("soak-readers-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("temp dir");
        let (out, err) = (dir.join("o"), dir.join("e"));
        let sh = Path::new("/bin/sh");
        let run = |script: &str| run_query(sh, &["-c".to_owned(), script.to_owned()], &out, &err).0;
        assert_eq!(run("echo '{\"a\":1}'"), Outcome::Ok);
        assert!(matches!(
            run("echo 'Error: database is locked; DevMap context: x' >&2; exit 1"),
            Outcome::Busy(_)
        ));
        assert!(matches!(run("echo 'Error: nope' >&2; exit 2"), Outcome::Other(_)));
        assert!(matches!(
            run_query(Path::new("/nonexistent/devmap"), &[], &out, &err).0,
            Outcome::Other(_)
        ));
        fs::remove_dir_all(&dir).expect("cleanup");
    }
}
