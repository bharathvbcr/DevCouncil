//! What a generation write stores for each freshly extracted file, and in what
//! order.
//!
//! The payload JSON of a fresh row is produced before the row is inserted, and
//! how it is produced -- one extraction at a time, or a chunk at a time on
//! several threads -- is an implementation choice. These tests pin what must
//! not depend on that choice:
//!
//! * **Characterization.** Every `file_payloads` row holds exactly the three
//!   documents the contract names, the payload ids follow extraction order, and
//!   a carried file does not get a new payload. They were written against the
//!   serial loop and pass on it.
//! * **Determinism.** Two stores written from the same input, under any thread
//!   count and any chunk size, hold identical rows in every table, ids
//!   included. Row ids are assigned by insertion order, so identical tables are
//!   the proof that the insert order did not move.

use super::write::{serialize_payload, PayloadJson, PayloadTuning};
use super::*;
use devmap_analyze::analyze;
use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
use devmap_resolve::Resolver;
use rusqlite::params;

/// One generation's input: the extractions to write and what is declared
/// about them.
struct Step {
    extractions: Vec<Extraction>,
    opts: GenerationWriteOpts,
}

/// `modules` Python files, one of them syntactically broken so that a
/// `parse_outcome` is more than the default, plus a hub that imports two of
/// them. `revision` changes only the hub and the files named in `edited`.
fn sources(modules: usize, revision: usize, edited: &[usize]) -> Vec<(String, String)> {
    let mut files: Vec<(String, String)> = (0..modules)
        .map(|index| {
            let bump = if edited.contains(&index) { revision } else { 0 };
            (
                format!("pkg/mod_{index}.py"),
                format!(
                    "VALUE_{index} = 'lit-{index}-{bump}'\n\n\ndef leaf_{index}():\n    return {bump}\n\n\ndef caller_{index}():\n    return leaf_{index}() + nowhere_{index}()\n"
                ),
            )
        })
        .collect();
    files.push((
        "pkg/broken.py".to_string(),
        format!("def broken(:\n    return {revision}\n"),
    ));
    files
}

fn extract_all(files: &[(String, String)]) -> Vec<Extraction> {
    files
        .iter()
        .map(|(path, body)| extract_file(path, body))
        .collect()
}

fn resolve_and_analyze(
    extractions: &[Extraction],
) -> (devmap_resolve::model::ResolutionResult, AnalysisSummary) {
    let mut resolver = Resolver::new();
    resolver.index_extractions(extractions);
    let resolution = resolver.resolve_all(extractions).unwrap();
    let analysis = analyze(extractions, &resolution);
    (resolution, analysis)
}

/// Three generations over `modules` files: a cold write, an edit to one file,
/// and an edit to another together with the deletion of a third.
fn scenario(modules: usize) -> Vec<Step> {
    let cold = extract_all(&sources(modules, 0, &[]));
    let edit = extract_all(&sources(modules, 1, &[0]));
    let mut last_files = sources(modules, 2, &[0, 2.min(modules.saturating_sub(1))]);
    let deleted = "pkg/mod_1.py".to_string();
    last_files.retain(|(path, _)| *path != deleted);
    let last = extract_all(&last_files);
    let mut steps = vec![Step {
        extractions: cold,
        opts: GenerationWriteOpts::default(),
    }];
    if modules > 0 {
        steps.push(Step {
            extractions: edit,
            opts: GenerationWriteOpts {
                affected_paths: vec!["pkg/mod_0.py".to_string(), "pkg/broken.py".to_string()],
                ..GenerationWriteOpts::default()
            },
        });
    }
    if modules > 2 {
        steps.push(Step {
            extractions: last,
            opts: GenerationWriteOpts {
                affected_paths: vec![
                    "pkg/mod_0.py".to_string(),
                    "pkg/mod_2.py".to_string(),
                    "pkg/broken.py".to_string(),
                ],
                deleted_paths: vec![deleted],
                ..GenerationWriteOpts::default()
            },
        });
    }
    steps
}

