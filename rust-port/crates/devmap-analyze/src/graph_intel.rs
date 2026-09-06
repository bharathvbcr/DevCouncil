//! Graph intelligence: the hubs a repository leans on, and its import cycles.
//!
//! This is the port of `src/devcouncil/indexing/graph/intel.py`'s
//! `god_nodes` and `circular_imports`, which reached `code_graph.json`'s
//! `meta` through `enrich_graph_intel`. That function lost both of its call
//! sites in `d232dea` when the Python graph builder was retired, and nothing
//! has called it since — so the two panels `viz.py:520-522` renders from those
//! keys have shown `(none)` on every graph written after the cutover,
//! regardless of what the repository contains.
//!
//! Recomputing them here rather than reviving the Python is the same decision
//! `d232dea` made: the kernel already holds the edges, and a second producer
//! of an artifact the kernel owns is exactly what that commit removed.
//!
//! **`hotspots` is not here.** It is churn × coupling — `git log
//! --since=90.days --name-only` scored against fan-in — and the churn half
//! needs repository history this crate does not read. Fabricating a
//! coupling-only score under the same name would be a different metric wearing
//! the old one's label.

use std::collections::{BTreeMap, BTreeSet};

use devmap_extract::model::EdgeKind;
use devmap_resolve::model::ResolvedEdge;

/// Most ranked hubs to emit. The Python writer used 15 and `viz.py` slices to
/// 30; the smaller of the two is the one that ever bound the output.
pub const GOD_NODE_CAP: usize = 15;

/// Most import cycles to emit. `circular_imports` used 50 and `viz.py` slices
/// to 30, so 30 is what a reader could ever see.
pub const IMPORT_CYCLE_CAP: usize = 30;

/// One heavily-connected node, in the shape `viz.py:967` indexes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct GodNode {
    pub id: String,
    pub path: String,
    pub name: String,
    pub degree: u32,
    pub fan_in: u32,
    pub fan_out: u32,
}

/// One strongly connected component of the file import graph.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ImportCycle {
    pub nodes: Vec<String>,
    pub length: usize,
}

/// The whole report, each list with the bounds of its own cap.
#[derive(Debug, Clone, Default)]
pub struct GraphIntel {
    pub god_nodes: Vec<GodNode>,
    /// Ranked candidates before the cap. Carried so a reader never mistakes
    /// the sample for the population.
    pub god_nodes_total: usize,
    pub circular_imports: Vec<ImportCycle>,
    pub circular_imports_total: usize,
}

impl GraphIntel {
    pub fn god_nodes_truncated(&self) -> bool {
        self.god_nodes_total > self.god_nodes.len()
    }

    pub fn circular_imports_truncated(&self) -> bool {
        self.circular_imports_total > self.circular_imports.len()
    }
}

/// The `inferred` floor, matching `code_graph.rs`'s `INFERRED_FLOOR_MILLIS`.
///
/// Compared in milliconfidence rather than as a float, through the same
/// `confidence_millis` the graph writer uses, because an edge persisted at 0.4
/// reads back from SQLite as something a `>= 0.4` float comparison can reject.
/// Two surfaces disagreeing about which edges count is precisely what a shared
/// rounding helper exists to prevent.
const METRIC_FLOOR_MILLIS: i64 = 400;

/// Whether an edge should influence the ranking.
///
/// `Imports` and `Calls` only, and only at or above the `inferred` tier — the
/// same `{extracted, inferred}` set the Python original used. Its reason is
/// worth keeping: one ambiguous call fans out to every candidate, so admitting
/// those inflates exactly the hubs this ranking exists to find. An edge the
/// resolver declined to resolve is not evidence that a symbol is central.
fn is_metric_edge(edge: &ResolvedEdge) -> bool {
    matches!(edge.edge_kind, EdgeKind::Imports | EdgeKind::Calls)
        && devmap_extract::model::confidence_millis(edge.confidence.0) >= METRIC_FLOOR_MILLIS
}

/// A node's file, which is everything before the `::` in its id.
fn file_of(node_id: &str) -> &str {
    match node_id.find("::") {
        Some(at) => &node_id[..at],
        None => node_id,
    }
}

/// A node's own name, which is everything after the last `::`.
fn name_of(node_id: &str) -> &str {
    match node_id.rfind("::") {
        Some(at) => &node_id[at + 2..],
        None => node_id,
    }
}

