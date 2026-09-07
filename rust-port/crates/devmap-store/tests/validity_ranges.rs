//! Schema v18 — edges and unresolved calls are stored as validity ranges.
//!
//! Measured on this repository before the change, six consecutive builds with a
//! single line appended to one file between each (`probe.sh`, release kernel,
//! schema 17):
//!
//! ```text
//!   cold      store 158,416,896  gens 1  generation_edges 102,078  generation_unresolved  89,743
//!   edit 1    store 228,196,352  gens 2  generation_edges 204,157  generation_unresolved 179,486
//!   edit 2-5  store ~228,200,000 gens 2  generation_edges 204,16x  generation_unresolved 179,486
//! ```
//!
//! One changed file rewrote **191,822 rows** and took the store from 158 MB to
//! 228 MB. What actually changed between those two generations, compared
//! NULL-safe over the whole stored tuple, was **one edge row and zero
//! unresolved rows**:
//!
//! ```text
//!   edges       101,446 distinct tuples -> 101,447   appeared 1  disappeared 0
//!   unresolved   65,567 distinct tuples ->  65,567   appeared 0  disappeared 0
//! ```
//!
//! So the rows are not re-derived because they changed; they are re-derived
//! because the relation is keyed by generation. v18 keys them by a
//! `[valid_from, valid_to)` range instead: a build compares the freshly
//! resolved tuple multiset against the currently-valid one in memory, closes
//! what disappeared, inserts what appeared, and leaves the rest untouched.
//!
//! Two facts from the same measurement shape these tests, and both are easy to
//! get wrong:
//!
//! * **Duplicate tuples are real.** 475 edge tuples and 12,424 unresolved
//!   tuples occur more than once in one generation (1,111 and 36,600 rows).
//!   A set-valued delta would silently drop them, and the write path's
//!   `edge_ord == analysis.total_edges` equality would then refuse the build.
//! * **Two edges can differ only in `resolution`.** Six rows on this corpus
//!   tie on every key `edge_read_order` compares before `ordinal` — for
//!   example `tests/unit/test_local_llm_calibration.py -> …config.py` at
//!   confidence 1, once as `ImportScoped` and once as `ReceiverType`. Their
//!   relative order was decided by the resolver's emission ordinal, which a
//!   validity-range store cannot carry forward. The comparator takes
//!   `resolution` as its final key instead, which orders exactly those rows
//!   and leaves the remaining ties between rows that are byte-identical.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use devmap_analyze::{analyze, AnalysisSummary};
use devmap_extract::extract_file;
use devmap_extract::model::EdgeKind;
use devmap_resolve::model::{
    Resolution, ResolutionResult, ResolvedEdge, UnresolvedClass, UnresolvedKind,
    UnresolvedReference,
};
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};
use rusqlite::Connection;

fn tmp_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "devmap-{label}-{}-{stamp}-{seq}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Extract, resolve and analyse a small tree, exactly as a build does.
fn pipeline(
    files: &[(&str, &str)],
) -> (
    Vec<devmap_extract::Extraction>,
    ResolutionResult,
    AnalysisSummary,
) {
    let extractions: Vec<_> = files
        .iter()
        .map(|(path, body)| extract_file(path, body))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let analysis = analyze(&extractions, &resolution);
    (extractions, resolution, analysis)
}

/// How many rows the database physically holds for the edges of *every*
/// retained generation.
///
/// Shape-independent on purpose. Before v18 the rows live in the
/// `generation_edges` table itself; after it they live in `edge_rows` and
/// `generation_edges` is a view over the validity ranges. The property under
/// test — "storing the same edge set twice does not store it twice" — is about
/// the bytes on disk, not about which relation holds them, and a probe that
/// only knew one shape could not state it.
fn stored_edge_rows(conn: &Connection) -> i64 {
    let relation = base_relation(conn, "generation_edges", "edge_rows");
    conn.query_row(&format!("SELECT COUNT(*) FROM {relation}"), [], |row| {
        row.get(0)
    })
    .unwrap()
}

