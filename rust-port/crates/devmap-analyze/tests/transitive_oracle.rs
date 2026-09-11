//! Independent distance relaxation, permutations and concurrent bounded walks.
use devmap_analyze::traversal::{traverse_graph_indexed, AdjacencyIndex, TraversalLimits};
use devmap_extract::model::{Confidence, EdgeKind};
use devmap_resolve::model::ResolvedEdge;
use std::collections::BTreeSet;

fn edge(from: usize, to: usize, kind: EdgeKind, confidence: f32) -> ResolvedEdge {
    ResolvedEdge {
        source_file: "graph.rs".into(),
        target_file: "graph.rs".into(),
        source_symbol: format!("graph.rs::n{from}"),
        target_symbol: format!("graph.rs::n{to}"),
        edge_kind: kind,
        confidence: Confidence(confidence),
        resolution: None,
        details: None,
        evidence: None,
    }
}

fn next(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state >> 32
}

#[test]
fn transitive_walks_match_independent_relaxation_over_512_hostile_graphs() {
    const NODES: usize = 24;
    let kinds = [
        EdgeKind::Calls,
        EdgeKind::References,
        EdgeKind::Contains,
        EdgeKind::Defines,
        EdgeKind::Imports,
        EdgeKind::MemberOf,
    ];
    let mut state = 0xdecafbad;
    for case in 0..512 {
        let mut specs = Vec::new();
        for _ in 0..120 {
            specs.push((
                next(&mut state) as usize % NODES,
                next(&mut state) as usize % NODES,
                kinds[next(&mut state) as usize % kinds.len()],
                if next(&mut state).is_multiple_of(3) {
                    0.4
                } else {
                    1.0
                },
            ));
        }
        let edges: Vec<_> = specs.iter().map(|&(a, b, k, c)| edge(a, b, k, c)).collect();
        let mut shuffled = edges.clone();
        for i in (1..shuffled.len()).rev() {
            shuffled.swap(i, next(&mut state) as usize % (i + 1));
        }
        let seeds = [0, case % NODES, 0];
        let starts: Vec<_> = seeds.iter().map(|n| format!("graph.rs::n{n}")).collect();
        for reverse in [false, true] {
            for floor in [0.0, 0.75] {
                let allowed: Vec<_> = specs
                    .iter()
                    .filter(|(_, _, kind, c)| {
                        *c >= floor
                            && (!reverse || matches!(kind, EdgeKind::Calls | EdgeKind::References))
                    })
                    .collect();
                // Repeated relaxation over a flat edge list; no traversal queue,
                // adjacency index, node policy helper or production matcher.
                let mut distance = [usize::MAX; NODES];
                for seed in seeds {
                    distance[seed] = 0;
                }
                for _ in 0..NODES {
                    for &&(a, b, _, _) in &allowed {
                        let (from, to) = if reverse { (b, a) } else { (a, b) };
                        distance[to] = distance[to].min(distance[from].saturating_add(1));
                    }
                }
                for depth in [0, 1, 2, 3, 6, 64] {
                    let expected: BTreeSet<_> = (0..NODES)
                        .filter(|&n| distance[n] <= depth)
                        .map(|n| format!("graph.rs::n{n}"))
                        .collect();
                    let expected_edges: BTreeSet<_> = allowed
                        .iter()
                        .filter(|&&&(a, b, _, _)| distance[if reverse { b } else { a }] < depth)
                        .map(|&&(a, b, k, _)| {
                            (
                                format!("graph.rs::n{a}"),
                                format!("graph.rs::n{b}"),
                                format!("{k:?}"),
                            )
                        })
                        .collect();
                    let capped = allowed
                        .iter()
                        .any(|&&(a, b, _, _)| distance[if reverse { b } else { a }] == depth);
                    for graph in [&edges, &shuffled] {
                        let index =
                            AdjacencyIndex::build(graph, reverse).with_min_confidence(floor);
                        let walk = traverse_graph_indexed(
                            &starts,
                            &index,
                            TraversalLimits {
                                max_depth: depth,
                                max_nodes: 500,
                            },
                        );
                        assert_eq!(
                            walk.visited_nodes, expected,
                            "case={case}, reverse={reverse}, depth={depth}, floor={floor}"
                        );
                        let actual: BTreeSet<_> = walk
                            .traversed_edges
                            .iter()
                            .map(|e| (e.source.clone(), e.target.clone(), e.edge_kind.clone()))
                            .collect();
                        assert_eq!(actual, expected_edges);
                        assert_eq!(walk.stop.depth_capped, capped);
                        assert_eq!(walk.stop.is_incomplete(), capped);
                    }
                }
            }
        }
    }
}

#[test]
fn duplicate_flood_does_not_hide_a_late_distinct_seed() {
    let mut starts = vec!["graph.rs::n0".to_string(); 100_000];
    starts.push("graph.rs::n1".to_string());
    let index = AdjacencyIndex::build(&[], false);
    let walk = traverse_graph_indexed(
        &starts,
        &index,
        TraversalLimits {
            max_depth: 64,
            max_nodes: 2,
        },
    );
    assert_eq!(walk.visited_nodes.len(), 2);
    assert!(!walk.stop.is_incomplete());
}

#[test]
fn concurrent_walks_preserve_caps_and_order_on_a_shared_wide_index() {
    let edges: Vec<_> = (1..=100_000)
        .map(|n| edge(0, n, EdgeKind::Calls, 1.0))
        .collect();
    let index = AdjacencyIndex::build(&edges, false);
    std::thread::scope(|scope| {
        for _ in 0..8 {
            let index = &index;
            scope.spawn(move || {
                for cap in [0, 1, 2, 64, 5000] {
                    let walk = traverse_graph_indexed(
                        &["graph.rs::n0".into()],
                        index,
                        TraversalLimits {
                            max_depth: 64,
                            max_nodes: cap,
                        },
                    );
                    assert_eq!(walk.visited_nodes.len(), cap);
                    assert_eq!(walk.traversed_edges.len(), cap.saturating_sub(1));
                    assert!(walk.stop.is_incomplete());
                    if cap > 1 {
                        assert_eq!(walk.traversed_edges[0].target, "graph.rs::n1");
                    }
                }
            });
        }
    });
}
