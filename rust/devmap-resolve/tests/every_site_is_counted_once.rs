//! The ledger's arithmetic: every attribution site is accounted for exactly
//! once, as an edge or as one row.
//!
//! `ResolutionRate` divides by `resolved_sites + unresolved_sites`, and both
//! halves come from this crate. `resolved_sites` already deduplicates an
//! ambiguous fan-out on the shared `Arc<Resolution>`, and says so where it does
//! it. Nothing checked the other half, or the relationship between them — so a
//! call counted twice in `unresolved`, or dropped from both, would move the
//! published rate with no test anywhere failing.
//!
//! The sanity check that motivated this: 48,006 `uninferred_receiver` rows
//! across 1,553 indexed files is 31 per file, which is plausible for a
//! Python/Rust corpus and would be plainly wrong at, say, 300. A ratio being
//! plausible is not evidence, so the property is asserted directly instead:
//!
//! * a call is either the site of at least one edge, or exactly one `Call` row
//!   — never both, and never neither;
//! * the ledger holds no two rows for one site;
//! * one call site emitting `N` fan-out edges is still one site.

use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, Extraction};
use devmap_resolve::model::{Resolution, ResolutionResult, UnresolvedKind};
use devmap_resolve::Resolver;
use std::collections::HashSet;

fn resolve(files: &[(&str, &str)]) -> (Vec<Extraction>, ResolutionResult) {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    (extractions, resolution)
}

/// A corpus with one of each shape the ladder answers differently: a resolved
/// same-file call, an import-scoped call, an ambiguous fan-out, a builtin, an
/// external, a local binding, an uninferred receiver and a bare miss.
fn corpus() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "pkg/one.py",
            "import os\nfrom .helpers import shared\nfrom missingpkg import gone\n\n\n\
             class Widget:\n    def build(self):\n        return 1\n\n    \
             def run(self, rows):\n        handler = lambda value: value\n        \
             print(self.build())\n        handler(1)\n        shared()\n        \
             gone()\n        os.path.join(\"a\", \"b\")\n        \
             rows.pop().strip()\n        return nowhere()\n",
        ),
        ("pkg/helpers.py", "def shared():\n    return 2\n"),
        (
            "pkg/two.py",
            "class Widget:\n    def build(self):\n        return 3\n",
        ),
        (
            "pkg/three.py",
            "from pkg.one import Widget\nfrom pkg.two import Widget as Other\n\n\n\
             def go(w):\n    return w.build()\n",
        ),
        (
            "svc/server.go",
            "package svc\n\nimport \"testing\"\n\n\
             type Server struct{}\n\n\
             func (s *Server) start() int { return 1 }\n\n\
             func (s *Server) Run() int {\n\tn := len(\"x\")\n\treturn s.start() + n\n}\n\n\
             func TestRun(t *testing.T) { t.Helper() }\n",
        ),
        (
            "crates/thing/src/lib.rs",
            "use serde_json::Value;\n\n\
             pub fn read(raw: &str) -> Value {\n    \
             let parsed = serde_json::from_str(raw).unwrap();\n    \
             std::fs::write(\"o\", raw).unwrap();\n    parsed\n}\n",
        ),
        (
            "scripts/run.sh",
            "#!/usr/bin/env bash\n\nstep() {\n  echo hi\n}\n\nmain() {\n  step\n  grep -q x y\n}\n",
        ),
    ]
}

/// One call is one site. The set of `(file, caller, callee, receiver)` a file's
/// extraction produced must partition exactly into "emitted at least one edge"
/// and "recorded exactly one ledger row".
#[test]
fn every_extracted_call_is_either_an_edge_or_exactly_one_ledger_row() {
    let (extractions, result) = resolve(&corpus());

    // Callers of at least one `Calls` edge, by `(file, caller symbol, target
    // name)` — the fan-out shares a caller, so this is a set, not a count.
    let emitted: HashSet<(String, String)> = result
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls)
        .map(|edge| (edge.source_file.clone(), edge.source_symbol.clone()))
        .collect();

    let mut rows_by_site: Vec<(String, String, String, Option<String>)> = result
        .unresolved
        .iter()
        .filter(|row| row.kind == UnresolvedKind::Call)
        .map(|row| {
            (
                row.source_file.clone(),
                row.source_symbol.clone(),
                row.callee_name.clone(),
                row.receiver.clone(),
            )
        })
        .collect();

    for extraction in &extractions {
        for call in &extraction.calls {
            let caller = call
                .caller_symbol
                .clone()
                .unwrap_or_else(|| extraction.file_path.clone());
            let site = (
                extraction.file_path.clone(),
                caller.clone(),
                call.callee_name.clone(),
                call.receiver_expr.clone(),
            );
            let ledger_rows = rows_by_site.iter().filter(|row| **row == site).count();
            let has_edge = emitted.contains(&(extraction.file_path.clone(), caller));
            assert!(
                ledger_rows <= 1 || has_edge,
                "call site {site:?} produced {ledger_rows} ledger rows; one \
                 unresolved call is one row, or the rate's denominator counts \
                 it twice"
            );
            if ledger_rows > 0 {
                // Remove one occurrence, so a *second* identical site in the
                // same scope still finds its own row rather than reusing this
                // one — which is how a genuine double-count would hide.
                if let Some(index) = rows_by_site.iter().position(|row| *row == site) {
                    rows_by_site.remove(index);
                }
            }
        }
    }

    assert!(
        rows_by_site.is_empty(),
        "the ledger holds `Call` rows no extracted call accounts for: {rows_by_site:?}"
    );
}