/// Python package barrels. `__init__.py` files import their siblings and are
/// imported by them, so nearly every package is a cycle through one — true,
/// and not actionable. The Python original excluded them for the same reason
/// and counted them separately.
fn is_package_init(path: &str) -> bool {
    path.replace('\\', "/").rsplit('/').next() == Some("__init__.py")
}

/// Rank the graph's hubs and find its import cycles.
pub fn graph_intel(edges: &[ResolvedEdge]) -> GraphIntel {
    GraphIntel {
        god_nodes: Vec::new(),
        god_nodes_total: 0,
        circular_imports: Vec::new(),
        circular_imports_total: 0,
    }
    .with_god_nodes(edges)
    .with_import_cycles(edges)
}

impl GraphIntel {
    fn with_god_nodes(mut self, edges: &[ResolvedEdge]) -> Self {
        // BTreeMap, not HashMap: the ranking is emitted into an artifact whose
        // bytes must be identical for two renderings of one generation (R4),
        // and a degree tie broken by hash order is exactly how that fails.
        let mut degree: BTreeMap<&str, u32> = BTreeMap::new();
        let mut fan_in: BTreeMap<&str, u32> = BTreeMap::new();
        let mut fan_out: BTreeMap<&str, u32> = BTreeMap::new();
        for edge in edges {
            if !is_metric_edge(edge) {
                continue;
            }
            let (source, target) = (edge.source_symbol.as_str(), edge.target_symbol.as_str());
            *degree.entry(source).or_insert(0) += 1;
            *degree.entry(target).or_insert(0) += 1;
            *fan_out.entry(source).or_insert(0) += 1;
            *fan_in.entry(target).or_insert(0) += 1;
        }
        // Test files are excluded, not merely down-weighted. A shared fixture
        // or mock accumulates enormous fan-in and would crowd out every real
        // hub, which is the failure the Python original names in its docstring.
        let mut ranked: Vec<(&str, u32)> = degree
            .into_iter()
            .filter(|(id, _)| !devmap_extract::wiring::is_test_path(file_of(id)))
            .collect();
        // By degree descending, then by id ascending. The second key is not
        // cosmetic: without it a tie is resolved by whatever order the map
        // yielded, and the artifact's bytes change under a reader with nothing
        // about the repository having changed.
        ranked.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(right.0)));

        self.god_nodes_total = ranked.len();
        self.god_nodes = ranked
            .into_iter()
            .take(GOD_NODE_CAP)
            .map(|(id, deg)| GodNode {
                id: id.to_string(),
                path: file_of(id).to_string(),
                name: name_of(id).to_string(),
                degree: deg,
                fan_in: fan_in.get(id).copied().unwrap_or(0),
                fan_out: fan_out.get(id).copied().unwrap_or(0),
            })
            .collect();
        self
    }

    fn with_import_cycles(mut self, edges: &[ResolvedEdge]) -> Self {
        // File-level import edges only. A symbol-level edge cannot make a file
        // cycle, and a self-import is not one either.
        let mut index_of: BTreeMap<&str, u32> = BTreeMap::new();
        let mut names: Vec<&str> = Vec::new();
        let mut pairs: BTreeSet<(u32, u32)> = BTreeSet::new();
        for edge in edges {
            if edge.edge_kind != EdgeKind::Imports {
                continue;
            }
            let (source, target) = (edge.source_file.as_str(), edge.target_file.as_str());
            if source == target || is_package_init(source) || is_package_init(target) {
                continue;
            }
            let source_index = match index_of.get(source) {
                Some(existing) => *existing,
                None => {
                    let next = names.len() as u32;
                    names.push(source);
                    index_of.insert(source, next);
                    next
                }
            };
            let target_index = match index_of.get(target) {
                Some(existing) => *existing,
                None => {
                    let next = names.len() as u32;
                    names.push(target);
                    index_of.insert(target, next);
                    next
                }
            };
            pairs.insert((source_index, target_index));
        }
        let mut adjacency: Vec<Vec<u32>> = vec![Vec::new(); names.len()];
        for (source, target) in pairs {
            adjacency[source as usize].push(target);
        }

        let mut components: Vec<Vec<String>> = strongly_connected_components(&adjacency)
            .into_iter()
            // A component of one is a file, not a cycle.
            .filter(|component| component.len() >= 2)
            .map(|component| {
                let mut members: Vec<String> = component
                    .into_iter()
                    .map(|index| names[index as usize].to_string())
                    .collect();
                members.sort();
                members
            })
            .collect();
        // Smallest first: a two-file cycle is the one somebody can actually
        // break, and a fifty-file component is a fact about the architecture.
        // Tie-broken by contents so the order is not the traversal's.
        components
            .sort_by(|left, right| left.len().cmp(&right.len()).then_with(|| left.cmp(right)));

        self.circular_imports_total = components.len();
        self.circular_imports = components
            .into_iter()
            .take(IMPORT_CYCLE_CAP)
            .map(|nodes| ImportCycle {
                length: nodes.len(),
                nodes,
            })
            .collect();
        self
    }
}

