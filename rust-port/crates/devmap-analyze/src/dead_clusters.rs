//! Abandoned subsystems that keep themselves alive.
//!
//! Liveness is a one-hop inbound-edge join and never transitive. A subsystem
//! whose functions call each other therefore has an inbound edge on **every**
//! symbol, and the kernel reports zero of it — the classic dead-code case, and
//! the single largest recall hole in the analysis.
//!
//! This is the traversal that closes it: strongly connected components over the
//! resolved call graph, then the components nothing outside them reaches.
//!
//! Three properties the implementation is built around:
//!
//! * **One finding per cluster, not one per member.** A 40-symbol dead cluster
//!   reported 40 times blows past `DEAD_CANDIDATE_CAP` and pushes real
//!   single-symbol findings out of the ranked list. The cluster *is* the
//!   finding.
//! * **Iterative Tarjan.** The natural formulation is recursive, and a 10,000
//!   node cycle is expressible in real code — a generated state machine, a
//!   mutually recursive parser. Recursion there is a stack overflow, which is a
//!   crash rather than a wrong answer, and no `catch_unwind` exists on this
//!   path because the CLI builds with `panic = "abort"`.
//! * **Deterministic edges only.** An ambiguous edge is evidence that a symbol
//!   *may* be called; letting one keep a cluster alive would silently suppress
//!   findings on exactly the speculative ground the confidence ladder exists to
//!   keep out of verdicts.

use devmap_extract::model::{EdgeKind, Extraction};
use devmap_resolve::model::{Resolution, ResolutionResult, ResolvedEdge};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};

/// Most clusters reported. Shares the reasoning behind `DEAD_CANDIDATE_CAP`:
/// this is a debt list a human reads, and an unbounded one is not read at all.
pub const DEAD_CLUSTER_CAP: usize = 50;

/// Most members named per cluster.
///
/// The count is always exact — `size` is the real membership — and the *list*
/// is a sample. Carrying both is the difference between a capped sample and a
/// capped sample presented as complete.
pub const DEAD_CLUSTER_MEMBER_CAP: usize = 25;

/// Confidence ceiling for a cluster verdict.
///
/// Deliberately inside the `inferred` band (0.4–0.89) and never `extracted`. A
/// cluster verdict rests on the *whole graph* being complete in a way a
/// single-symbol verdict does not: one missed call edge anywhere into the
/// component makes the entire finding wrong, where the same missed edge costs a
/// single-symbol finding only itself. That is a strictly weaker claim and it
/// gets a strictly lower tier.
pub const DEAD_CLUSTER_CONFIDENCE: f32 = 0.5;

/// A group of symbols that reference only each other, reachable from nothing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeadClusterReport {
    pub cluster_id: u32,
    /// Graph ids of the members, sorted, capped at [`DEAD_CLUSTER_MEMBER_CAP`].
    pub members: Vec<String>,
    /// The real membership count, never the length of `members`.
    pub size: usize,
    pub confidence: f32,
    pub reason: String,
}

/// The scan, with what it had to leave out.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DeadClusterScan {
    pub clusters: Vec<DeadClusterReport>,
    /// Files whose every declared symbol sits in a dead cluster.
    ///
    /// This is what makes `unreachable_files` answerable. The key shipped
    /// hardcoded `[]` beside an unconditional
    /// `liveness_unreachable_unreliable: true`, so four Python consumers
    /// suppressed it permanently and it was a third state — neither a computed
    /// empty nor an honest absence.
    ///
    /// **Not a BFS from entry roots.** That is the computation the `unreliable`
    /// flag was warning about, and it is genuinely noisy for routers, dynamic
    /// imports and JSX. This is a different and much narrower claim: every
    /// symbol this file declares belongs to a component that nothing outside
    /// reaches, where "reaches" already excludes ambiguous edges and already
    /// exempts anything exported or wired. A file with one live symbol does not
    /// qualify.
    pub unreachable_files: Vec<String>,
    /// Clusters found but not reported, because of [`DEAD_CLUSTER_CAP`].
    pub truncated_clusters: usize,
    /// Whether the graph was too large to walk at all. See
    /// [`DEAD_CLUSTER_MAX_NODES`].
    pub refused_oversized_graph: bool,
}

/// Beyond this many distinct symbols in the call graph, the scan refuses.
///
/// Tarjan is linear, so this is not about asymptotics — it is about the memory
/// three index vectors over every node cost on a graph this kernel has measured
/// at 944,000 edges. Refusing loudly beats a walk that succeeds by consuming
/// the machine, and `refused_oversized_graph` says which happened rather than
/// leaving an empty result to read as "no dead clusters".
pub const DEAD_CLUSTER_MAX_NODES: usize = 400_000;