fn write_step(store: &Store, step: &Step) -> Result<u32> {
    let (resolution, analysis) = resolve_and_analyze(&step.extractions);
    store.save_generation_with_opts(&step.extractions, &resolution, &analysis, step.opts.clone())
}

/// Every row of every table the write path fills, as one sorted list of
/// quoted rows per table.
///
/// Wall-clock columns and the history table are left out: they differ between
/// two runs of the same input by construction. Everything else is compared,
/// the autoincrement ids included, which is the point.
fn dump(store: &Store) -> Vec<(String, Vec<String>)> {
    const SKIPPED_COLUMNS: &[&str] = &["created_at"];
    let conn = lock_conn(&store.conn).expect("connection");
    let tables: Vec<String> = conn
        .prepare(
            "SELECT name FROM sqlite_master
              WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
                AND name NOT IN ('build_history', 'extraction_cache', 'extraction_retry',
                                 'extraction_cache_v4', 'pending_state', 'pending_paths')
                AND name NOT LIKE 'nodes_fts\\_%' ESCAPE '\\' OR name = 'nodes_fts_map'
              ORDER BY name",
        )
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<_>>()
        .unwrap();
    let mut out = Vec::new();
    for table in tables {
        let columns: Vec<String> = conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<_>>()
            .unwrap();
        let columns: Vec<String> = columns
            .into_iter()
            .filter(|column| !SKIPPED_COLUMNS.contains(&column.as_str()))
            .collect();
        let expr = columns
            .iter()
            .map(|column| format!("quote({column})"))
            .collect::<Vec<_>>()
            .join(" || '|' || ");
        let mut rows: Vec<String> = conn
            .prepare(&format!("SELECT {expr} FROM {table}"))
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<_>>()
            .unwrap();
        rows.sort();
        out.push((table, rows));
    }
    out
}

fn rows_of<'a>(dump: &'a [(String, Vec<String>)], table: &str) -> &'a [String] {
    dump.iter()
        .find(|(name, _)| name == table)
        .map(|(_, rows)| rows.as_slice())
        .unwrap_or_else(|| panic!("no table {table} in the dump"))
}

