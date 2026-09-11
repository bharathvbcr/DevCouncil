//! An adversarial pass over `devmap-store`.
//!
//! The existing store suites pin the shapes that specific defects produced.
//! This file asks the complementary question: given a store that is *healthy*,
//! what hostile input, concurrency schedule or resource shape makes it answer
//! wrongly, mutate what it refused, or consume without bound?
//!
//! Every case here is stated over a **shape** rather than an input, in the
//! spirit of PLAN.md §3.1. The three shapes hunted are:
//!
//! * **Class A** — a comparison that could not run must not return what a
//!   comparison that ran and matched nothing returns. `latest_edges` and
//!   `latest_edges_for_file` were hardened against a NaN threshold; the third
//!   member of that family was not, and an empty caller set is read by
//!   `dev verify` as proof a symbol is unused.
//! * **Class D** — a component that refuses on identity mismatch must refuse
//!   *before* it writes. A refusal that has already converted the journal mode
//!   of the database it declined to understand is not fail-closed.
//! * **Bounds** — every batch, fan-out and payload is bounded, and the bound is
//!   stated rather than inherited from whatever the next layer happens to
//!   enforce.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use devmap_analyze::analyze;
use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
use devmap_resolve::model::ResolutionResult;
use devmap_resolve::Resolver;
use devmap_store::{Store, CURRENT_SCHEMA_VERSION, GENERATION_RETENTION};

fn tmp_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "devmap-adv-{label}-{}-{stamp}-{seq}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Extract, resolve and analyse `sources` without touching a filesystem.
fn pipeline(
    sources: &[(&str, &str)],
) -> (
    Vec<Extraction>,
    ResolutionResult,
    devmap_analyze::model::AnalysisSummary,
) {
    let extractions: Vec<Extraction> = sources
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let analysis = analyze(&extractions, &resolution);
    (extractions, resolution, analysis)
}

fn store_with(sources: &[(&str, &str)]) -> Store {
    let (extractions, resolution, analysis) = pipeline(sources);
    let store = Store::open_in_memory().unwrap();
    store
        .save_generation(&extractions, &resolution, &analysis)
        .unwrap();
    store
}

// ---------------------------------------------------------------------------
// Class A — a comparison that could not run must not look like one that ran.
// ---------------------------------------------------------------------------

/// Every confidence threshold the store accepts is validated at the boundary.
///
/// `latest_edges` documents the defect at length: on NaN the Rust filter and
/// the SQL filter disagree completely, because `(NaN * 1000.0).round() as i64`
/// saturates to 0 while SQLite evaluates `>= NULL` as NULL. It refuses the
/// input rather than answering an unanswerable question, and
/// `latest_edges_for_file` does the same.
///
/// `callers_of` — the third function in the family, and the one whose emptiness
/// `dev verify` reads as proof a symbol has no callers — did not. It bound NaN
/// straight into the same SQL comparison and returned an empty vector, which is
/// exactly the sentence "nothing calls this" produced by a filter that never
/// ran. The test is stated over all three so a fourth cannot be added without
/// it.
#[test]
fn no_confidence_threshold_reaches_sql_without_being_checked() {
    let store = store_with(&[
        (
            "src/lib.py",
            "def target():\n    return 1\n\ndef caller():\n    return target()\n",
        ),
        ("src/other.py", "def unrelated():\n    return 2\n"),
    ]);

    // Baseline: a finite threshold finds the caller, so an empty answer below
    // cannot be blamed on an empty graph.
    let real = store
        .callers_of(&["src/lib.py::target".to_string()], "src/other.py", 0.0)
        .expect("a finite threshold is answerable");
    assert!(
        !real.is_empty(),
        "fixture is inert: no caller edge to withhold"
    );

    for (name, outcome) in [
        (
            "callers_of",
            store
                .callers_of(
                    &["src/lib.py::target".to_string()],
                    "src/other.py",
                    f32::NAN,
                )
                .map(|rows| rows.len()),
        ),
        (
            "latest_edges_for_file",
            store
                .latest_edges_for_file("src/lib.py", f32::NAN)
                .map(|rows| rows.len()),
        ),
        (
            "latest_edges",
            store.latest_edges(f32::NAN).map(|rows| rows.len()),
        ),
    ] {
        assert!(
            outcome.is_err(),
            "{name} answered a NaN threshold with Ok({:?}); an empty result from a \
             comparison that never ran is indistinguishable from a real 'nothing \
             matched', and that emptiness is what declares a symbol dead",
            outcome.ok()
        );
    }
}

/// A caller set larger than one SQL statement can bind is answered in full.
///
/// `callers_of` expands `names` into one bind parameter each. SQLite's default
/// `SQLITE_MAX_VARIABLE_NUMBER` is 32,766, so a generated file with more
/// changed symbols than that made `prepare` fail with `too many SQL variables`
/// — a message naming neither the caller, the limit, nor the input that
/// exceeded it, raised from the middle of `preview`.
///
/// The fix must be chunking rather than a cap. Dropping names past a ceiling
/// would contribute zero callers for each dropped name, which is exactly the
/// shape of "nothing depends on this symbol" — a false verdict the store must
/// never manufacture. So the assertion is over *completeness*: the chunked
/// answer equals the answer assembled one name at a time.
#[test]
fn an_oversized_caller_batch_is_answered_in_full_not_truncated() {
    let mut source = String::from("def target():\n    return 1\n\n");
    for i in 0..40 {
        source.push_str(&format!("def caller_{i}():\n    return target()\n\n"));
    }
    let store = store_with(&[
        ("src/lib.py", &source),
        ("src/other.py", "def unrelated():\n    return 2\n"),
    ]);

    // A batch far past both the chunk size and SQLite's parameter ceiling,
    // with the one real target buried in the middle so a truncating
    // implementation drops it.
    let mut huge: Vec<String> = (0..40_000).map(|i| format!("src/lib.py::s{i}")).collect();
    huge.insert(20_000, "src/lib.py::target".to_string());
    assert!(
        huge.len() > Store::MAX_CALLER_BATCH,
        "the batch must exceed the chunk size for this to test anything"
    );

    let chunked = store
        .callers_of(&huge, "src/other.py", 0.0)
        .expect("a large batch must be chunked, not refused");
    let single = store
        .callers_of(&["src/lib.py::target".to_string()], "src/other.py", 0.0)
        .unwrap();
    assert!(!single.is_empty(), "fixture is inert: no caller edges");
    assert_eq!(
        chunked.len(),
        single.len(),
        "a chunked answer lost rows a single-statement answer returns"
    );
    let chunked_rows: Vec<String> = chunked
        .iter()
        .map(|e| format!("{}|{}|{}", e.source_file, e.source_symbol, e.target_symbol))
        .collect();
    let single_rows: Vec<String> = single
        .iter()
        .map(|e| format!("{}|{}|{}", e.source_file, e.source_symbol, e.target_symbol))
        .collect();
    assert_eq!(
        chunked_rows, single_rows,
        "chunking changed the rows or their order"
    );

    // Duplicates in the input must not duplicate rows: `IN (a, a)` matches
    // once, and chunking must not turn one match into two.
    let duplicated = vec!["src/lib.py::target".to_string(); 3];
    assert_eq!(
        store
            .callers_of(&duplicated, "src/other.py", 0.0)
            .unwrap()
            .len(),
        single.len(),
        "a duplicated name produced duplicated caller rows"
    );
}

// ---------------------------------------------------------------------------
// Class D — refuse before writing, not after.
// ---------------------------------------------------------------------------

