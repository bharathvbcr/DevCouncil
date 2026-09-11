//! Schema v19 — the delta compares only the files whose row set changed.
//!
//! v18 made the *write* a difference between the freshly resolved multiset and
//! the one already valid. It left the *comparison* whole: deciding which stored
//! rows were still wanted read back every live row of `edge_rows` and
//! `unresolved_rows` and compared it field by field. Measured on a `git archive`
//! corpus of this repository (1,612 files, 18,564 symbols, 107,257 edges,
//! 91,703 unresolved calls), release binary, one-file incremental builds, the
//! two spans `save_generation_timed` charges break down as:
//!
//! ```text
//!   edges       95 ms   kind labels 1 · path ids 9 · identity index 17 ·
//!                       scan 68 (cursor 15, source_file_id 2, decode+compare 51)
//!   unresolved 139 ms   identity index 47 ·
//!                       scan 92 (cursor 14, source_file 2, decode+compare 76)
//! ```
//!
//! — 127 ms of decoding and comparing rows that a build storing *one* row had
//! no reason to look at. v19 records, per source file, a digest of the rows that
//! file contributed, and the next build compares only the files whose freshly
//! resolved rows disagree with it.
//!
//! That is a soundness claim, and this file is what stands behind it. The two
//! ways such a scheme fails are the two it has to rule out:
//!
//! * **A column the digest misses.** A file whose rows change only in a field
//!   the digest does not cover reads as unchanged and keeps a stale row for
//!   ever. `every_identity_column_moves_the_digest` varies each stored column of
//!   each relation in turn and requires the scoped write to land where a full
//!   comparison lands.
//! * **A file the digest never should have trusted.** The digest is over what
//!   the resolver produced, not over what the caller called affected, precisely
//!   so that an edge from an untouched file into a moved target is caught.
//!   `a_file_the_edit_never_touched_is_still_compared_when_its_edges_move` is
//!   that case, and it is the one an affected-set-scoped delta gets wrong.
//!
//! Over both, `a_scoped_delta_writes_the_same_store_as_a_full_comparison` runs
//! an edit sequence through both paths and requires the stores to hold the same
//! rows over the same validity ranges.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use devmap_analyze::{analyze, AnalysisSummary};
use devmap_extract::extract_file;
use devmap_extract::model::{Confidence, EdgeKind};
use devmap_resolve::model::{
    LangFamily, Resolution, ResolutionResult, ResolvedEdge, UnresolvedClass, UnresolvedKind,
    UnresolvedReference,
};
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};
use rusqlite::Connection;

/// A private temporary directory.
///
/// Named from the process id and an atomic sequence rather than from the clock:
/// `SystemTime::now()` has collided between threads of one `cargo test` run on
/// this platform, and two tests sharing a directory is a failure that reads as
/// a store defect.
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
    files: &[(String, String)],
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

/// Every column of one edge row, as the store holds it.
///
/// **Every** column, deliberately: this is what the tests below compare, so a
/// column left out here is a column whose staleness they cannot see — which is
/// the exact defect they exist to catch, reintroduced in the probe.
const EDGE_COLUMNS: &str = "sp.path, tp.path, e.source_symbol, e.target_symbol, e.edge_kind,
     printf('%.17g', e.confidence), COALESCE(e.resolution, '<none>'),
     COALESCE(CAST(e.candidate_total AS TEXT), '<none>')";

const UNRESOLVED_COLUMNS: &str = "source_file, source_symbol, callee_name, reason, classification,
     COALESCE(receiver, '<none>')";

fn render(row: &rusqlite::Row, columns: usize) -> rusqlite::Result<String> {
    let mut parts: Vec<String> = Vec::with_capacity(columns);
    for index in 0..columns {
        parts.push(row.get::<_, String>(index)?);
    }
    Ok(parts.join("|"))
}