/// Whether this edge is evidence that its target is reached.
///
/// Structural edges say where a symbol lives, not that anything uses it —
/// `Contains` alone would put every symbol in a file into one component with
/// the file. Ambiguous and unresolved edges are excluded because a cluster kept
/// alive by a guess is a finding silently suppressed.
fn is_reaching_edge(edge: &ResolvedEdge) -> bool {
    if matches!(
        edge.edge_kind,
        EdgeKind::Contains | EdgeKind::Defines | EdgeKind::MemberOf
    ) {
        return false;
    }
    !matches!(
        edge.resolution.as_deref(),
        Some(Resolution::AmbiguousGlobal { .. }) | Some(Resolution::Unresolved { .. })
    )
}

/// Strongly connected components, iteratively.
///
/// Tarjan's algorithm with the recursion made explicit. The `call_stack` holds
/// `(node, next child index)` so a resumed frame continues where it left off,
/// which is what the recursive form gets from the language.
///
/// Returns components in reverse topological order, as Tarjan does; callers
/// here do not depend on that, but changing it silently would be a trap.
fn strongly_connected_components(adjacency: &[Vec<u32>]) -> Vec<Vec<u32>> {
    let n = adjacency.len();
    const UNVISITED: u32 = u32::MAX;

    let mut index = vec![UNVISITED; n];
    let mut lowlink = vec![0u32; n];
    let mut on_stack = vec![false; n];
    let mut stack: Vec<u32> = Vec::new();
    let mut components: Vec<Vec<u32>> = Vec::new();
    let mut next_index: u32 = 0;

    for root in 0..n {
        if index[root] != UNVISITED {
            continue;
        }
        // `(node, position in that node's adjacency list)`.
        let mut call_stack: Vec<(u32, usize)> = vec![(root as u32, 0)];
        index[root] = next_index;
        lowlink[root] = next_index;
        next_index += 1;
        stack.push(root as u32);
        on_stack[root] = true;

        while let Some(&mut (node, ref mut child_position)) = call_stack.last_mut() {
            let neighbours = &adjacency[node as usize];
            if *child_position < neighbours.len() {
                let child = neighbours[*child_position];
                *child_position += 1;
                if index[child as usize] == UNVISITED {
                    index[child as usize] = next_index;
                    lowlink[child as usize] = next_index;
                    next_index += 1;
                    stack.push(child);
                    on_stack[child as usize] = true;
                    call_stack.push((child, 0));
                } else if on_stack[child as usize] {
                    lowlink[node as usize] = lowlink[node as usize].min(index[child as usize]);
                }
                continue;
            }

            // This node's children are exhausted: close it out.
            call_stack.pop();
            if let Some(&(parent, _)) = call_stack.last() {
                lowlink[parent as usize] = lowlink[parent as usize].min(lowlink[node as usize]);
            }
            if lowlink[node as usize] == index[node as usize] {
                let mut component = Vec::new();
                while let Some(member) = stack.pop() {
                    on_stack[member as usize] = false;
                    component.push(member);
                    if member == node {
                        break;
                    }
                }
                components.push(component);
            }
        }
    }
    components
}

/// Symbols something outside the call graph is known to reach.
///
/// Seeded from the machinery that already answers this for single symbols, so a
/// cluster containing an exported symbol, a route handler or a framework entry
/// point is live for the same reason and by the same rule. Without it, a
/// perfectly ordinary set of mutually recursive exported functions is a
/// "cluster nothing reaches".
fn externally_reachable_symbols(extractions: &[Extraction]) -> BTreeSet<String> {
    let mut reachable = BTreeSet::new();
    for ext in extractions {
        for symbol in &ext.symbols {
            if symbol.is_exported {
                reachable.insert(symbol.qualified_name.clone());
            }
        }
        // Every wiring annotation is evidence that something outside the
        // resolvable call graph reaches its target — that is the type's whole
        // purpose. A file-scoped annotation names the file, which no symbol id
        // equals, so it is expanded to the file's symbols here.
        for annotation in &ext.wiring {
            if annotation.target_symbol == ext.file_path {
                for symbol in &ext.symbols {
                    reachable.insert(symbol.qualified_name.clone());
                }
            } else {
                reachable.insert(annotation.target_symbol.clone());
            }
        }
    }
    reachable
}

