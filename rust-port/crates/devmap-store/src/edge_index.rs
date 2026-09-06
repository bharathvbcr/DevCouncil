//! Adjacency over one generation's edges, built once and reused.
//!
//! Every graph question the query engine answers — `impact`, `trace`, `deps`,
//! `explore`, `affected_tests` — used to begin by materialising the whole
//! generation: `latest_edges` cloned all 271,543 rows out of the cache, the
//! engine converted each one into a `ResolvedEdge`, the traversal built an
//! adjacency map over all of them, and only then did the walk look at the
//! target. On the ScholarLM corpus that was 92 ms for a question whose answer
//! touches a few dozen edges, against 3 ms for `status`.
//!
//! The cost is not the walk; it is arriving at it. This index moves that work
//! to once per *generation* instead of once per *question*: the edge rows are
//! the ones the store already caches, and what is added is four maps of
//! `u32` edge ids — forward and reverse by symbol, forward and reverse by file
//! — so a question costs the edges it reaches plus a hash lookup.
//!
//! Freshness is by construction. The index is keyed by generation id, so a
//! build that commits a new generation invalidates it by existing; there is no
//! separate invalidation path for a long-lived `devmap mcp` to forget to call.

use std::collections::HashMap;
use std::sync::Arc;

use devmap_analyze::model::AnalysisDisclosure;
use devmap_analyze::traversal::{EdgeView, GraphIndex};
use devmap_extract::model::{confidence_millis, EdgeKind};

use crate::db::StoredEdge;

/// The stored spelling of an edge kind, as `EdgeKind`.
///
/// The one owner of the mapping. It is a `Debug` rendering on the way in
/// (`save_generation` writes `format!("{kind:?}")`), so a second hand-written
/// table anywhere else is a table that can drift from this one; the query
/// crate's `resolved_edge_from_stored` calls through to here rather than
/// keeping its own copy.
///
/// An unknown spelling is an error, never a default: it means the store was
/// written by a binary that knows an edge kind this one does not, and silently
/// dropping such edges would answer "nothing depends on this" from a graph that
/// was only partly read.
pub fn edge_kind_from_stored(kind: &str) -> Result<EdgeKind, UnknownEdgeKind> {
    Ok(match kind {
        "Imports" => EdgeKind::Imports,
        "Calls" => EdgeKind::Calls,
        "Contains" => EdgeKind::Contains,
        "Defines" => EdgeKind::Defines,
        "Instantiates" => EdgeKind::Instantiates,
        "Extends" => EdgeKind::Extends,
        "Implements" => EdgeKind::Implements,
        "SubscribesTo" => EdgeKind::SubscribesTo,
        "HandlesRoute" => EdgeKind::HandlesRoute,
        "WiredTo" => EdgeKind::WiredTo,
        "MemberOf" => EdgeKind::MemberOf,
        "DependsOn" => EdgeKind::DependsOn,
        "TaintFlow" => EdgeKind::TaintFlow,
        "References" => EdgeKind::References,
        other => return Err(UnknownEdgeKind(other.to_string())),
    })
}

/// An edge's evidence tier, and whether it was read or guessed.
pub use devmap_resolve::model::Evidence as EdgeResolution;
/// The evidence tier an edge was built from, as it is spelled in the store.
///
/// `devmap_resolve::model::ResolutionKind`, under the name this crate has
/// always used for it. The column holds the *kind*, not the payload, because
/// the payload is either already in the row (`SameFile`'s target) or is
/// evidence the row cannot carry (`AmbiguousGlobal`'s candidate list). The
/// kind is what the honesty invariants are stated over — its `confidence()` is
/// a function of it alone — and the spelling, the confidence table and the
/// variant set all have one owner there, so nothing in this crate can drift
/// from the resolver.
pub use devmap_resolve::model::ResolutionKind as StoredResolutionKind;
/// Where an edge's resolution kind came from — `devmap_resolve`'s enum. The
/// store only ever produces `Stored` and `Reconstructed`; `Resolver` is the
/// value an edge carries before it is written.
pub use devmap_resolve::model::ResolutionSource;

/// The stored spelling of a resolution kind — [`StoredResolutionKind::label`],
/// through the resolution's own `kind()`. Kept as a function so the write path
/// reads as it always did; the table it used to hold is the resolver's now.
pub fn resolution_kind_label(resolution: &devmap_resolve::model::Resolution) -> &'static str {
    resolution.kind().label()
}

