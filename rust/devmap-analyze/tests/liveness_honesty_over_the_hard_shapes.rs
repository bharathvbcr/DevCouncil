//! Liveness must not call a symbol dead at the confident tier while an edge
//! reaches it.
//!
//! `devmap dead --confidence extracted` is the tier `CLAUDE.md` tells agents to
//! act on, and acting on it means deleting code. The nine shapes below are the
//! ones where the resolver is most likely to have produced an edge the liveness
//! pass then fails to see — a bare name a method of the same name could absorb,
//! an import shadowing a builtin, one class declared twice in a package, a
//! re-export chain, a decorator-registered route, a receiver typed two
//! statements above the call and then reassigned, `self.method()` declared only
//! by a parent, a Go interface method against a concrete method of the same
//! name, and a TypeScript default export renamed at the import.
//!
//! The invariant is stated over the whole corpus rather than shape by shape: if
//! the resolution holds any non-structural edge whose target is a symbol, no
//! dead report at `extracted` confidence may name that symbol. A shape-by-shape
//! list would pass while the *next* shape regressed.
//!
//! Companion to `devmap-resolve/tests/resolution_honesty.rs`, which holds the
//! same corpus to the confidence tiers its evidence entitles it to.

use std::collections::BTreeSet;

use devmap_analyze::analyze_liveness;
use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, Extraction};
use devmap_resolve::model::Resolution;
use devmap_resolve::Resolver;

/// `confidence_label` in `devmap-query` calls anything at or above this
/// `extracted` — the tier an agent is told to act on. Spelled out here because
/// `devmap-analyze` does not depend on that crate, and the number is the point.
const EXTRACTED_FLOOR: f32 = 0.9;

/// The same nine shapes `resolution_honesty.rs` resolves. Duplicated rather
/// than shared because the two crates have no common test target, and a shape
/// that exists in only one of the two files is the failure both are guarding.
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
    ("chain/deep.py", "def target():\n    return 1\n"),
    ("chain/mid.py", "from chain.deep import target\n"),
    ("chain/top.py", "from chain.mid import target\n"),
    (
        "chain/app.py",
        "from chain.top import target\n\n\ndef go():\n    return target()\n",
    ),
    (
        "web/routes.py",
        "from web.framework import app\n\n\n@app.route(\"/x\")\ndef handler():\n    return 1\n",
    ),
    (
        "web/framework.py",
        "class App:\n    def route(self, path):\n        return path\n\n\napp = App()\n",
    ),
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
        "inherit/base.py",
        "class Base:\n    def helper(self):\n        return 1\n",
    ),
    (
        "inherit/child.py",
        "from inherit.base import Base\n\n\nclass Child(Base):\n    def run(self):\n        \
         return self.helper()\n",
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
    (
        "ts/widget.ts",
        "export default function widget() {\n  return 1;\n}\n",
    ),
    (
        "ts/app.ts",
        "import renamed from \"./widget\";\n\nexport function go() {\n  return renamed();\n}\n",
    ),
];

fn corpus() -> (Vec<Extraction>, devmap_resolve::model::ResolutionResult) {
    let extractions: Vec<Extraction> = CORPUS
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    (extractions, resolution)
}

#[test]
fn no_symbol_an_edge_reaches_is_reported_dead_at_the_confident_tier() {
    let (extractions, resolution) = corpus();
    assert!(
        resolution.edges.len() > 20,
        "the fixture must exercise the ladder, got {} edges",
        resolution.edges.len()
    );

    // Every symbol something points at, by (file, name). Structural edges are
    // excluded on purpose: `Contains`, `Defines`, `MemberOf` say where a symbol
    // *lives*, not that anything uses it, and counting them would make this
    // invariant vacuously true for the whole graph.
    let mut reached: BTreeSet<(String, String)> = BTreeSet::new();
    for edge in &resolution.edges {
        if matches!(
            edge.edge_kind,
            EdgeKind::Contains | EdgeKind::Defines | EdgeKind::MemberOf
        ) {
            continue;
        }
        if matches!(
            edge.resolution.as_deref(),
            Some(Resolution::Structural { .. })
        ) {
            continue;
        }
        // The stored spelling is qualified (`Class.method`); the dead report
        // names the bare symbol. Both spellings are admitted so a match cannot
        // be missed by a qualification difference.
        let bare = edge
            .target_symbol
            .rsplit(['.', ':', '/'])
            .next()
            .unwrap_or(&edge.target_symbol)
            .to_string();
        reached.insert((edge.target_file.clone(), edge.target_symbol.clone()));
        reached.insert((edge.target_file.clone(), bare));
    }

    let reports = analyze_liveness(&extractions, &resolution);
    for report in &reports {
        if report.is_exempt || report.confidence < EXTRACTED_FLOOR {
            continue;
        }
        let key = (report.file_path.clone(), report.symbol_name.clone());
        assert!(
            !reached.contains(&key),
            "`{}` in {} is reported dead at {:.2} — the tier an agent is told to \
             act on by deleting it — while the resolution holds an edge that \
             reaches it",
            report.symbol_name,
            report.file_path,
            report.confidence
        );
    }
}

/// The OFF direction. A rule that never fires is worth as much as one that
/// always does: a symbol nothing reaches must still be found.
#[test]
fn a_symbol_nothing_reaches_is_still_reported() {
    let mut files = CORPUS.to_vec();
    files.push(("orphan.py", "def nobody_calls_this():\n    return 1\n"));
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);

    let reports = analyze_liveness(&extractions, &resolution);
    assert!(
        reports
            .iter()
            .any(|report| report.symbol_name == "nobody_calls_this" && !report.is_exempt),
        "the pass found nothing at all, so the assertion above proves nothing: \
         {} reports",
        reports.len()
    );
}