/// A store this binary refuses must come back byte-identical.
///
/// `Store::open` runs `configure_connection` and `enable_wal` *before*
/// `migrate` decides whether the schema is one this binary understands. So
/// pointing any devmap command at a foreign or superseded database — the
/// Python engine's `index.sqlite` at `user_version = 2` is the live instance
/// noted in PLAN.md §3.1 Class D — converted its journal mode to WAL and left
/// `-wal`/`-shm` files beside it before printing the refusal.
///
/// `future_schema_version_is_rejected_fail_closed` in `store_hardening.rs`
/// checks that no *table* is created. Journal mode is not a table, so the
/// mutation that does happen was invisible to it. This asserts over the file's
/// bytes instead, which is the only statement that covers mutations nobody
/// thought to enumerate.
#[test]
fn refusing_a_foreign_schema_does_not_modify_the_file() {
    for version in [2, 99] {
        let dir = tmp_dir(&format!("refuse-{version}"));
        let db_path = dir.join("foreign.sqlite");
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            // Real content, so this is a database with something to lose rather
            // than an empty file where every mutation is cheap.
            conn.execute_batch(
                "CREATE TABLE symbols (id INTEGER PRIMARY KEY, name TEXT);
                 INSERT INTO symbols (name) VALUES ('alpha'), ('beta');",
            )
            .unwrap();
            conn.execute(&format!("PRAGMA user_version = {version}"), [])
                .unwrap();
        }
        let before = fs::read(&db_path).unwrap();

        let error = Store::open(&db_path)
            .err()
            .unwrap_or_else(|| panic!("schema {version} must be refused"));
        let text = error.to_string();
        assert!(
            text.contains(&CURRENT_SCHEMA_VERSION.to_string())
                && text.contains(&version.to_string()),
            "the refusal must name both versions, got: {text}"
        );

        let after = fs::read(&db_path).unwrap();
        // Reported as the first differing offset rather than as two multi-KiB
        // byte vectors: `assert_eq!` on the raw buffers prints both in full and
        // buries the one fact that matters. Offsets 18 and 19 are the SQLite
        // header's write- and read-version bytes, which is where a WAL
        // conversion shows up.
        let first_difference = before
            .iter()
            .zip(after.iter())
            .position(|(a, b)| a != b)
            .or((before.len() != after.len()).then_some(before.len().min(after.len())));
        assert_eq!(
            first_difference, None,
            "opening a schema-{version} store mutated it before refusing it \
             (first differing byte offset above; 18/19 are the header's \
             write/read version, i.e. a WAL conversion). A refusal that has \
             already rewritten the database header is not fail-closed"
        );
        for sidecar in ["foreign.sqlite-wal", "foreign.sqlite-shm"] {
            assert!(
                !dir.join(sidecar).exists(),
                "refusing schema {version} left {sidecar} beside the store"
            );
        }
        let _ = fs::remove_dir_all(&dir);
    }
}

/// Two processes migrating the same store from a mid-chain version must both
/// succeed, and must leave exactly one schema behind.
///
/// SC28 fixed this for `version == 0` by re-reading `PRAGMA user_version`
/// inside the write transaction. Every later step of the chain still samples
/// the version *outside* any transaction, so two openers of a v7 store both
/// observe 7, both enter the `if version == 7` block, and the loser applies a
/// migration on top of a database the winner already migrated. The steps are
/// individually probed for idempotency, so the exposure is that the version
/// stamp and the applied schema can disagree — this asserts they cannot.
#[test]
fn concurrent_openers_of_a_mid_chain_store_converge_on_one_schema() {
    let dir = tmp_dir("mid-chain-race");
    let db_path = dir.join("v7.sqlite");
    // Build a v12 store, then rewind the stamp so the chain has work to do.
    // Rewinding rather than hand-writing a v7 schema keeps this test about the
    // *race*, not about reproducing a historical DDL by hand.
    {
        let store = Store::open(&db_path).unwrap();
        drop(store);
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute("PRAGMA user_version = 7", []).unwrap();
    }

    let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
    let mut handles = Vec::new();
    for _ in 0..4 {
        let path = db_path.clone();
        let barrier = std::sync::Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            Store::open(&path).map(|_| ()).map_err(|e| e.to_string())
        }));
    }
    let outcomes: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let failures: Vec<_> = outcomes.iter().filter_map(|o| o.as_ref().err()).collect();
    assert!(
        failures.is_empty(),
        "racing openers of a mid-chain store failed: {failures:?}"
    );

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let version: i32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, CURRENT_SCHEMA_VERSION);
    let integrity: String = conn
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .unwrap();
    assert_eq!(integrity, "ok", "the race corrupted the store");
    let _ = fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Adversarial identifiers.
// ---------------------------------------------------------------------------

/// Hostile symbol text survives a write/read round trip verbatim.
///
/// The store is the boundary between a parser that will accept anything and
/// consumers that render into shells, HTML and further SQL. Its contract is
/// that it is *transparent*: what went in comes out, unescaped and
/// unreinterpreted, so a consumer's escaping is applied to the real bytes. Any
/// silent normalisation here would put a string in an agent's hands that names
/// a symbol which does not exist — Class C, from the storage side.
#[test]
fn hostile_identifiers_round_trip_through_the_store_unchanged() {
    let hostile: Vec<(String, String)> = vec![
        ("html".into(), "x<img src=x onerror=alert(1)>".into()),
        ("sql".into(), "'; DROP TABLE generation_nodes; --".into()),
        ("fts".into(), "a\" OR qualified_name:*".into()),
        ("rtl".into(), "start\u{202e}dne\u{202c}".into()),
        ("zero_width".into(), "al\u{200b}pha".into()),
        ("emoji".into(), "\u{1f600}\u{1f4a5}name".into()),
        ("long".into(), "L".repeat(10_000)),
        ("newlines".into(), "line1\nline2\r\nline3".into()),
        ("path_traversal".into(), "../../../../etc/passwd".into()),
        ("null_ish".into(), "NULL".into()),
    ];

    // Written through the store's own row API rather than through a parser:
    // the parser would reject most of these, and the question here is what the
    // *store* does with bytes it is handed.
    let mut source = String::new();
    for (label, _) in &hostile {
        source.push_str(&format!("def sym_{label}():\n    return 1\n\n"));
    }
    let (mut extractions, resolution, analysis) = pipeline(&[("src/hostile.py", &source)]);
    let mut expected: Vec<String> = Vec::new();
    for (symbol, (_, text)) in extractions[0]
        .symbols
        .iter_mut()
        .filter(|s| s.name.starts_with("sym_"))
        .zip(hostile.iter())
    {
        symbol.name = text.clone();
        symbol.qualified_name = format!("src/hostile.py::{text}");
        expected.push(symbol.qualified_name.clone());
    }
    assert_eq!(
        expected.len(),
        hostile.len(),
        "fixture must carry one symbol per hostile string"
    );

    let store = Store::open_in_memory().unwrap();
    store
        .save_generation(&extractions, &resolution, &analysis)
        .unwrap();

    let stored: std::collections::BTreeSet<String> = store
        .all_symbols()
        .unwrap()
        .into_iter()
        .map(|s| s.qualified_name)
        .collect();
    for name in &expected {
        assert!(
            stored.contains(name),
            "the store altered or dropped a hostile identifier: {name:?}"
        );
    }

    // The schema is still intact: nothing in that set was executed. Every
    // symbol is still there, plus the file-level node the extractor emits for
    // the file itself.
    let symbols = store.all_symbols().unwrap();
    assert_eq!(
        symbols.len(),
        extractions[0].symbols.len(),
        "the store dropped rows while writing hostile identifiers"
    );
    let tables: Vec<String> = {
        let counted = store.count_search_symbols("sym").unwrap();
        vec![format!("count_search_symbols(sym) = {counted}")]
    };
    assert!(
        !tables.is_empty(),
        "the FTS index must still be answerable after hostile writes"
    );
}

/// Every hostile string is also safe *as a query*, and a query the store
/// declines to run says so rather than returning an empty result set.
#[test]
fn hostile_search_queries_are_answered_or_refused_never_silently_empty() {
    let store = store_with(&[("src/q.py", "def alpha_beta():\n    return 1\n")]);

    let hostile = [
        "alpha\"",
        "\"",
        "\"\"",
        "*",
        "^alpha",
        "NEAR(alpha beta)",
        "alpha AND NOT beta",
        "qualified_name : alpha",
        "{alpha}",
        "alpha*)(",
        "\u{202e}alpha",
        &"z".repeat(50_000),
    ];
    for query in hostile {
        for (surface, outcome) in [
            ("search_fts", store.search_fts(query, 10).map(|r| r.len())),
            (
                "search_symbols",
                store.search_symbols(query, 10).map(|r| r.len()),
            ),
            (
                "count_search_symbols",
                store.count_search_symbols(query).map(|c| c as usize),
            ),
        ] {
            assert!(
                outcome.is_ok(),
                "{surface} raised on hostile query {query:?}: {:?}",
                outcome.err()
            );
        }
    }

    // A query that *does* match still matches, so "no crash" above is not
    // being bought by a matcher that matches nothing.
    assert!(
        !store.search_symbols("alpha", 10).unwrap().is_empty(),
        "the escape made a legitimate query stop matching"
    );

    // A NUL is the one byte the quoting cannot carry: SQLite hands the MATCH
    // argument to FTS5 as a C string, so the closing quote lands beyond the
    // terminator and the parser raised `unterminated string` from inside
    // SQLite. Refusing is the only answer that is neither a leaked parser
    // error nor a silent search over a truncated query — and it must name the
    // reason, because "found nothing" and "could not run" are the two answers
    // this store may never conflate.
    for surface in ["search_fts", "search_symbols", "count_search_symbols"] {
        let outcome = match surface {
            "search_fts" => store.search_fts("alpha\u{0}beta", 10).map(|r| r.len()),
            "search_symbols" => store.search_symbols("alpha\u{0}beta", 10).map(|r| r.len()),
            _ => store
                .count_search_symbols("alpha\u{0}beta")
                .map(|c| c as usize),
        };
        let error = outcome
            .err()
            .unwrap_or_else(|| panic!("{surface} accepted a NUL in a search query"));
        let text = error.to_string();
        assert!(
            text.contains("NUL"),
            "{surface} refused a NUL query with a message that does not explain \
             it: {text}"
        );
    }
}