/// Decode a stored resolution kind.
///
/// An unknown spelling is an error, never a default — the same rule
/// [`edge_kind_from_stored`] states: it means the store was written by a binary
/// that knows a tier this one does not, and quietly rounding it to some
/// neighbouring tier would put a confidence claim on an edge whose evidence
/// this binary cannot read.
pub fn resolution_kind_from_stored(
    kind: &str,
) -> Result<StoredResolutionKind, UnknownResolutionKind> {
    StoredResolutionKind::from_label(kind).ok_or_else(|| UnknownResolutionKind(kind.to_string()))
}

/// The resolution an edge row carries, or the reconstruction that stands in for
/// one it does not.
///
/// The fallback is deliberately the *naive* reading — the only one a row
/// without the column supports — and it is labelled
/// [`ResolutionSource::Reconstructed`] so nothing can mistake it for the
/// resolver's own record. It gets `Structural` wrong on purpose-built edges
/// whose endpoints share a file, and `ImportScoped` wrong on every cross-file
/// edge; that is what "this generation did not store its evidence" looks like
/// when it is said out loud instead of guessed over.
pub fn edge_resolution(edge: &StoredEdge) -> Result<EdgeResolution, UnknownResolutionKind> {
    match edge.resolution.as_deref() {
        Some(kind) => Ok(EdgeResolution {
            kind: resolution_kind_from_stored(kind)?,
            source: ResolutionSource::Stored,
        }),
        None => Ok(reconstructed_resolution(edge)),
    }
}

/// The guess a row without the column supports, always labelled as one.
fn reconstructed_resolution(edge: &StoredEdge) -> EdgeResolution {
    EdgeResolution {
        kind: if edge.source_file == edge.target_file {
            StoredResolutionKind::SameFile
        } else {
            StoredResolutionKind::UniqueGlobal
        },
        source: ResolutionSource::Reconstructed,
    }
}

/// A stored edge kind this binary does not know.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownEdgeKind(pub String);

/// A stored resolution kind this binary does not know.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownResolutionKind(pub String);

impl std::fmt::Display for UnknownResolutionKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "stored generation has unknown resolution kind {:?}",
            self.0
        )
    }
}

impl std::error::Error for UnknownResolutionKind {}

impl std::fmt::Display for UnknownEdgeKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "stored generation has unknown edge kind {:?}",
            self.0
        )
    }
}

impl std::error::Error for UnknownEdgeKind {}

/// Whether `confidence` clears `min_confidence` under the store's rounding.
///
/// The same comparison `Store::latest_edges` applies, and the same one the SQL
/// applies, so an indexed walk and a filtered read cannot disagree about which
/// edges exist. NaN is refused before it reaches here — see
/// `checked_min_confidence`.
pub(crate) fn admits(confidence: f32, min_confidence: f32) -> bool {
    (confidence * 1000.0).round() as i64 >= (min_confidence * 1000.0).round() as i64
}

/// One generation's edges, with the adjacency a bounded walk needs.
///
/// Edge ids are indices into [`Self::edges`], and every id list this type hands
/// out is ascending — which is to say, in the generation's own stable edge
/// order (`confidence DESC, source path, target path, symbols, kind`). That
/// order is the final tie-break of every answer derived from a walk, so it is
/// part of the contract and not an accident of the container (R4).
pub struct GenerationEdges {
    edges: Arc<Vec<StoredEdge>>,
    /// How much of the corpus the generation that produced these edges
    /// actually read, travelling with the adjacency rather than beside it.
    ///
    /// A walk over this index is only as complete as the graph it walks, and
    /// the two facts have to come from one generation or the qualification
    /// describes a different snapshot than the answer. Holding it here is what
    /// makes that structural: there is no way to obtain the adjacency without
    /// it. `None` means the disclosure could not be read, which is a distinct
    /// answer from a disclosure saying coverage was complete.
    analysis: Option<AnalysisDisclosure>,
    /// Parsed once. The stored `edge_kind` string stays available for the
    /// `EdgeIdentity` label, so nothing formats a kind per crossed edge.
    kinds: Vec<EdgeKind>,
    /// The evidence tier behind each edge, decoded once, in edge-id order.
    ///
    /// Travels with the adjacency for the same reason `analysis` does: a
    /// consumer holding an edge has to be able to ask what it was resolved by
    /// *and* whether that answer was read or reconstructed, and taking the two
    /// from separate reads lets them describe different generations.
    resolutions: Vec<EdgeResolution>,
    /// How many edges with a *stored* kind carry a confidence that kind does
    /// not entitle. Zero on every generation a correct writer produced; the
    /// number is reported, never repaired, because the row is the evidence.
    confidence_mismatches: usize,
    by_source_symbol: HashMap<Box<str>, Vec<u32>>,
    by_target_symbol: HashMap<Box<str>, Vec<u32>>,
    by_source_file: HashMap<Box<str>, Vec<u32>>,
    by_target_file: HashMap<Box<str>, Vec<u32>>,
    empty: Vec<u32>,
}

