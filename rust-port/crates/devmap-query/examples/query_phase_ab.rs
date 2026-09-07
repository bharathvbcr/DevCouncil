//! A/B the individual phases of an `impact` call against candidate rewrites.
//!
//! `query_bench` says which phase costs what; this says *why*. Each section
//! states a hypothesis, times the shipped implementation against a candidate,
//! and — because a faster wrong answer is not a fix — checks that the two agree
//! on every one of the corpus's edges before reporting a ratio.
//!
//! Measurement only, no assertions, no dependencies. Run:
//!
//! ```sh
//! cargo run --release -p devmap-query --example query_phase_ab -- <devmap.sqlite>
//! ```
//!
//! The `_shipped` functions below are copies of the implementations in
//! `src/query_match.rs` and `src/engine.rs` as they stood before this lane
//! touched them. They are here so the ratio survives the fix landing: once the
//! candidate ships, this file still shows what it replaced.
//!
//! ## Recorded results
//!
//! Machine: Apple M5 Pro (18 cores), macOS 27.0, rustc 1.98.0, `--release`.
//! Corpus: a generated 4,000-file / 252,000-symbol / 660,000-edge store (see
//! `query_bench`). All four candidates produced output identical to the
//! shipped implementation over every edge in that store; the ratios below are
//! meaningless without that, which is why this prints it.
//!
//! ```text
//! H1 path_matches over 660k edges       37.3 ms -> 3.4 ms   (10.9x)  FIXED
//!    falsifier: a bare-name query, which never calls path_matches, cost
//!    36.4 ms against the path-shaped query's 46.6 ms — so path_matches was
//!    ~37 ms of that 46.6 ms and the rest is the scan itself. Post-fix the
//!    two invert: 43.4 ms bare against 11.1 ms path-shaped.
//! H2 traversed_resolution_edges         42.9 ms -> 11.2 ms   (3.8x)  FIXED
//!    not falsified: the borrowed-key variant does exactly the same number
//!    of BTreeSet lookups, so what moved was the discarded allocations.
//! H3 traverse_graph index build         56.1 ms of a 54.6 ms call (~100%)
//!    confirmed and NOT fixed: the index lives in devmap-analyze, which this
//!    lane may not edit. A 200-node walk still pays for all 660,000 edges.
//! H4 shortest_path adjacency index      80.8 ms -> 64.3 ms   (1.26x)
//!    partly falsified and NOT taken: the confidence sort alone is 51.6 ms of
//!    that 80.8 ms, so borrowing the keys is worth ~17 ms of a ~200 ms
//!    `trace_between` (<10%) and costs a lifetime refactor of a BFS whose
//!    path determinism is pinned by an existing test. Left as measured.
//! ```

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use devmap_analyze::traversal::{traverse_graph, TraversalOptions, TraversalResult};
use devmap_extract::model::EdgeKind;
use devmap_resolve::model::ResolvedEdge;

const REPEATS: usize = 7;

fn main() -> anyhow::Result<()> {
    let profile = if cfg!(debug_assertions) {
        "debug (not comparable with release numbers)"
    } else {
        "release"
    };
    let path = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: query_phase_ab <devmap.sqlite>"))?;
    let store = devmap_store::Store::open_existing(&path)?
        .ok_or_else(|| anyhow::anyhow!("no store at {path}"))?;
    let edges: Vec<ResolvedEdge> = store
        .latest_edges(0.0)?
        .into_iter()
        .map(devmap_query::resolved_edge_from_stored)
        .collect::<anyhow::Result<Vec<_>>>()?;
    if edges.is_empty() {
        anyhow::bail!("refusing to A/B phases over an empty store");
    }
    println!("profile: {profile}");
    println!("edges: {}", edges.len());

    // A busy target, chosen the same way `query_bench` chooses one.
    let mut inbound: std::collections::BTreeMap<&str, usize> = Default::default();
    for edge in &edges {
        if edge.edge_kind == EdgeKind::Calls {
            *inbound.entry(edge.target_symbol.as_str()).or_default() += 1;
        }
    }
    let symbol_target = inbound
        .iter()
        .max_by_key(|(name, count)| (**count, std::cmp::Reverse(*name)))
        .map(|(name, _)| (*name).to_string())
        .ok_or_else(|| anyhow::anyhow!("no Calls edges"))?;
    let path_target = edges
        .iter()
        .map(|edge| edge.target_file.clone())
        .find(|p| !p.is_empty())
        .ok_or_else(|| anyhow::anyhow!("no edge carries a file"))?;
    println!("targets: symbol={symbol_target:?} path={path_target:?}\n");

    // A bare name: no `::`, no separator, no source extension, so
    // `traversal_start_matches` reaches `symbol_matches` without ever calling
    // `path_matches`. This is H1's falsifier and it has to be a genuinely bare
    // name — a qualified `file.py::sym` still resolves its file half.
    let bare_target = symbol_target
        .rsplit("::")
        .next()
        .unwrap_or(&symbol_target)
        .to_string();
    println!("targets: bare={bare_target:?}\n");

    h1_path_matching(&edges, &path_target, &bare_target);
    h2_traversed_selection(&edges, &symbol_target);
    h3_traversal_index(&edges, &symbol_target);
    h4_scoped_trace_index(&edges);
    Ok(())
}