/// The stored payload documents of one path, newest first.
fn payloads_of(store: &Store, path: &str) -> Vec<(i64, String, String, String)> {
    let conn = lock_conn(&store.conn).expect("connection");
    let rows: Vec<(i64, String, String, String)> = conn
        .prepare(
            "SELECT fp.payload_id, fp.parse_outcome_json, fp.engine_json, fp.extraction_json
           FROM file_payloads fp JOIN paths p ON p.id = fp.file_id
          WHERE p.path = ?1 ORDER BY fp.payload_id DESC",
        )
        .unwrap()
        .query_map(params![path], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .unwrap()
        .collect::<Result<_>>()
        .unwrap();
    rows
}

fn payload_order(store: &Store) -> Vec<String> {
    let conn = lock_conn(&store.conn).expect("connection");
    let rows: Vec<String> = conn
        .prepare(
            "SELECT p.path FROM file_payloads fp JOIN paths p ON p.id = fp.file_id
          ORDER BY fp.payload_id",
        )
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<_>>()
        .unwrap();
    rows
}

/// The three documents a fresh row is contracted to hold, built the way the
/// contract states them rather than by calling the code under test.
fn expected_documents(extraction: &Extraction) -> (String, String, String) {
    let mut durable = extraction.for_durable_store();
    durable.source_code = None;
    (
        serde_json::to_string(&extraction.parse_outcome).unwrap(),
        serde_json::to_string(&extraction.engine).unwrap(),
        serde_json::to_string(&durable).unwrap(),
    )
}

/// A fresh row holds the parse outcome, the engine and the durable extraction,
/// each as the JSON `serde_json` produces for it, and nothing else.
#[test]
fn a_fresh_payload_row_holds_the_three_documents_the_contract_names() {
    let store = Store::open_in_memory().expect("store");
    let steps = scenario(6);
    write_step(&store, &steps[0]).expect("cold write");
    for extraction in &steps[0].extractions {
        let rows = payloads_of(&store, &extraction.file_path);
        assert_eq!(rows.len(), 1, "{}: one payload", extraction.file_path);
        let (_, parse, engine, body) = &rows[0];
        let (want_parse, want_engine, want_body) = expected_documents(extraction);
        assert_eq!(
            parse, &want_parse,
            "{}: parse outcome",
            extraction.file_path
        );
        assert_eq!(engine, &want_engine, "{}: engine", extraction.file_path);
        assert_eq!(body, &want_body, "{}: extraction", extraction.file_path);
    }
}

/// Payload ids are handed out in extraction order, because the insert order is
/// extraction order. A write that serialized in parallel and inserted in
/// completion order would reorder these and still store every document.
#[test]
fn payload_ids_follow_extraction_order() {
    let store = Store::open_in_memory().expect("store");
    let steps = scenario(40);
    write_step(&store, &steps[0]).expect("cold write");
    let want: Vec<String> = steps[0]
        .extractions
        .iter()
        .map(|extraction| extraction.file_path.clone())
        .collect();
    assert_eq!(payload_order(&store), want);
}

/// A carried file keeps its payload; only the files the write names get new
/// ones, and a deleted file gets none.
#[test]
fn an_incremental_write_adds_payloads_only_for_the_files_it_rewrites() {
    let store = Store::open_in_memory().expect("store");
    let steps = scenario(6);
    assert_eq!(steps.len(), 3);
    write_step(&store, &steps[0]).expect("cold write");
    let cold_order = payload_order(&store);
    assert_eq!(cold_order.len(), 7, "six modules and the broken file");

    write_step(&store, &steps[1]).expect("edit write");
    let mut order = payload_order(&store);
    assert_eq!(
        &order[cold_order.len()..],
        &["pkg/mod_0.py".to_string(), "pkg/broken.py".to_string()],
        "the two affected files, in extraction order, after the cold payloads"
    );

    write_step(&store, &steps[2]).expect("edit and delete write");
    order = payload_order(&store);
    assert_eq!(
        &order[cold_order.len() + 2..],
        &[
            "pkg/mod_0.py".to_string(),
            "pkg/mod_2.py".to_string(),
            "pkg/broken.py".to_string()
        ],
        "the three affected files; the deleted file gets none"
    );
    assert_eq!(
        payloads_of(&store, "pkg/mod_1.py").len(),
        1,
        "the deleted file keeps only the payload of the generation that had it"
    );
    let live: i64 = lock_conn(&store.conn)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM generation_file_rows WHERE generation_id = 3",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(live, 6, "seven files less the deleted one");
}

/// The dump is not vacuous: the tables the comparison is about hold rows.
#[test]
fn the_table_dump_covers_the_relations_the_write_fills() {
    let store = Store::open_in_memory().expect("store");
    for step in scenario(6) {
        write_step(&store, &step).expect("write");
    }
    let dump = dump(&store);
    for table in [
        "paths",
        "file_payloads",
        "generation_file_rows",
        "generation_nodes",
        "generation_literals",
        "edge_rows",
        "generation_file_digests",
        "nodes_fts_map",
    ] {
        assert!(
            !rows_of(&dump, table).is_empty(),
            "{table} is empty, so comparing it proves nothing"
        );
    }
}

// --- What must not depend on how the documents are produced -----------------

fn pool(threads: usize) -> rayon::ThreadPool {
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .expect("thread pool")
}

fn write_step_tuned(store: &Store, step: &Step, tuning: &PayloadTuning<'_>) -> Result<u32> {
    let (resolution, analysis) = resolve_and_analyze(&step.extractions);
    store
        .save_generation_tuned(
            &step.extractions,
            &resolution,
            &analysis,
            step.opts.clone(),
            "unknown",
            tuning,
        )
        .map(|(generation, _)| generation)
}

fn tuned<'pool>(chunk: usize, pool: &'pool rayon::ThreadPool) -> PayloadTuning<'pool> {
    PayloadTuning {
        chunk,
        pool: Some(pool),
        serialize: serialize_payload,
    }
}