/// Every physical row of both ranged relations, with its validity range.
///
/// The whole stored tuple and the whole range, so "the same store" means the
/// same rows valid over the same generations and not merely the same rows a
/// reader can currently see. File ids are joined back to their paths: an id is
/// an interning artefact, and two stores that agree about every edge but number
/// their paths differently are the same store for every reader.
fn ranged_rows(db: &Path) -> Vec<String> {
    let conn = Connection::open(db).unwrap();
    let mut out: Vec<String> = Vec::new();
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {EDGE_COLUMNS}, e.valid_from,
                    COALESCE(CAST(e.valid_to AS TEXT), '<open>')
               FROM edge_rows e
               JOIN paths sp ON sp.id = e.source_file_id
               JOIN paths tp ON tp.id = e.target_file_id"
        ))
        .unwrap();
    out.extend(
        stmt.query_map([], |row| {
            Ok(format!(
                "edge|{}|[{},{})",
                render(row, 8)?,
                row.get::<_, i64>(8)?,
                row.get::<_, String>(9)?,
            ))
        })
        .unwrap()
        .map(Result::unwrap),
    );
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {UNRESOLVED_COLUMNS}, valid_from,
                    COALESCE(CAST(valid_to AS TEXT), '<open>')
               FROM unresolved_rows"
        ))
        .unwrap();
    out.extend(
        stmt.query_map([], |row| {
            Ok(format!(
                "unresolved|{}|[{},{})",
                render(row, 6)?,
                row.get::<_, i64>(6)?,
                row.get::<_, String>(7)?,
            ))
        })
        .unwrap()
        .map(Result::unwrap),
    );
    out.sort();
    out
}

/// The rows one generation can see, which is what every reader asks for.
fn rows_at(db: &Path, generation: u32) -> Vec<String> {
    let conn = Connection::open(db).unwrap();
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {EDGE_COLUMNS}
               FROM generation_edges e
               JOIN paths sp ON sp.id = e.source_file_id
               JOIN paths tp ON tp.id = e.target_file_id
              WHERE e.generation_id = ?1"
        ))
        .unwrap();
    let mut out: Vec<String> = stmt
        .query_map([generation], |row| Ok(format!("edge|{}", render(row, 8)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {UNRESOLVED_COLUMNS}
               FROM generation_unresolved WHERE generation_id = ?1"
        ))
        .unwrap();
    out.extend(
        stmt.query_map([generation], |row| {
            Ok(format!("unresolved|{}", render(row, 6)?))
        })
        .unwrap()
        .map(Result::unwrap),
    );
    out.sort();
    out
}

/// One module per index, each with a call that resolves and a call that cannot.
///
/// The stray call is what puts rows in the unresolved ledger, which is the
/// dearer half of what this file is about and is empty in a fixture where
/// everything resolves.
fn tree(revision: usize, modules: usize) -> Vec<(String, String)> {
    let mut files: Vec<(String, String)> = (0..modules)
        .map(|index| {
            (
                format!("mod_{index}.py"),
                format!(
                    "def leaf_{index}():\n    return {index}\n\n\n\
                     def caller_{index}():\n    return leaf_{index}()\n\n\n\
                     def stray_{index}():\n    return nowhere_{index}()\n"
                ),
            )
        })
        .collect();
    files.push((
        "hub.py".to_string(),
        format!(
            "# rev {revision}\nfrom mod_0 import caller_0\nfrom mod_1 import caller_1\n\n\n\
             def hub():\n    return caller_0() + caller_1()\n"
        ),
    ));
    files
}

/// One step of an edit sequence: the tree as it now stands, and what the caller
/// declares affected and deleted.
struct Step {
    label: &'static str,
    files: Vec<(String, String)>,
    affected: Vec<String>,
    deleted: Vec<String>,
}

/// The sequence both paths are driven through. Each step is a shape the scoping
/// has to get right, and its comment says which.
fn edit_sequence() -> Vec<Step> {
    let base = tree(0, 8);
    let mut steps = vec![Step {
        label: "cold",
        files: base.clone(),
        affected: Vec::new(),
        deleted: Vec::new(),
    }];

    // Nothing changed at all — every file's digest matches, so the scoped path
    // reads back no rows whatsoever. The case the change exists for, and the
    // case where a wrong answer is invisible without this comparison.
    steps.push(Step {
        label: "no-op rebuild",
        files: base.clone(),
        affected: vec!["hub.py".into()],
        deleted: Vec::new(),
    });

    // One file's own rows change: `leaf_3` gains a second caller, so `mod_3.py`
    // holds an edge it did not and every other file is untouched.
    let mut files = base.clone();
    files[3]
        .1
        .push_str("\n\ndef second_3():\n    return leaf_3()\n");
    steps.push(Step {
        label: "one file gains an edge",
        files: files.clone(),
        affected: vec!["mod_3.py".into()],
        deleted: Vec::new(),
    });

    // The same file loses a row: a digest must move on a removal as well as on
    // an addition, which one carrying no row count could get wrong.
    let mut files = files.clone();
    files[3].1 = files[3]
        .1
        .replace("def stray_3():\n    return nowhere_3()\n", "");
    steps.push(Step {
        label: "one file loses an unresolved call",
        files: files.clone(),
        affected: vec!["mod_3.py".into()],
        deleted: Vec::new(),
    });

    // **The case an affected-set-scoped delta gets wrong.** `caller_0` is
    // renamed in `mod_0.py`; `hub.py` is not edited and is not declared
    // affected, but the edge it had into `mod_0.py::caller_0` no longer
    // resolves. Its digest moves because the *resolver's* output for it moved.
    let mut files = files.clone();
    files[0].1 = files[0].1.replace("caller_0", "caller_zero");
    steps.push(Step {
        label: "an untouched file's edge target is renamed",
        files: files.clone(),
        affected: vec!["mod_0.py".into()],
        deleted: Vec::new(),
    });

    // A file leaves the tree. Its rows must close even though it contributes no
    // fresh digest to compare against — absence is not equality.
    let mut files = files.clone();
    files.remove(5);
    steps.push(Step {
        label: "a file is deleted",
        files: files.clone(),
        affected: Vec::new(),
        deleted: vec!["mod_5.py".into()],
    });

    // A file joins it, with no stored digest at all.
    let mut files = files.clone();
    files.push((
        "late.py".into(),
        "from mod_1 import caller_1\n\n\ndef late():\n    return caller_1()\n".into(),
    ));
    steps.push(Step {
        label: "a file is added",
        files: files.clone(),
        affected: vec!["late.py".into()],
        deleted: Vec::new(),
    });

    // Back to the no-op, on a store whose digests have been through all of it.
    steps.push(Step {
        label: "settled",
        files,
        affected: vec!["late.py".into()],
        deleted: Vec::new(),
    });
    steps
}