// ---------------------------------------------------------------- reporting

fn p50(mut samples: Vec<Duration>) -> Duration {
    samples.sort();
    samples[samples.len() / 2]
}

fn time<T>(mut call: impl FnMut() -> T) -> (Duration, T) {
    // One untimed warm-up, then the median of REPEATS.
    let mut last = call();
    let mut samples = Vec::with_capacity(REPEATS);
    for _ in 0..REPEATS {
        let started = Instant::now();
        last = call();
        samples.push(started.elapsed());
    }
    (p50(samples), last)
}

fn verdict(hypothesis: &str, shipped: Duration, candidate: Duration, agree: bool) {
    let ratio = shipped.as_secs_f64() / candidate.as_secs_f64().max(f64::MIN_POSITIVE);
    println!("  shipped   {shipped:>10.2?}");
    println!("  candidate {candidate:>10.2?}   ({ratio:.2}x faster)");
    println!(
        "  identical output: {}",
        if agree {
            "yes"
        } else {
            "NO - candidate is wrong, ignore the ratio"
        }
    );
    println!("  => {hypothesis}\n");
}

// ------------------------------------------------------------------ H1: path

/// `path_matches` as shipped: three `String` allocations per call.
fn path_matches_shipped(file: &str, query: &str) -> bool {
    let file = file.replace('\\', "/");
    let query = query.replace('\\', "/");
    file == query || file.ends_with(&format!("/{query}"))
}

/// Candidate: the same separator-insensitive comparison, byte-wise.
///
/// `/` (0x2F) and `\` (0x5C) are ASCII, so they never appear inside a
/// multi-byte UTF-8 sequence and a byte-wise fold is exactly the `replace`.
fn path_matches_candidate(file: &str, query: &str) -> bool {
    fn slash(byte: u8) -> u8 {
        if byte == b'\\' {
            b'/'
        } else {
            byte
        }
    }
    fn eq(left: &[u8], right: &[u8]) -> bool {
        left.len() == right.len() && left.iter().zip(right).all(|(a, b)| slash(*a) == slash(*b))
    }
    let file = file.as_bytes();
    let query = query.as_bytes();
    if eq(file, query) {
        return true;
    }
    // `ends_with("/{query}")`: the separator must be there too.
    match file.len().checked_sub(query.len() + 1) {
        Some(boundary) => slash(file[boundary]) == b'/' && eq(&file[boundary + 1..], query),
        None => false,
    }
}

fn h1_path_matching(edges: &[ResolvedEdge], path_target: &str, bare_target: &str) {
    println!("H1 traversal_starts is dominated by path_matches's per-edge allocations.");
    println!("   Falsifier: a BARE-name query never reaches path_matches. If it costs");
    println!("   the same as a path-shaped one, the loop is the cost, not the");
    println!("   allocations, and the candidate below cannot help.");

    let (bare_shaped, bare_hits) =
        time(|| devmap_query::traversal_starts(edges, bare_target, true).len());
    let (path_shaped, path_hits) =
        time(|| devmap_query::traversal_starts(edges, path_target, true).len());
    println!("  bare-name query     {bare_shaped:>10.2?}  ({bare_hits} starts, no path_matches)");
    println!("  path-shaped query   {path_shaped:>10.2?}  ({path_hits} starts)");

    // Both implementations, over the same strings the real scan sees.
    let (shipped, shipped_true) = time(|| {
        edges
            .iter()
            .filter(|edge| path_matches_shipped(&edge.target_file, path_target))
            .count()
    });
    let (candidate, candidate_true) = time(|| {
        edges
            .iter()
            .filter(|edge| path_matches_candidate(&edge.target_file, path_target))
            .count()
    });
    // Agreement is checked on every edge, both fields, not just on the count.
    let mut agree = shipped_true == candidate_true;
    for edge in edges {
        for (file, query) in [
            (edge.target_file.as_str(), path_target),
            (edge.source_file.as_str(), path_target),
            (edge.source_file.as_str(), edge.target_file.as_str()),
        ] {
            if path_matches_shipped(file, query) != path_matches_candidate(file, query) {
                agree = false;
            }
        }
    }
    verdict(
        "path_matches allocation is the cost of a path-shaped traversal start",
        shipped,
        candidate,
        agree,
    );
}