fn dump_after(steps: &[Step], tuning: &PayloadTuning<'_>) -> Vec<(String, Vec<String>)> {
    let store = Store::open_in_memory().expect("store");
    for step in steps {
        write_step_tuned(&store, step, tuning).expect("write");
    }
    dump(&store)
}

/// An empty generation, which has no fresh extraction at all.
fn empty_scenario() -> Vec<Step> {
    vec![Step {
        extractions: Vec::new(),
        opts: GenerationWriteOpts::default(),
    }]
}

/// Two writes of one input hold identical rows in every table, ids included,
/// whatever the thread count and whatever the chunk size -- on both sides of
/// every chunk boundary.
///
/// The reference is one thread and a chunk of one, which is the serial loop in
/// all but name. The fresh counts are chosen against the chunk sizes under
/// test: 0 (nothing to serialize), 1, one short of a chunk, exactly a chunk,
/// and a chunk plus one, for chunks of 4 and for the production chunk of 64.
#[test]
fn the_tables_do_not_depend_on_thread_count_or_chunk_size() {
    let serial_pool = pool(1);
    let serial = tuned(1, &serial_pool);
    let wide_pool = pool(8);
    let two_pool = pool(2);

    let production = PayloadTuning::default().chunk;
    let mut cases: Vec<(String, Vec<Step>)> = vec![("empty".to_string(), empty_scenario())];
    for fresh in [1, 3, 4, 5, 8, 9, production, production + 1] {
        // A cold write holds `modules + 1` files: the modules and the broken one.
        cases.push((format!("{fresh} fresh"), scenario(fresh - 1)));
    }

    for (label, steps) in &cases {
        let want = dump_after(steps, &serial);
        for (chunk, pool) in [
            (1, &wide_pool),
            (2, &two_pool),
            (4, &wide_pool),
            (4, &serial_pool),
            (production, &wide_pool),
            (usize::MAX, &wide_pool),
        ] {
            assert_eq!(
                dump_after(steps, &tuned(chunk, pool)),
                want,
                "{label}: chunk {chunk} on {} threads wrote different rows than the serial loop",
                pool.current_num_threads()
            );
        }
        assert_eq!(
            dump_after(steps, &PayloadTuning::default()),
            want,
            "{label}: the production tuning wrote different rows than the serial loop"
        );
    }
}

/// A serializer that fails for two files, so that which one the write names
/// shows whether the first in iteration order wins.
fn fails_on_mod_3_and_mod_5(extraction: &Extraction) -> Result<PayloadJson> {
    if extraction.file_path.ends_with("/mod_3.py") || extraction.file_path.ends_with("/mod_5.py") {
        return Err(refusal(format!(
            "injected serialization failure for {}",
            extraction.file_path
        )));
    }
    serialize_payload(extraction)
}

fn panics_on_mod_2_and_mod_4(extraction: &Extraction) -> Result<PayloadJson> {
    if extraction.file_path.ends_with("/mod_2.py") || extraction.file_path.ends_with("/mod_4.py") {
        panic!("injected panic for {}", extraction.file_path);
    }
    serialize_payload(extraction)
}

fn nothing_was_written(store: &Store) {
    let conn = lock_conn(&store.conn).expect("connection: the store must not be poisoned");
    for table in [
        "generations",
        "paths",
        "file_payloads",
        "generation_file_rows",
        "generation_nodes",
        "generation_literals",
    ] {
        let rows: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(rows, 0, "{table} kept rows of a write that failed");
    }
}

