use devmap_analyze::*;
use devmap_extract::extract_file;
use devmap_extract::model::*;
use devmap_resolve::*;
use std::collections::BTreeMap;

#[test]
fn test_traversal_kernel_enqueued_vs_visited_g21() {
    // closes G21. The cap is enforced per enqueue so one high-fanout node
    // cannot overshoot the caller's budget in a single adjacency expansion.
    let edges: Vec<ResolvedEdge> = (0..20)
        .map(|index| ResolvedEdge {
            source_file: "root.py".to_string(),
            target_file: format!("leaf_{index}.py"),
            source_symbol: "root".to_string(),
            target_symbol: format!("leaf_{index}"),
            edge_kind: EdgeKind::Calls,
            confidence: Confidence::DETERMINISTIC,
            resolution: None,
            details: None,
            evidence: None,
        })
        .collect();
    let result = traverse_graph(
        &["root".to_string()],
        &edges,
        &TraversalOptions {
            max_depth: 3,
            max_nodes: 3,
            reverse: false,
        },
    );
    assert_eq!(result.visited_nodes.len(), 3);
    assert!(result.traversed_edges.len() <= 2);
}

#[test]
fn test_impact_never_walks_up_defines_edges() {
    let edge = ResolvedEdge {
        source_file: "container.py".to_string(),
        target_file: "container.py".to_string(),
        source_symbol: "Container".to_string(),
        target_symbol: "Container.child".to_string(),
        edge_kind: EdgeKind::Defines,
        confidence: Confidence::DETERMINISTIC,
        resolution: None,
        details: None,
        evidence: None,
    };
    let result = traverse_graph(
        &["Container.child".to_string()],
        &[edge],
        &TraversalOptions {
            max_depth: 3,
            max_nodes: 10,
            reverse: true,
        },
    );
    assert_eq!(
        result.visited_nodes,
        ["Container.child".to_string()].into_iter().collect()
    );
    assert!(result.traversed_edges.is_empty());
}

#[test]
fn test_x6_failed_parse_never_dead_candidate() {
    // closes X6
    let mut bad_file = extract_file("broken.py", "def ((((( invalid syntax");
    bad_file.parse_outcome = ParseOutcome::Failed {
        reason: "Parse error".to_string(),
    };

    let resolution = ResolutionResult {
        edges: vec![],
        receiver_types: BTreeMap::new(),
        reexport_chains: BTreeMap::new(),
        unresolved: Vec::new(),
    };

    let dead_reports = analyze_liveness(&[bad_file], &resolution);
    assert!(
        dead_reports
            .iter()
            .all(|r| r.is_exempt || r.confidence < 0.5),
        "Failed parse files must never be reported as confirmed dead code"
    );
}

#[test]
fn test_x6_partial_error_range_symbol_is_exempt() {
    let ext = extract_file("broken.py", "def broken():\n    value =\n");
    assert!(matches!(ext.parse_outcome, ParseOutcome::Partial { .. }));
    assert!(ext.symbols.iter().any(|symbol| symbol.name == "broken"));

    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&ext));
    let result = resolver.resolve_all(std::slice::from_ref(&ext));
    let reports = analyze_liveness(&[ext], &result);
    let broken = reports
        .iter()
        .find(|report| report.symbol_name == "broken")
        .expect("partially parsed symbol should remain visible as exempt evidence");
    assert!(broken.is_exempt);
    assert_eq!(
        broken.exemption_reason.as_deref(),
        Some("Symbol overlaps a tree-sitter parse error")
    );
}