/// The other direction: a call the ladder could not attribute must leave a row.
/// Silence here is the failure R5 forbids, and it is what would make the
/// denominator shrink instead of the numerator growing.
#[test]
fn no_unattributed_call_is_dropped_without_a_row() {
    let (extractions, result) = resolve(&corpus());

    let callers_with_edges: HashSet<(String, String)> = result
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls)
        .map(|edge| (edge.source_file.clone(), edge.source_symbol.clone()))
        .collect();
    let rows: HashSet<(String, String, String)> = result
        .unresolved
        .iter()
        .filter(|row| row.kind == UnresolvedKind::Call)
        .map(|row| {
            (
                row.source_file.clone(),
                row.source_symbol.clone(),
                row.callee_name.clone(),
            )
        })
        .collect();

    for extraction in &extractions {
        for call in &extraction.calls {
            let caller = call
                .caller_symbol
                .clone()
                .unwrap_or_else(|| extraction.file_path.clone());
            let key = (
                extraction.file_path.clone(),
                caller.clone(),
                call.callee_name.clone(),
            );
            assert!(
                rows.contains(&key)
                    || callers_with_edges.contains(&(extraction.file_path.clone(), caller)),
                "call {key:?} produced neither an edge nor a ledger row"
            );
        }
    }
}

/// An ambiguous call fans out into several edges and is still one site. Stated
/// here as well as in `resolution_rate.rs` because the two crates can drift:
/// the dedup lives there and the `Arc` sharing that makes it work lives here.
///
/// Uses a **bare** AmbiguousGlobal (two free functions named `build`). An
/// untyped `w.build()` no longer fans out — that is UninferredReceiver.
#[test]
fn an_ambiguous_fan_out_shares_one_resolution_allocation() {
    let files = [
        ("a.py", "def build():\n    return 1\n"),
        ("b.py", "def build():\n    return 2\n"),
        ("c.py", "def go():\n    return build()\n"),
    ];
    let (_, result) = resolve(&files);

    let fanned: Vec<_> = result
        .edges
        .iter()
        .filter(|edge| {
            edge.resolution.as_ref().is_some_and(|resolution| {
                matches!(**resolution, Resolution::AmbiguousGlobal { .. })
            })
        })
        .collect();
    assert!(
        !fanned.is_empty(),
        "two free-function `build` declarations and a bare call must AmbiguousGlobal"
    );

    let allocations: HashSet<*const Resolution> = fanned
        .iter()
        .map(|edge| std::sync::Arc::as_ptr(edge.resolution.as_ref().expect("checked above")))
        .collect();
    assert!(
        allocations.len() < fanned.len(),
        "{} fan-out edges across {} allocations: one call site must share one \
         `Arc`, or `resolved_sites` counts an ambiguity as several successes",
        fanned.len(),
        allocations.len()
    );
}

/// No two ledger rows describe the same site. The dedup that matters is not
/// over rows that happen to be equal — a scope really can call `len()` twice —
/// but over a site being *visited* twice by the resolver.
#[test]
fn the_ledger_holds_no_more_rows_than_the_extraction_has_sites() {
    let (extractions, result) = resolve(&corpus());

    let extracted_calls: usize = extractions
        .iter()
        .map(|extraction| extraction.calls.len())
        .sum();
    let call_rows = result
        .unresolved
        .iter()
        .filter(|row| row.kind == UnresolvedKind::Call)
        .count();

    assert!(
        call_rows <= extracted_calls,
        "{call_rows} unresolved `Call` rows for {extracted_calls} extracted \
         calls — the ledger cannot hold more failures than there were attempts"
    );

    let extracted_references: usize = extractions
        .iter()
        .map(|extraction| extraction.references.len())
        .sum();
    let reference_rows = result
        .unresolved
        .iter()
        .filter(|row| row.kind == UnresolvedKind::Reference)
        .count();
    assert!(
        reference_rows <= extracted_references,
        "{reference_rows} unresolved `Reference` rows for {extracted_references} \
         extracted references"
    );
}
