//! Shared Graph Traversal Kernel.
//!
//! Closes:
//! - G8: Parametric depth limit.
//! - G21: Enqueued set tracked separately from Visited set to prevent queue re-addition deadlocks.
//! - CodeGraph #536/#774: Impact semantics (no upward contains, instantiates is caller, children fold at same depth).

use devmap_resolve::model::*;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EdgeIdentity {
    pub source: String,
    pub target: String,
    pub edge_kind: String,
    pub line: u32,
    pub col: u32,
}

#[derive(Debug, Clone)]
pub struct TraversalOptions {
    pub max_depth: usize,
    pub max_nodes: usize,
    pub reverse: bool, // true for impact/callers, false for callee trace
}

impl Default for TraversalOptions {
    fn default() -> Self {
        Self {
            max_depth: 10,
            max_nodes: 1000,
            reverse: false,
        }
    }
}

/// What the walk gave up on, so a caller can tell a complete blast radius from
/// a capped one.
///
/// `TraversalResult` carried only what was found. Five paths in
/// `traverse_graph` decline silently — starts beyond `max_nodes`, a frontier
/// pruned at `max_depth`, the node cap, the enqueue cap, and the recorded-edge
/// cap — and every one produced a result indistinguishable from a walk that
/// ran to completion. `impact` and `trace` then reported `truncated: false`
/// with `total` equal to what survived, because the truncation function counted
/// what it *received* rather than what existed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TraversalStop {
    /// Start nodes dropped by `max_nodes` before the walk began.
    pub starts_dropped: usize,
    /// A node with unexpanded neighbours sat at `max_depth`.
    pub depth_capped: bool,
    /// `max_nodes` stopped an expansion or an enqueue.
    pub node_capped: bool,
    /// Edges the walk crossed but did not record, capped by `max_nodes`.
    pub edges_unrecorded: usize,
}

impl TraversalStop {
    /// True when the walk withheld something it would otherwise have reported.
    pub fn is_incomplete(&self) -> bool {
        self.starts_dropped > 0
            || self.depth_capped
            || self.node_capped
            || self.edges_unrecorded > 0
    }

    /// A caller-facing sentence, or `None` when the walk was complete.
    pub fn reason(&self, max_depth: usize, max_nodes: usize) -> Option<String> {
        if !self.is_incomplete() {
            return None;
        }
        let mut parts = Vec::new();
        if self.depth_capped {
            parts.push(format!("stopped at depth {max_depth}"));
        }
        if self.node_capped {
            parts.push(format!("stopped at {max_nodes} nodes"));
        }
        if self.starts_dropped > 0 {
            parts.push(format!("{} start nodes dropped", self.starts_dropped));
        }
        if self.edges_unrecorded > 0 {
            parts.push(format!(
                "{} traversed edges unrecorded",
                self.edges_unrecorded
            ));
        }
        Some(format!(
            "the walk did not complete: {}; the result is a lower bound, not the \
             full blast radius",
            parts.join(", ")
        ))
    }
}

#[derive(Debug, Clone)]
pub struct TraversalResult {
    pub visited_nodes: BTreeSet<String>,
    pub traversed_edges: Vec<EdgeIdentity>,
    pub max_depth_reached: usize,
    /// Why the walk ended, if it ended early. See [`TraversalStop`].
    pub stop: TraversalStop,
}

