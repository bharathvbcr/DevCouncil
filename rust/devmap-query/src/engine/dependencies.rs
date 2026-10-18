//! The parts of `dependencies` both engines must agree on, in one place.
//!
//! The store-backed and in-memory engines read different structures (an edge
//! index, a resolved edge list) and answer the same question. Everything that
//! is a *decision* rather than a read lives here, so the two cannot drift:
//! the order of the page, what counts as an inbound edge, and what the
//! informational note says.

use std::cmp::Ordering;
use std::collections::BTreeSet;

use devmap_extract::model::EdgeKind;
use devmap_resolve::model::ResolvedEdge;

use crate::rung::Rung;

/// `Contains` and `MemberOf` are a symbol's own declaration inside its file or
/// type. They are structure, not something the target depends on, and on a file
/// they outnumber every real dependency.
fn is_declaration(kind: EdgeKind) -> bool {
    matches!(kind, EdgeKind::Contains | EdgeKind::MemberOf)
}

/// The one order of a `dependencies` page.
///
/// Declaration edges sort after every real dependency kind, so a page squeezed
/// by the budget leads with what the target uses; they stay in the answer (and
/// in `total`) because they are true edges and dropping them would make the two
/// engines disagree with the stored rows. Within each group: confidence
/// descending, then a full key, so the content of a truncated page does not
/// depend on which engine or which storage order produced it.
pub(super) fn dependency_order(a: &ResolvedEdge, b: &ResolvedEdge) -> Ordering {
    is_declaration(a.edge_kind)
        .cmp(&is_declaration(b.edge_kind))
        .then_with(|| b.confidence.0.total_cmp(&a.confidence.0))
        .then_with(|| a.source_file.cmp(&b.source_file))
        .then_with(|| a.source_symbol.cmp(&b.source_symbol))
        .then_with(|| a.target_file.cmp(&b.target_file))
        .then_with(|| a.target_symbol.cmp(&b.target_symbol))
        .then_with(|| format!("{:?}", a.edge_kind).cmp(&format!("{:?}", b.edge_kind)))
}

/// What an edge must point at to count as inbound to the target.
pub(super) enum InboundTarget<'a> {
    /// An edge from another file into this one.
    File(&'a str),
    /// An edge from another symbol into one of these `(symbol, file)` nodes.
    Symbols(&'a BTreeSet<(String, String)>),
}

impl InboundTarget<'_> {
    /// Whether one edge is inbound. Both engines ask this of each edge exactly
    /// once, so the count they state is a count of edges, never of starts.
    pub(super) fn admits(
        &self,
        source_file: &str,
        source_symbol: &str,
        target_file: &str,
        target_symbol: &str,
    ) -> bool {
        match self {
            InboundTarget::File(file) => target_file == *file && source_file != *file,
            InboundTarget::Symbols(nodes) => {
                source_symbol != target_symbol
                    && nodes.contains(&(target_symbol.to_string(), target_file.to_string()))
            }
        }
    }
}

/// How many distinct symbols a file query's spelling also names. A file node
/// (no `::`) is not a symbol, so a file never shadows itself.
pub(super) fn shadowed_symbol_count<'a>(
    query: &str,
    symbols: impl Iterator<Item = &'a str>,
) -> usize {
    if query.contains(['/', '\\']) {
        return 0;
    }
    symbols
        .filter(|symbol| symbol.contains("::") && crate::query_match::symbol_matches(symbol, query))
        .collect::<BTreeSet<_>>()
        .len()
}

/// What an answer knows that is not a defect of the answer.
pub(super) struct DependencyFacts<'a> {
    pub(super) query: &'a str,
    /// Outbound edges before any `min_rung` cut.
    pub(super) outbound_before_rung: usize,
    /// Outbound edges after it.
    pub(super) outbound_after_rung: usize,
    pub(super) inbound: usize,
    pub(super) min_rung: Option<Rung>,
    /// Symbols a file answer's spelling also names; zero for a symbol query.
    pub(super) shadowed_symbols: usize,
}

/// The informational note: `Response::note`, never `walk_incomplete`.
///
/// Emptiness is judged *before* the rung cut. An answer emptied by `min_rung`
/// is not a target that depends on nothing, and says how many edges the rung
/// hid. The inbound pointer appears only when nothing leaves the target at all.
pub(super) fn dependency_note(facts: &DependencyFacts<'_>) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if facts.outbound_after_rung == 0 {
        if facts.outbound_before_rung > 0 {
            let rung = facts.min_rung.map_or("the requested rung", Rung::label);
            parts.push(format!(
                "{} outbound edge(s) of {} hidden by min_rung={rung}",
                facts.outbound_before_rung, facts.query
            ));
        } else if facts.inbound > 0 {
            parts.push(format!(
                "{} depends on nothing in the index, but {} inbound edge(s) point at it; \
                 `dependencies` lists outbound edges only — ask `impact` for what reaches it",
                facts.query, facts.inbound
            ));
        }
    }
    if facts.shadowed_symbols > 0 {
        parts.push(format!(
            "{} is an indexed file and also spells {} symbol(s); the file was answered — \
             use `file::symbol` to ask about a symbol",
            facts.query, facts.shadowed_symbols
        ));
    }
    (!parts.is_empty()).then(|| parts.join("; "))
}
