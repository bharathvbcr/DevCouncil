//! What a generation write spends, charged to the relation that spent it.
//!
//! `persist:write` is a single number, and on this repository it is 0.30 s of a
//! 1.10 s one-file incremental build — the largest single thing a hook waits
//! for after an edit. A single number cannot say which relation is responsible,
//! and the relations have nothing in common as fixes: v18 put the edges and the
//! unresolved ledger on validity ranges and left the nodes, the full-text map,
//! the file rows, the dead symbols and the coverage gaps as full per-generation
//! copies. Deciding whether the next rung is worth its migration means knowing
//! which of those the 0.30 s is.
//!
//! The instrument is therefore part of the write path rather than a profiler
//! run beside it: the node and full-text inserts are interleaved by
//! construction — an FTS rowid is derived from the node ordinal the same loop
//! just used — so nothing outside the loop can separate them.
//!
//! Two properties, and both are about honesty rather than about speed:
//!
//! 1. **The parts are parts.** Their sum never exceeds the write that contains
//!    them, exactly as a sub-phase never exceeds its stage.
//! 2. **The parts account for the write.** An instrument that names nine
//!    relations and attributes three percent of the time to them is worse than
//!    no instrument, because a reader acts on it. The unattributed remainder is
//!    the glue between the passes, and it must stay the remainder.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use devmap_analyze::{analyze, AnalysisSummary};
use devmap_extract::extract_file;
use devmap_resolve::model::ResolutionResult;
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};

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

/// Large enough that a whole generation of node rows is visibly more than the
/// fixed cost of opening a transaction, small enough to build in a unit test.
fn tree(revision: usize, modules: usize) -> Vec<(String, String)> {
    let mut files: Vec<(String, String)> = (0..modules)
        .map(|index| {
            (
                format!("mod_{index}.py"),
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

/// The relations the write path touches, in the order it touches them.
///
/// Named here rather than read back off the breakdown so that a relation
/// dropped from the instrument is a failure rather than a shorter list: an
/// instrument that silently stops charging one of its relations reports the
/// same shape as one that charged it nothing.
const RELATIONS: &[&str] = &[
    "file_rows",
    "nodes",
    "fts",
    "edges",
    "unresolved",
    "gaps",
    "dead",
    "history",
    "commit",
];

#[test]
fn a_generation_write_charges_every_relation_it_writes() {
    let dir = tmp_dir("write-breakdown-names");
    let store = Store::open(dir.join("devmap.sqlite")).unwrap();
    let files = tree(0, 150);
    let (extractions, resolution, analysis) = pipeline(&as_refs(&files));

    let (_gen, breakdown) = store
        .save_generation_timed(
            &extractions,
            &resolution,
            &analysis,
            GenerationWriteOpts::default(),
            "unknown",
        )
        .unwrap();

    let named: Vec<&str> = breakdown.parts().iter().map(|(label, _)| *label).collect();
    assert_eq!(
        named, RELATIONS,
        "the write charges one span per relation, in write order"
    );
    for (label, seconds) in breakdown.parts() {
        assert!(
            seconds.is_finite() && seconds >= 0.0,
            "{label} reported {seconds}s, which is not a duration"
        );
    }
    let _ = fs::remove_dir_all(&dir);
}

/// The split is a split: it never exceeds the write, and it accounts for it.
///
/// Measured on the incremental generation, because that is the build a hook
/// waits for and the one the split exists to explain.
#[test]
fn the_split_accounts_for_the_write_without_exceeding_it() {
    let dir = tmp_dir("write-breakdown-sums");
    let store = Store::open(dir.join("devmap.sqlite")).unwrap();

    let first = tree(0, 150);
    let (extractions, resolution, analysis) = pipeline(&as_refs(&first));
    store
        .save_generation_with_opts(
            &extractions,
            &resolution,
            &analysis,
            GenerationWriteOpts::default(),
        )
        .unwrap();

    let second = tree(1, 150);
    let (extractions, resolution, analysis) = pipeline(&as_refs(&second));
    let opts = GenerationWriteOpts {
        affected_paths: vec!["hub.py".to_string()],
        ..Default::default()
    };
    let started = Instant::now();
    let (_gen, breakdown) = store
        .save_generation_timed(&extractions, &resolution, &analysis, opts, "unknown")
        .unwrap();
    let wall = started.elapsed().as_secs_f64();

    let charged: f64 = breakdown.parts().iter().map(|(_, secs)| secs).sum();
    assert!(
        charged <= wall + 1e-6,
        "the parts of a write cannot outlast the write: charged {charged}s of {wall}s \
         ({:?})",
        breakdown.parts()
    );
    assert!(
        charged >= wall * 0.5,
        "an instrument that leaves most of the write unattributed points at nothing: \
         charged {charged}s of {wall}s ({:?})",
        breakdown.parts()
    );
    let _ = fs::remove_dir_all(&dir);
}