/// Bytes of the input graph this walk is allowed to copy.
///
/// Zero, and that is the point. The adjacency index borrows every edge instead
/// of cloning it, so the only owned strings a walk allocates are for the nodes
/// and edges it actually **reports** — a set bounded by `max_nodes`. Before
/// this, `adj` held `Vec<ResolvedEdge>` and every edge was deep-cloned (four
/// `String`s each) before `max_nodes` was consulted at all: a 1,000-node
/// question over DevCouncil's ~944,000-edge graph copied the whole graph first.
///
/// One thing this does **not** fix: the index is still built once per call, so
/// a single walk is still O(edges) in time no matter how small its caps. That
/// is inherent to being handed an unindexed slice — you cannot know what is
/// adjacent to a node without looking at every edge — and removing it means
/// keeping an index across queries, which is a decision for the caller that
/// owns the query loop, not for this function.
pub fn traverse_graph(
    start_nodes: &[String],
    edges: &[ResolvedEdge],
    opts: &TraversalOptions,
) -> TraversalResult {
    // Borrowed keys and borrowed edges. `edges` and `start_nodes` outlive the
    // walk, so nothing here needs to own a copy of a name it did not create.
    let mut adj: BTreeMap<&str, Vec<&ResolvedEdge>> = BTreeMap::new();
    for edge in edges {
        let key: &str = if opts.reverse {
            &edge.target_symbol
        } else {
            &edge.source_symbol
        };
        adj.entry(key).or_default().push(edge);
    }

    let mut visited: BTreeSet<&str> = BTreeSet::new();
    let mut enqueued: BTreeSet<&str> = BTreeSet::new(); // G21: Separate enqueued tracking
    let mut queue: VecDeque<(&str, usize)> = VecDeque::new();
    let mut traversed_edges = Vec::new();
    let mut max_depth_reached = 0;
    let mut stop = TraversalStop {
        starts_dropped: start_nodes.len().saturating_sub(opts.max_nodes),
        ..TraversalStop::default()
    };

    for start in start_nodes.iter().take(opts.max_nodes) {
        if enqueued.insert(start.as_str()) {
            queue.push_back((start.as_str(), 0));
        }
    }

    while let Some((curr, depth)) = queue.pop_front() {
        if !visited.insert(curr) {
            continue;
        }
        max_depth_reached = max_depth_reached.max(depth);
        if depth >= opts.max_depth || visited.len() >= opts.max_nodes {
            // Only a prune that actually cost the walk an expansion is a
            // decline. A node with no outgoing edges is fully explored, and
            // counting it would make every bounded walk call itself partial.
            if adj.contains_key(curr) {
                if depth >= opts.max_depth {
                    stop.depth_capped = true;
                }
                if visited.len() >= opts.max_nodes {
                    stop.node_capped = true;
                }
            }
            continue;
        }

        if let Some(neighbors) = adj.get(curr) {
            // Priority ordering: contains -> calls -> rest.
            //
            // Cloned because the sort must not disturb the shared index, but
            // this is now a vector of pointers rather than of edges — a node
            // with 100,000 inbound edges copies 800 KB of pointers, not 20 MB
            // of re-allocated strings.
            let mut sorted_neighbors = neighbors.clone();
            sorted_neighbors.sort_by(|a, b| {
                let priority = |edge: &ResolvedEdge| match edge.edge_kind {
                    devmap_extract::model::EdgeKind::Contains
                    | devmap_extract::model::EdgeKind::Defines => 0,
                    devmap_extract::model::EdgeKind::Calls => 1,
                    _ => 2,
                };
                priority(a)
                    .cmp(&priority(b))
                    .then_with(|| a.source_symbol.cmp(&b.source_symbol))
                    .then_with(|| a.target_symbol.cmp(&b.target_symbol))
                    .then_with(|| a.source_file.cmp(&b.source_file))
                    .then_with(|| a.target_file.cmp(&b.target_file))
            });

            for edge in sorted_neighbors {
                // Impact must not walk *upward* through containment. A symbol
                // is contained by its file, so following that edge in reverse
                // reaches the file and from there every sibling symbol in it —
                // turning "what depends on this" into "everything nearby".
                // Both structural kinds are excluded: `Contains` is the kind
                // actually emitted today, `Defines` is kept so a future
                // producer of it cannot silently reopen this hole.
                if opts.reverse
                    && matches!(
                        edge.edge_kind,
                        devmap_extract::model::EdgeKind::Contains
                            | devmap_extract::model::EdgeKind::Defines
                    )
                {
                    continue;
                }
                // File-level topology (package imports, Go package stars) is
                // impact for a *file* query. Following it from a symbol node
                // turns `impact Type.method` into "every importer of this
                // package" — the ScholarLM `segment` flood.
                if opts.reverse
                    && is_symbol_node(curr)
                    && matches!(
                        edge.edge_kind,
                        devmap_extract::model::EdgeKind::Imports
                            | devmap_extract::model::EdgeKind::MemberOf
                    )
                {
                    continue;
                }
                let next_node: &str = if opts.reverse {
                    &edge.source_symbol
                } else {
                    &edge.target_symbol
                };

                if !enqueued.contains(next_node) && enqueued.len() >= opts.max_nodes {
                    stop.node_capped = true;
                    continue;
                }

                if traversed_edges.len() < opts.max_nodes.saturating_sub(1) {
                    // The only place the walk allocates from edge text, and it
                    // is bounded by `max_nodes` rather than by the graph.
                    traversed_edges.push(EdgeIdentity {
                        source: edge.source_symbol.clone(),
                        target: edge.target_symbol.clone(),
                        edge_kind: format!("{:?}", edge.edge_kind),
                        line: 0,
                        col: 0,
                    });
                } else {
                    // Walked, but never reported: without this counter the edge
                    // is simply absent from `total`.
                    stop.edges_unrecorded += 1;
                }

                if enqueued.insert(next_node) {
                    queue.push_back((next_node, depth + 1));
                }
            }
        }
    }

    TraversalResult {
        stop,
        visited_nodes: visited.into_iter().map(str::to_string).collect(),
        traversed_edges,
        max_depth_reached,
    }
}