impl std::fmt::Debug for GenerationEdges {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GenerationEdges")
            .field("edges", &self.edges.len())
            .field("source_symbols", &self.by_source_symbol.len())
            .field("target_symbols", &self.by_target_symbol.len())
            .finish()
    }
}

impl GenerationEdges {
    /// Index `edges`, sharing their storage rather than copying it.
    ///
    /// Fails on an unknown edge kind, exactly where the old per-request
    /// conversion failed, so a store from a newer binary is refused rather than
    /// half-read.
    ///
    /// `analysis` must describe the same generation as `edges`. It is a
    /// parameter rather than something set afterwards so the production caller
    /// cannot build an index and forget it; a hand-built index in a test passes
    /// `None`, which is the truth for one.
    pub fn build(
        edges: Arc<Vec<StoredEdge>>,
        analysis: Option<AnalysisDisclosure>,
    ) -> Result<Self, UnknownEdgeKind> {
        // No stored resolutions: every edge's tier is reconstructed, and says
        // so. That is the truth for a hand-built index in a test, and for a
        // generation written before the column existed.
        Self::build_with_resolutions(edges, analysis, None)
    }

    /// [`Self::build`] with the generation's decoded resolution column.
    ///
    /// `resolutions` is one entry per edge, in the same order. `None` means the
    /// generation carries no resolution column at all, and every edge's tier is
    /// then reconstructed — which each entry says of itself. A length mismatch
    /// is refused rather than zipped short: a shifted alignment would attach
    /// one edge's evidence to another's, which is worse than having none.
    pub fn build_with_resolutions(
        edges: Arc<Vec<StoredEdge>>,
        analysis: Option<AnalysisDisclosure>,
        resolutions: Option<Vec<EdgeResolution>>,
    ) -> Result<Self, UnknownEdgeKind> {
        // Ids are `u32`. A generation with more edges than that cannot be
        // addressed, and answering over a silently truncated prefix would be a
        // wrong answer rather than a bounded one, so it is refused by the
        // caller before it gets here; the assert documents the invariant.
        assert!(
            edges.len() <= u32::MAX as usize,
            "a generation with more than u32::MAX edges cannot be indexed"
        );
        if let Some(resolutions) = &resolutions {
            assert_eq!(
                resolutions.len(),
                edges.len(),
                "a resolution column that does not line up with its edges would \
                 attribute one edge's evidence to another"
            );
        }
        let mut kinds = Vec::with_capacity(edges.len());
        for edge in edges.iter() {
            kinds.push(edge_kind_from_stored(&edge.edge_kind)?);
        }
        let resolutions = match resolutions {
            Some(resolutions) => resolutions,
            None => edges.iter().map(reconstructed_resolution).collect(),
        };
        // The read-side half of the honesty invariant. On the way in,
        // `ResolvedEdge::resolved` makes `confidence` a function of the
        // resolution; here the two are read back separately and compared, so a
        // row whose confidence no longer matches the evidence it names — a
        // tampered store, a bug in a writer, a migration that touched one
        // column — is counted rather than trusted. Only a *stored* kind can be
        // judged: a reconstructed one is a guess about the row, and a guess
        // cannot convict the row of disagreeing with it. Compared in
        // milliconfidence for the reason `Confidence::to_millis` exists.
        let confidence_mismatches = edges
            .iter()
            .zip(&resolutions)
            .filter(|(edge, resolution)| {
                resolution.source == ResolutionSource::Stored
                    && confidence_millis(edge.confidence)
                        != resolution.kind.confidence().to_millis()
            })
            .count();
        let mut by_source_symbol: HashMap<Box<str>, Vec<u32>> = HashMap::new();
        let mut by_target_symbol: HashMap<Box<str>, Vec<u32>> = HashMap::new();
        let mut by_source_file: HashMap<Box<str>, Vec<u32>> = HashMap::new();
        let mut by_target_file: HashMap<Box<str>, Vec<u32>> = HashMap::new();
        for (id, edge) in edges.iter().enumerate() {
            let id = id as u32;
            push(&mut by_source_symbol, &edge.source_symbol, id);
            push(&mut by_target_symbol, &edge.target_symbol, id);
            push(&mut by_source_file, &edge.source_file, id);
            push(&mut by_target_file, &edge.target_file, id);
        }
        Ok(Self {
            edges,
            analysis,
            kinds,
            resolutions,
            confidence_mismatches,
            by_source_symbol,
            by_target_symbol,
            by_source_file,
            by_target_file,
            empty: Vec::new(),
        })
    }