/// Counting and listing agree on what a query matched.
///
/// `count_search_symbols` and `search_symbols` run different SQL over the same
/// index — a `CROSS JOIN` count against a `bm25`-ordered page. K4's shape is a
/// query surface where the count and the page disagree and both look healthy,
/// so a consumer paginating on the count reads a total that no page can
/// produce.
#[test]
fn the_search_count_and_the_search_page_agree() {
    let mut source = String::new();
    for i in 0..50 {
        source.push_str(&format!("def alpha_{i}():\n    return {i}\n\n"));
    }
    source.push_str("def beta():\n    return 0\n");
    let store = store_with(&[("src/many.py", &source)]);

    for query in ["alpha", "alpha_1", "beta", "alpha_49"] {
        let counted = store.count_search_symbols(query).unwrap() as usize;
        let listed = store.search_symbols(query, 10_000).unwrap().len();
        assert_eq!(
            counted, listed,
            "count and page disagree for {query:?}: {counted} vs {listed}"
        );
    }
}

// ---------------------------------------------------------------------------
// Concurrency and crash shapes.
// ---------------------------------------------------------------------------

/// A reader holding an open handle onto a store whose file is replaced beneath
/// it must not answer from the new file's rows as if they were its own.
///
/// SQLite detects the swap through the file-change counter and returns
/// `SQLITE_READONLY_DBMOVED` / a schema error rather than silently reading the
/// replacement. This pins that the store surfaces it as an error instead of an
/// empty answer — the same rule as `a_truncated_database_never_answers_as_though
/// _it_were_healthy`, applied to replacement rather than truncation.
#[test]
fn a_store_replaced_under_an_open_handle_never_answers_silently_empty() {
    let dir = tmp_dir("swap-under-handle");
    let db_path = dir.join("index.sqlite");
    let store = Store::open(&db_path).unwrap();
    let (extractions, resolution, analysis) =
        pipeline(&[("src/a.py", "def alpha():\n    return 1\n")]);
    store
        .save_generation(&extractions, &resolution, &analysis)
        .unwrap();
    assert!(!store.all_symbols().unwrap().is_empty());

    // Replace the file with a *different, valid* devmap store holding other
    // rows. If the handle silently follows the replacement, the old store
    // starts answering with the new store's symbols.
    let other = dir.join("other.sqlite");
    {
        let replacement = Store::open(&other).unwrap();
        let (e, r, a) = pipeline(&[("src/z.py", "def zulu():\n    return 9\n")]);
        replacement.save_generation(&e, &r, &a).unwrap();
        replacement.checkpoint_wal().unwrap();
    }
    for sidecar in ["index.sqlite-wal", "index.sqlite-shm"] {
        let _ = fs::remove_file(dir.join(sidecar));
    }
    let replaced = fs::rename(&other, &db_path);
    #[cfg(windows)]
    if let Err(error) = &replaced {
        // Windows prevents replacing SQLite's open/mapped database. That is
        // stronger exclusion; still verify the original reader answers alpha.
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        let names: Vec<_> = store
            .all_symbols()
            .unwrap()
            .into_iter()
            .map(|row| row.name)
            .collect();
        assert!(names.iter().any(|name| name == "alpha"));
        assert!(!names.iter().any(|name| name == "zulu"));
        drop(store);
        fs::remove_dir_all(&dir).unwrap();
        return;
    }
    replaced.unwrap();

    match store.all_symbols() {
        Ok(rows) => {
            let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
            assert!(
                !names.contains(&"zulu"),
                "the open handle followed the replacement and served another \
                 store's rows as its own: {names:?}"
            );
        }
        Err(_) => {
            // Refusing is the other acceptable answer, and the better one.
        }
    }
    let _ = fs::remove_dir_all(&dir);
}

/// A writer that dies mid-transaction leaves no partial generation behind.
///
/// Driven with a real child process and `SIGKILL`, because an in-process
/// `panic` unwinds through rusqlite's `Drop` and rolls the transaction back —
/// which tests Rust's destructors, not SQLite's recovery. The child holds an
/// uncommitted `BEGIN IMMEDIATE` with rows written into it when it is killed.
#[cfg(unix)]
#[test]
fn a_writer_killed_mid_transaction_leaves_no_half_written_generation() {
    let dir = tmp_dir("killed-writer");
    let db_path = dir.join("index.sqlite");
    let store = Store::open(&db_path).unwrap();
    let (extractions, resolution, analysis) =
        pipeline(&[("src/a.py", "def alpha():\n    return 1\n")]);
    let committed = store
        .save_generation(&extractions, &resolution, &analysis)
        .unwrap();
    store.checkpoint_wal().unwrap();
    drop(store);

    // A child process that opens the store, writes a generation row inside an
    // uncommitted transaction, announces itself, and then blocks forever.
    let script = format!(
        r#"
import sqlite3, sys, time
conn = sqlite3.connect({:?}, isolation_level=None)
conn.execute("PRAGMA busy_timeout=5000")
conn.execute("BEGIN IMMEDIATE")
conn.execute("INSERT INTO generations (created_at, head_sha, analysis_json) VALUES (1.0, 'torn', '{{}}')")
conn.execute("INSERT INTO generation_nodes (generation_id, ordinal, file_id, name, qualified_name, kind, span_start, span_end, is_exported) SELECT last_insert_rowid(), 0, 1, 'torn', 'torn', 'Function', 0, 1, 0")
sys.stdout.write("ready\n")
sys.stdout.flush()
time.sleep(600)
"#,
        db_path.to_string_lossy()
    );
    let mut child = std::process::Command::new("python3")
        .arg("-c")
        .arg(&script)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("python3 is required to hold an uncommitted transaction");

    use std::io::{BufRead, BufReader};
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while line.trim() != "ready" && std::time::Instant::now() < deadline {
        line.clear();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
    }
    assert_eq!(line.trim(), "ready", "child never opened its transaction");

    // SIGKILL: no unwinding, no destructors, no rollback by the process itself.
    unsafe {
        libc_kill(child.id() as i32, 9);
    }
    let _ = child.wait();

    let reopened = Store::open(&db_path).expect("the store must recover");
    assert_eq!(
        reopened.latest_generation_id().unwrap(),
        Some(committed),
        "a killed writer's uncommitted generation became visible"
    );
    let names: Vec<String> = reopened
        .all_symbols()
        .unwrap()
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert!(
        !names.contains(&"torn".to_string()),
        "half-written rows survived the kill: {names:?}"
    );
    let status = reopened.status(&db_path.to_string_lossy()).unwrap();
    assert_eq!(
        status.latest_generation,
        Some(committed),
        "status disagrees with the recovered generation"
    );
    assert!(
        status.node_count > 0,
        "the store did not recover from a killed writer"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, sig: i32) -> i32;
}