/// Find components of the resolved call graph that nothing outside reaches.
pub fn dead_clusters(extractions: &[Extraction], resolution: &ResolutionResult) -> DeadClusterScan {
    // Node ids, assigned in first-seen order over a deterministically ordered
    // edge list. `resolution.edges` is sorted by the resolver (R4), so the ids
    // — and therefore the cluster ids below — are stable across runs.
    let mut id_of: HashMap<&str, u32> = HashMap::new();
    let mut names: Vec<&str> = Vec::new();
    let mut reaching: Vec<(u32, u32)> = Vec::new();

    for edge in &resolution.edges {
        if !is_reaching_edge(edge) {
            continue;
        }
        let source: &str = &edge.source_symbol;
        let target: &str = &edge.target_symbol;
        let source_id = match id_of.get(source) {
            Some(existing) => *existing,
            None => {
                if names.len() >= DEAD_CLUSTER_MAX_NODES {
                    return DeadClusterScan {
                        refused_oversized_graph: true,
                        ..Default::default()
                    };
                }
                let id = names.len() as u32;
                names.push(source);
                id_of.insert(source, id);
                id
            }
        };
        let target_id = match id_of.get(target) {
            Some(existing) => *existing,
            None => {
                if names.len() >= DEAD_CLUSTER_MAX_NODES {
                    return DeadClusterScan {
                        refused_oversized_graph: true,
                        ..Default::default()
                    };
                }
                let id = names.len() as u32;
                names.push(target);
                id_of.insert(target, id);
                id
            }
        };
        reaching.push((source_id, target_id));
    }

    if names.is_empty() {
        return DeadClusterScan::default();
    }

    let mut adjacency: Vec<Vec<u32>> = vec![Vec::new(); names.len()];
    for (source, target) in &reaching {
        adjacency[*source as usize].push(*target);
    }

    let components = strongly_connected_components(&adjacency);

    // Which component each node landed in, so an edge can be classified as
    // internal or incoming in constant time.
    let mut component_of: Vec<u32> = vec![u32::MAX; names.len()];
    for (component_id, members) in components.iter().enumerate() {
        for member in members {
            component_of[*member as usize] = component_id as u32;
        }
    }

    // A component is reached from outside if any edge crosses into it.
    let mut has_external_inbound = vec![false; components.len()];
    for (source, target) in &reaching {
        let from = component_of[*source as usize];
        let into = component_of[*target as usize];
        if from != into {
            has_external_inbound[into as usize] = true;
        }
    }

    let externally_reachable = externally_reachable_symbols(extractions);

    let mut clustered_symbols: BTreeSet<String> = BTreeSet::new();
    let mut found: Vec<DeadClusterReport> = Vec::new();
    for (component_id, members) in components.iter().enumerate() {
        // A single node is a cluster only if it calls itself; otherwise it is
        // an ordinary symbol, and the single-symbol pass already owns it. This
        // pass exists for the case that pass structurally cannot see.
        let is_cycle = members.len() > 1
            || members
                .first()
                .is_some_and(|node| adjacency[*node as usize].contains(node));
        if !is_cycle || has_external_inbound[component_id] {
            continue;
        }

        let mut member_names: Vec<String> = members
            .iter()
            .map(|id| names[*id as usize].to_string())
            .collect();
        member_names.sort();

        // One externally reachable member makes the whole component live: an
        // exported symbol can be called from outside the corpus, and once it is
        // reached, everything it recurses with is reached too.
        if member_names
            .iter()
            .any(|name| externally_reachable.contains(name))
        {
            continue;
        }

        clustered_symbols.extend(member_names.iter().cloned());
        let size = member_names.len();
        let shown = size.min(DEAD_CLUSTER_MEMBER_CAP);
        found.push(DeadClusterReport {
            cluster_id: component_id as u32,
            members: member_names.into_iter().take(shown).collect(),
            size,
            confidence: DEAD_CLUSTER_CONFIDENCE,
            reason: cluster_reason(size, shown),
        });
    }

    // Largest first: a 40-symbol abandoned subsystem is worth more of a
    // reader's attention than a two-function recursion, and the cap below has
    // to keep the ones that matter.
    found.sort_by(|left, right| {
        right
            .size
            .cmp(&left.size)
            .then_with(|| left.cluster_id.cmp(&right.cluster_id))
    });
    let truncated_clusters = found.len().saturating_sub(DEAD_CLUSTER_CAP);
    found.truncate(DEAD_CLUSTER_CAP);

    // Derived after truncation deliberately: the *file* claim rests on the
    // clusters that were found, not on the ones that fit in the report. Basing
    // it on the truncated list would make a file's reachability depend on how
    // many other clusters happened to exist.
    let unreachable_files = files_wholly_inside_clusters(extractions, &clustered_symbols);

    DeadClusterScan {
        clusters: found,
        unreachable_files,
        truncated_clusters,
        refused_oversized_graph: false,
    }
}

