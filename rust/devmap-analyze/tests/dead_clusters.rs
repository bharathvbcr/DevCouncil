//! W1.1 — an abandoned cycle stops being invisible.
//!
//! Liveness is a one-hop inbound-edge join. A subsystem whose functions call
//! each other has an inbound edge on every symbol, so the kernel reported
//! **zero** of it: the classic dead-code case, and the single largest recall
//! hole in the analysis.
//!
//! Measured on `three_mutually_recursive_functions` below, against the
//! pre-change kernel: `dead_symbols` named none of the three, because each had
//! a caller. The cluster pass names them once, as one finding.
//!
//! Every test here also asserts its OFF direction, because the failure mode of
//! a reachability pass is not a wrong answer — it is a pass that reports
//! everything, or nothing, and both look like working code from one side.

use devmap_analyze::*;
use devmap_extract::extract_file;
use devmap_extract::model::{Extraction, ExtractionEngine, ParseOutcome};
use devmap_resolve::model::ResolutionResult;
use devmap_resolve::Resolver;

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

fn scan(files: &[(&str, &str)]) -> DeadClusterScan {
    let (extractions, resolution) = resolve(files);
    dead_clusters(&extractions, &resolution)
}

/// Three functions that only call each other, reachable from nothing.
const ABANDONED: &str = "def alpha():\n    return beta()\n\n\n\
                         def beta():\n    return gamma()\n\n\n\
                         def gamma():\n    return alpha()\n";

/// The same shape, plus an entry point that reaches it.
const REACHED: &str = "def alpha():\n    return beta()\n\n\n\
                       def beta():\n    return gamma()\n\n\n\
                       def gamma():\n    return alpha()\n\n\n\
                       def main():\n    return alpha()\n";

/// The recall hole, closed.
#[test]
fn three_mutually_recursive_functions_are_one_finding() {
    let (extractions, resolution) = resolve(&[("orphan.py", ABANDONED)]);

    // The premise: the single-symbol pass cannot see this, because every member
    // has an inbound edge. Without this assertion the test would pass against a
    // kernel that had simply started reporting all three individually.
    let singly_reported: Vec<&DeadSymbolReport> = analyze_liveness(&extractions, &resolution)
        .iter()
        .filter(|r| !r.is_exempt && ["alpha", "beta", "gamma"].contains(&r.symbol_name.as_str()))
        .cloned()
        .collect::<Vec<_>>()
        .leak()
        .iter()
        .collect();
    assert!(
        singly_reported.is_empty(),
        "each of these has a caller, so the one-hop join must still not report \
         them — that is the hole this pass exists to fill: {singly_reported:?}"
    );

    let scan = dead_clusters(&extractions, &resolution);
    assert_eq!(
        scan.clusters.len(),
        1,
        "one abandoned cycle is one finding, not three: {scan:?}"
    );
    let cluster = &scan.clusters[0];
    assert_eq!(cluster.size, 3);
    assert!(
        cluster.members.iter().any(|m| m.contains("alpha"))
            && cluster.members.iter().any(|m| m.contains("beta"))
            && cluster.members.iter().any(|m| m.contains("gamma")),
        "every member is named: {cluster:?}"
    );
}

/// The OFF direction: the same shape, reached from an entry point.
#[test]
fn a_cycle_something_reaches_is_not_reported() {
    let scan = scan(&[("app.py", REACHED)]);
    assert!(
        scan.clusters.is_empty(),
        "an inbound edge from outside the component makes it live: {scan:?}"
    );
}

/// An exported member makes the whole component live.
///
/// Reached from outside the corpus entirely, which no edge can show. Without
/// this, a perfectly ordinary pair of mutually recursive public functions is a
/// "cluster nothing reaches" — and that is most recursive code in a library.
#[test]
fn an_exported_member_keeps_the_cluster_alive() {
    let scan = scan(&[(
        "lib.ts",
        "export function alpha(): number {\n  return beta();\n}\n\n\
         export function beta(): number {\n  return alpha();\n}\n",
    )]);
    assert!(
        scan.clusters.is_empty(),
        "an exported symbol can be called from outside the corpus: {scan:?}"
    );
}

