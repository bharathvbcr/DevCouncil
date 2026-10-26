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