/// A symbol-scoped exemption must name the check that actually fired.
///
/// The file-wide reason is the wrong answer here twice over: it is not why this
/// symbol is exempt, and reporting it implies the whole file was exempted when
/// only one symbol was.
#[test]
fn symbol_scoped_exemption_reports_its_own_reason_not_the_files() {
    let ext = extract_file(
        "svc/registry.go",
        "package svc\nfunc init() {}\nfunc unusedHelper() {}\n",
    );
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&ext));
    let result = resolver.resolve_all(std::slice::from_ref(&ext));
    let reports = analyze_liveness(&[ext], &result);

    let init = reports
        .iter()
        .find(|report| report.symbol_name == "init")
        .expect("init must stay visible as exempt evidence, not vanish");
    assert!(init.is_exempt);
    assert_eq!(
        init.exemption_reason.as_deref(),
        Some("Go runtime calls init() before main; it cannot be referenced")
    );
    assert!(init.confidence < 0.5, "got {}", init.confidence);

    let helper = reports
        .iter()
        .find(|report| report.symbol_name == "unusedHelper")
        .expect("an ordinary unused function is still a dead candidate");
    assert!(
        !helper.is_exempt && helper.confidence >= 0.9,
        "the symbol-scoped exemption must not spill onto its neighbours: {helper:?}"
    );
}

/// A file-scoped wiring annotation still exempts every symbol in the file.
///
/// The scope split must not silently downgrade the pre-existing file rules.
#[test]
fn file_scoped_exemption_still_covers_every_symbol_in_the_file() {
    let ext = extract_file(
        "api/service_pb2.py",
        "# Code generated by the protocol buffer compiler.  DO NOT EDIT!\ndef one(): pass\ndef two(): pass\n",
    );
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&ext));
    let result = resolver.resolve_all(std::slice::from_ref(&ext));
    let reports = analyze_liveness(&[ext], &result);

    for name in ["one", "two"] {
        let report = reports
            .iter()
            .find(|report| report.symbol_name == name)
            .unwrap_or_else(|| panic!("{name} should be reported"));
        assert!(report.is_exempt, "{name}: {report:?}");
        assert_eq!(report.exemption_reason.as_deref(), Some("Generated code"));
    }
}

#[test]
fn test_n4_n5_communities_connected_and_status() {
    // closes N4, N5
    let file_a = extract_file("a.py", "def foo(): pass\n");
    let resolution = ResolutionResult {
        edges: vec![],
        receiver_types: BTreeMap::new(),
        reexport_chains: BTreeMap::new(),
        unresolved: Vec::new(),
    };

    let comms = detect_communities(&[file_a], &resolution).communities;
    assert!(!comms.is_empty());
    for comm in comms {
        assert!(!comm.members.is_empty());
        assert!(comm.cohesion_score >= 0.0 && comm.cohesion_score <= 1.0);
    }
}

#[test]
fn test_ambiguous_calls_do_not_prove_a_candidate_live() {
    let first = extract_file("pkg/a.py", "def process(): pass\n");
    let second = extract_file("pkg/b.py", "def process(): pass\n");
    let caller = extract_file("main.py", "def run(): process()\n");

    let mut resolver = Resolver::new();
    resolver.index_extractions(&[first.clone(), second.clone(), caller.clone()]);
    let resolution = resolver.resolve_all(&[first.clone(), second.clone(), caller]);
    let reports = analyze_liveness(&[first, second], &resolution);

    assert!(reports.iter().any(|report| {
        report.file_path == "pkg/a.py"
            && report.symbol_name == "process"
            && !report.is_exempt
            && report.confidence <= 0.4
            && report.exemption_reason.as_deref() == Some("only_ambiguous_callers")
    }));
    assert!(reports.iter().any(|report| {
        report.file_path == "pkg/b.py"
            && report.symbol_name == "process"
            && !report.is_exempt
            && report.confidence <= 0.4
            && report.exemption_reason.as_deref() == Some("only_ambiguous_callers")
    }));
}

// ---------------------------------------------------------------------------
// N5 — weighted Louvain + connectivity post-pass.
// These fail against the previous connected-components implementation, which
// returned a single community for any connected graph.
// ---------------------------------------------------------------------------

/// Files are only nodes here; the partition is driven entirely by `edges`.
fn nodes(paths: &[&str]) -> Vec<Extraction> {
    paths
        .iter()
        .map(|p| extract_file(p, "def noop(): pass\n"))
        .collect()
}