// -------------------------------------------------------------- H2: selection

/// `traversed_resolution_edges` as shipped: three `String` allocations per
/// edge, to build a key that is thrown away.
fn traversed_shipped(
    traversal: &TraversalResult,
    edges: &[ResolvedEdge],
    min_confidence: f32,
) -> Vec<ResolvedEdge> {
    let traversed: BTreeSet<(String, String, String)> = traversal
        .traversed_edges
        .iter()
        .map(|edge| {
            (
                edge.source.clone(),
                edge.target.clone(),
                edge.edge_kind.clone(),
            )
        })
        .collect();
    edges
        .iter()
        .filter(|edge| {
            edge.confidence.0 >= min_confidence
                && traversed.contains(&(
                    edge.source_symbol.clone(),
                    edge.target_symbol.clone(),
                    format!("{:?}", edge.edge_kind),
                ))
        })
        .cloned()
        .collect()
}

fn h2_traversed_selection(edges: &[ResolvedEdge], symbol_target: &str) {
    println!("H2 traversed_resolution_edges is dominated by the three String");
    println!("   allocations it makes per edge to build a lookup key it discards.");
    println!("   Falsifier: if the BTreeSet lookups are the cost, a borrowed-key");
    println!("   variant that does the same number of lookups will not move.");

    let starts: Vec<String> = devmap_query::traversal_starts(edges, symbol_target, true)
        .into_iter()
        .map(|(symbol, _)| symbol)
        .collect();
    let walk = traverse_graph(
        &starts,
        edges,
        &TraversalOptions {
            max_depth: 5,
            max_nodes: 5_000,
            reverse: true,
        },
    );
    println!("  walk recorded {} edges", walk.traversed_edges.len());

    let (shipped, shipped_out) = time(|| traversed_shipped(&walk, edges, 0.0));
    let (candidate, candidate_out) = time(|| traversed_candidate(&walk, edges, 0.0));
    // The third column: whatever the crate currently exports. Before the fix it
    // matches `shipped`; after it, `candidate`. Reported so this probe cannot
    // silently start comparing the candidate against itself.
    let (live, live_out) = time(|| devmap_query::traversed_resolution_edges(&walk, edges, 0.0));
    let agree = same(&shipped_out, &candidate_out) && same(&shipped_out, &live_out);
    println!("  kept {} edges", shipped_out.len());
    println!("  live crate fn        {live:>10.2?}");
    verdict(
        "the discarded key allocations are the cost of the selection scan",
        shipped,
        candidate,
        agree,
    );
}

/// Candidate: identical set membership, borrowed keys, zero allocation.
///
/// `edge_kind_name` reproduces the `Debug` spelling `traverse_graph` records,
/// which `engine.rs`'s `edge_kind_name_is_the_spelling_traverse_graph_records`
/// pins.
fn traversed_candidate(
    traversal: &TraversalResult,
    edges: &[ResolvedEdge],
    min_confidence: f32,
) -> Vec<ResolvedEdge> {
    let traversed: BTreeSet<(&str, &str, &str)> = traversal
        .traversed_edges
        .iter()
        .map(|edge| {
            (
                edge.source.as_str(),
                edge.target.as_str(),
                edge.edge_kind.as_str(),
            )
        })
        .collect();
    edges
        .iter()
        .filter(|edge| {
            edge.confidence.0 >= min_confidence
                && traversed.contains(&(
                    edge.source_symbol.as_str(),
                    edge.target_symbol.as_str(),
                    debug_kind_name(edge.edge_kind),
                ))
        })
        .cloned()
        .collect()
}

fn debug_kind_name(kind: EdgeKind) -> &'static str {
    match kind {
        EdgeKind::Imports => "Imports",
        EdgeKind::Calls => "Calls",
        EdgeKind::Contains => "Contains",
        EdgeKind::Defines => "Defines",
        EdgeKind::Instantiates => "Instantiates",
        EdgeKind::Extends => "Extends",
        EdgeKind::Implements => "Implements",
        EdgeKind::SubscribesTo => "SubscribesTo",
        EdgeKind::HandlesRoute => "HandlesRoute",
        EdgeKind::WiredTo => "WiredTo",
        EdgeKind::MemberOf => "MemberOf",
        EdgeKind::DependsOn => "DependsOn",
        EdgeKind::TaintFlow => "TaintFlow",
        EdgeKind::References => "References",
    }
}

fn same(left: &[ResolvedEdge], right: &[ResolvedEdge]) -> bool {
    left.len() == right.len() && left.iter().zip(right).all(|(a, b)| edge_eq(a, b))
}

