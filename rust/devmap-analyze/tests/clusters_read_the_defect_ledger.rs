//! A cluster verdict must not outrank the single-symbol verdict on the same
//! evidence.
//!
//! W0.4 established the rule for one symbol: if an unresolved call site names
//! it, "nothing calls this" is a statement about the resolver rather than about
//! the code, and the finding is demoted. The single-symbol cascade also demotes
//! on an *ambiguous* caller, at the same 0.4, under `only_ambiguous_callers`.
//!
//! The component pass consulted neither. It excludes ambiguous and unresolved
//! edges from `is_reaching_edge` — correctly, so a guess cannot keep a cluster
//! alive — and then never looks at them again. So the very evidence that
//! demotes a single symbol is, for a cluster, simply discarded.
//!
//! Reproduced below, and measured before the fix:
//!
//! * `Widget.alpha`, named by an untyped-receiver call the ladder could not
//!   bind, is reported at **0.4** with the unresolved-namesake reason.
//! * `Task.alpha`, named by **the same call site**, is reported inside a
//!   cluster at **0.5**, and the reason says it is "reached by nothing
//!   outside the component" — which is false; `drive` reaches it.
//!
//! Identical evidence, and the strictly stronger claim gets the higher
//! confidence and the more absolute prose. `Task.alpha` never appears in the
//! single-symbol list at all — its sibling calls it deterministically — so the
//! cluster row is the *only* thing that names it, at the wrong tier.
//!
//! The second half of this file is a different hazard in the same pass:
//! `Imports` edges carry file paths in both symbol positions, so an ordinary
//! circular import is a two-node strongly connected component. Today nothing
//! comes of it only because every `File` symbol is emitted `is_exported: true`
//! at all three construction sites and therefore lands in
//! `externally_reachable_symbols`. Nothing states that dependency and nothing
//! tests it; the day a `File` node stops being "exported" — a reasonable change
//! to stop file nodes reading as public API — every circular import in every
//! repository becomes a dead cluster.

use devmap_analyze::{
    analyze, dead_clusters, DeadClusterScan, DEAD_CLUSTER_QUALIFIED_CONFIDENCE,
    UNRESOLVED_NAMESAKE_REASON,
};
use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, ExtractedSymbol, Extraction, Span, SymbolKind};
use devmap_resolve::model::{Resolution, ResolutionResult, ResolvedEdge, UnresolvedClass};
use devmap_resolve::Resolver;
use std::sync::Arc;

/// `Task.alpha` and `Task.betaStep` call only each other, both same-file and
/// therefore deterministic — so they are a genuine component.
const CLUSTER: &str = concat!(
    "class Task\n",
    "private fun Task.alpha(): String = betaStep()\n",
    "private fun Task.betaStep(): String = alpha()\n",
);

/// A second, unrelated `alpha`. Its only job is to give the unresolved
/// `it.alpha()` call site a namesake in more than one place — the defect
/// ledger matches by bare name, so both `Task.alpha` and `Widget.alpha` are
/// demoted by the same site.
const OTHER: &str = concat!(
    "class Widget\n",
    "private fun Widget.alpha(): String = \"w\"\n",
);

/// Calls `alpha` through a receiver nothing can type. The ladder no longer
/// widens an explicit-receiver miss into `AmbiguousGlobal` (that fabricated
/// edges for every `map.get()` / `String::new()`), so the site lands in the
/// unresolved ledger as `UninferredReceiver` — the same evidence class the
/// single-symbol namesake veto and the cluster qualifier both read.
const CALLER: &str = concat!(
    "fun drive(items: List<Any>): String {\n",
    "    return items.map { it.alpha() }.toString()\n",
    "}\n",
);

fn corpus() -> Vec<Extraction> {
    vec![
        extract_file("app/Caller.kt", CALLER),
        extract_file("app/Cluster.kt", CLUSTER),
        extract_file("app/Other.kt", OTHER),
    ]
}

