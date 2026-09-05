//! An edge read back out of the store must carry the confidence the resolver
//! gave it — no more.
//!
//! `ResolvedEdge::resolved` is the only *write* path, so `confidence` cannot
//! disagree with `resolution` on the way in. The read path is a different
//! crate: `devmap_query::resolved_edge_from_stored` rebuilds an edge from a
//! database row that has no `resolution` column, sets `resolution: None`, and
//! copies the stored `confidence` verbatim. Everything downstream — the
//! `extracted`/`inferred`/`ambiguous` label in `code_graph.json`, the
//! `min_confidence` filter, the dead-code tier — reads that copy.
//!
//! So the round trip is the invariant, and it is asserted over the nine shapes
//! `devmap-resolve/tests/resolution_honesty.rs` attacks: the same corpus, the
//! same ladder, written and read back. What this file cannot assert, and says
//! so rather than implying otherwise, is that a *re-read* edge's confidence
//! still matches its evidence: the evidence is not persisted, so the check is
//! the round trip plus the write-side invariant, never a second independent
//! reading of the same fact.

#![cfg(feature = "parse")]

use std::collections::BTreeMap;

use devmap_extract::extract_file;
use devmap_extract::model::{Confidence, Extraction};
use devmap_query::resolved_edge_from_stored;
use devmap_resolve::Resolver;
use devmap_store::Store;

const CORPUS: &[(&str, &str)] = &[
    (
        "svc.py",
        "class Service:\n    def process(self, rows):\n        return rows\n\n\n\
         def process(rows):\n    return rows\n",
    ),
    (
        "bare_caller.py",
        "from svc import Service\n\n\ndef run(rows):\n    return process(rows)\n",
    ),
    ("shadowlib.py", "def len(rows):\n    return 0\n"),
    (
        "shadow_user.py",
        "from shadowlib import len\n\n\ndef count(rows):\n    return len(rows)\n",
    ),
    ("pkg/first.py", "class Widget:\n    pass\n"),
    ("pkg/second.py", "class Widget:\n    pass\n"),
    ("pkg/use.py", "def make():\n    return Widget()\n"),
    (
        "recv/engines.py",
        "class Engine:\n    def start(self):\n        return 1\n\n\n\
         class Other:\n    def start(self):\n        return 2\n",
    ),
    (
        "recv/drive.py",
        "from recv.engines import Engine, Other\n\n\n\
         def go():\n    e = Engine()\n    n = 0\n    return e.start()\n\n\n\
         def reassigned():\n    e = Engine()\n    e = Other()\n    return e.start()\n",
    ),
    (
        "geo/shape.go",
        "package geo\n\ntype Shape interface {\n\tArea() float64\n}\n\n\
         type Square struct{ side float64 }\n\n\
         func (s Square) Area() float64 { return s.side * s.side }\n",
    ),
    (
        "geo/measure.go",
        "package geo\n\nfunc Measure(s Shape) float64 { return s.Area() }\n",
    ),
];

type EdgeKey = (String, String, String, String, String);

fn key(
    source_file: &str,
    target_file: &str,
    source_symbol: &str,
    target_symbol: &str,
    edge_kind: &str,
) -> EdgeKey {
    (
        source_file.to_string(),
        target_file.to_string(),
        source_symbol.to_string(),
        target_symbol.to_string(),
        edge_kind.to_string(),
    )
}

#[test]
fn a_stored_edge_reads_back_at_the_confidence_the_resolver_gave_it() {
    let extractions: Vec<Extraction> = CORPUS
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let analysis = devmap_analyze::analyze(&extractions, &resolution);

    let store = Store::open_in_memory().expect("store");
    store
        .save_generation(&extractions, &resolution, &analysis)
        .expect("generation");

    // What the resolver decided, keyed by the edge's identity. `min` where two
    // edges share one identity: if the two disagree the round trip must still
    // not come back *above* what the evidence earned.
    let mut written: BTreeMap<EdgeKey, f32> = BTreeMap::new();
    for edge in &resolution.edges {
        let identity = key(
            &edge.source_file,
            &edge.target_file,
            &edge.source_symbol,
            &edge.target_symbol,
            &format!("{:?}", edge.edge_kind),
        );
        let slot = written.entry(identity).or_insert(edge.confidence.0);
        *slot = slot.min(edge.confidence.0);
    }
    assert!(
        written.len() > 20,
        "the fixture must exercise the ladder, got {} distinct edges",
        written.len()
    );

    let stored = store.latest_edges(0.0).expect("edges read back");
    assert!(
        !stored.is_empty(),
        "the generation persisted no edges, so this test would prove nothing"
    );
    let mut checked = 0usize;
    for row in stored {
        let identity = key(
            &row.source_file,
            &row.target_file,
            &row.source_symbol,
            &row.target_symbol,
            &row.edge_kind,
        );
        let edge = resolved_edge_from_stored(row).expect("the stored kind decodes");
        let Some(expected) = written.get(&identity) else {
            panic!("the store returned an edge the resolver never wrote: {identity:?}");
        };
        assert!(
            (edge.confidence.0 - expected).abs() < f32::EPSILON,
            "{identity:?} was written at {expected} and read back at {}",
            edge.confidence.0
        );
        assert!(
            edge.confidence.0 <= Confidence::DETERMINISTIC.0,
            "a stored edge read back above the ceiling: {identity:?} at {}",
            edge.confidence.0
        );
        checked += 1;
    }
    assert_eq!(
        checked,
        written.len(),
        "every written edge must be accounted for on the way back"
    );
}

/// The ambiguous half, kept separate because it is the one an over-claim would
/// hurt most: `pkg/use.py` names a `Widget` two files declare, and the fan-out
/// must still be at the speculative floor after a round trip through SQLite's
/// REAL column.
#[test]
fn an_ambiguous_edge_does_not_gain_confidence_by_being_stored() {
    let extractions: Vec<Extraction> = CORPUS
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let analysis = devmap_analyze::analyze(&extractions, &resolution);
    let store = Store::open_in_memory().expect("store");
    store
        .save_generation(&extractions, &resolution, &analysis)
        .expect("generation");

    let ambiguous: Vec<_> = store
        .latest_edges(0.0)
        .expect("edges")
        .into_iter()
        .filter(|row| row.source_file == "pkg/use.py" && row.target_symbol.contains("Widget"))
        .collect();
    assert!(
        ambiguous.len() > 1,
        "the fixture must produce the fan-out, got {ambiguous:?}"
    );
    for row in ambiguous {
        assert!(
            row.confidence <= Confidence::SPECULATIVE.0,
            "an ambiguous pick came back above the speculative floor: {row:?}"
        );
    }
}