fn edges(pairs: &[(&str, &str, usize)]) -> ResolutionResult {
    let mut edges = Vec::new();
    for (src, dst, multiplicity) in pairs {
        for i in 0..*multiplicity {
            edges.push(ResolvedEdge {
                source_file: (*src).to_string(),
                target_file: (*dst).to_string(),
                source_symbol: format!("caller_{}", i),
                target_symbol: "noop".to_string(),
                edge_kind: EdgeKind::Calls,
                confidence: Confidence::DETERMINISTIC,
                resolution: None,
                details: None,
                evidence: None,
            });
        }
    }
    ResolutionResult {
        edges,
        receiver_types: BTreeMap::new(),
        reexport_chains: BTreeMap::new(),
        unresolved: Vec::new(),
    }
}

fn community_of<'a>(comms: &'a [CommunityReport], path: &str) -> &'a CommunityReport {
    comms
        .iter()
        .find(|c| c.members.iter().any(|m| m == path))
        .unwrap_or_else(|| panic!("{path} belongs to no community"))
}

#[test]
fn test_n5_louvain_splits_clusters_joined_by_a_bridge() {
    // Two triangles joined by one edge. Connected components sees one blob;
    // modularity optimisation must recover the two clusters.
    let files = nodes(&["a1.py", "a2.py", "a3.py", "b1.py", "b2.py", "b3.py"]);
    let resolution = edges(&[
        ("a1.py", "a2.py", 5),
        ("a2.py", "a3.py", 5),
        ("a3.py", "a1.py", 5),
        ("b1.py", "b2.py", 5),
        ("b2.py", "b3.py", 5),
        ("b3.py", "b1.py", 5),
        ("a1.py", "b1.py", 1), // the bridge
    ]);

    let comms = detect_communities(&files, &resolution).communities;

    assert!(
        comms.len() >= 2,
        "bridged clusters must not collapse into one community, got {}",
        comms.len()
    );
    assert_eq!(
        community_of(&comms, "a1.py").community_id,
        community_of(&comms, "a3.py").community_id,
        "densely-linked files belong together"
    );
    assert_ne!(
        community_of(&comms, "a1.py").community_id,
        community_of(&comms, "b1.py").community_id,
        "a single bridge must not merge two dense clusters"
    );
}

#[test]
fn test_n5_edge_weight_is_call_multiplicity() {
    // Identical topology, one changed multiplicity. An algorithm that treats
    // edges as present/absent cannot tell these two graphs apart, so a
    // differing partition is proof the weight reaches the modularity term.
    let two_triangles = |bridge: usize| {
        edges(&[
            ("a1.py", "a2.py", 5),
            ("a2.py", "a3.py", 5),
            ("a3.py", "a1.py", 5),
            ("b1.py", "b2.py", 5),
            ("b2.py", "b3.py", 5),
            ("b3.py", "b1.py", 5),
            ("a1.py", "b1.py", bridge),
        ])
    };
    let files = nodes(&["a1.py", "a2.py", "a3.py", "b1.py", "b2.py", "b3.py"]);

    let light = detect_communities(&files, &two_triangles(1)).communities;
    let heavy = detect_communities(&files, &two_triangles(60)).communities;

    assert_ne!(
        community_of(&light, "a1.py").community_id,
        community_of(&light, "b1.py").community_id,
        "a single call across the bridge must not merge the clusters"
    );
    assert_eq!(
        community_of(&heavy, "a1.py").community_id,
        community_of(&heavy, "b1.py").community_id,
        "sixty calls across the bridge must pull its endpoints together"
    );
}