/// The write fails at the first failing extraction in iteration order, with
/// that extraction's message, however the extractions are split into chunks
/// and however many threads reach the failures first; and the failed write
/// leaves nothing behind.
#[test]
fn a_serialization_failure_names_the_first_failing_extraction_and_rolls_back() {
    let steps = scenario(8);
    let serial_pool = pool(1);
    let wide_pool = pool(8);
    let mut seen = Vec::new();
    for (chunk, pool) in [
        (1, &serial_pool),
        (2, &wide_pool),
        (4, &wide_pool),
        (usize::MAX, &wide_pool),
        (usize::MAX, &serial_pool),
    ] {
        let store = Store::open_in_memory().expect("store");
        let tuning = PayloadTuning {
            serialize: fails_on_mod_3_and_mod_5,
            ..tuned(chunk, pool)
        };
        let error = write_step_tuned(&store, &steps[0], &tuning)
            .expect_err("a failing serializer must fail the write")
            .to_string();
        assert!(
            error.contains("injected serialization failure for pkg/mod_3.py"),
            "chunk {chunk}: the first failing extraction in iteration order is mod_3: {error}"
        );
        assert!(
            !error.contains("mod_5.py"),
            "chunk {chunk}: a later failure pre-empted the first: {error}"
        );
        nothing_was_written(&store);
        // The store is usable after the failed write.
        write_step_tuned(&store, &steps[0], &tuned(chunk, pool))
            .expect("a good write after a failed one");
        seen.push(error);
    }
    assert!(
        seen.windows(2).all(|pair| pair[0] == pair[1]),
        "the refusal text must not depend on the chunking: {seen:?}"
    );
}

/// A serializer that panics does not take the process's store with it: the
/// write fails naming the file, rolls back, and the connection mutex is not
/// poisoned.
#[test]
fn a_panic_in_the_serializer_is_a_failed_write_not_a_poisoned_store() {
    let steps = scenario(8);
    let wide_pool = pool(8);
    for chunk in [1, 4, usize::MAX] {
        let store = Store::open_in_memory().expect("store");
        let tuning = PayloadTuning {
            serialize: panics_on_mod_2_and_mod_4,
            ..tuned(chunk, &wide_pool)
        };
        let error = write_step_tuned(&store, &steps[0], &tuning)
            .expect_err("a panicking serializer must fail the write")
            .to_string();
        assert!(
            error.contains("panicked for pkg/mod_2.py") && error.contains("injected panic"),
            "chunk {chunk}: the first panicking extraction is named with its reason: {error}"
        );
        nothing_was_written(&store);
        write_step_tuned(&store, &steps[0], &tuned(chunk, &wide_pool))
            .expect("the store is not poisoned by the caught panic");
        assert_eq!(payload_order(&store).len(), 9);
    }
}

/// An insert that fails before a later extraction's serialization failure is
/// the error the write reports, as in the serial loop, where the later
/// extraction was never reached. A chunk that serialized the later one first
/// must not change that.
#[test]
fn an_earlier_insert_failure_is_not_pre_empted_by_a_later_serialization_failure() {
    let steps = scenario(8);
    let wide_pool = pool(8);
    let mut seen = Vec::new();
    for chunk in [1, 4, usize::MAX] {
        let store = Store::open_in_memory().expect("store");
        lock_conn(&store.conn)
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER refuse_mod_2 BEFORE INSERT ON file_payloads
                 WHEN NEW.file_id = (SELECT id FROM paths WHERE path = 'pkg/mod_2.py')
                 BEGIN SELECT RAISE(ABORT, 'injected insert failure'); END;",
            )
            .unwrap();
        let tuning = PayloadTuning {
            serialize: fails_on_mod_3_and_mod_5,
            ..tuned(chunk, &wide_pool)
        };
        let error = write_step_tuned(&store, &steps[0], &tuning)
            .expect_err("both failures are armed")
            .to_string();
        assert!(
            error.contains("injected insert failure"),
            "chunk {chunk}: mod_2's insert fails before mod_3 is reached: {error}"
        );
        seen.push(error);
    }
    assert!(seen.windows(2).all(|pair| pair[0] == pair[1]), "{seen:?}");
}

/// A chunk of nothing is not a tuning. `chunks(0)` would panic; the write
/// refuses it before it touches the store.
#[test]
fn a_zero_chunk_is_refused_not_a_panic() {
    let store = Store::open_in_memory().expect("store");
    let steps = scenario(3);
    let one = pool(1);
    let error = write_step_tuned(&store, &steps[0], &tuned(0, &one))
        .expect_err("a zero chunk")
        .to_string();
    assert!(error.contains("at least one extraction"), "{error}");
    nothing_was_written(&store);
}