    /// The generation's edge rows, in their stored order.
    pub fn edges(&self) -> &Arc<Vec<StoredEdge>> {
        &self.edges
    }

    /// The coverage disclosure of the generation these edges came from.
    ///
    /// `None` is "could not be read", not "complete" — see the field.
    pub fn analysis(&self) -> Option<&AnalysisDisclosure> {
        self.analysis.as_ref()
    }

    pub fn len(&self) -> usize {
        self.edges.len()
    }

    pub fn is_empty(&self) -> bool {
        self.edges.is_empty()
    }

    /// The edge behind an id from any of this index's id lists.
    pub fn edge(&self, id: u32) -> &StoredEdge {
        &self.edges[id as usize]
    }

    pub fn kind(&self, id: u32) -> EdgeKind {
        self.kinds[id as usize]
    }

    /// The evidence tier behind an edge, and whether it was read or guessed.
    pub fn resolution(&self, id: u32) -> EdgeResolution {
        self.resolutions[id as usize]
    }

    /// Stored edges whose confidence contradicts the resolution kind the store
    /// recorded for them. See the field.
    pub fn confidence_mismatches(&self) -> usize {
        self.confidence_mismatches
    }

    /// Whether the confidence floor admits this edge.
    pub fn admits(&self, id: u32, min_confidence: f32) -> bool {
        admits(self.edge(id).confidence, min_confidence)
    }

    /// Distinct symbols on one side, each with the ids of its edges.
    ///
    /// Iterated by the traversal-start matcher, which has to run a predicate
    /// per *distinct symbol* rather than per edge: the corpus this exists for
    /// has 41,276 symbols and 271,543 edges.
    pub fn symbols(&self, reverse: bool) -> impl Iterator<Item = (&str, &[u32])> {
        let map = if reverse {
            &self.by_target_symbol
        } else {
            &self.by_source_symbol
        };
        map.iter().map(|(key, ids)| (&**key, ids.as_slice()))
    }

    /// Distinct file paths on one side, each with the ids of its edges.
    pub fn files(&self, reverse: bool) -> impl Iterator<Item = (&str, &[u32])> {
        let map = if reverse {
            &self.by_target_file
        } else {
            &self.by_source_file
        };
        map.iter().map(|(key, ids)| (&**key, ids.as_slice()))
    }

    /// Ids of the edges leaving `symbol`, ascending.
    pub fn from_source_symbol(&self, symbol: &str) -> &[u32] {
        self.by_source_symbol
            .get(symbol)
            .map(Vec::as_slice)
            .unwrap_or(&self.empty)
    }

    /// Ids of the edges entering `symbol`, ascending.
    pub fn into_target_symbol(&self, symbol: &str) -> &[u32] {
        self.by_target_symbol
            .get(symbol)
            .map(Vec::as_slice)
            .unwrap_or(&self.empty)
    }

    /// A one-direction view a bounded walk can consume.
    ///
    /// The direction travels with the view, so a walk cannot be given a
    /// direction that disagrees with the adjacency it reads — see
    /// [`GraphIndex::reverse`]. Three words over the shared index, so a caller
    /// that needs both directions holds both for the cost of two pointers.
    pub fn directed(&self, reverse: bool, min_confidence: f32) -> DirectedEdges<'_> {
        DirectedEdges {
            index: self,
            reverse,
            min_confidence,
        }
    }

    /// Bytes of adjacency this index holds, excluding the shared edge rows.
    ///
    /// Reported rather than assumed: the index is retained for the life of a
    /// generation in a long-lived daemon, so "how much does it cost to keep"
    /// has to be answerable without a profiler.
    pub fn adjacency_bytes(&self) -> usize {
        fn map_bytes(map: &HashMap<Box<str>, Vec<u32>>) -> usize {
            map.iter()
                .map(|(key, ids)| {
                    key.len()
                        + std::mem::size_of::<Box<str>>()
                        + std::mem::size_of::<Vec<u32>>()
                        + ids.capacity() * std::mem::size_of::<u32>()
                })
                .sum()
        }
        self.kinds.capacity() * std::mem::size_of::<EdgeKind>()
            + map_bytes(&self.by_source_symbol)
            + map_bytes(&self.by_target_symbol)
            + map_bytes(&self.by_source_file)
            + map_bytes(&self.by_target_file)
    }
}

fn push(map: &mut HashMap<Box<str>, Vec<u32>>, key: &str, id: u32) {
    match map.get_mut(key) {
        Some(ids) => ids.push(id),
        None => {
            map.insert(key.into(), vec![id]);
        }
    }
}