#[test]
fn test_n5_every_community_is_internally_connected() {
    // The guarantee the post-pass exists to provide.
    let files = nodes(&["p.py", "q.py", "r.py", "s.py", "lonely.py"]);
    let resolution = edges(&[
        ("p.py", "q.py", 3),
        ("r.py", "s.py", 3),
        ("q.py", "r.py", 1),
    ]);

    let comms = detect_communities(&files, &resolution).communities;

    let adjacency: Vec<(String, String)> = resolution
        .edges
        .iter()
        .map(|e| (e.source_file.clone(), e.target_file.clone()))
        .collect();

    for comm in &comms {
        let members: std::collections::BTreeSet<&str> =
            comm.members.iter().map(|m| m.as_str()).collect();
        let mut reached: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        let start = *members.iter().next().unwrap();
        let mut queue = vec![start];
        reached.insert(start);
        while let Some(curr) = queue.pop() {
            for (a, b) in &adjacency {
                let next = if a == curr && members.contains(b.as_str()) {
                    Some(b.as_str())
                } else if b == curr && members.contains(a.as_str()) {
                    Some(a.as_str())
                } else {
                    None
                };
                if let Some(n) = next {
                    if reached.insert(n) {
                        queue.push(n);
                    }
                }
            }
        }
        assert_eq!(
            reached.len(),
            members.len(),
            "community {} is internally disconnected: {:?}",
            comm.community_id,
            comm.members
        );
    }
}

#[test]
fn test_n5_cohesion_is_measured_not_assumed() {
    // A file with no edges cannot be measured; it must score 0.0 rather than a
    // flattering 1.0 — an unexamined value must never read as a perfect one.
    let files = nodes(&["alone.py"]);
    let comms = detect_communities(&files, &edges(&[])).communities;

    assert_eq!(comms.len(), 1);
    assert_eq!(
        comms[0].cohesion_score, 0.0,
        "an isolated file has no measurable cohesion"
    );

    // A fully-internal cluster keeps all of its weight and scores 1.0.
    let files = nodes(&["m.py", "n.py"]);
    let comms = detect_communities(&files, &edges(&[("m.py", "n.py", 4)])).communities;
    let scored = community_of(&comms, "m.py");
    assert_eq!(
        scored.cohesion_score, 1.0,
        "a community with no outgoing weight is fully cohesive"
    );
}

#[test]
fn test_n5_partition_is_deterministic() {
    let files = nodes(&["a1.py", "a2.py", "a3.py", "b1.py", "b2.py", "b3.py"]);
    let resolution = edges(&[
        ("a1.py", "a2.py", 4),
        ("a2.py", "a3.py", 4),
        ("b1.py", "b2.py", 4),
        ("b2.py", "b3.py", 4),
        ("a3.py", "b1.py", 1),
    ]);

    let expected =
        serde_json::to_string(&detect_communities(&files, &resolution).communities).unwrap();
    for _ in 0..16 {
        let actual =
            serde_json::to_string(&detect_communities(&files, &resolution).communities).unwrap();
        assert_eq!(actual, expected, "partition changed between identical runs");
    }
}

/// The ledger is part of the durable analysis summary, so the count survives
/// into the persisted generation rather than living only in memory.
#[test]
fn unresolved_call_count_reaches_the_analysis_summary() {
    let source = extract_file(
        "app.py",
        "def run():\n    missing_one()\n    missing_two()\n",
    );
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&source));
    let resolution = resolver.resolve_all(std::slice::from_ref(&source));
    let analysis = analyze(std::slice::from_ref(&source), &resolution);
    assert_eq!(analysis.unresolved_calls, 2);
}

/// Reverse traversal (impact) must not climb containment edges.
///
/// A symbol is contained by its file, so following that edge backwards makes
/// the *file* an answer to "what depends on `victim`?". Containment is not a
/// dependency, and admitting it puts a file node in every impact set.
/// cargo-mutants found the original guard here to be dead code — it named
/// `Defines`, a kind nothing constructs — so deleting it changed no outcome.
#[test]
fn impact_does_not_climb_containment_to_the_containing_file() {
    let source = extract_file(
        "app.py",
        "def victim():\n    return 1\n\ndef unrelated_sibling():\n    return 2\n",
    );
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&source));
    let resolution = resolver.resolve_all(std::slice::from_ref(&source));

    assert!(
        resolution
            .edges
            .iter()
            .any(|edge| edge.edge_kind == EdgeKind::Contains
                && edge.target_symbol == "app.py::victim"),
        "fixture must actually exercise a containment edge"
    );

    let result = traverse_graph(
        &["app.py::victim".to_string()],
        &resolution.edges,
        &TraversalOptions {
            max_depth: 5,
            max_nodes: 100,
            reverse: true,
        },
    );
    assert!(
        !result.visited_nodes.iter().any(|node| node == "app.py"),
        "impact climbed the containment edge to the containing file: {:?}",
        result.visited_nodes
    );
}