/// Tarjan's strongly connected components, iterative.
///
/// Iterative rather than recursive because the input is a whole repository's
/// import graph and a recursive walk is one deep chain away from overflowing
/// the stack.
///
/// **This duplicates `dead_clusters::strongly_connected_components`**, which is
/// the same algorithm over the same shape of adjacency list and is private to
/// that module. The correct end state is one implementation: make that one
/// `pub(crate)` and delete this. It is written out here only because
/// `dead_clusters.rs` belongs to another lane in this pass and may not be
/// edited — the visibility change is a one-word diff and is reported as the
/// follow-up.
fn strongly_connected_components(adjacency: &[Vec<u32>]) -> Vec<Vec<u32>> {
    let count = adjacency.len();
    let mut index_of: Vec<u32> = vec![u32::MAX; count];
    let mut low_link: Vec<u32> = vec![0; count];
    let mut on_stack: Vec<bool> = vec![false; count];
    let mut stack: Vec<u32> = Vec::new();
    let mut components: Vec<Vec<u32>> = Vec::new();
    let mut next_index: u32 = 0;

    for root in 0..count {
        if index_of[root] != u32::MAX {
            continue;
        }
        // (node, how many of its successors have been visited)
        let mut call_stack: Vec<(u32, usize)> = vec![(root as u32, 0)];
        index_of[root] = next_index;
        low_link[root] = next_index;
        next_index += 1;
        stack.push(root as u32);
        on_stack[root] = true;

        while let Some((node, cursor)) = call_stack.pop() {
            let successors = &adjacency[node as usize];
            if cursor < successors.len() {
                let successor = successors[cursor];
                call_stack.push((node, cursor + 1));
                if index_of[successor as usize] == u32::MAX {
                    index_of[successor as usize] = next_index;
                    low_link[successor as usize] = next_index;
                    next_index += 1;
                    stack.push(successor);
                    on_stack[successor as usize] = true;
                    call_stack.push((successor, 0));
                } else if on_stack[successor as usize] {
                    low_link[node as usize] =
                        low_link[node as usize].min(index_of[successor as usize]);
                }
                continue;
            }
            // Every successor is done: fold this node's low-link into its
            // parent's, then close the component if this node roots one.
            if let Some((parent, _)) = call_stack.last() {
                low_link[*parent as usize] =
                    low_link[*parent as usize].min(low_link[node as usize]);
            }
            if low_link[node as usize] == index_of[node as usize] {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_two_node_cycle_is_one_component() {
        let components = strongly_connected_components(&[vec![1], vec![0]]);
        assert_eq!(components.len(), 1);
        assert_eq!(components[0].len(), 2);
    }

    #[test]
    fn a_chain_is_all_singletons() {
        let components = strongly_connected_components(&[vec![1], vec![2], vec![]]);
        assert_eq!(components.len(), 3);
        assert!(components.iter().all(|component| component.len() == 1));
    }

    #[test]
    fn a_long_chain_does_not_overflow_the_stack() {
        // The reason this walk is iterative. A recursive Tarjan on a chain this
        // long overflows a default thread stack.
        let depth = 200_000usize;
        let mut adjacency: Vec<Vec<u32>> = Vec::with_capacity(depth);
        for index in 0..depth {
            adjacency.push(if index + 1 < depth {
                vec![index as u32 + 1]
            } else {
                Vec::new()
            });
        }
        assert_eq!(strongly_connected_components(&adjacency).len(), depth);
    }

    #[test]
    fn package_inits_are_recognised_on_both_separators() {
        assert!(is_package_init("a/b/__init__.py"));
        assert!(is_package_init("a\\b\\__init__.py"));
        assert!(!is_package_init("a/b/init.py"));
    }

    #[test]
    fn identity_helpers_split_on_the_symbol_separator() {
        assert_eq!(file_of("a/b.py::Klass::method"), "a/b.py");
        assert_eq!(name_of("a/b.py::Klass::method"), "method");
        assert_eq!(file_of("a/b.py"), "a/b.py");
        assert_eq!(name_of("a/b.py"), "a/b.py");
    }
}