/// A straight chain is not a cluster.
///
/// `a -> b -> c` with nothing calling `a` is three ordinary dead symbols, which
/// the single-symbol pass already owns. Reporting it here would duplicate every
/// finding in the corpus into two lists.
#[test]
fn an_acyclic_chain_is_not_a_cluster() {
    let scan = scan(&[(
        "chain.py",
        "def a():\n    return b()\n\n\ndef b():\n    return c()\n\n\ndef c():\n    return 1\n",
    )]);
    assert!(
        scan.clusters.is_empty(),
        "a chain has no cycle; its head is an ordinary single-symbol finding: {scan:?}"
    );
}

/// A single self-recursive function is a cluster of one.
///
/// `def loop(): return loop()` has an inbound edge — its own — so the one-hop
/// join misses it for exactly the same reason it misses a three-cycle. Treating
/// size 1 as "not a cycle" would leave the simplest case of the bug open.
#[test]
fn a_self_recursive_function_is_a_cluster_of_one() {
    let scan = scan(&[("solo.py", "def looper():\n    return looper()\n")]);
    assert_eq!(scan.clusters.len(), 1, "{scan:?}");
    assert_eq!(scan.clusters[0].size, 1);
}

/// An ordinary uncalled function is *not* a cluster of one.
///
/// The discriminator for size 1 is the self-edge. Without it every symbol in
/// the corpus would be its own cluster.
#[test]
fn an_ordinary_uncalled_function_is_not_a_cluster() {
    let scan = scan(&[(
        "solo.py",
        "def orphan():\n    return 1\n\n\ndef main():\n    return 2\n",
    )]);
    assert!(scan.clusters.is_empty(), "{scan:?}");
}

/// A cluster verdict never reaches the `extracted` tier.
///
/// It rests on the whole graph being complete in a way a single-symbol verdict
/// does not: one missed call edge into the component makes the entire finding
/// wrong, where the same missed edge costs a single-symbol finding only itself.
#[test]
fn a_cluster_verdict_stays_below_the_confident_tier() {
    let scan = scan(&[("orphan.py", ABANDONED)]);
    let cluster = &scan.clusters[0];
    assert!(
        cluster.confidence < 0.9,
        "a whole-graph claim is weaker than a one-symbol claim: {cluster:?}"
    );
    assert!(
        cluster.confidence >= 0.4,
        "and it is still worth reporting: {cluster:?}"
    );
}

/// A degraded corpus caps the cluster verdict too.
#[test]
fn coverage_loss_caps_a_cluster_finding() {
    let (mut extractions, _) = resolve(&[("orphan.py", ABANDONED)]);
    // Same-language coverage hole. An HCL call-blind file cannot reference
    // Python, so language-scoped completeness would leave the cluster uncapped.
    let mut hole = extract_file("lost.py", "def never_seen():\n    return 0\n");
    hole.parse_outcome = ParseOutcome::Fallback {
        reason: "forced pattern recovery for cluster coverage fixture".to_string(),
    };
    hole.engine = ExtractionEngine::RegexFallback {
        requested_language: "python".to_string(),
    };
    hole.imports.clear();
    hole.calls.clear();
    extractions.push(hole);
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);

    let summary = analyze(&extractions, &resolution);
    assert!(
        !summary.dead_clusters.clusters.is_empty(),
        "the cluster is still found: {:?}",
        summary.dead_clusters
    );
    for cluster in &summary.dead_clusters.clusters {
        assert!(
            cluster.confidence <= COVERAGE_LOSS_CONFIDENCE_CAP,
            "a corpus with unread calls must not publish a cluster at the tier \
             a complete corpus would: {cluster:?}"
        );
    }
}

/// The member list is a capped sample and says so; the size is exact.
#[test]
fn a_large_cluster_reports_an_exact_size_and_a_capped_sample() {
    // A ring of N mutually recursive functions, larger than the member cap.
    let n = DEAD_CLUSTER_MEMBER_CAP + 7;
    let mut source = String::new();
    for i in 0..n {
        source.push_str(&format!(
            "def f{i}():\n    return f{next}()\n\n\n",
            i = i,
            next = (i + 1) % n
        ));
    }
    let scan = scan(&[("ring.py", &source)]);
    assert_eq!(scan.clusters.len(), 1, "{scan:?}");
    let cluster = &scan.clusters[0];
    assert_eq!(cluster.size, n, "the count is the real membership");
    assert_eq!(
        cluster.members.len(),
        DEAD_CLUSTER_MEMBER_CAP,
        "the list is a sample"
    );
    assert!(
        cluster
            .reason
            .contains(&format!("{DEAD_CLUSTER_MEMBER_CAP} of {n}")),
        "a capped sample presented as complete is the error this whole work \
         order is about: {cluster:?}"
    );
}