/// Files every one of whose declared symbols is in a dead cluster.
///
/// The `File` symbol itself is excluded from the test: it is the node the file
/// *is*, not something the file declares, and it never joins a call cycle. A
/// file that declares nothing else is skipped entirely rather than counted
/// unreachable, because "declares nothing" is not evidence of anything.
fn files_wholly_inside_clusters(
    extractions: &[Extraction],
    clustered: &BTreeSet<String>,
) -> Vec<String> {
    if clustered.is_empty() {
        return Vec::new();
    }
    let mut unreachable: Vec<String> = extractions
        .iter()
        .filter(|ext| {
            let mut declared = ext
                .symbols
                .iter()
                .filter(|symbol| symbol.kind != devmap_extract::model::SymbolKind::File)
                .peekable();
            if declared.peek().is_none() {
                return false;
            }
            declared.all(|symbol| clustered.contains(&symbol.qualified_name))
        })
        .map(|ext| ext.file_path.clone())
        .collect();
    unreachable.sort();
    unreachable.dedup();
    unreachable
}

fn cluster_reason(size: usize, shown: usize) -> String {
    let mut reason = format!(
        "{size} symbols that reference only each other, reached by nothing outside the \
         component — an abandoned cycle is invisible to the one-hop liveness join, because \
         every member has an inbound edge from another member"
    );
    if shown < size {
        reason.push_str(&format!(
            "; {shown} of {size} members listed",
            shown = shown,
            size = size
        ));
    }
    reason
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The algorithm, on graphs small enough to reason about by hand.
    #[test]
    fn tarjan_finds_the_components() {
        // 0 -> 1 -> 2 -> 0  (one component), 3 -> 4 (two singletons)
        let adjacency = vec![vec![1], vec![2], vec![0], vec![4], vec![]];
        let mut sizes: Vec<usize> = strongly_connected_components(&adjacency)
            .into_iter()
            .map(|c| c.len())
            .collect();
        sizes.sort_unstable();
        assert_eq!(sizes, vec![1, 1, 3]);
    }

    /// A graph with no edges has one component per node, and no cycles.
    #[test]
    fn tarjan_on_an_edgeless_graph_finds_only_singletons() {
        let adjacency = vec![vec![], vec![], vec![]];
        let components = strongly_connected_components(&adjacency);
        assert_eq!(components.len(), 3);
        assert!(components.iter().all(|c| c.len() == 1));
    }

    /// A self-loop is a component of one, and the walk must terminate.
    #[test]
    fn tarjan_handles_a_self_loop() {
        let adjacency = vec![vec![0]];
        assert_eq!(strongly_connected_components(&adjacency), vec![vec![0]]);
    }

    /// The adversarial case, and the reason this is iterative.
    ///
    /// A 100,000-node cycle in a recursive Tarjan is 100,000 stack frames. The
    /// CLI builds with `panic = "abort"` and has no `catch_unwind` anywhere, so
    /// the failure would be a process death on input a user can write — a
    /// generated state machine or a mutually recursive parser reaches this
    /// shape. Ten times the size the plan asked for, to leave headroom.
    #[test]
    fn a_hundred_thousand_node_cycle_does_not_overflow_the_stack() {
        let n = 100_000usize;
        let adjacency: Vec<Vec<u32>> = (0..n).map(|i| vec![((i + 1) % n) as u32]).collect();
        let components = strongly_connected_components(&adjacency);
        assert_eq!(components.len(), 1);
        assert_eq!(components[0].len(), n);
    }

    /// A deep *chain* is the other stack shape: no cycle, maximum depth.
    #[test]
    fn a_hundred_thousand_node_chain_does_not_overflow_the_stack() {
        let n = 100_000usize;
        let adjacency: Vec<Vec<u32>> = (0..n)
            .map(|i| {
                if i + 1 < n {
                    vec![(i + 1) as u32]
                } else {
                    vec![]
                }
            })
            .collect();
        let components = strongly_connected_components(&adjacency);
        assert_eq!(components.len(), n, "a chain is n singletons");
    }
}