/// Drive the sequence into a store, with the scoping on or off.
fn drive(db: &Path, verify_every_row: bool) -> Vec<u32> {
    let store = Store::open(db).unwrap();
    edit_sequence()
        .into_iter()
        .map(|step| {
            let (extractions, resolution, analysis) = pipeline(&step.files);
            store
                .save_generation_with_opts(
                    &extractions,
                    &resolution,
                    &analysis,
                    GenerationWriteOpts {
                        affected_paths: step.affected,
                        deleted_paths: step.deleted,
                        verify_every_row,
                        ..Default::default()
                    },
                )
                .unwrap_or_else(|error| panic!("step {:?} failed: {error}", step.label))
        })
        .collect()
}

/// **The equivalence.** The same edit sequence, once with the delta scoped by
/// the per-file digests and once comparing every stored row, must leave stores
/// holding the same rows over the same validity ranges.
///
/// Stated over the physical rows and not only over what a reader sees, because
/// the two differ exactly where the scoping is wrong in the way that is hardest
/// to notice: a row that should have been closed and was not stays invisible to
/// the newest generation for as long as its replacement covers it, and surfaces
/// later as a duplicate.
#[test]
fn a_scoped_delta_writes_the_same_store_as_a_full_comparison() {
    let dir = tmp_dir("v19-equivalence");
    let scoped = dir.join("scoped.sqlite");
    let full = dir.join("full.sqlite");

    let scoped_gens = drive(&scoped, false);
    let full_gens = drive(&full, true);

    assert_eq!(
        scoped_gens, full_gens,
        "the two paths must commit the same generations"
    );
    // Non-vacuity: a fixture whose steps all resolved to nothing would compare
    // two empty stores and pass.
    let rows = ranged_rows(&scoped);
    assert!(
        rows.len() > 40,
        "fixture precondition: the sequence must store a substantial number of \
         rows, or the comparison below proves nothing (got {})",
        rows.len()
    );
    assert_eq!(
        rows,
        ranged_rows(&full),
        "a digest-scoped delta and a full comparison must leave the same rows \
         over the same validity ranges"
    );
    for generation in &scoped_gens {
        assert_eq!(
            rows_at(&scoped, *generation),
            rows_at(&full, *generation),
            "generation {generation} reads back differently under the two paths"
        );
    }
    let _ = fs::remove_dir_all(&dir);
}