/// Every dead-symbol report for `symbol`, so an assertion can name the tier it saw.
fn dead_report<'a>(
    analysis: &'a AnalysisSummary,
    symbol: &str,
) -> Option<&'a devmap_analyze::DeadSymbolReport> {
    analysis
        .dead_symbols
        .iter()
        .find(|report| report.symbol_name == symbol)
}

fn analyze_files(files: &[(&str, &str)]) -> AnalysisSummary {
    let extractions: Vec<_> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    analyze(&extractions, &resolution)
}

/// SC6a. A sealed-interface marker method is never called anywhere — that is the
/// point of the pattern — and being unexported it cannot be rescued by
/// `is_exported` either. Deleting it breaks the build, so it must not be
/// reported as confidently dead just because the interface sits in another file.
#[test]
fn go_method_implementing_interface_from_another_file_is_exempt() {
    let analysis = analyze_files(&[
        (
            "ast/node.go",
            "package ast\n\
             type Node interface {\n\
             \tisNode()\n\
             }\n",
        ),
        (
            "ast/leaf.go",
            "package ast\n\
             type Leaf struct{}\n\
             func (l *Leaf) isNode() {}\n\
             func (l *Leaf) orphan() string { return \"\" }\n",
        ),
    ]);

    let marker = dead_report(&analysis, "Leaf.isNode")
        .expect("Leaf.isNode must still be reported, as an exempt row");
    assert!(
        marker.is_exempt && marker.confidence < 0.9,
        "Leaf.isNode implements `Node` declared in ast/node.go and must not be \
         confidently dead, got {marker:?}"
    );
    assert_eq!(
        marker.exemption_reason.as_deref(),
        Some("implements interface `Node`; calls reach it through the interface"),
        "the cross-file exemption must give the same reason as the same-file one"
    );

    let orphan = dead_report(&analysis, "Leaf.orphan")
        .expect("Leaf.orphan is genuinely unused and must be reported");
    assert!(
        !orphan.is_exempt && orphan.confidence >= 0.9,
        "a method matching no interface spec must stay confidently dead — the \
         exemption leaked to every method on the type: {orphan:?}"
    );
}

/// The exemption is scoped to the declaring Go package and to the declared
/// arity. Widening either would silently disable dead-method detection for Go.
#[test]
fn go_interface_exemption_does_not_cross_packages_or_arities() {
    let analysis = analyze_files(&[
        (
            "ast/node.go",
            "package ast\n\
             type Node interface {\n\
             \tisNode()\n\
             \ttag(depth int) string\n\
             }\n",
        ),
        (
            "ast/leaf.go",
            "package ast\n\
             type Leaf struct{}\n\
             func (l *Leaf) isNode() {}\n\
             func (l *Leaf) tag() string { return \"\" }\n",
        ),
        (
            "render/bar.go",
            "package render\n\
             type Bar struct{}\n\
             func (b *Bar) isNode() {}\n",
        ),
    ]);

    let in_package = dead_report(&analysis, "Leaf.isNode").expect("Leaf.isNode must be reported");
    assert!(
        in_package.is_exempt,
        "control: the same-package implementation must be exempt, else the rest \
         of this test proves nothing: {in_package:?}"
    );

    let wrong_arity =
        dead_report(&analysis, "Leaf.tag").expect("Leaf.tag is unused and must be reported");
    assert!(
        !wrong_arity.is_exempt && wrong_arity.confidence >= 0.9,
        "`tag()` takes no parameters and cannot satisfy `tag(depth int) string`, \
         so it must stay confidently dead: {wrong_arity:?}"
    );

    let other_package =
        dead_report(&analysis, "Bar.isNode").expect("Bar.isNode is unused and must be reported");
    assert!(
        !other_package.is_exempt && other_package.confidence >= 0.9,
        "`isNode` is unexported, so its name is qualified by package `ast` and no \
         type in package `render` can satisfy `ast.Node`: {other_package:?}"
    );
}