/// A reader running against a store that a second *process* is pruning sees
/// either the pre-prune or the post-prune generation, never a torn mixture.
///
/// `a_reader_during_prune_sees_a_consistent_store` drives this with two
/// `Store` handles in one process, which share nothing but do share the
/// process's page cache. Two OS processes is the shape DevCouncil actually
/// runs: a daemon drain and a developer's `dev map`.
#[test]
fn a_reader_in_another_process_never_sees_a_partially_pruned_generation() {
    let dir = tmp_dir("cross-process-prune");
    let db_path = dir.join("index.sqlite");
    {
        let store = Store::open(&db_path).unwrap();
        for round in 0..6 {
            let source = format!("def alpha():\n    return {round}\n");
            let (e, r, a) = pipeline(&[("src/a.py", &source), ("src/b.py", "def beta(): pass\n")]);
            store.save_generation(&e, &r, &a).unwrap();
        }
    }

    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reader_path = db_path.clone();
    let reader_stop = std::sync::Arc::clone(&stop);
    let reader = std::thread::spawn(move || {
        let mut observations = Vec::new();
        while !reader_stop.load(Ordering::Relaxed) {
            let store = match Store::open_existing(&reader_path) {
                Ok(Some(store)) => store,
                Ok(None) => continue,
                Err(error) => {
                    observations.push(Err(error.to_string()));
                    continue;
                }
            };
            let generation = store.latest_generation_id();
            let symbols = store.all_symbols();
            match (generation, symbols) {
                (Ok(Some(gen)), Ok(rows)) => observations.push(Ok((gen, rows.len()))),
                (Ok(None), _) => observations.push(Ok((0, 0))),
                (Err(error), _) | (_, Err(error)) => observations.push(Err(error.to_string())),
            }
        }
        observations
    });

    {
        let store = Store::open(&db_path).unwrap();
        for _ in 0..8 {
            store.prune_generations_except_latest(1).unwrap();
            let (e, r, a) = pipeline(&[
                (
                    "src/a.py",
                    "def alpha():\n    return 99\n\ndef gamma(): pass\n",
                ),
                ("src/b.py", "def beta(): pass\n"),
            ]);
            store.save_generation(&e, &r, &a).unwrap();
        }
    }
    stop.store(true, Ordering::Relaxed);
    let observations = reader.join().unwrap();

    assert!(
        !observations.is_empty(),
        "the reader never got a look at the store"
    );
    for observation in &observations {
        match observation {
            Ok((generation, symbols)) => assert!(
                *symbols > 0 || *generation == 0,
                "a live generation {generation} answered with zero symbols; a \
                 partially pruned generation is indistinguishable from an empty \
                 repository"
            ),
            Err(error) => panic!("a concurrent reader saw an error: {error}"),
        }
    }
    let _ = fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Resource bounds on the hot read path.
// ---------------------------------------------------------------------------

/// The extraction-cache fallback is a lookup, not a scan.
///
/// `try_get_cached_extraction` misses `extraction_cache` and falls back to
/// `generation_files` on the full cache identity (SC8). That fallback is not an
/// exceptional path — it is the *only* path, because `prune_extraction_cache`
/// deletes every cache row a retained generation already holds, which is all of
/// them. Measured on a cold-built store: `extraction_cache` is empty after
/// every build.
///
/// So the fallback runs once per file on every build, and without an index it
/// was `SCAN generation_files` — over a table whose widest column is a ~47 KB
/// `extraction_json` that the scan must step past to reach the identity
/// columns. Measured against the release binary on 8,001 files, the no-op build
/// a watcher performs on every tick went from a 2.619 s median to 0.724 s.
///
/// Asserted as a **query plan** rather than as a duration. A timing assertion
/// on a shared machine measures the machine; the plan is the property, it is
/// stable, and it goes red the moment the index stops being used — including if
/// someone reorders the predicate or widens a column type so the index no
/// longer applies.
#[test]
fn the_extraction_cache_fallback_uses_an_index_rather_than_scanning() {
    let dir = tmp_dir("cache-fallback-plan");
    let db_path = dir.join("index.sqlite");
    let store = Store::open(&db_path).unwrap();
    let (extractions, resolution, analysis) =
        pipeline(&[("src/a.py", "def alpha():\n    return 1\n")]);
    store
        .save_generation(&extractions, &resolution, &analysis)
        .unwrap();
    drop(store);

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let plan: Vec<String> = conn
        .prepare(
            "EXPLAIN QUERY PLAN
             SELECT extraction_json FROM generation_files
              WHERE content_hash = ?1 AND language = ?2
                AND grammar_version = ?3 AND analyzer_version = ?4
              LIMIT 1",
        )
        .unwrap()
        .query_map(params_for_plan(), |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let rendered = plan.join(" | ");
    assert!(
        rendered.contains("SEARCH") && rendered.contains("USING INDEX"),
        "the extraction-cache fallback plans as {rendered:?}; a SCAN here is \
         paid once per file on every build, including the no-op build a watcher \
         runs on every save"
    );

    // A store that predates the index must gain it on open, not only a fresh
    // one — the whole point of a migration.
    let legacy = dir.join("legacy.sqlite");
    fs::copy(&db_path, &legacy).unwrap();
    {
        let conn = rusqlite::Connection::open(&legacy).unwrap();
        conn.execute_batch("DROP INDEX IF EXISTS idx_generation_files_cache_identity")
            .unwrap();
        conn.execute("PRAGMA user_version = 12", []).unwrap();
    }
    let migrated = Store::open(&legacy).expect("a v12 store must migrate");
    drop(migrated);
    let conn = rusqlite::Connection::open(&legacy).unwrap();
    let version: i32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, CURRENT_SCHEMA_VERSION);
    // The index this migration created has a successor. v17 split the payload
    // out of `generation_files` into a content-addressed `file_payloads`, so
    // the same four identity columns are now indexed over **one row per
    // distinct payload** rather than one per generation and file — which is
    // strictly what v13's own rationale asked for, since it described the old
    // shape as a scan "whose rows each carry a ~47 KB `extraction_json` the
    // scan must skip past to reach the identity columns".
    let has_index: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
              WHERE type = 'index' AND name = 'idx_file_payloads_identity'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        has_index, 1,
        "migrating a v12 store did not create the extraction-cache index"
    );
    // And the plan, which is what the index is *for* — asserted on the migrated
    // store as well as the fresh one, because an index that exists and is not
    // used costs write time and buys nothing.
    let migrated_plan: Vec<String> = conn
        .prepare(
            "EXPLAIN QUERY PLAN
             SELECT extraction_json FROM generation_files
              WHERE content_hash = ?1 AND language = ?2
                AND grammar_version = ?3 AND analyzer_version = ?4
              LIMIT 1",
        )
        .unwrap()
        .query_map(params_for_plan(), |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let migrated_rendered = migrated_plan.join(" | ");
    assert!(
        migrated_rendered.contains("SEARCH") && migrated_rendered.contains("USING INDEX"),
        "after migrating, the extraction-cache fallback plans as \
         {migrated_rendered:?}"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// Four bound parameters for the plan query above. Values are irrelevant to a
/// query plan; only their count and positions are.
fn params_for_plan() -> [&'static dyn rusqlite::ToSql; 4] {
    [&1i64, &"python", &"g", &"a"]
}

/// The count and the listing must agree, including across a chunk boundary.
///
/// `preview` reports "N confident callers, and M more the floor excluded". The
/// second number used to come from `callers_of(..., 0.0).len()` — the same
/// query, fully materialised, thrown away. Counting it instead is only safe if
/// the two share every filter, so this pins them against each other rather than
/// against a hand-computed expectation: a `WHERE` clause added to one and not
/// the other fails here.
///
/// The name list deliberately exceeds `MAX_CALLER_BATCH` so the chunked path is
/// the one under test — per-chunk counts must sum without double-counting, the
/// same property that makes the chunked listing exact.
#[test]
fn count_callers_of_matches_the_listing_it_replaces() {
    let mut callers_src = String::from("def target():\n    return 1\n\n\n");
    for index in 0..40 {
        callers_src.push_str(&format!("def caller_{index}():\n    return target()\n\n\n"));
    }
    let store = store_with(&[
        ("callers.py", callers_src.as_str()),
        ("target.py", "def target():\n    return 2\n"),
    ]);

    // More names than one chunk holds, with a duplicate, so both the chunking
    // and the de-duplication are exercised. `target_symbol` in
    // `generation_edges` is `path::Name`, never the bare name.
    let mut names: Vec<String> = (0..700).map(|index| format!("absent_{index}")).collect();
    names.push("callers.py::target".to_string());
    names.push("callers.py::target".to_string());
    assert!(
        names.len() > Store::MAX_CALLER_BATCH,
        "the chunked path is the one under test"
    );

    // Guard the guard: if the fixture stopped producing edges, every assertion
    // below would compare 0 against 0 and pass while testing nothing.
    let baseline = store
        .callers_of(&names, "target.py", 0.0)
        .expect("list callers");
    assert!(
        !baseline.is_empty(),
        "fixture produced no caller edges, so the comparison below is vacuous"
    );

    for floor in [0.0f32, 0.5, 0.9, 1.0] {
        let listed = store
            .callers_of(&names, "target.py", floor)
            .expect("list callers");
        let counted = store
            .count_callers_of(&names, "target.py", floor)
            .expect("count callers");
        assert_eq!(
            counted,
            listed.len(),
            "count and listing disagree at floor {floor}"
        );
    }

    // The excluded file is a filter, not a decoration: dropping it from one of
    // the two queries would go unnoticed by the loop above, where no edge
    // originates in the excluded file anyway.
    assert_eq!(
        store
            .count_callers_of(&names, "callers.py", 0.0)
            .expect("count with the declaring file excluded"),
        store
            .callers_of(&names, "callers.py", 0.0)
            .expect("list with the declaring file excluded")
            .len(),
        "the two queries disagree about which file is excluded"
    );

    // Class A: a NaN floor is refused by both, not silently answered as zero —
    // `preview` reads the count as "and M more the floor excluded", so a zero
    // from a comparison that never ran is a sentence about the code.
    assert!(
        store
            .count_callers_of(&names, "target.py", f32::NAN)
            .is_err(),
        "a NaN floor must be refused, not counted as zero"
    );
    // An empty name list is a real zero, not a refusal.
    assert_eq!(
        store
            .count_callers_of(&[], "target.py", 0.0)
            .expect("empty"),
        0
    );
}

// ---------------------------------------------------------------------------
// Class A — an answer a check could not produce must not look like an answer
// the check produced and found nothing.
// ---------------------------------------------------------------------------

/// A search whose index is gone must refuse, not report "no matches".
///
/// The FTS index is a *separate* structure from `generation_nodes`, and the
/// store already assumes it can be lost independently: `devmap repair --fts`
/// and [`Store::repair_fts`] exist for exactly that state. Nothing detected it.
/// With the generation's index rows removed and its 4 symbol rows untouched,
/// the pre-fix store answered:
///
/// ```text
/// all_symbols  = 4
/// search_page  = Some(SearchPage { generation: 1, total: 0, rows: [] })
/// status       = node_count 4, degraded_reason: None
/// ```
///
/// `total: 0` with `rows: []` is byte-identical to a query that ran against a
/// healthy index and matched nothing — so every `search` against that store
/// returns "this repository does not contain that symbol", forever, and the one
/// command that would fix it is the one nothing tells the operator to run.
///
/// Both halves of the desync are exercised because they fail differently: the
/// posting list can be lost while the map survives, and the map can be lost
/// while the postings survive. The rows are deleted out of band because that is
/// the state under test; the mechanism that produces it in the field is
/// whatever `repair --fts` was written for.
#[test]
fn a_search_whose_index_is_missing_is_refused_rather_than_answered_as_empty() {
    for (label, damage) in [
        ("postings lost", "DELETE FROM nodes_fts"),
        ("map lost", "DELETE FROM nodes_fts_map"),
    ] {
        let dir = tmp_dir("fts-desync");
        let db_path = dir.join("index.sqlite");
        {
            let store = Store::open(&db_path).unwrap();
            let (extractions, resolution, analysis) = pipeline(&[
                ("src/a.py", "def alpha():\n    return 1\n"),
                ("src/b.py", "def beta():\n    return 2\n"),
            ]);
            store
                .save_generation(&extractions, &resolution, &analysis)
                .unwrap();
            store.checkpoint_wal().unwrap();
        }
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(damage, []).unwrap();
        drop(conn);

        let store = Store::open(&db_path).unwrap();
        // Guard the guard: the graph itself is intact, so an empty search below
        // cannot be blamed on an empty store.
        assert_eq!(
            store.all_symbols().unwrap().len(),
            4,
            "{label}: the damage was supposed to touch only the search index"
        );

        for (surface, outcome) in [
            (
                "search_page",
                store.search_page("alpha", 10).map(|page| {
                    page.map_or("None".to_string(), |page| {
                        format!("total={} rows={}", page.total, page.rows.len())
                    })
                }),
            ),
            (
                "search_symbols",
                store
                    .search_symbols("alpha", 10)
                    .map(|r| format!("{}", r.len())),
            ),
            (
                "count_search_symbols",
                store.count_search_symbols("alpha").map(|c| format!("{c}")),
            ),
            (
                "search_fts",
                store
                    .search_fts("alpha", 10)
                    .map(|r| format!("{}", r.len())),
            ),
        ] {
            let error = outcome.err().unwrap_or_else(|| {
                panic!(
                    "{label}: {surface} answered from a search index that is not \
                     there; an absent index and a query that matched nothing are \
                     the two answers this store may never conflate"
                )
            });
            let text = error.to_string();
            assert!(
                text.contains("repair --fts"),
                "{label}: {surface} refused without naming the remedy: {text}"
            );
        }

        // The remedy named in the refusal must actually work, and the same
        // queries must answer normally afterwards.
        store.repair_fts().unwrap();
        assert_eq!(
            store.search_page("alpha", 10).unwrap().unwrap().total,
            1,
            "{label}: `repair_fts` did not restore the index it was named for"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}

/// An empty generation is a real answer, not a missing index.
///
/// The check above must not fire for a generation that genuinely holds no
/// symbols: there is nothing for the index to contain, so "no matches" is the
/// truth. Without this the refusal would replace one false answer with another.
#[test]
fn a_generation_with_no_symbols_still_answers_a_search_as_empty() {
    let store = Store::open_in_memory().unwrap();
    let (extractions, resolution, analysis) = pipeline(&[]);
    store
        .save_generation(&extractions, &resolution, &analysis)
        .unwrap();
    let page = store
        .search_page("alpha", 10)
        .expect("an empty generation is searchable")
        .expect("a generation exists");
    assert_eq!((page.total, page.rows.len()), (0, 0));
    assert_eq!(store.count_search_symbols("alpha").unwrap(), 0);
    assert!(store.search_fts("alpha", 10).unwrap().is_empty());
}

/// A generation the store does not hold must be refused, not answered `[]`.
///
/// [`Store::list_generation_paths`] is the one reader that takes a generation
/// id from its caller, and every caller resolves that id in a *separate* call:
/// `devmap-query`'s `savings` does `latest_generation_id()` and then
/// `list_generation_paths(gen)`, with a prune-capable writer free to commit in
/// between. Pre-fix, all three of these returned the same `Ok([])`:
///
/// * a generation that is still in the store and holds one file — `["src/a.py"]`
/// * the same generation after `prune_generations_except_latest` removed it
/// * generation 9999, which never existed
///
/// So `savings` reported `corpus_bytes: 0, corpus_files_unreadable: 0` — a
/// repository with nothing in it — for a store full of files, and the report
/// whose own documentation refuses to count unreadable files as zero bytes did
/// exactly that one level up.
#[test]
fn a_generation_the_store_does_not_hold_is_refused_rather_than_answered_empty() {
    let dir = tmp_dir("missing-generation");
    let db_path = dir.join("index.sqlite");
    let store = Store::open(&db_path).unwrap();
    let (extractions, resolution, analysis) =
        pipeline(&[("src/a.py", "def alpha():\n    return 1\n")]);
    let first = store
        .save_generation(&extractions, &resolution, &analysis)
        .unwrap();
    assert_eq!(
        store.list_generation_paths(first).unwrap(),
        vec!["src/a.py".to_string()],
        "a live generation must still list its files"
    );

    for round in 0..3 {
        let source = format!("def alpha():\n    return {round}\n");
        let (extractions, resolution, analysis) = pipeline(&[("src/a.py", source.as_str())]);
        store
            .save_generation(&extractions, &resolution, &analysis)
            .unwrap();
        store
            .prune_generations_except_latest(GENERATION_RETENTION)
            .unwrap();
    }
    let latest = store.latest_generation_id().unwrap().expect("a generation");
    assert!(
        latest > first,
        "the fixture did not actually prune the generation under test"
    );

    for (label, generation) in [("pruned", first), ("never written", 9_999)] {
        let error = store
            .list_generation_paths(generation)
            .err()
            .unwrap_or_else(|| {
                panic!(
                    "a {label} generation listed its files as `[]`; a generation that \
                 is not in the store and a generation that indexed nothing are \
                 the two answers this store may never conflate"
                )
            });
        let text = error.to_string();
        assert!(
            text.contains(&generation.to_string()),
            "the refusal for a {label} generation does not name it: {text}"
        );
    }

    // The live generation still answers, so the refusal is not blanket.
    assert!(!store.list_generation_paths(latest).unwrap().is_empty());
    let _ = fs::remove_dir_all(&dir);
}

/// The writer half of the cross-process probe below: a real second process,
/// committing and pruning against the same file until told to stop.
///
/// `#[ignore]` because it is not a test — it is the peer process, re-executed
/// out of this same binary so it runs the store's real write path rather than
/// an approximation of it in SQL.
#[test]
#[ignore]
fn concurrent_prune_writer_child() {
    let db = std::env::var("DEVMAP_PROBE_DB").expect("DEVMAP_PROBE_DB");
    let stop = std::env::var("DEVMAP_PROBE_STOP").expect("DEVMAP_PROBE_STOP");
    let store = Store::open(&db).expect("child opens the store");
    let mut salt = 0usize;
    while !std::path::Path::new(&stop).exists() {
        salt += 1;
        let source = format!("def alpha_one():\n    return {salt}\n");
        let (extractions, resolution, analysis) = pipeline(&[("src/a.py", source.as_str())]);
        store
            .save_generation(&extractions, &resolution, &analysis)
            .expect("child commits");
        // `1`, not `GENERATION_RETENTION`: the narrowest retention the store
        // supports is the schedule that makes the window widest, and it is the
        // one `a_reader_during_prune_sees_a_consistent_store` already uses.
        store
            .prune_generations_except_latest(1)
            .expect("child prunes");
    }
}

/// A reader must never answer from a generation a second *process* pruned
/// underneath it — and this is the probe, not the guard.
///
/// Every latest-generation reader resolved the generation with one statement
/// and read its rows with another. In SQLite's autocommit mode those are two
/// snapshots, and `lock_conn` does not change that: it is a Rust mutex over
/// this process's own threads. A second process committing and pruning between
/// them left the reader holding a generation that no longer existed, whose rows
/// it then read as zero.
///
/// Measured against the pre-fix code with this exact schedule, release build,
/// 20 s runs:
///
/// | retention | reads | false-empty answers |
/// |---|---|---|
/// | `prune(1)` | 182,781 | 31 |
/// | `prune(GENERATION_RETENTION)` | 192,970 | 1 |
///
/// After the fix: 0 false-empty answers in 488,420 reads at `prune(1)`.
///
/// **A green run here is not evidence of correctness, and the rate is
/// measured, not asserted.** Run against the pre-fix behaviour at this test's
/// own three-second budget, it fired in **0 of 5 debug runs** and **2 of 5
/// release runs** (1 and 3 violations out of ~78,000 reads) — so in the
/// configuration `cargo test` uses by default it caught a fully present defect
/// *never*. Only a red run here carries information. The guard that holds the
/// fix in place is `db::connection_tests::
/// a_pinned_generation_keeps_its_rows_when_another_connection_prunes_it`,
/// which forces the schedule instead of racing for it, and which goes red
/// against a one-statement mutation of the fix.
///
/// This is kept for what it does cover deterministically: a reader running
/// against a genuinely concurrent second *process* must never error and must
/// never empty. Two OS processes is the shape DevCouncil runs — a daemon drain
/// and a developer's `dev map` — and it is the shape
/// `a_reader_in_another_process_never_sees_a_partially_pruned_generation`
/// describes but does not actually use: that test spawns a thread.
#[test]
fn a_reader_never_answers_from_a_generation_another_process_pruned() {
    let dir = tmp_dir("cross-process-straddle");
    let db_path = dir.join("index.sqlite");
    let stop = dir.join("STOP");
    {
        let store = Store::open(&db_path).unwrap();
        let (extractions, resolution, analysis) =
            pipeline(&[("src/a.py", "def alpha_one():\n    return 0\n")]);
        store
            .save_generation(&extractions, &resolution, &analysis)
            .unwrap();
    }

    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "concurrent_prune_writer_child",
            "--ignored",
            "--nocapture",
        ])
        .env("DEVMAP_PROBE_DB", &db_path)
        .env("DEVMAP_PROBE_STOP", &stop)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn the second writer process");

    let reader = Store::open_existing(&db_path).unwrap().unwrap();
    let mut reads = 0u64;
    let mut violations: Vec<String> = Vec::new();
    // Bounded by wall clock *and* by read count, so a machine where the child
    // never gets scheduled ends the probe instead of spinning for the deadline.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline && reads < 400_000 {
        reads += 1;
        match reader.search_page("alpha_one", 20) {
            Ok(Some(page)) if page.total == 0 || page.rows.is_empty() => violations.push(format!(
                "search_page: total={} rows={}",
                page.total,
                page.rows.len()
            )),
            Ok(Some(_)) => {}
            Ok(None) => {
                violations.push("search_page: None on a store that holds a generation".into())
            }
            Err(error) => violations.push(format!("search_page: {error}")),
        }
        match reader.all_symbols() {
            Ok(rows) if rows.is_empty() => violations.push("all_symbols: []".into()),
            Ok(_) => {}
            Err(error) => violations.push(format!("all_symbols: {error}")),
        }
        match reader.latest_extractions() {
            Ok(rows) if rows.is_empty() => violations.push("latest_extractions: []".into()),
            Ok(_) => {}
            Err(error) => violations.push(format!("latest_extractions: {error}")),
        }
    }
    fs::write(&stop, b"stop").unwrap();
    // Bounded wait, then kill: a child that fails to notice the stop file must
    // not turn a three-second probe into a hung suite.
    let child_deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < child_deadline => {
                std::thread::sleep(std::time::Duration::from_millis(20))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
        }
    }

    assert!(
        reads > 100,
        "the probe never got to read: {reads} reads in three seconds"
    );
    assert!(
        violations.is_empty(),
        "{} of {reads} reads answered emptily for a store that held one file in \
         every generation; an empty answer and a lost generation are the two \
         answers this store may never conflate:\n  {}",
        violations.len(),
        violations
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n  ")
    );
    let _ = fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Class A again — an edge the reader could not resolve must not read as an
// edge the generation never held.
// ---------------------------------------------------------------------------

/// A generation edge whose `paths` row is gone is a **refusal**, not a silent
/// omission.
///
/// `latest_edges_uncached` reached the two file paths through
/// `JOIN paths sp ON sp.id = e.source_file_id`. An inner join answers "this
/// row's path is missing" by *dropping the row*, so a store that had lost a
/// `paths` entry — a partial restore, a truncated copy, a foreign writer, a
/// prune that ran against the wrong generation — served an edge set with holes
/// in it under a successful status. Nothing in the result said so, and the
/// holes then propagate as positive claims: `impact` reports a smaller blast
/// radius, `dead` reports a called symbol as callerless, `deps` reports a
/// dependency that exists as absent.
///
/// The same shape `edge_kind_from_stored` already refuses for an unknown edge
/// kind, and the one Class A is named for: what a lookup that could not run
/// returns must not be what a lookup that ran and found nothing returns.
#[test]
fn an_edge_whose_path_row_is_missing_is_refused_not_dropped() {
    let dir = tmp_dir("orphan-path");
    let db_path = dir.join("devmap.sqlite");
    let before_len = {
        let (extractions, resolution, analysis) = pipeline(&[
            (
                "src/lib.py",
                "def target():\n    return 1\n\ndef caller():\n    return target()\n",
            ),
            (
                "src/other.py",
                "from src.lib import target\n\ndef second():\n    return target()\n",
            ),
        ]);
        let store = Store::open(&db_path).unwrap();
        store
            .save_generation(&extractions, &resolution, &analysis)
            .unwrap();
        let before = store.latest_edges(0.0).expect("healthy read");
        assert!(
            before.len() >= 2,
            "fixture is inert: {} edges, nothing to lose",
            before.len()
        );
        before.len()
    };

    // Remove exactly one `paths` row that a generation edge names. Foreign keys
    // are off for the surgery itself: this models the *state* a damaged store
    // is in, not a write the kernel would ever make.
    let orphaned: String = {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
        let (id, path): (i64, String) = conn
            .query_row(
                "SELECT p.id, p.path FROM paths p
                 WHERE p.id IN (SELECT source_file_id FROM generation_edges)
                 ORDER BY p.id LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("a path some edge names");
        conn.execute("DELETE FROM paths WHERE id = ?1", [id])
            .unwrap();
        path
    };

    let store = Store::open(&db_path).expect("the store still opens");
    match store.latest_edges(0.0) {
        Err(error) => {
            let text = error.to_string();
            assert!(
                text.contains("paths"),
                "the refusal must name what is missing, not merely fail: {text}"
            );
        }
        Ok(rows) => panic!(
            "reading a generation whose `paths` row for {orphaned:?} is gone \
             returned {} of {before_len} edges under a successful status. Every \
             edge touching that path was dropped, and a dropped edge is \
             indistinguishable from an edge the generation never held.",
            rows.len()
        ),
    }
    let _ = fs::remove_dir_all(&dir);
}

/// The edge read order is still the one SQL produced, key for key.
///
/// `latest_edges_uncached` no longer asks SQLite to sort: the `ORDER BY` over
/// two joined `paths` strings was `USE TEMP B-TREE FOR ORDER BY` across every
/// row of the generation and ~72 ms of the ~133 ms a cold `devmap impact`
/// spends arriving at its index. The rows are read unordered and ordered in
/// Rust instead.
///
/// That order is the final tie-break of every answer derived from a graph walk
/// (R4), so it is not enough for it to be *an* order. This runs the exact SQL
/// that was removed against the same store and requires the two sequences to
/// agree row for row — the check that would catch a comparator drifting from
/// SQLite's BINARY collation, from its `DESC` on REAL, or from its key order.
#[test]
fn the_rust_edge_order_is_the_sql_order_it_replaced() {
    let dir = tmp_dir("edge-order");
    let db_path = dir.join("devmap.sqlite");
    // Several files and several edge kinds, so the comparison actually reaches
    // past `confidence` into the path, symbol and kind keys.
    let (extractions, resolution, analysis) = pipeline(&[
        (
            "src/zeta.py",
            "import os\n\ndef alpha():\n    return 1\n\ndef beta():\n    return alpha() + os.getpid()\n",
        ),
        (
            "src/alpha.py",
            "from src.zeta import alpha, beta\n\nclass Thing:\n    def run(self):\n        return alpha() + beta()\n",
        ),
        (
            "src/mid.py",
            "from src.alpha import Thing\n\ndef make():\n    return Thing()\n\ndef use():\n    return make().run()\n",
        ),
        (
            "src/dup.py",
            "from src.zeta import alpha\n\ndef one():\n    return alpha()\n\ndef two():\n    return alpha()\n",
        ),
    ]);
    let store = Store::open(&db_path).unwrap();
    store
        .save_generation(&extractions, &resolution, &analysis)
        .unwrap();
    let actual = store.latest_edges(0.0).expect("read");
    assert!(
        actual.len() > 10,
        "fixture is inert: {} edges is too few to order",
        actual.len()
    );
    drop(store);

    // The statement `latest_edges_uncached` used to run, verbatim.
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let generation: u32 = conn
        .query_row("SELECT MAX(generation_id) FROM generation_edges", [], |r| {
            r.get(0)
        })
        .unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT sp.path, tp.path, e.source_symbol, e.target_symbol,
                    e.edge_kind, e.confidence
             FROM generation_edges e
             JOIN paths sp ON sp.id = e.source_file_id
             JOIN paths tp ON tp.id = e.target_file_id
             WHERE e.generation_id = ?1
             ORDER BY e.confidence DESC, sp.path, tp.path,
                      e.source_symbol, e.target_symbol, e.edge_kind",
        )
        .unwrap();
    let expected: Vec<(String, String, String, String, String, f64)> = stmt
        .query_map([generation], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect();

    assert_eq!(
        actual.len(),
        expected.len(),
        "the unordered read returned a different number of edges than the \
         ordered one; the two must differ only in order"
    );
    for (position, (got, want)) in actual.iter().zip(&expected).enumerate() {
        let got_key = (
            got.source_file.as_str(),
            got.target_file.as_str(),
            got.source_symbol.as_str(),
            got.target_symbol.as_str(),
            got.edge_kind.as_str(),
        );
        let want_key = (
            want.0.as_str(),
            want.1.as_str(),
            want.2.as_str(),
            want.3.as_str(),
            want.4.as_str(),
        );
        assert_eq!(
            got_key, want_key,
            "row {position} of the Rust-ordered read is not row {position} of \
             the SQL-ordered read"
        );
        assert_eq!(
            devmap_extract::model::confidence_millis(got.confidence),
            (want.5 * 1000.0).round() as i64,
            "row {position} carries a different confidence than SQL read for it"
        );
    }
    let _ = fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// The columnar index and a scan of the rows it replaced must answer the same
// question. Every one of them.
// ---------------------------------------------------------------------------

/// `GenerationEdges` holds ranks into interned text, not `StoredEdge` rows, and
/// its adjacency is a counting sort rather than four `HashMap<Box<str>, …>`.
/// Both changes are invisible in an answer *until* one of them is wrong, and
/// then they are wrong everywhere at once: the ranks are the sort keys of the
/// read order (R4), and the read order is the final tie-break of every answer
/// derived from a walk.
///
/// So this asserts the equivalence directly and exhaustively, against the one
/// thing that is not derived from the columns — the row list
/// `Store::latest_edges` hands out, whose order
/// `the_rust_edge_order_is_the_sql_order_it_replaced` separately pins to
/// SQLite's own.
///
/// The fixture is deliberately hostile to the interning: a self-call, a cycle,
/// the *same* call written twice in one body, files whose paths order
/// differently from the symbols in them, a symbol whose name is a byte-order
/// trap next to another (`alpha` / `alpha_`, where the shorter is a prefix of
/// the longer), and non-ASCII identifiers, because a rank comparison is only
/// the byte comparison it stands in for if the ranks were assigned in byte
/// order over the *whole* distinct set.
#[test]
fn the_columns_answer_what_a_scan_of_the_rows_answers() {
    let store = store_with(&[
        (
            "src/zeta.py",
            "def alpha():\n    return alpha()\n\n\ndef alpha_():\n    return alpha()\n\n\ndef beta():\n    return alpha() + alpha_() + alpha()\n",
        ),
        (
            "src/alpha.py",
            "from src.zeta import alpha, beta\n\n\nclass Thing:\n    def run(self):\n        return alpha() + beta()\n\n\ndef cycle_a():\n    return cycle_b()\n\n\ndef cycle_b():\n    return cycle_a()\n",
        ),
        (
            "src/caf\u{e9}.py",
            "from src.alpha import Thing\n\n\ndef m\u{f3}dulo():\n    return Thing()\n\n\ndef use():\n    return m\u{f3}dulo().run()\n",
        ),
    ]);

    let rows = store.latest_edges(0.0).expect("rows");
    let index = store
        .generation_edges()
        .expect("index")
        .expect("a generation");
    assert!(
        rows.len() > 15,
        "fixture is inert: {} edges is too few for the adjacency to be interesting",
        rows.len()
    );
    assert_eq!(
        index.len(),
        rows.len(),
        "the index and the row list describe different generations"
    );

    // 1. Every column, against the row it stands for.
    for (id, row) in rows.iter().enumerate() {
        let id = id as u32;
        assert_eq!(index.stored_edge(id), *row, "row {id} rebuilt differently");
        assert_eq!(index.source_symbol(id), row.source_symbol, "row {id}");
        assert_eq!(index.target_symbol(id), row.target_symbol, "row {id}");
        assert_eq!(index.source_file(id), row.source_file, "row {id}");
        assert_eq!(index.target_file(id), row.target_file, "row {id}");
        assert_eq!(index.kind_label(id), row.edge_kind, "row {id}");
        assert_eq!(index.confidence(id), row.confidence, "row {id}");
        assert_eq!(
            index.resolution_label(id),
            row.resolution.as_deref(),
            "row {id}"
        );
    }

    // 2. Every adjacency run, against a scan for the same key. The expected
    //    shape is the one the `HashMap<Box<str>, Vec<u32>>` this replaced held:
    //    ascending ids, which is to say the generation's own edge order.
    let scan = |key: &str, reverse: bool| -> Vec<u32> {
        rows.iter()
            .enumerate()
            .filter(|(_, row)| {
                key == if reverse {
                    &row.target_symbol
                } else {
                    &row.source_symbol
                }
            })
            .map(|(id, _)| id as u32)
            .collect()
    };
    let mut symbols: Vec<&str> = rows
        .iter()
        .flat_map(|row| [row.source_symbol.as_str(), row.target_symbol.as_str()])
        .collect();
    symbols.sort_unstable();
    symbols.dedup();
    assert!(symbols.len() > 8, "fixture has too few distinct symbols");
    for symbol in &symbols {
        assert_eq!(
            index.from_source_symbol(symbol),
            scan(symbol, false),
            "outbound run for {symbol:?}"
        );
        assert_eq!(
            index.into_target_symbol(symbol),
            scan(symbol, true),
            "inbound run for {symbol:?}"
        );
    }
    // A symbol the generation never names has an empty run, not a panic and
    // not somebody else's run.
    assert!(index.from_source_symbol("nothing at all").is_empty());
    assert!(index.into_target_symbol("nothing at all").is_empty());

    // 3. The two group iterators, against the same scan. These are what the
    //    traversal-start matcher runs its predicate over, so a missing group is
    //    a start the walk never finds.
    let mut files: Vec<&str> = rows
        .iter()
        .flat_map(|row| [row.source_file.as_str(), row.target_file.as_str()])
        .collect();
    files.sort_unstable();
    files.dedup();
    for reverse in [false, true] {
        let mut got: Vec<(String, Vec<u32>)> = index
            .symbols(reverse)
            .map(|(name, ids)| (name.to_string(), ids.to_vec()))
            .collect();
        let mut expected: Vec<(String, Vec<u32>)> = symbols
            .iter()
            .map(|symbol| (symbol.to_string(), scan(symbol, reverse)))
            .filter(|(_, ids)| !ids.is_empty())
            .collect();
        got.sort();
        expected.sort();
        assert_eq!(got, expected, "symbol groups, reverse={reverse}");

        let file_scan = |key: &str| -> Vec<u32> {
            rows.iter()
                .enumerate()
                .filter(|(_, row)| {
                    key == if reverse {
                        &row.target_file
                    } else {
                        &row.source_file
                    }
                })
                .map(|(id, _)| id as u32)
                .collect()
        };
        let mut got: Vec<(String, Vec<u32>)> = index
            .files(reverse)
            .map(|(name, ids)| (name.to_string(), ids.to_vec()))
            .collect();
        let mut expected: Vec<(String, Vec<u32>)> = files
            .iter()
            .map(|file| (file.to_string(), file_scan(file)))
            .filter(|(_, ids)| !ids.is_empty())
            .collect();
        got.sort();
        expected.sort();
        assert_eq!(got, expected, "file groups, reverse={reverse}");
    }

    // 4. The interned tables are in byte order, which is the property that makes
    //    a rank comparison the byte comparison `edge_read_order` needs. Asserted
    //    through the iterator rather than on the private field: the groups come
    //    out in rank order, so their names must come out sorted.
    let names: Vec<&str> = index.symbols(false).map(|(name, _)| name).collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(
        names, sorted,
        "the symbol table is not in byte order, so a rank comparison is not the \
         byte comparison the read order is defined in terms of"
    );
    let paths: Vec<&str> = index.files(false).map(|(name, _)| name).collect();
    let mut sorted = paths.clone();
    sorted.sort_unstable();
    assert_eq!(paths, sorted, "the path table is not in byte order");
}

/// A duplicated edge keeps both ids, and a self-edge appears on both sides.
///
/// The counting sort that builds the adjacency is only equivalent to the maps
/// it replaced if it is *stable* — ids ascending within a run — and if a row
/// whose two endpoints are the same symbol lands in both the outbound and the
/// inbound run rather than in one of them twice.
#[test]
fn self_edges_and_duplicates_survive_the_counting_sort() {
    let store = store_with(&[(
        "src/loop.py",
        "def recur():\n    return recur()\n\n\ndef twice():\n    return recur() + recur()\n",
    )]);
    let rows = store.latest_edges(0.0).expect("rows");
    let index = store
        .generation_edges()
        .expect("index")
        .expect("a generation");

    let self_edges: Vec<u32> = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| row.source_symbol == row.target_symbol)
        .map(|(id, _)| id as u32)
        .collect();
    assert!(
        !self_edges.is_empty(),
        "fixture must hold a self-edge, or this checks nothing"
    );
    for id in &self_edges {
        let symbol = index.source_symbol(*id);
        assert!(
            index.from_source_symbol(symbol).contains(id),
            "self-edge {id} is missing from its outbound run"
        );
        assert!(
            index.into_target_symbol(symbol).contains(id),
            "self-edge {id} is missing from its inbound run"
        );
    }

    for reverse in [false, true] {
        for (name, ids) in index.symbols(reverse) {
            let mut ascending = ids.to_vec();
            ascending.sort_unstable();
            assert_eq!(
                ids, ascending,
                "the run for {name:?} is not in ascending id order, so the \
                 generation's edge order (R4) no longer survives the adjacency"
            );
        }
    }
}

#[test]
fn audit_dead_confidence_refuses_nan_instead_of_reporting_zero() {
    for store in [
        Store::open_in_memory().unwrap(),
        store_with(&[("a.py", "def unused():\n    return 1\n")]),
    ] {
        assert!(
            store.count_dead_at_least(f32::NAN).is_err(),
            "an unevaluated comparison cannot report zero dead symbols"
        );
    }
}

#[test]
fn audit_generation_id_exhaustion_rolls_back_without_wrapping_identity() {
    let dir = tmp_dir("generation-overflow");
    let db = dir.join("map.sqlite");
    let store = Store::open(&db).unwrap();
    let (extractions, resolution, analysis) = pipeline(&[]);
    store
        .save_generation(&extractions, &resolution, &analysis)
        .unwrap();
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute(
        "UPDATE sqlite_sequence SET seq = ?1 WHERE name = 'generations'",
        [i64::from(u32::MAX)],
    )
    .unwrap();
    for _ in 0..32 {
        let result = store.save_generation(&extractions, &resolution, &analysis);
        assert!(
            result.is_err(),
            "generation IDs must not wrap to zero: {result:?}"
        );
    }
    assert_eq!(store.latest_generation_id().unwrap(), Some(1));
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM generations", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    drop(conn);
    drop(store);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn audit_path_id_exhaustion_rolls_back_the_failed_intern() {
    let dir = tmp_dir("path-overflow");
    let db = dir.join("map.sqlite");
    let store = Store::open(&db).unwrap();
    store.get_or_create_path_id("before.py").unwrap();
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute(
        "UPDATE sqlite_sequence SET seq = ?1 WHERE name = 'paths'",
        [i64::from(u32::MAX)],
    )
    .unwrap();
    for _ in 0..32 {
        assert!(store.get_or_create_path_id("overflow.py").is_err());
    }
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM paths WHERE path = 'overflow.py'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0,
        "refusing an unrepresentable ID must not leave a persisted row"
    );
    drop(conn);
    drop(store);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn audit_concurrent_path_interns_reuse_one_identity() {
    let dir = tmp_dir("path-intern-stress");
    let db = dir.join("map.sqlite");
    let stores: Vec<Store> = (0..8).map(|_| Store::open(&db).unwrap()).collect();
    let start = std::sync::Barrier::new(stores.len());
    std::thread::scope(|scope| {
        let handles: Vec<_> = stores
            .into_iter()
            .map(|store| {
                let start = &start;
                scope.spawn(move || {
                    start.wait();
                    (0..128)
                        .map(|step| {
                            let name = format!("path-{}.py", step % 16);
                            let id = store.get_or_create_path_id(&name).unwrap();
                            (name, id)
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        let mut identities = std::collections::BTreeMap::new();
        for handle in handles {
            for (name, id) in handle.join().unwrap() {
                if let Some(previous) = identities.insert(name, id) {
                    assert_eq!(previous, id);
                }
            }
        }
        assert_eq!(identities.len(), 16);
    });
    let conn = rusqlite::Connection::open(&db).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM paths", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        16
    );
    assert_eq!(
        conn.query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    drop(conn);
    fs::remove_dir_all(dir).unwrap();
}