/// One direction of a [`GenerationEdges`], under one confidence floor.
pub struct DirectedEdges<'a> {
    index: &'a GenerationEdges,
    reverse: bool,
    min_confidence: f32,
}

impl DirectedEdges<'_> {
    /// The confidence floor this view was built with.
    pub fn min_confidence(&self) -> f32 {
        self.min_confidence
    }
}

impl GraphIndex for DirectedEdges<'_> {
    fn reverse(&self) -> bool {
        self.reverse
    }

    fn neighbors(&self, node: &str) -> &[u32] {
        if self.reverse {
            self.index.into_target_symbol(node)
        } else {
            self.index.from_source_symbol(node)
        }
    }

    fn edge(&self, id: u32) -> EdgeView<'_> {
        let edge = self.index.edge(id);
        EdgeView {
            source_symbol: &edge.source_symbol,
            target_symbol: &edge.target_symbol,
            source_file: &edge.source_file,
            target_file: &edge.target_file,
            kind: self.index.kind(id),
        }
    }

    fn kind_label(&self, id: u32) -> &str {
        &self.index.edge(id).edge_kind
    }

    fn admits(&self, id: u32) -> bool {
        self.index.admits(id, self.min_confidence)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge(source: &str, target: &str, kind: &str, confidence: f32) -> StoredEdge {
        StoredEdge {
            source_file: format!("{source}.py"),
            target_file: format!("{target}.py"),
            source_symbol: source.to_string(),
            target_symbol: target.to_string(),
            edge_kind: kind.to_string(),
            confidence,
            resolution: None,
        }
    }

    #[test]
    fn every_edge_kind_this_binary_writes_round_trips_through_the_parser() {
        // The stored form is a `Debug` rendering, so the parser is only correct
        // as long as it names every variant the writer can emit. Anything new
        // fails here rather than at a user's `impact` call.
        for kind in [
            EdgeKind::Imports,
            EdgeKind::Calls,
            EdgeKind::Contains,
            EdgeKind::Defines,
            EdgeKind::Instantiates,
            EdgeKind::Extends,
            EdgeKind::Implements,
            EdgeKind::SubscribesTo,
            EdgeKind::HandlesRoute,
            EdgeKind::WiredTo,
            EdgeKind::MemberOf,
            EdgeKind::DependsOn,
            EdgeKind::TaintFlow,
            EdgeKind::References,
        ] {
            let stored = format!("{kind:?}");
            assert_eq!(edge_kind_from_stored(&stored), Ok(kind), "{stored}");
        }
    }

    #[test]
    fn an_unknown_kind_is_refused_rather_than_dropped() {
        let error = edge_kind_from_stored("FromTheFuture").expect_err("must refuse");
        assert!(error.to_string().contains("FromTheFuture"));
        let refused =
            GenerationEdges::build(Arc::new(vec![edge("a", "b", "FromTheFuture", 1.0)]), None);
        assert!(refused.is_err(), "an index must not half-read a generation");
    }

    #[test]
    fn id_lists_are_ascending_so_the_graph_order_survives() {
        let rows = vec![
            edge("a", "b", "Calls", 1.0),
            edge("c", "b", "Calls", 0.9),
            edge("a", "d", "Calls", 0.8),
        ];
        let index = GenerationEdges::build(Arc::new(rows), None).expect("index");
        assert_eq!(index.from_source_symbol("a"), &[0, 2]);
        assert_eq!(index.into_target_symbol("b"), &[0, 1]);
        assert_eq!(index.from_source_symbol("nothing"), &[] as &[u32]);
    }

    #[test]
    fn duplicate_and_self_edges_keep_every_id() {
        let rows = vec![
            edge("a", "a", "Calls", 1.0),
            edge("a", "a", "Calls", 1.0),
            edge("a", "a", "Contains", 1.0),
        ];
        let index = GenerationEdges::build(Arc::new(rows), None).expect("index");
        assert_eq!(index.from_source_symbol("a"), &[0, 1, 2]);
        assert_eq!(index.into_target_symbol("a"), &[0, 1, 2]);
    }

    #[test]
    fn the_confidence_floor_is_the_stores_rounding_rule() {
        let rows = vec![edge("a", "b", "Calls", 0.7495)];
        let index = GenerationEdges::build(Arc::new(rows), None).expect("index");
        // Rounds to 750 >= 750: admitted, exactly as `latest_edges` admits it.
        assert!(index.admits(0, 0.75));
        assert!(!index.admits(0, 0.76));
    }
}