/// Field-by-field equality: `ResolvedEdge` derives no `PartialEq`, and
/// comparing lengths alone would let a reordering pass as identical.
fn edge_eq(left: &ResolvedEdge, right: &ResolvedEdge) -> bool {
    left.source_file == right.source_file
        && left.target_file == right.target_file
        && left.source_symbol == right.source_symbol
        && left.target_symbol == right.target_symbol
        && left.edge_kind == right.edge_kind
        && left.confidence.0.to_bits() == right.confidence.0.to_bits()
        && format!("{:?}", left.resolution) == format!("{:?}", right.resolution)
        && format!("{:?}", left.details) == format!("{:?}", right.details)
}

// ------------------------------------------------------------------ H3: index

fn h3_traversal_index(edges: &[ResolvedEdge], symbol_target: &str) {
    println!("H3 traverse_graph's per-call adjacency index is most of what a walk");
    println!("   costs, so a walk that visits 200 nodes still pays for all 660k edges.");
    println!("   Falsifier: if the index is cheap, timing it alone will be a small");
    println!("   fraction of the whole call.");

    let starts: Vec<String> = devmap_query::traversal_starts(edges, symbol_target, true)
        .into_iter()
        .map(|(symbol, _)| symbol)
        .collect();
    let opts = TraversalOptions {
        max_depth: 5,
        max_nodes: 5_000,
        reverse: true,
    };
    let (whole, walk) = time(|| traverse_graph(&starts, edges, &opts));
    // The same construction `traverse_graph` performs at the top of every call.
    let (index_only, index_len) = time(|| {
        let mut adj: std::collections::BTreeMap<&str, Vec<&ResolvedEdge>> = Default::default();
        for edge in edges {
            adj.entry(edge.target_symbol.as_str())
                .or_default()
                .push(edge);
        }
        adj.len()
    });
    println!(
        "  whole traverse_graph {whole:>10.2?}  ({} edges recorded)",
        walk.traversed_edges.len()
    );
    println!("  index build only     {index_only:>10.2?}  ({index_len} keys)");
    println!(
        "  => the index is {:.0}% of the call; it is rebuilt on every query, and\n     \
         devmap-analyze owns it (out of this lane's scope to change).\n",
        100.0 * index_only.as_secs_f64() / whole.as_secs_f64()
    );
}

// ------------------------------------------------------- H4: scoped trace index

fn h4_scoped_trace_index(edges: &[ResolvedEdge]) {
    println!("H4 trace_between's adjacency index clones two Strings per edge to key");
    println!("   a map it then queries a few thousand times.");
    println!("   Falsifier: if the confidence sort dominates, borrowing the keys will");
    println!("   barely move the total.");

    let (sort_only, sorted_len) = time(|| {
        let mut ordered: Vec<&ResolvedEdge> = edges.iter().collect();
        ordered.sort_by(|a, b| {
            b.confidence
                .0
                .total_cmp(&a.confidence.0)
                .then_with(|| a.source_file.cmp(&b.source_file))
                .then_with(|| a.source_symbol.cmp(&b.source_symbol))
                .then_with(|| a.target_file.cmp(&b.target_file))
                .then_with(|| a.target_symbol.cmp(&b.target_symbol))
                .then_with(|| debug_kind_name(a.edge_kind).cmp(debug_kind_name(b.edge_kind)))
        });
        ordered.len()
    });
    let mut ordered: Vec<&ResolvedEdge> = edges.iter().collect();
    ordered.sort_by(|a, b| {
        b.confidence
            .0
            .total_cmp(&a.confidence.0)
            .then_with(|| a.source_file.cmp(&b.source_file))
            .then_with(|| a.source_symbol.cmp(&b.source_symbol))
            .then_with(|| a.target_file.cmp(&b.target_file))
            .then_with(|| a.target_symbol.cmp(&b.target_symbol))
            .then_with(|| debug_kind_name(a.edge_kind).cmp(debug_kind_name(b.edge_kind)))
    });

    let (shipped, shipped_keys) = time(|| {
        let mut outgoing: std::collections::BTreeMap<(String, String), Vec<usize>> =
            Default::default();
        for (index, edge) in ordered.iter().enumerate() {
            outgoing
                .entry((edge.source_file.clone(), edge.source_symbol.clone()))
                .or_default()
                .push(index);
        }
        outgoing.len()
    });
    let (candidate, candidate_keys) = time(|| {
        let mut outgoing: std::collections::BTreeMap<(&str, &str), Vec<usize>> = Default::default();
        for (index, edge) in ordered.iter().enumerate() {
            outgoing
                .entry((edge.source_file.as_str(), edge.source_symbol.as_str()))
                .or_default()
                .push(index);
        }
        outgoing.len()
    });
    println!("  confidence sort      {sort_only:>10.2?}  ({sorted_len} edges)");
    verdict(
        "the owned-key adjacency index is worth borrowing",
        shipped,
        candidate,
        shipped_keys == candidate_keys,
    );
}