/// A file the caller never declared affected, whose rows changed anyway.
///
/// This is why the digest is taken over the resolver's output and not over the
/// affected set. `hub.py` is byte-identical across the two builds and is not in
/// `affected_paths`; renaming `caller_0` in `mod_0.py` breaks the edge it had,
/// and the write has to notice from `hub.py`'s own resolved rows.
#[test]
fn a_file_the_edit_never_touched_is_still_compared_when_its_edges_move() {
    let dir = tmp_dir("v19-moved-target");
    let db = dir.join("index.sqlite");
    let store = Store::open(&db).unwrap();

    let before = tree(0, 3);
    let (extractions, resolution, analysis) = pipeline(&before);
    let g1 = store
        .save_generation(&extractions, &resolution, &analysis)
        .unwrap();
    let stale = |rows: &[String]| -> bool {
        rows.iter()
            .any(|row| row.starts_with("edge|hub.py|") && row.contains("mod_0.py::caller_0"))
    };
    let served = rows_at(&db, g1);
    assert!(
        stale(&served),
        "fixture precondition: hub.py must resolve an edge into \
         mod_0.py::caller_0, got {served:#?}"
    );

    let mut after = before.clone();
    after[0].1 = after[0].1.replace("caller_0", "caller_zero");
    assert_eq!(
        after[3].1, before[3].1,
        "fixture precondition: hub.py's bytes must not change"
    );
    let (extractions, resolution, analysis) = pipeline(&after);
    let g2 = store
        .save_generation_with_opts(
            &extractions,
            &resolution,
            &analysis,
            GenerationWriteOpts {
                // Deliberately only the file that was edited. A write that
                // scoped on this rather than on what was resolved would never
                // look at hub.py's rows.
                affected_paths: vec!["mod_0.py".into()],
                ..Default::default()
            },
        )
        .unwrap();

    let served = rows_at(&db, g2);
    assert!(
        !stale(&served),
        "the second generation still serves hub.py's edge into the renamed \
         symbol: the write trusted a digest for a file whose resolved rows had \
         moved, or scoped on the affected set instead of on them.\n{served:#?}"
    );
    assert!(
        stale(&rows_at(&db, g1)),
        "closing hub.py's row must not take it from the generation that held it"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// One edge and one ledger row, both from `hub.py`, that a caller can vary one
/// stored column at a time.
fn one_file_resolution(
    mutate: impl Fn(&mut ResolvedEdge, &mut UnresolvedReference),
) -> ResolutionResult {
    let mut edge = ResolvedEdge::resolved(
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
    );
    let mut unresolved = UnresolvedReference {
        source_file: "hub.py".into(),
        source_symbol: "hub.py::hub".into(),
        callee_name: "mystery".into(),
        kind: UnresolvedKind::Call,
        resolution: Resolution::Unresolved {
            reason: "no candidate".into(),
        },
        class: UnresolvedClass::Unresolved,
        receiver: Some("obj".into()),
    };
    mutate(&mut edge, &mut unresolved);
    ResolutionResult {
        edges: vec![edge],
        receiver_types: Default::default(),
        reexport_chains: Default::default(),
        unresolved: vec![unresolved],
    }
}

fn ambiguous(count: usize) -> Arc<Resolution> {
    Arc::new(Resolution::AmbiguousGlobal {
        candidates: (0..count)
            .map(|index| (format!("mod_{index}.py"), "leaf".to_string()))
            .collect(),
        family: LangFamily::Python,
    })
}

/// Write two generations of `one_file_resolution` into a fresh store and return
/// what the second one serves. `hub.py` is never declared affected, so under a
/// scoped write only its digest can carry the change.
fn two_generations(
    label: &str,
    before: &dyn Fn(&mut ResolvedEdge, &mut UnresolvedReference),
    after: &dyn Fn(&mut ResolvedEdge, &mut UnresolvedReference),
    verify_every_row: bool,
) -> (Vec<String>, Vec<String>) {
    let dir = tmp_dir(label);
    let db = dir.join("index.sqlite");
    let store = Store::open(&db).unwrap();
    let extractions = vec![
        extract_file("mod.py", "def leaf():\n    return 1\n"),
        extract_file("hub.py", "def hub():\n    return 0\n"),
    ];

    let first = one_file_resolution(before);
    let analysis = analyze(&extractions, &first);
    let g1 = store
        .save_generation(&extractions, &first, &analysis)
        .unwrap();
    let served_first = rows_at(&db, g1);

    let second = one_file_resolution(after);
    let analysis = analyze(&extractions, &second);
    let g2 = store
        .save_generation_with_opts(
            &extractions,
            &second,
            &analysis,
            GenerationWriteOpts {
                affected_paths: vec!["mod.py".into()],
                verify_every_row,
                ..Default::default()
            },
        )
        .unwrap();
    let served_second = rows_at(&db, g2);
    let _ = fs::remove_dir_all(&dir);
    (served_first, served_second)
}

/// **The failure to hunt: a column the digest does not cover.**
///
/// A digest over a hand-picked subset of an identity's columns lets a build keep
/// a row whose stored value it no longer agrees with — the carry-forward
/// staleness `EdgeTuple`'s doc comment refuses, arriving by a new route. The
/// implementation avoids it structurally, by digesting `EdgeTuple` and
/// `UnresolvedTuple` themselves rather than a list of fields; "structurally" is
/// an argument, and this is the check.
///
/// Every stored column of both relations is varied in turn, on a file the caller
/// does *not* declare affected, and each case is run twice — once comparing
/// every row and once scoped. The full comparison is what says the case varies
/// something the store records at all, so a case that turns out to be a no-op
/// fails as a broken fixture rather than passing as evidence. The scoped run
/// then has to land in the same place.
///
/// Each case supplies both shapes rather than only the changed one, because two
/// columns cannot otherwise be isolated: `candidate_total` is non-NULL only for
/// `AmbiguousGlobal`, so varying it from the default resolution would vary
/// `resolution` too and pass on that column's evidence instead of its own.
#[test]
fn every_identity_column_moves_the_digest() {
    type Mutate = Box<dyn Fn(&mut ResolvedEdge, &mut UnresolvedReference)>;
    fn unchanged() -> Mutate {
        Box::new(|_: &mut ResolvedEdge, _: &mut UnresolvedReference| {})
    }
    let cases: Vec<(&str, Mutate, Mutate)> = vec![
        (
            "edge_rows.target_file_id",
            unchanged(),
            Box::new(|edge: &mut ResolvedEdge, _: &mut UnresolvedReference| {
                edge.target_file = "other.py".into();
            }),
        ),
        (
            "edge_rows.source_symbol",
            unchanged(),
            Box::new(|edge: &mut ResolvedEdge, _: &mut UnresolvedReference| {
                edge.source_symbol = "hub.py::other".into();
            }),
        ),
        (
            "edge_rows.target_symbol",
            unchanged(),
            Box::new(|edge: &mut ResolvedEdge, _: &mut UnresolvedReference| {
                edge.target_symbol = "mod.py::other".into();
            }),
        ),
        (
            "edge_rows.edge_kind",
            unchanged(),
            Box::new(|edge: &mut ResolvedEdge, _: &mut UnresolvedReference| {
                edge.edge_kind = EdgeKind::Imports;
            }),
        ),
        (
            "edge_rows.confidence",
            unchanged(),
            Box::new(|edge: &mut ResolvedEdge, _: &mut UnresolvedReference| {
                edge.confidence = Confidence::LOW;
            }),
        ),
        (
            "edge_rows.resolution",
            unchanged(),
            Box::new(|edge: &mut ResolvedEdge, _: &mut UnresolvedReference| {
                edge.resolution = Some(Arc::new(Resolution::ReceiverType {
                    target_symbol: "leaf".into(),
                    target_file: "mod.py".into(),
                    receiver_type: "Mod".into(),
                }));
            }),
        ),
        (
            "edge_rows.candidate_total",
            Box::new(|edge: &mut ResolvedEdge, _: &mut UnresolvedReference| {
                edge.resolution = Some(ambiguous(2));
            }),
            Box::new(|edge: &mut ResolvedEdge, _: &mut UnresolvedReference| {
                edge.resolution = Some(ambiguous(3));
            }),
        ),
        (
            "unresolved_rows.source_symbol",
            unchanged(),
            Box::new(|_: &mut ResolvedEdge, row: &mut UnresolvedReference| {
                row.source_symbol = "hub.py::elsewhere".into();
            }),
        ),
        (
            "unresolved_rows.callee_name",
            unchanged(),
            Box::new(|_: &mut ResolvedEdge, row: &mut UnresolvedReference| {
                row.callee_name = "enigma".into();
            }),
        ),
        (
            "unresolved_rows.reason",
            unchanged(),
            Box::new(|_: &mut ResolvedEdge, row: &mut UnresolvedReference| {
                row.resolution = Resolution::Unresolved {
                    reason: "a different reason".into(),
                };
            }),
        ),
        (
            "unresolved_rows.classification",
            unchanged(),
            Box::new(|_: &mut ResolvedEdge, row: &mut UnresolvedReference| {
                row.class = UnresolvedClass::Builtin;
            }),
        ),
        (
            "unresolved_rows.receiver",
            unchanged(),
            Box::new(|_: &mut ResolvedEdge, row: &mut UnresolvedReference| {
                row.receiver = None;
            }),
        ),
    ];

    let mut blind: Vec<String> = Vec::new();
    for (column, before, after) in cases {
        let (full_first, full_second) = two_generations("v19-column-full", &before, &after, true);
        assert_ne!(
            full_first, full_second,
            "fixture: case {column} varies nothing the store records, so the \
             scoped run below would prove nothing about it"
        );
        let (_, scoped_second) = two_generations("v19-column", &before, &after, false);
        if scoped_second != full_second {
            blind.push(format!(
                "{column}\n      scoped serves {scoped_second:?}\n      full   serves {full_second:?}"
            ));
        }
    }

    assert!(
        blind.is_empty(),
        "the per-file digest is blind to {} stored column(s); a file whose rows \
         differ only there reads as unchanged and keeps the previous \
         generation's row for ever:\n    {}",
        blind.len(),
        blind.join("\n    ")
    );
}

/// Multiplicity, which is the reason the digest sums rather than XORs.
///
/// 475 edge tuples and 12,424 ledger tuples of this repository occur more than
/// once in a single generation, so a file's rows are a multiset and its digest
/// has to be one. `XOR` is the other obvious order-independent combiner and it
/// is its own inverse, which makes a row cancel its own duplicate: every even
/// multiplicity digests to the same thing as zero.
///
/// The fixture is therefore a swap and not a doubling. `{X, X, Y, Y}` becomes
/// `{X, X, X, X}` — four rows before and four after, so the live-row count
/// beside the digest cannot answer this one, and a `XOR` digest reads both as
/// the empty set. Only the sum tells them apart, which is the choice this test
/// exists to pin.
#[test]
fn a_repeated_row_changes_the_digest_when_its_multiplicity_does() {
    let dir = tmp_dir("v19-multiplicity");
    let db = dir.join("index.sqlite");
    let store = Store::open(&db).unwrap();
    let extractions = vec![
        extract_file("mod.py", "def leaf():\n    return 1\n"),
        extract_file("hub.py", "def hub():\n    return 0\n"),
    ];

    // Two distinct rows of each relation, so a multiset of four can be built
    // out of them two ways with the same size.
    let x = one_file_resolution(|_, _| {});
    let y = one_file_resolution(|edge, row| {
        edge.target_symbol = "mod.py::other".into();
        row.callee_name = "enigma".into();
    });
    let mix = |x_copies: usize, y_copies: usize| ResolutionResult {
        edges: std::iter::repeat_n(x.edges[0].clone(), x_copies)
            .chain(std::iter::repeat_n(y.edges[0].clone(), y_copies))
            .collect(),
        receiver_types: Default::default(),
        reexport_chains: Default::default(),
        unresolved: std::iter::repeat_n(x.unresolved[0].clone(), x_copies)
            .chain(std::iter::repeat_n(y.unresolved[0].clone(), y_copies))
            .collect(),
    };

    let multiplicities = |generation: u32| -> BTreeMap<String, usize> {
        let mut seen: BTreeMap<String, usize> = BTreeMap::new();
        for row in rows_at(&db, generation) {
            *seen.entry(row).or_default() += 1;
        }
        seen
    };

    let paired = mix(2, 2);
    let analysis = analyze(&extractions, &paired);
    let g1 = store
        .save_generation(&extractions, &paired, &analysis)
        .unwrap();
    let before = multiplicities(g1);
    assert_eq!(
        before.values().copied().collect::<Vec<_>>(),
        vec![2, 2, 2, 2],
        "fixture precondition: two rows of each relation, stored twice each"
    );

    let doubled = mix(4, 0);
    let analysis = analyze(&extractions, &doubled);
    let g2 = store
        .save_generation_with_opts(
            &extractions,
            &doubled,
            &analysis,
            GenerationWriteOpts {
                affected_paths: vec!["mod.py".into()],
                ..Default::default()
            },
        )
        .unwrap();

    let after = multiplicities(g2);
    assert_eq!(
        after.values().copied().collect::<Vec<_>>(),
        vec![4, 4],
        "the generation still serves the multiset it replaced. The row count is \
         the same on both sides, so only the digest could tell them apart, and \
         a combiner that is its own inverse cannot: it folds every row into its \
         duplicate and reads both multisets as empty.\n  before {before:?}\n  \
         after  {after:?}"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// A store that arrives with no digests at all compares every row, and records
/// them on the way.
///
/// The v18→v19 rung has no backfill, so this is the state every existing store
/// migrates into. It has to be *correct*, not merely slower: a build that read
/// an absent digest as a matching one would keep every stale row in the store.
#[test]
fn a_generation_with_no_recorded_digests_is_compared_in_full() {
    let dir = tmp_dir("v19-no-digests");
    let files = tree(0, 4);
    let mut changed = files.clone();
    changed[1]
        .1
        .push_str("\n\ndef extra_1():\n    return leaf_1()\n");

    // The same two generations twice: once into a store whose first
    // generation's digests are deleted before the second build — a migrated
    // v18 store exactly — and once with every row compared. What the two serve
    // has to agree, which is the property, rather than a row count that would
    // have to be guessed.
    let build_both = |db: &Path, erase_digests: bool| -> (u32, u32) {
        let store = Store::open(db).unwrap();
        let (extractions, resolution, analysis) = pipeline(&files);
        let g1 = store
            .save_generation(&extractions, &resolution, &analysis)
            .unwrap();
        let conn = Connection::open(db).unwrap();
        let recorded: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM generation_file_digests WHERE generation_id = ?1",
                [g1],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            recorded > 0,
            "fixture precondition: a v19 build records digests, so deleting \
             them below is a real reduction to the migrated state"
        );
        if erase_digests {
            conn.execute("DELETE FROM generation_file_digests", [])
                .unwrap();
        }
        drop(conn);
        let (extractions, resolution, analysis) = pipeline(&changed);
        let g2 = store
            .save_generation_with_opts(
                &extractions,
                &resolution,
                &analysis,
                GenerationWriteOpts {
                    affected_paths: vec!["mod_1.py".into()],
                    verify_every_row: !erase_digests,
                    ..Default::default()
                },
            )
            .unwrap();
        (g1, g2)
    };

    let migrated = dir.join("migrated.sqlite");
    let full = dir.join("full.sqlite");
    let (_, g2) = build_both(&migrated, true);
    let (_, full_g2) = build_both(&full, false);

    let served = rows_at(&migrated, g2);
    assert!(
        served.iter().any(|row| row.contains("mod_1.py::extra_1")),
        "a store with no recorded digests must compare every row and write the \
         new one; instead the new generation serves {served:#?}"
    );
    assert_eq!(
        served,
        rows_at(&full, full_g2),
        "a build that found no digests must land where a build comparing every \
         row lands: reading an absent digest as a matching one keeps every \
         stale row in the store"
    );

    let conn = Connection::open(&migrated).unwrap();
    let recorded: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM generation_file_digests WHERE generation_id = ?1",
            [g2],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        recorded > 0,
        "the build that ran without digests must still leave them behind, or \
         every later build pays the full comparison too"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// Absence is not equality, even when the row count agrees.
///
/// The companion to the test above, and the one that pins the *other* half of
/// the skip condition. That one changes the number of rows, which the live-row
/// count catches on its own — so it would pass even if a missing digest were
/// read as a matching one. This one changes a file's rows without changing how
/// many there are: one edge before and one after, differing only in
/// `resolution`. Nothing but the digest can tell them apart, there is no digest,
/// and the only correct reading of that is "compare it".
///
/// A store migrating from v18 is in exactly this state for one build, so the
/// case is the first thing every existing installation does.
#[test]
fn an_absent_digest_is_not_a_matching_one() {
    let dir = tmp_dir("v19-absent-not-equal");
    let db = dir.join("index.sqlite");
    let store = Store::open(&db).unwrap();
    let extractions = vec![
        extract_file("mod.py", "def leaf():\n    return 1\n"),
        extract_file("hub.py", "def hub():\n    return 0\n"),
    ];

    let first = one_file_resolution(|_, _| {});
    let analysis = analyze(&extractions, &first);
    let g1 = store
        .save_generation(&extractions, &first, &analysis)
        .unwrap();
    let before = rows_at(&db, g1);

    let conn = Connection::open(&db).unwrap();
    conn.execute("DELETE FROM generation_file_digests", [])
        .unwrap();
    drop(conn);

    let second = one_file_resolution(|edge, _| {
        edge.resolution = Some(Arc::new(Resolution::ReceiverType {
            target_symbol: "leaf".into(),
            target_file: "mod.py".into(),
            receiver_type: "Mod".into(),
        }));
    });
    let analysis = analyze(&extractions, &second);
    let g2 = store
        .save_generation_with_opts(
            &extractions,
            &second,
            &analysis,
            GenerationWriteOpts {
                affected_paths: vec!["mod.py".into()],
                ..Default::default()
            },
        )
        .unwrap();

    let after = rows_at(&db, g2);
    assert_eq!(
        before.len(),
        after.len(),
        "fixture precondition: the row count must not move, or the live-row \
         count would answer this and the digest would not have to"
    );
    assert_ne!(
        before, after,
        "with no digest recorded, the write skipped hub.py anyway and the new \
         generation still serves the old resolution: a missing digest was read \
         as a matching one.\n  before {before:?}\n  after  {after:?}"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// Rows that left the store behind the write path's back are re-derived, not
/// trusted away.
///
/// **The defect this pins was in the first version of the scoping**, and
/// `incremental_equivalence.rs` is what found it: a digest is a claim a
/// previous build recorded about what it wrote, and the store is what is
/// actually there. `drop_stored_edges` deletes live rows to stand in for a
/// kernel that recorded fewer of them, and the build must still commit the cold
/// answer. Scoping on the digest alone read the file as unchanged, skipped it,
/// and left the deleted rows missing for ever — which is why the skip is
/// conditioned on a count taken from the rows as well as on the digest.
///
/// It lives here as well as in the CLI gate because the property belongs to the
/// write path: the gate found it, and a gate three crates away is a slow way to
/// learn that this function trusted the wrong thing.
#[test]
fn rows_deleted_behind_the_write_path_are_written_again() {
    let dir = tmp_dir("v19-rows-vanished");
    let db = dir.join("index.sqlite");
    let store = Store::open(&db).unwrap();
    let files = tree(0, 6);

    let (extractions, resolution, analysis) = pipeline(&files);
    let g1 = store
        .save_generation(&extractions, &resolution, &analysis)
        .unwrap();
    let expected = rows_at(&db, g1);

    // Five live edge rows and two ledger rows vanish from files the next build
    // will not declare affected. Nothing about the *sources* changes, so a
    // freshly resolved digest for those files matches what generation 1
    // recorded exactly.
    let removed = {
        let conn = Connection::open(&db).unwrap();
        let edges = conn
            .execute(
                "DELETE FROM edge_rows WHERE edge_id IN (
                   SELECT e.edge_id FROM edge_rows e
                    JOIN paths p ON p.id = e.source_file_id
                   WHERE e.valid_to IS NULL AND p.path <> 'mod_1.py' LIMIT 5)",
                [],
            )
            .unwrap();
        let ledger = conn
            .execute(
                "DELETE FROM unresolved_rows WHERE unresolved_id IN (
                   SELECT unresolved_id FROM unresolved_rows
                    WHERE valid_to IS NULL AND source_file <> 'mod_1.py' LIMIT 2)",
                [],
            )
            .unwrap();
        (edges, ledger)
    };
    assert_eq!(
        removed,
        (5, 2),
        "fixture precondition: the generation had rows to remove"
    );

    let mut changed = files.clone();
    changed[1]
        .1
        .push_str("\n\ndef extra_1():\n    return leaf_1()\n");
    let (extractions, resolution, analysis) = pipeline(&changed);
    let g2 = store
        .save_generation_with_opts(
            &extractions,
            &resolution,
            &analysis,
            GenerationWriteOpts {
                affected_paths: vec!["mod_1.py".into()],
                ..Default::default()
            },
        )
        .unwrap();

    let served = rows_at(&db, g2);
    let missing: Vec<&String> = expected
        .iter()
        .filter(|row| !served.contains(row) && !row.contains("mod_1.py"))
        .collect();
    assert!(
        missing.is_empty(),
        "{} row(s) deleted behind the write path were never written back — the \
         build trusted a digest about rows the store no longer held:\n  {:#?}",
        missing.len(),
        missing
    );
    let _ = fs::remove_dir_all(&dir);
}

/// The digests belong to the generation that wrote them, and go when it does.
#[test]
fn pruning_a_generation_takes_its_digests_with_it() {
    let dir = tmp_dir("v19-prune");
    let db = dir.join("index.sqlite");
    let store = Store::open(&db).unwrap();

    let mut generations: Vec<u32> = Vec::new();
    for revision in 0..(devmap_store::GENERATION_RETENTION + 2) {
        let files = tree(revision, 3);
        let (extractions, resolution, analysis) = pipeline(&files);
        generations.push(
            store
                .save_generation_with_opts(
                    &extractions,
                    &resolution,
                    &analysis,
                    GenerationWriteOpts {
                        affected_paths: vec!["hub.py".into()],
                        ..Default::default()
                    },
                )
                .unwrap(),
        );
    }
    store
        .prune_generations_except_latest(devmap_store::GENERATION_RETENTION)
        .unwrap();

    let conn = Connection::open(&db).unwrap();
    let orphans: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM generation_file_digests
              WHERE generation_id NOT IN (SELECT id FROM generations)",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        orphans, 0,
        "the prune left digest rows behind for generations it deleted"
    );
    let newest = generations.last().copied().unwrap();
    let kept: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM generation_file_digests WHERE generation_id = ?1",
            [newest],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        kept > 0,
        "the prune took the newest generation's digests, which are the only \
         ones any build reads"
    );
    let _ = fs::remove_dir_all(&dir);
}