/// An ambiguous edge must not keep a cluster alive.
///
/// A speculative resolution is evidence a symbol *may* be reached, not proof
/// that it is. Admitting it here would silently suppress findings on exactly
/// the ground the confidence ladder exists to keep out of verdicts.
#[test]
fn an_ambiguous_inbound_edge_does_not_keep_a_cluster_alive() {
    // `caller` calls `alpha`, but two files declare `alpha`, so the resolver
    // cannot say which — the edge into the component is ambiguous.
    let (extractions, resolution) = resolve(&[
        ("orphan.py", ABANDONED),
        ("decoy.py", "def alpha():\n    return 0\n"),
        ("caller.py", "def entry():\n    return alpha()\n"),
    ]);

    let ambiguous = resolution.edges.iter().any(|e| {
        matches!(
            e.resolution.as_deref(),
            Some(devmap_resolve::model::Resolution::AmbiguousGlobal { .. })
        )
    });
    if !ambiguous {
        eprintln!("note: fixture produced no ambiguous edge; property untested here");
        return;
    }

    let scan = dead_clusters(&extractions, &resolution);
    assert!(
        !scan.clusters.is_empty(),
        "an ambiguous edge into the component is not proof it is reached: {scan:?}"
    );
}

/// Cluster ids are stable across runs.
///
/// They are assigned by traversal order over `resolution.edges`, which the
/// resolver sorts (R4). A finding whose id moves between builds cannot be
/// tracked, acknowledged, or ratcheted.
#[test]
fn cluster_ids_are_deterministic() {
    let first = scan(&[("orphan.py", ABANDONED)]);
    let second = scan(&[("orphan.py", ABANDONED)]);
    assert_eq!(first, second);
}

/// An empty corpus produces an empty scan, not a panic.
#[test]
fn an_empty_corpus_produces_an_empty_scan() {
    let scan = scan(&[]);
    assert_eq!(scan, DeadClusterScan::default());
    assert!(!scan.refused_oversized_graph);
}

/// W2.4 — `unreachable_files` can be non-empty, which is the whole point.
///
/// The key shipped hardcoded `[]` beside an unconditional
/// `liveness_unreachable_unreliable: true`, so four Python consumers suppressed
/// it permanently: it was a third state, neither a computed empty nor an honest
/// absence. Every key in the envelope must be able to be non-empty for some
/// input, and this is that input.
#[test]
fn a_file_wholly_inside_a_dead_cluster_is_unreachable() {
    let scan = scan(&[("orphan.py", ABANDONED)]);
    assert_eq!(
        scan.unreachable_files,
        vec!["orphan.py".to_string()],
        "every symbol this file declares is in a component nothing reaches: {scan:?}"
    );
}

/// One live symbol keeps the file reachable.
///
/// The claim is about the whole file. A file with an abandoned cycle *and* a
/// working function is not unreachable, and reporting it would be the mass
/// false positive that made the entry-root BFS untrustworthy in the first
/// place.
#[test]
fn a_file_with_one_live_symbol_is_not_unreachable() {
    let scan = scan(&[
        (
            "mixed.py",
            "def alpha():\n    return beta()\n\n\ndef beta():\n    return alpha()\n\n\n\
             def used():\n    return 1\n",
        ),
        ("main.py", "from mixed import used\n\n\nused()\n"),
    ]);
    assert!(
        !scan.clusters.is_empty(),
        "the abandoned cycle is still found: {scan:?}"
    );
    assert!(
        scan.unreachable_files.is_empty(),
        "a file with a live symbol is not unreachable: {scan:?}"
    );
}

/// A file that declares nothing is not evidence of anything.
///
/// Without the guard, every `.md`, `.json` and `.yaml` in a tree — each of
/// which contributes only its own `File` node — would vacuously satisfy "all
/// declared symbols are in a cluster" and flood the list.
#[test]
fn a_file_that_declares_nothing_is_not_unreachable() {
    let scan = scan(&[
        ("orphan.py", ABANDONED),
        ("README.md", "# Title\n\nProse.\n"),
        ("data.json", "{\"a\": 1}\n"),
    ]);
    assert_eq!(
        scan.unreachable_files,
        vec!["orphan.py".to_string()],
        "prose and data declare nothing, so they are not unreachable: {scan:?}"
    );
}