fn is_symbol_node(id: &str) -> bool {
    id.contains("::")
        || (id.contains('.')
            && !id.contains('/')
            && !id.contains('\\')
            && !matches!(
                id.rsplit('.').next().unwrap_or(""),
                "go" | "py"
                    | "rs"
                    | "ts"
                    | "tsx"
                    | "js"
                    | "jsx"
                    | "c"
                    | "h"
                    | "cc"
                    | "cpp"
                    | "cs"
                    | "java"
                    | "kt"
                    | "swift"
                    | "rb"
                    | "php"
                    | "vue"
                    | "svelte"
            ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use devmap_extract::model::{Confidence, EdgeKind};

    /// `is_symbol_node` decides whether a node id names a symbol or a file, and
    /// every clause of it matters.
    ///
    /// Mutation testing replaced the whole function with `true` and with
    /// `false`, flipped each `&&`, and deleted each `!` — none of it noticed.
    /// This is the guard that stops `impact Type.method` from following
    /// file-level topology upward and returning every importer of the package:
    /// the documented `segment` flood. Always-true suppresses legitimate
    /// file-level impact; always-false reopens the flood.
    #[test]
    fn symbol_nodes_are_distinguished_from_file_nodes() {
        // Qualified identities are symbols.
        assert!(is_symbol_node("pkg/svc.go::Server.handle"));
        assert!(is_symbol_node("app.py::helper"));

        // `Type.method` with no path separator is a symbol.
        assert!(is_symbol_node("Server.handle"));

        // File paths are not symbols, however they are spelled.
        assert!(!is_symbol_node("pkg/svc.go"), "a path is not a symbol");
        assert!(!is_symbol_node("svc.go"), "a bare filename is not a symbol");
        assert!(!is_symbol_node("app.py"));
        assert!(!is_symbol_node("web\\app.tsx"), "Windows separators too");

        // A bare identifier is neither qualified nor dotted.
        assert!(!is_symbol_node("plain"));
    }

    fn edge(source: &str, target: &str, edge_kind: EdgeKind) -> ResolvedEdge {
        ResolvedEdge {
            source_file: "a.go".to_string(),
            target_file: "b.go".to_string(),
            source_symbol: source.to_string(),
            target_symbol: target.to_string(),
            edge_kind,
            confidence: Confidence::DETERMINISTIC,
            resolution: None,
            details: None,
        }
    }

    /// Reverse traversal from a *symbol* must not climb file-level topology.
    ///
    /// Following `Imports`/`MemberOf` upward from a symbol turns
    /// `impact Type.method` into "every importer of this package". The guard
    /// combines `reverse && is_symbol_node && matches!(kind)`; flipping its
    /// `&&` to `||` was undetected, and would suppress forward traversal or
    /// file-level impact entirely.
    #[test]
    fn reverse_impact_from_a_symbol_does_not_follow_package_topology() {
        let edges = vec![
            // A real caller: must be followed.
            edge("a.go::Caller", "b.go::Target.run", EdgeKind::Calls),
            // Package-level topology hanging off the same node: must not be.
            edge("importer.go", "b.go::Target.run", EdgeKind::Imports),
            edge("member.go", "b.go::Target.run", EdgeKind::MemberOf),
        ];
        let opts = TraversalOptions {
            max_depth: 5,
            max_nodes: 100,
            reverse: true,
        };
        let walk = traverse_graph(&["b.go::Target.run".to_string()], &edges, &opts);

        assert!(
            walk.visited_nodes.contains("a.go::Caller"),
            "a real caller must be reached: {:?}",
            walk.visited_nodes
        );
        assert!(
            !walk.visited_nodes.contains("importer.go"),
            "impact on a symbol must not climb to package importers: {:?}",
            walk.visited_nodes
        );
        assert!(
            !walk.visited_nodes.contains("member.go"),
            "impact on a symbol must not climb MemberOf topology: {:?}",
            walk.visited_nodes
        );

        // The same topology IS impact for a *file* query, so the guard must be
        // scoped to symbol starts rather than suppressing the edge kind wholesale.
        let file_walk = traverse_graph(
            &["b.go".to_string()],
            &[edge("importer.go", "b.go", EdgeKind::Imports)],
            &opts,
        );
        assert!(
            file_walk.visited_nodes.contains("importer.go"),
            "file-level impact must still follow imports: {:?}",
            file_walk.visited_nodes
        );
    }

    /// Neighbours are visited in priority order: containment, then calls, then
    /// everything else.
    ///
    /// Deleting either priority arm survived, because nothing observed the
    /// order. It is observable under a cap: with room for only a few recorded
    /// edges, the ones kept must be the high-priority ones, so a caller reading
    /// a truncated impact result still sees structure before incidental
    /// references.
    #[test]
    fn neighbours_are_recorded_in_containment_then_call_priority() {
        // Target names are chosen so alphabetical order DISAGREES with priority
        // order. With names that happen to sort the same way, deleting a
        // priority arm changes nothing and the arm looks untested when it is
        // merely unobservable.
        let edges = vec![
            edge("f.go::start", "f.go::a_ref", EdgeKind::References),
            edge("f.go::start", "f.go::z_call", EdgeKind::Calls),
            edge("f.go::start", "f.go::m_contains", EdgeKind::Contains),
        ];
        let walk = traverse_graph(
            &["f.go::start".to_string()],
            &edges,
            &TraversalOptions {
                max_depth: 1,
                max_nodes: 100,
                reverse: false,
            },
        );
        let order: Vec<&str> = walk
            .traversed_edges
            .iter()
            .map(|e| e.target.as_str())
            .collect();
        assert_eq!(
            order,
            vec!["f.go::m_contains", "f.go::z_call", "f.go::a_ref"],
            "containment must precede calls, which must precede other kinds, \
             regardless of how the target names sort"
        );
    }

    /// The recorded-edge budget is exclusive and leaves room for the node cap.
    ///
    /// `traversed_edges.len() < max_nodes - 1` was mutable to `<=`, recording
    /// one edge more than the budget allows — the kind of off-by-one that only
    /// shows up when a result is already truncated.
    /// A walk that stopped at a cap must say so.
    ///
    /// Every decline in `traverse_graph` produced a `TraversalResult`
    /// indistinguishable from a complete walk, so `impact` reported a
    /// depth-limited blast radius as the whole answer. The default depth is 3
    /// and any real graph is deeper, so this fired on essentially every call.
    #[test]
    fn a_capped_walk_reports_that_it_stopped_early() {
        // a -> b -> c -> d, walked only two levels deep.
        let edges = vec![
            edge("a", "b", EdgeKind::Calls),
            edge("b", "c", EdgeKind::Calls),
            edge("c", "d", EdgeKind::Calls),
        ];
        let shallow = traverse_graph(
            &["a".to_string()],
            &edges,
            &TraversalOptions {
                max_depth: 2,
                max_nodes: 1000,
                reverse: false,
            },
        );
        assert!(
            shallow.stop.depth_capped,
            "the depth cap pruned a node with unexplored neighbours"
        );
        assert!(shallow.stop.is_incomplete());
        assert!(shallow
            .stop
            .reason(2, 1000)
            .expect("a capped walk has a reason")
            .contains("depth 2"));

        // Deep enough to finish: no claim of incompleteness.
        let full = traverse_graph(
            &["a".to_string()],
            &edges,
            &TraversalOptions {
                max_depth: 10,
                max_nodes: 1000,
                reverse: false,
            },
        );
        assert!(
            !full.stop.is_incomplete(),
            "a walk that ran out of graph is complete, not capped: {:?}",
            full.stop
        );
        assert_eq!(full.stop.reason(10, 1000), None);

        // The node cap is its own reason and is reported separately.
        let narrow = traverse_graph(
            &["a".to_string()],
            &edges,
            &TraversalOptions {
                max_depth: 10,
                max_nodes: 2,
                reverse: false,
            },
        );
        assert!(
            narrow.stop.node_capped,
            "the node cap stopped this walk: {:?}",
            narrow.stop
        );

        // Start nodes dropped before the walk begins are counted too.
        let starts: Vec<String> = (0..5).map(|i| format!("s{i}")).collect();
        let dropped = traverse_graph(
            &starts,
            &edges,
            &TraversalOptions {
                max_depth: 10,
                max_nodes: 2,
                reverse: false,
            },
        );
        assert_eq!(dropped.stop.starts_dropped, 3);
    }

    #[test]
    fn recorded_edges_stay_within_their_budget() {
        // Several edges land on the SAME two targets, so edges outnumber nodes
        // and the edge budget actually binds. With one edge per node the node
        // cap stops the walk first and the budget is never reached — which is
        // why an off-by-one here can hide.
        let mut edges = Vec::new();
        for kind in [EdgeKind::Calls, EdgeKind::References, EdgeKind::MemberOf] {
            edges.push(edge("f.go::start", "f.go::a", kind));
            edges.push(edge("f.go::start", "f.go::b", kind));
        }
        let walk = traverse_graph(
            &["f.go::start".to_string()],
            &edges,
            &TraversalOptions {
                max_depth: 3,
                max_nodes: 3,
                reverse: false,
            },
        );
        assert_eq!(
            walk.traversed_edges.len(),
            2,
            "with max_nodes=3 exactly 2 edges may be recorded, found {:?}",
            walk.traversed_edges
        );
    }

    /// Depth and node caps are enforced, and the bound is exclusive.
    #[test]
    fn traversal_respects_its_depth_and_node_caps() {
        let edges: Vec<_> = (0..10)
            .map(|i| {
                edge(
                    &format!("f.go::n{i}"),
                    &format!("f.go::n{}", i + 1),
                    EdgeKind::Calls,
                )
            })
            .collect();

        let shallow = traverse_graph(
            &["f.go::n0".to_string()],
            &edges,
            &TraversalOptions {
                max_depth: 2,
                max_nodes: 100,
                reverse: false,
            },
        );
        assert!(
            shallow.max_depth_reached <= 2,
            "depth cap exceeded: {}",
            shallow.max_depth_reached
        );
        assert!(
            !shallow.visited_nodes.contains("f.go::n5"),
            "a node beyond the depth cap was reached: {:?}",
            shallow.visited_nodes
        );

        let capped = traverse_graph(
            &["f.go::n0".to_string()],
            &edges,
            &TraversalOptions {
                max_depth: 100,
                max_nodes: 3,
                reverse: false,
            },
        );
        assert!(
            capped.visited_nodes.len() <= 3,
            "node cap exceeded: {:?}",
            capped.visited_nodes
        );
    }
}