fn resolve(extractions: &[Extraction]) -> ResolutionResult {
    let mut resolver = Resolver::new();
    resolver.index_extractions(extractions);
    resolver.resolve_all(extractions)
}

/// The fixture really does produce the shape this file is about. Asserted
/// separately so that if the resolver ever binds `it.alpha()` deterministically,
/// *this* is the test that fails and explains why the ones below changed.
#[test]
fn the_fixture_really_is_one_ambiguous_call_into_a_real_component() {
    let extractions = corpus();
    let resolution = resolve(&extractions);

    let unresolved_alpha = resolution.unresolved.iter().any(|row| {
        row.callee_name == "alpha"
            && row.receiver.as_deref() == Some("it")
            && matches!(row.class, UnresolvedClass::UninferredReceiver)
    });
    assert!(
        unresolved_alpha,
        "fixture assumption: `drive` names `alpha` through an untyped \
         receiver that the ladder could not bind: {:?}",
        resolution.unresolved
    );

    // And there must be no confident Calls edge that would keep the cluster
    // "reached" for free — otherwise the demotion path never fires.
    let confident_into_cluster = resolution.edges.iter().any(|edge| {
        edge.edge_kind == EdgeKind::Calls
            && edge.target_symbol == "app/Cluster.kt::Task.alpha"
            && edge.source_symbol == "app/Caller.kt::drive"
    });
    assert!(
        !confident_into_cluster,
        "fixture assumption: the untyped receiver must not produce a confident \
         Calls edge into the cluster: {:?}",
        resolution.edges
    );

    let scan = dead_clusters(&extractions, &resolution);
    assert_eq!(
        scan.clusters.len(),
        1,
        "fixture assumption: exactly one component is found: {scan:?}"
    );
    assert_eq!(scan.clusters[0].size, 2);
}

/// The headline. The same call site, the same unresolved namesake, two
/// verdicts — and the stronger claim must not be the more confident one.
#[test]
fn a_cluster_does_not_outrank_the_single_symbol_verdict_on_the_same_evidence() {
    let extractions = corpus();
    let resolution = resolve(&extractions);
    let summary = analyze(&extractions, &resolution);

    let single = summary
        .dead_symbols
        .iter()
        .find(|report| report.symbol_name == "Widget.alpha" && !report.is_exempt)
        .unwrap_or_else(|| {
            panic!(
                "`Widget.alpha` is named by an unresolved call site and must be \
                 reported qualified: {:?}",
                summary.dead_symbols
            )
        });
    assert_eq!(
        single.exemption_reason.as_deref(),
        Some(UNRESOLVED_NAMESAKE_REASON),
        "fixture assumption: the single-symbol path sees the unresolved namesake"
    );

    let cluster = summary
        .dead_clusters
        .clusters
        .first()
        .unwrap_or_else(|| panic!("the component is still found: {:?}", summary.dead_clusters));

    assert!(
        cluster.confidence <= single.confidence,
        "`Task.alpha` and `Widget.alpha` are reached by one call site through \
         one unresolved namesake. The component claim is strictly stronger than \
         the symbol claim, so it cannot carry the higher confidence: cluster \
         {} vs symbol {}",
        cluster.confidence,
        single.confidence
    );
}

/// And the prose must stop asserting something the edge list contradicts.
#[test]
fn a_qualified_cluster_says_what_reaches_it_instead_of_claiming_nothing_does() {
    let extractions = corpus();
    let resolution = resolve(&extractions);
    let summary = analyze(&extractions, &resolution);
    let cluster = &summary.dead_clusters.clusters[0];

    assert!(
        !cluster
            .reason
            .contains("reached by nothing outside the component"),
        "an ambiguous edge names a member, so this sentence is false: {:?}",
        cluster.reason
    );
    assert!(
        cluster.reason.contains("could not")
            || cluster.reason.contains("ambiguous")
            || cluster.reason.contains("unresolved"),
        "and the replacement must name the evidence rather than go quiet: {:?}",
        cluster.reason
    );
    assert!(
        (cluster.confidence - DEAD_CLUSTER_QUALIFIED_CONFIDENCE).abs() < 1e-6,
        "a qualified cluster lands on its own declared tier: {cluster:?}"
    );
}