/// The same probe for the unresolved-call ledger.
fn stored_unresolved_rows(conn: &Connection) -> i64 {
    let relation = base_relation(conn, "generation_unresolved", "unresolved_rows");
    conn.query_row(&format!("SELECT COUNT(*) FROM {relation}"), [], |row| {
        row.get(0)
    })
    .unwrap()
}

fn base_relation(conn: &Connection, view_name: &str, base_name: &str) -> String {
    let is_view: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name = ?1 AND type = 'view'",
            [view_name],
            |row| row.get::<_, i64>(0),
        )
        .map(|count| count > 0)
        .unwrap();
    if is_view {
        base_name.to_string()
    } else {
        view_name.to_string()
    }
}

/// Every edge of a generation, as the readers see it, in read order.
fn edges_at(conn: &Connection, generation: u32) -> Vec<String> {
    let mut stmt = conn
        .prepare(
            "SELECT sp.path, tp.path, e.source_symbol, e.target_symbol, e.edge_kind,
                    printf('%.17g', e.confidence), COALESCE(e.resolution, '<none>'),
                    COALESCE(CAST(e.candidate_total AS TEXT), '<none>')
             FROM generation_edges e
             JOIN paths sp ON sp.id = e.source_file_id
             JOIN paths tp ON tp.id = e.target_file_id
             WHERE e.generation_id = ?1",
        )
        .unwrap();
    let mut rows: Vec<String> = stmt
        .query_map([generation], |row| {
            Ok(format!(
                "{}|{}|{}|{}|{}|{}|{}|{}",
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
            ))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    rows.sort();
    rows
}

fn unresolved_at(conn: &Connection, generation: u32) -> Vec<String> {
    let mut stmt = conn
        .prepare(
            "SELECT source_file, source_symbol, callee_name, reason, classification,
                    COALESCE(receiver, '<none>')
             FROM generation_unresolved WHERE generation_id = ?1",
        )
        .unwrap();
    let mut rows: Vec<String> = stmt
        .query_map([generation], |row| {
            Ok(format!(
                "{}|{}|{}|{}|{}|{}",
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    rows.sort();
    rows
}

/// A tree large enough that a whole generation of rows is visibly more than a
/// one-file delta, and small enough to build in a unit test.
fn tree(revision: usize, modules: usize) -> Vec<(String, String)> {
    let mut files: Vec<(String, String)> = (0..modules)
        .map(|index| {
            (
                format!("mod_{index}.py"),
                // The unattributed call is deliberate: it is what puts rows in
                // `generation_unresolved`, which is half of what this file is
                // about and is empty in a fixture where everything resolves.
                format!(
                    "def leaf_{index}():\n    return {index}\n\n\ndef caller_{index}():\n    return leaf_{index}()\n\n\ndef stray_{index}():\n    return nowhere_{index}()\n"
                ),
            )
        })
        .collect();
    files.push((
        "hub.py".to_string(),
        format!(
            "# rev {revision}\nfrom mod_0 import caller_0\nfrom mod_1 import caller_1\n\n\ndef hub():\n    return caller_0() + caller_1()\n"
        ),
    ));
    files
}

fn as_refs(files: &[(String, String)]) -> Vec<(&str, &str)> {
    files
        .iter()
        .map(|(path, body)| (path.as_str(), body.as_str()))
        .collect()
}

fn commit(store: &Store, files: &[(String, String)], affected: &[&str]) -> u32 {
    let (extractions, resolution, analysis) = pipeline(&as_refs(files));
    let opts = GenerationWriteOpts {
        affected_paths: affected.iter().map(|p| (*p).to_string()).collect(),
        ..Default::default()
    };
    store
        .save_generation_with_opts(&extractions, &resolution, &analysis, opts)
        .unwrap()
}

/// A one-line edit to one file must not rewrite the whole edge set.
///
/// The measured "before" is in the module header: 102,078 edge rows became
/// 204,157 for a delta of one tuple. At this fixture's scale the same shape is
/// exact — the second generation resolves to *the same* edge multiset, so a
/// validity-range store writes nothing at all.
#[test]
fn a_rebuild_with_the_same_edges_does_not_store_them_a_second_time() {
    let dir = tmp_dir("v18-edge-delta");
    let db = dir.join("index.sqlite");
    let store = Store::open(&db).unwrap();
    let files = tree(0, 12);

    let g1 = commit(&store, &files, &[]);
    let conn = Connection::open(&db).unwrap();
    let after_first = stored_edge_rows(&conn);
    assert!(
        after_first > 0,
        "fixture precondition: the first generation stored edges"
    );

    let g2 = commit(&store, &files, &["hub.py"]);
    let after_second = stored_edge_rows(&conn);

    assert_eq!(
        edges_at(&conn, g1),
        edges_at(&conn, g2),
        "fixture precondition: the two generations must hold the same edge set"
    );
    assert_eq!(
        after_second, after_first,
        "a second generation with an identical edge set stored {after_second} \
         physical edge rows where the first stored {after_first}: every edge of \
         the repository is re-materialised under the new generation id, which is \
         the 102,078 -> 204,157 growth measured on this corpus for a one-tuple \
         change"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// The same property for the unresolved-call ledger, which on this corpus does
/// not change *at all* between two one-file builds (89,743 rows, 0 appeared,
/// 0 disappeared) and is rewritten in full anyway.
#[test]
fn a_rebuild_with_the_same_unresolved_calls_does_not_store_them_a_second_time() {
    let dir = tmp_dir("v18-unresolved-delta");
    let db = dir.join("index.sqlite");
    let store = Store::open(&db).unwrap();
    let files = tree(0, 12);

    let g1 = commit(&store, &files, &[]);
    let conn = Connection::open(&db).unwrap();
    let after_first = stored_unresolved_rows(&conn);
    assert!(
        after_first > 0,
        "fixture precondition: the first generation stored unresolved calls"
    );

    let g2 = commit(&store, &files, &["hub.py"]);
    let after_second = stored_unresolved_rows(&conn);

    assert_eq!(
        unresolved_at(&conn, g1),
        unresolved_at(&conn, g2),
        "fixture precondition: the two generations must hold the same ledger"
    );
    assert_eq!(
        after_second, after_first,
        "a second generation with an identical unresolved ledger stored \
         {after_second} physical rows where the first stored {after_first}"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// Every generation still reads back exactly what it was given.
///
/// The equivalence the whole change rests on, asserted through the reader
/// rather than the base table: a generation whose rows are shared with its
/// predecessor must serve the same edge set a generation that owned them
/// outright did.
#[test]
fn each_generation_reads_back_exactly_the_edges_it_was_given() {
    let dir = tmp_dir("v18-per-gen-read");
    let db = dir.join("index.sqlite");
    let store = Store::open(&db).unwrap();

    let first = tree(0, 8);
    let g1 = commit(&store, &first, &[]);
    let conn = Connection::open(&db).unwrap();
    let edges_g1 = edges_at(&conn, g1);
    let unresolved_g1 = unresolved_at(&conn, g1);

    // A second revision that genuinely changes the graph: one module gains a
    // function that the hub calls, so edges both appear and disappear.
    let mut second = tree(1, 8);
    second[0].1 =
        "def leaf_0():\n    return 0\n\n\ndef extra_0():\n    return leaf_0()\n".to_string();
    let g2 = commit(&store, &second, &["mod_0.py", "hub.py"]);

    assert_eq!(
        edges_at(&conn, g1),
        edges_g1,
        "committing a newer generation changed what the older one serves"
    );
    assert_eq!(
        unresolved_at(&conn, g1),
        unresolved_g1,
        "committing a newer generation changed the older one's unresolved ledger"
    );
    assert_ne!(
        edges_at(&conn, g2),
        edges_g1,
        "fixture precondition: the second revision must change the edge set"
    );

    // And the newer generation is exactly what a store with no history stores.
    let cold_dir = tmp_dir("v18-per-gen-cold");
    let cold_db = cold_dir.join("index.sqlite");
    let cold_store = Store::open(&cold_db).unwrap();
    let cold_gen = commit(&cold_store, &second, &[]);
    let cold_conn = Connection::open(&cold_db).unwrap();
    assert_eq!(
        edges_at(&conn, g2),
        edges_at(&cold_conn, cold_gen),
        "an incremental generation must serve exactly the cold answer"
    );
    assert_eq!(
        unresolved_at(&conn, g2),
        unresolved_at(&cold_conn, cold_gen),
        "an incremental unresolved ledger must be exactly the cold one"
    );
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cold_dir);
}

/// An edge that disappears and comes back is a *new* row, not a reopened one.
///
/// The failure this forbids is the obvious optimisation: finding the closed row
/// for a returning tuple and clearing its `valid_to`. That makes the edge
/// retroactively present in the generation that did not have it.
#[test]
fn an_edge_that_disappears_and_returns_is_absent_from_the_generation_that_lost_it() {
    let dir = tmp_dir("v18-reappear");
    let db = dir.join("index.sqlite");
    let store = Store::open(&db).unwrap();

    let with_call = vec![
        (
            "mod.py".to_string(),
            "def leaf():\n    return 1\n".to_string(),
        ),
        (
            "hub.py".to_string(),
            "from mod import leaf\n\n\ndef hub():\n    return leaf()\n".to_string(),
        ),
    ];
    let without_call = vec![
        (
            "mod.py".to_string(),
            "def leaf():\n    return 1\n".to_string(),
        ),
        (
            "hub.py".to_string(),
            "from mod import leaf\n\n\ndef hub():\n    return 0\n".to_string(),
        ),
    ];

    let g1 = commit(&store, &with_call, &[]);
    let g2 = commit(&store, &without_call, &["hub.py"]);
    let g3 = commit(&store, &with_call, &["hub.py"]);

    let conn = Connection::open(&db).unwrap();
    let calls_at = |generation: u32| -> Vec<String> {
        edges_at(&conn, generation)
            .into_iter()
            .filter(|row| row.contains("|Calls|"))
            .collect()
    };

    assert!(
        !calls_at(g2).iter().any(|row| row.contains("hub.py::hub")),
        "generation {g2} dropped the call and must not serve it: {:?}",
        calls_at(g2)
    );
    assert_eq!(
        calls_at(g1),
        calls_at(g3),
        "the returning generation must serve exactly what the original did"
    );
    assert!(
        calls_at(g1).iter().any(|row| row.contains("hub.py::hub")),
        "fixture precondition: the first generation had the call"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// Duplicate tuples keep their multiplicity.
///
/// 475 edge tuples on this repository occur more than once in a single
/// generation. A delta computed over a *set* rather than a multiset stores one
/// of each, and `save_generation`'s `edge_ord == analysis.total_edges` equality
/// then refuses the next build — the store would be unwritable rather than
/// merely wrong.
#[test]
fn duplicate_edges_keep_their_multiplicity_across_a_rebuild() {
    let dir = tmp_dir("v18-dups");
    let db = dir.join("index.sqlite");
    let store = Store::open(&db).unwrap();

    // Two edges identical in every stored column but `resolution` — the shape
    // measured on this corpus (`ImportScoped` and `ReceiverType` both score
    // `DETERMINISTIC`), plus one exact duplicate.
    let make = || {
        let mut edges = vec![
            ResolvedEdge::resolved(
                "hub.py".into(),
                "mod.py".into(),
                "hub.py::hub".into(),
                "mod.py::leaf".into(),
                EdgeKind::Calls,
                Arc::new(Resolution::ImportScoped {
                    target_symbol: "leaf".into(),
                    target_file: "mod.py".into(),
                    imported_from: "mod".into(),
                }),
                None,
            ),
            ResolvedEdge::resolved(
                "hub.py".into(),
                "mod.py".into(),
                "hub.py::hub".into(),
                "mod.py::leaf".into(),
                EdgeKind::Calls,
                Arc::new(Resolution::ReceiverType {
                    target_symbol: "leaf".into(),
                    target_file: "mod.py".into(),
                    receiver_type: "Mod".into(),
                }),
                None,
            ),
        ];
        edges.push(edges[0].clone());
        ResolutionResult {
            edges,
            receiver_types: Default::default(),
            reexport_chains: Default::default(),
            unresolved: Vec::new(),
        }
    };

    let extractions = vec![
        extract_file("mod.py", "def leaf():\n    return 1\n"),
        extract_file("hub.py", "def hub():\n    return 0\n"),
    ];
    let resolution = make();
    let analysis = analyze(&extractions, &resolution);
    let g1 = store
        .save_generation(&extractions, &resolution, &analysis)
        .unwrap();

    let conn = Connection::open(&db).unwrap();
    let count_at = |generation: u32| -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM generation_edges WHERE generation_id = ?1",
            [generation],
            |row| row.get(0),
        )
        .unwrap()
    };
    assert_eq!(count_at(g1), 3, "fixture precondition: three edge rows");

    let resolution = make();
    let analysis = analyze(&extractions, &resolution);
    let g2 = store
        .save_generation_with_opts(
            &extractions,
            &resolution,
            &analysis,
            GenerationWriteOpts {
                affected_paths: vec!["hub.py".into()],
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        count_at(g2),
        3,
        "the rebuild lost a duplicate edge: a delta over a set rather than a \
         multiset collapses the two rows this fixture writes twice"
    );
    assert_eq!(edges_at(&conn, g1), edges_at(&conn, g2));
    assert_eq!(
        stored_edge_rows(&conn),
        3,
        "an unchanged multiset must not be re-stored"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// The read order is a function of the edges, not of the order they arrived in.
///
/// `edge_read_order`'s final key was the resolver's emission `ordinal`, which a
/// validity-range store cannot carry forward: a row's ordinal belongs to the
/// generation that first inserted it. Six rows on this repository are ordered by
/// that key alone — pairs identical except for `resolution`, at equal
/// confidence. Making `resolution` the final key orders exactly those, and the
/// remaining ties are between rows whose every read column agrees.
#[test]
fn the_read_order_does_not_depend_on_the_order_the_resolver_emitted_them() {
    let import_scoped = || {
        ResolvedEdge::resolved(
            "hub.py".into(),
            "mod.py".into(),
            "hub.py::hub".into(),
            "mod.py::leaf".into(),
            EdgeKind::Calls,
            Arc::new(Resolution::ImportScoped {
                target_symbol: "leaf".into(),
                target_file: "mod.py".into(),
                imported_from: "mod".into(),
            }),
            None,
        )
    };
    let receiver_type = || {
        ResolvedEdge::resolved(
            "hub.py".into(),
            "mod.py".into(),
            "hub.py::hub".into(),
            "mod.py::leaf".into(),
            EdgeKind::Calls,
            Arc::new(Resolution::ReceiverType {
                target_symbol: "leaf".into(),
                target_file: "mod.py".into(),
                receiver_type: "Mod".into(),
            }),
            None,
        )
    };
    let extractions = vec![
        extract_file("mod.py", "def leaf():\n    return 1\n"),
        extract_file("hub.py", "def hub():\n    return 0\n"),
    ];

    let read_back = |edges: Vec<ResolvedEdge>| -> Vec<String> {
        let resolution = ResolutionResult {
            edges,
            receiver_types: Default::default(),
            reexport_chains: Default::default(),
            unresolved: Vec::new(),
        };
        let analysis = analyze(&extractions, &resolution);
        let store = Store::open_in_memory().unwrap();
        store
            .save_generation(&extractions, &resolution, &analysis)
            .unwrap();
        store
            .latest_edges(0.0)
            .unwrap()
            .into_iter()
            .map(|edge| {
                format!(
                    "{}|{}|{}",
                    edge.source_symbol,
                    edge.target_symbol,
                    edge.resolution.unwrap_or_else(|| "<none>".into())
                )
            })
            .collect()
    };

    let forwards = read_back(vec![import_scoped(), receiver_type()]);
    let backwards = read_back(vec![receiver_type(), import_scoped()]);
    assert_eq!(
        forwards, backwards,
        "two stores holding the same two edges read them back in different \
         orders because the comparator's last key was the order the resolver \
         happened to emit them in"
    );
}

/// Pruning reclaims the rows no retained generation can see, and only those.
#[test]
fn pruning_reclaims_rows_no_retained_generation_can_see() {
    let dir = tmp_dir("v18-prune");
    let db = dir.join("index.sqlite");
    let store = Store::open(&db).unwrap();

    // Three revisions, each dropping the previous one's extra function, so
    // every generation closes rows the one before it had.
    let mut revisions = Vec::new();
    for revision in 0..3usize {
        let mut files = tree(revision, 6);
        files[0].1 = format!(
            "def leaf_0():\n    return {revision}\n\n\ndef caller_0():\n    return leaf_0()\n\n\ndef rev_{revision}():\n    return caller_0()\n"
        );
        revisions.push(files);
    }
    let generations: Vec<u32> = revisions
        .iter()
        .enumerate()
        .map(|(index, files)| commit(&store, files, if index == 0 { &[] } else { &["mod_0.py"] }))
        .collect();

    let conn = Connection::open(&db).unwrap();
    let newest = *generations.last().unwrap();
    let newest_edges = edges_at(&conn, newest);
    let before = stored_edge_rows(&conn);
    let middle = generations[generations.len() - 2];
    let middle_edges = edges_at(&conn, middle);

    // Retention first, reclaim second. A row closed *inside* the window is
    // still reachable — the older retained generation sees it — so a prune that
    // reclaimed by "is this row closed?" rather than by "is its end before the
    // oldest generation we kept?" would take it and leave that generation short.
    store.prune_generations_except_latest(2).unwrap();
    assert_eq!(
        edges_at(&conn, middle),
        middle_edges,
        "pruning to two generations took rows the older retained one still sees"
    );
    assert!(
        !middle_edges.is_empty(),
        "fixture precondition: the second-newest generation has edges"
    );
    assert_ne!(
        middle_edges, newest_edges,
        "fixture precondition: the two retained generations must differ, or \
         nothing is closed inside the window"
    );
    let within_window: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM edge_rows WHERE valid_to IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        within_window > 0,
        "fixture precondition: some row ended inside the retained window"
    );

    store.prune_generations_except_latest(1).unwrap();

    assert_eq!(
        edges_at(&conn, newest),
        newest_edges,
        "pruning changed what the retained generation serves"
    );
    for older in &generations[..generations.len() - 1] {
        assert!(
            edges_at(&conn, *older).is_empty(),
            "generation {older} was pruned and must serve nothing"
        );
    }
    let after = stored_edge_rows(&conn);
    assert_eq!(
        after,
        newest_edges.len() as i64,
        "pruning left {after} physical edge rows for a generation of {} edges \
         (it held {before} before): rows whose validity ended before the \
         retained generation are unreachable and must be reclaimed",
        newest_edges.len()
    );
    let _ = fs::remove_dir_all(&dir);
}

/// A range that could be read two ways cannot be written at all.
///
/// The failure mode this closes is quiet by construction: read through
/// `valid_from <= g AND (valid_to IS NULL OR g < valid_to)`, a row whose
/// `valid_to` is at or below its `valid_from` simply matches nothing. It does
/// not error, it does not double an edge — it *disappears*, and a graph missing
/// an edge looks exactly like a graph that never had one.
///
/// So the invariant is enforced where it cannot be worked around: a `CHECK` on
/// the table. A hand-written `UPDATE` planting an inverted range is refused by
/// SQLite before it lands, which is why this test asserts the refusal rather
/// than asserting that `Store::open` notices afterwards. An open-time scan would
/// also be a scan of every edge row on every `devmap search` — 102,083 rows for
/// a check the schema can make free.
#[test]
fn a_row_whose_validity_range_is_inverted_cannot_be_written() {
    let dir = tmp_dir("v18-inverted-range");
    let db = dir.join("index.sqlite");
    let store = Store::open(&db).unwrap();
    let files = tree(0, 4);
    let generation = commit(&store, &files, &[]);
    let served = edges_at(&Connection::open(&db).unwrap(), generation);
    assert!(
        !served.is_empty(),
        "fixture precondition: the generation has edges"
    );
    drop(store);

    let conn = Connection::open(&db).unwrap();
    for (label, sql) in [
        (
            "valid_to equal to valid_from",
            "UPDATE edge_rows SET valid_to = valid_from
              WHERE edge_id = (SELECT MIN(edge_id) FROM edge_rows)",
        ),
        (
            "valid_to below valid_from",
            "UPDATE edge_rows SET valid_to = valid_from - 1
              WHERE edge_id = (SELECT MIN(edge_id) FROM edge_rows)",
        ),
        (
            "a NULL valid_from",
            "UPDATE edge_rows SET valid_from = NULL
              WHERE edge_id = (SELECT MIN(edge_id) FROM edge_rows)",
        ),
        (
            "an inverted range on the ledger",
            "UPDATE unresolved_rows SET valid_to = valid_from
              WHERE unresolved_id = (SELECT MIN(unresolved_id) FROM unresolved_rows)",
        ),
    ] {
        let refusal = conn.execute(sql, []);
        assert!(
            refusal.is_err(),
            "{label} was accepted; a row the range predicate can never match \
             would then be silently absent from every answer"
        );
    }
    drop(conn);

    // And the store is exactly as it was: a refused write left nothing behind.
    let reopened = Store::open(&db).unwrap();
    assert_eq!(
        edges_at(&Connection::open(&db).unwrap(), generation),
        served,
        "a refused range write changed what the generation serves"
    );
    // One physical row can never be served twice for one generation: the view
    // joins `generations`, whose id is a primary key, so an overlapping pair of
    // live rows is two rows — duplicates, which this store genuinely has — and
    // never one row counted twice.
    assert_eq!(
        reopened.latest_edges(0.0).unwrap().len(),
        served.len(),
        "the reader and the view disagree about how many edges the generation has"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// A read-only store serves every edge a writable one does.
#[test]
fn a_read_only_store_serves_the_same_edges() {
    let dir = tmp_dir("v18-read-only");
    let db = dir.join("index.sqlite");
    let store = Store::open(&db).unwrap();
    let files = tree(0, 6);
    commit(&store, &files, &[]);
    let writable = store.latest_edges(0.0).unwrap();
    store.checkpoint_wal().unwrap();
    drop(store);

    let mut perms = fs::metadata(&db).unwrap().permissions();
    perms.set_readonly(true);
    fs::set_permissions(&db, perms).unwrap();
    let mut dir_perms = fs::metadata(&dir).unwrap().permissions();
    dir_perms.set_readonly(true);
    fs::set_permissions(&dir, dir_perms).unwrap();

    let readonly = Store::open(&db).unwrap();
    assert!(readonly.is_read_only(), "the fixture must be read-only");
    assert_eq!(
        readonly.latest_edges(0.0).unwrap().len(),
        writable.len(),
        "a read-only store served fewer edges than the writable one it opened"
    );

    let mut dir_perms = fs::metadata(&dir).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    dir_perms.set_readonly(false);
    let _ = fs::set_permissions(&dir, dir_perms);
    let mut perms = fs::metadata(&db).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    let _ = fs::set_permissions(&db, perms);
    let _ = fs::remove_dir_all(&dir);
}

/// The unresolved ledger's delta keeps its rows addressable per generation.
#[test]
fn an_unresolved_call_that_goes_away_leaves_the_generation_that_had_it_intact() {
    let dir = tmp_dir("v18-unresolved-range");
    let db = dir.join("index.sqlite");
    let store = Store::open(&db).unwrap();

    let extractions = vec![extract_file("hub.py", "def hub():\n    return 0\n")];
    let with_ledger = ResolutionResult {
        edges: Vec::new(),
        receiver_types: Default::default(),
        reexport_chains: Default::default(),
        unresolved: vec![UnresolvedReference {
            source_file: "hub.py".into(),
            source_symbol: "hub.py::hub".into(),
            callee_name: "mystery".into(),
            kind: UnresolvedKind::Call,
            resolution: Resolution::Unresolved {
                reason: "no candidate".into(),
            },
            class: UnresolvedClass::Unresolved,
            receiver: Some("obj".into()),
        }],
    };
    let analysis = analyze(&extractions, &with_ledger);
    let g1 = store
        .save_generation(&extractions, &with_ledger, &analysis)
        .unwrap();

    let empty = ResolutionResult {
        edges: Vec::new(),
        receiver_types: Default::default(),
        reexport_chains: Default::default(),
        unresolved: Vec::new(),
    };
    let analysis = analyze(&extractions, &empty);
    let g2 = store
        .save_generation_with_opts(
            &extractions,
            &empty,
            &analysis,
            GenerationWriteOpts {
                affected_paths: vec!["hub.py".into()],
                ..Default::default()
            },
        )
        .unwrap();

    let conn = Connection::open(&db).unwrap();
    assert_eq!(
        unresolved_at(&conn, g1).len(),
        1,
        "the first generation's ledger row must survive the second build"
    );
    assert!(
        unresolved_at(&conn, g2).is_empty(),
        "the second generation resolved everything and must hold no ledger rows"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// A writer killed between closing rows and inserting their successors leaves
/// the generation it started from intact.
///
/// The delta write has a shape the whole-rewrite one did not: it *ends* rows
/// before it begins their replacements, so there is a moment when the store
/// holds neither. If that moment were durable, a killed build would leave a
/// generation missing every edge that had changed — a graph that is not wrong
/// about any edge it has and is silently short of the ones it does not.
///
/// Driven with a real child process and `SIGKILL`, because an in-process
/// `panic` unwinds through rusqlite's `Drop` and rolls the transaction back,
/// which tests Rust's destructors rather than SQLite's recovery. The child
/// holds an uncommitted `BEGIN IMMEDIATE` with every row closed inside it.
#[cfg(unix)]
#[test]
fn a_writer_killed_between_closing_and_inserting_leaves_every_edge_servable() {
    let dir = tmp_dir("v18-killed-mid-delta");
    let db = dir.join("index.sqlite");
    let store = Store::open(&db).unwrap();
    let files = tree(0, 6);
    let generation = commit(&store, &files, &[]);
    let before = store.latest_edges(0.0).unwrap();
    assert!(
        !before.is_empty(),
        "fixture precondition: the generation has edges"
    );
    store.checkpoint_wal().unwrap();
    drop(store);

    // A child that opens the store, closes every live edge and ledger row
    // inside an uncommitted transaction — the exact half-state the delta write
    // passes through — announces itself, and then blocks until it is killed.
    let script = format!(
        r#"
import sqlite3, sys, time
conn = sqlite3.connect({:?}, isolation_level=None)
conn.execute("PRAGMA busy_timeout=5000")
conn.execute("BEGIN IMMEDIATE")
conn.execute("INSERT INTO generations (created_at, head_sha, analysis_json) VALUES (9.0, 'torn', '{{}}')")
gen = conn.execute("SELECT max(id) FROM generations").fetchone()[0]
conn.execute("UPDATE edge_rows SET valid_to = ? WHERE valid_to IS NULL", (gen,))
conn.execute("UPDATE unresolved_rows SET valid_to = ? WHERE valid_to IS NULL", (gen,))
sys.stdout.write("closed\n")
sys.stdout.flush()
time.sleep(600)
"#,
        db.to_string_lossy()
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
    while line.trim() != "closed" && std::time::Instant::now() < deadline {
        line.clear();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
    }
    assert_eq!(line.trim(), "closed", "child never closed the rows");

    // SIGKILL: no unwinding, no destructors, no rollback by the process itself.
    unsafe {
        libc_kill(child.id() as i32, 9);
    }
    let _ = child.wait();

    let reopened = Store::open(&db).expect("the store must recover");
    assert_eq!(
        reopened.latest_generation_id().unwrap(),
        Some(generation),
        "the killed writer's uncommitted generation became visible"
    );
    assert_eq!(
        reopened.latest_edges(0.0).unwrap(),
        before,
        "the closes committed without their inserts: the recovered generation \
         serves fewer edges than the one it was built from"
    );
    let conn = Connection::open(&db).unwrap();
    let open_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM edge_rows WHERE valid_to IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        open_rows,
        before.len() as i64,
        "a killed close left rows ended that nothing ended"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, sig: i32) -> i32;
}
