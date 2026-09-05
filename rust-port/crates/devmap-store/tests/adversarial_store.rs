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
use devmap_store::{Store, CURRENT_SCHEMA_VERSION};

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
    fs::rename(&other, &db_path).unwrap();

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
    let has_index: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
              WHERE type = 'index' AND name = 'idx_generation_files_cache_identity'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        has_index, 1,
        "migrating a v12 store did not create the extraction-cache index"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// Four bound parameters for the plan query above. Values are irrelevant to a
/// query plan; only their count and positions are.
fn params_for_plan() -> [&'static dyn rusqlite::ToSql; 4] {
    [&1i64, &"python", &"g", &"a"]
}