/// A component nothing reaches at all — not even ambiguously — keeps the
/// confidence and the prose it has today. The demotion must be driven by
/// evidence, not applied to every cluster.
#[test]
fn an_unreached_cluster_is_not_demoted() {
    let extractions = vec![extract_file("app/Cluster.kt", CLUSTER)];
    let resolution = resolve(&extractions);
    let summary = analyze(&extractions, &resolution);

    let cluster = summary
        .dead_clusters
        .clusters
        .first()
        .unwrap_or_else(|| panic!("still a cluster: {:?}", summary.dead_clusters));
    assert!(
        cluster.confidence > DEAD_CLUSTER_QUALIFIED_CONFIDENCE,
        "nothing names these symbols, so the finding keeps its tier: {cluster:?}"
    );
    assert!(
        cluster
            .reason
            .contains("reached by nothing outside the component"),
        "and keeps its reason: {:?}",
        cluster.reason
    );
}

// ---------------------------------------------------------------------------
// The `Imports` hazard
// ---------------------------------------------------------------------------

/// A `File` symbol that is **not** exported, which no extractor emits today.
///
/// Built by hand precisely because it is unreachable from `extract_file`: the
/// point is that the component pass must not *depend* on a property three
/// construction sites happen to share and nothing enforces.
fn file_symbol(path: &str, exported: bool) -> ExtractedSymbol {
    ExtractedSymbol {
        name: path.rsplit('/').next().unwrap_or(path).to_string(),
        qualified_name: path.to_string(),
        kind: SymbolKind::File,
        span: Span {
            start_byte: 0,
            end_byte: 1,
        },
        is_exported: exported,
        docstring: None,
        signature: None,
        parent_symbol: None,
        body_signature: None,
        declaration_hash: None,
    }
}

fn import_edge(from: &str, to: &str) -> ResolvedEdge {
    ResolvedEdge::resolved(
        from.to_string(),
        to.to_string(),
        from.to_string(),
        to.to_string(),
        EdgeKind::Imports,
        Arc::new(Resolution::ImportScoped {
            target_symbol: to.to_string(),
            target_file: to.to_string(),
            imported_from: format!("./{to}"),
        }),
        None,
    )
}

fn scan_of(paths: [&str; 2], exported: bool) -> DeadClusterScan {
    let extractions: Vec<Extraction> = paths
        .iter()
        .map(|path| {
            let mut ext = extract_file(path, "def noop():\n    return 1\n");
            ext.symbols = vec![file_symbol(path, exported)];
            ext
        })
        .collect();
    let resolution = ResolutionResult {
        edges: vec![
            import_edge(paths[0], paths[1]),
            import_edge(paths[1], paths[0]),
        ],
        receiver_types: Default::default(),
        reexport_chains: Default::default(),
        unresolved: Vec::new(),
    };
    dead_clusters(&extractions, &resolution)
}

/// A circular import is not an abandoned subsystem, whatever `is_exported` says
/// about the file node.
///
/// `Imports` edges carry the *file path* in both symbol positions, so
/// `a.py -> b.py -> a.py` is a two-node component in the very graph this pass
/// walks. Today it is saved only by every `File` symbol being emitted exported;
/// this asserts the pass is right for a reason of its own.
#[test]
fn a_circular_import_is_never_a_dead_cluster() {
    for exported in [true, false] {
        let scan = scan_of(["pkg/a.py", "pkg/b.py"], exported);
        assert!(
            scan.clusters.is_empty(),
            "mutually importing files are not a cycle of symbols \
             (file symbols exported = {exported}): {scan:?}"
        );
        assert!(
            scan.unreachable_files.is_empty(),
            "and neither file is unreachable (exported = {exported}): {scan:?}"
        );
    }
}
