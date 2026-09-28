//! An `ask` answer shaped for reading: files first, then the source.
//!
//! `ask` already returns each hit's verbatim, hash-verified source. What it
//! does not return is the shape a reader starts from: which *files* the answer
//! lives in, which of them are tests, and how the hits relate to each other.
//! A flat list of symbols leaves the caller to rebuild that, usually by opening
//! the files the hits already quoted.
//!
//! This module rebuilds it from what the map already holds, and nothing else:
//!
//!   - **Files in rank order.** A file sits where its best hit sits.
//!   - **Role.** `test` when the path is a test path ([`is_test_path`]),
//!     otherwise `implementation`. A path rule, not a judgement about content —
//!     a test fixture under `src/` is `implementation` here, and says so by
//!     being wrong in a way anyone can check.
//!   - **Relations.** The admitted call edges *between hits*, as `calls` and
//!     `called_by` on each unit. Only edges at or above the same
//!     `min_confidence` floor the ask walk used, so the pack never shows a
//!     relation the ranking was not allowed to use.
//!   - **Folding.** A hit whose lines sit inside another hit's complete source
//!     keeps its lead and relations but not a second copy of the text;
//!     `contained_in` names the unit that shows it. A container whose source
//!     was capped does not fold anything — the inner text may be the part that
//!     was cut.
//!
//! Nothing here reads a file or re-ranks. Order, scores and budget accounting
//! are `ask`'s; the envelope fields are carried over unchanged, so
//! `tokens_used` still describes the answer before folding (an upper bound on
//! what the pack prints).

use std::collections::{BTreeSet, HashMap};

use devmap_extract::model::EdgeKind;
use devmap_store::GenerationEdges;
use serde::{Deserialize, Serialize};

use crate::engine::is_test_path;
use crate::model::{ResolutionAvailability, Response, SourceFreshness, SymbolHit};

/// What a file is to the question, by path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceRole {
    Implementation,
    Test,
}

impl EvidenceRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Implementation => "implementation",
            Self::Test => "test",
        }
    }
}

/// One `ask` hit, with its place among the other hits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceUnit {
    #[serde(flatten)]
    pub hit: SymbolHit,
    pub qualified_name: String,
    /// Qualified names of other hits this one calls, over admitted edges.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub calls: Vec<String>,
    /// Qualified names of other hits that call this one, over admitted edges.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub called_by: Vec<String>,
    /// Set when this unit's lines are inside another unit's complete source,
    /// which then carries the text. `hit.source_span` is empty when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contained_in: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceFile {
    pub file_path: String,
    pub role: EvidenceRole,
    /// The best score among this file's units: the file's rank.
    pub score: f32,
    /// Units in line order, which is the order a reader meets them.
    pub units: Vec<EvidenceUnit>,
}

/// An `ask` answer grouped by file. Envelope fields are `ask`'s, unchanged.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidencePack {
    pub files: Vec<EvidenceFile>,
    pub source_freshness: SourceFreshness,
    pub shown: u32,
    pub hidden: u32,
    pub total: u32,
    pub truncated: bool,
    pub tokens_used: u32,
    pub resolution: ResolutionAvailability,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub walk_incomplete: Option<String>,
}

/// Build the pack from an `ask` response and its index-aligned qualified names.
///
/// `edges` is `None` when the generation has no edge index; the pack is then
/// the same files and units with no relations, which is what the map knows.
pub fn assemble(
    response: Response<SymbolHit>,
    qualified: &[String],
    edges: Option<&GenerationEdges>,
    min_confidence: f32,
) -> EvidencePack {
    debug_assert_eq!(response.items.len(), qualified.len());
    let Response {
        source_freshness,
        items,
        shown,
        hidden,
        total,
        truncated,
        tokens_used,
        resolution,
        walk_incomplete,
        ..
    } = response;

    let mut units: Vec<EvidenceUnit> = items
        .into_iter()
        .zip(qualified.iter().cloned())
        .map(|(hit, qualified_name)| EvidenceUnit {
            hit,
            qualified_name,
            calls: Vec::new(),
            called_by: Vec::new(),
            contained_in: None,
        })
        .collect();

    if let Some(edges) = edges {
        relate(&mut units, edges, min_confidence);
    }
    fold_nested(&mut units);

    // Group in rank order: a file's position is its first (best) unit's.
    let mut order: Vec<String> = Vec::new();
    let mut by_file: HashMap<String, Vec<EvidenceUnit>> = HashMap::new();
    for unit in units {
        let path = unit.hit.file_path.clone();
        if !by_file.contains_key(&path) {
            order.push(path.clone());
        }
        by_file.entry(path).or_default().push(unit);
    }
    let files = order
        .into_iter()
        .map(|file_path| {
            let mut units = by_file.remove(&file_path).unwrap_or_default();
            let score = units
                .iter()
                .map(|unit| unit.hit.score)
                .fold(f32::NEG_INFINITY, f32::max);
            units.sort_by(|a, b| {
                a.hit
                    .span
                    .0
                    .cmp(&b.hit.span.0)
                    .then(b.hit.span.1.cmp(&a.hit.span.1))
                    .then(a.qualified_name.cmp(&b.qualified_name))
            });
            let role = if is_test_path(&file_path) {
                EvidenceRole::Test
            } else {
                EvidenceRole::Implementation
            };
            EvidenceFile {
                file_path,
                role,
                score,
                units,
            }
        })
        .collect();

    EvidencePack {
        files,
        source_freshness,
        shown,
        hidden,
        total,
        truncated,
        tokens_used,
        resolution,
        walk_incomplete,
    }
}

/// Attach the admitted call edges whose two ends are both hits.
///
/// Joined on `(file, qualified name)`, not the name alone: the same qualified
/// name in two files is two symbols, and an edge between one pair must not be
/// reported for the other.
fn relate(units: &mut [EvidenceUnit], edges: &GenerationEdges, min_confidence: f32) {
    let index: HashMap<(&str, &str), usize> = units
        .iter()
        .enumerate()
        .map(|(i, unit)| {
            (
                (unit.hit.file_path.as_str(), unit.qualified_name.as_str()),
                i,
            )
        })
        .collect();
    let mut calls: Vec<BTreeSet<String>> = vec![BTreeSet::new(); units.len()];
    let mut called_by: Vec<BTreeSet<String>> = vec![BTreeSet::new(); units.len()];
    for id in 0..edges.len() as u32 {
        if edges.kind(id) != EdgeKind::Calls || !edges.admits(id, min_confidence) {
            continue;
        }
        let Some(&from) = index.get(&(edges.source_file(id), edges.source_symbol(id))) else {
            continue;
        };
        let Some(&to) = index.get(&(edges.target_file(id), edges.target_symbol(id))) else {
            continue;
        };
        if from == to {
            continue;
        }
        calls[from].insert(units[to].qualified_name.clone());
        called_by[to].insert(units[from].qualified_name.clone());
    }
    for (unit, (calls, called_by)) in units.iter_mut().zip(calls.into_iter().zip(called_by)) {
        unit.calls = calls.into_iter().collect();
        unit.called_by = called_by.into_iter().collect();
    }
}

/// Drop the second copy of text a containing unit already shows.
///
/// The container must have its complete source (available and not capped);
/// among several candidates the tightest one is named. Two units with the
/// same span do not fold into each other.
fn fold_nested(units: &mut [EvidenceUnit]) {
    let shows_whole = |unit: &EvidenceUnit| {
        unit.hit.source_unavailable_reason.is_none()
            && unit.hit.source_span_omitted_bytes.is_none()
            && !unit.hit.source_span.is_empty()
            && unit.hit.span != (0, 0)
    };
    let mut folds: Vec<(usize, String)> = Vec::new();
    for (i, inner) in units.iter().enumerate() {
        if inner.hit.span == (0, 0) {
            continue;
        }
        let (start, end) = inner.hit.span;
        let container = units
            .iter()
            .enumerate()
            .filter(|&(j, outer)| {
                j != i
                    && outer.hit.file_path == inner.hit.file_path
                    && shows_whole(outer)
                    && outer.hit.span.0 <= start
                    && end <= outer.hit.span.1
                    && outer.hit.span != inner.hit.span
            })
            .min_by_key(|(_, outer)| outer.hit.span.1 - outer.hit.span.0);
        if let Some((_, outer)) = container {
            folds.push((i, outer.qualified_name.clone()));
        }
    }
    for (i, container) in folds {
        units[i].hit.source_span.clear();
        units[i].hit.source_span_omitted_bytes = None;
        units[i].contained_in = Some(container);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(path: &str, name: &str, span: (u32, u32), source: &str, score: f32) -> SymbolHit {
        SymbolHit {
            symbol_name: name.to_string(),
            file_path: path.to_string(),
            kind: "function".to_string(),
            span,
            source_span: source.to_string(),
            source_unavailable_reason: None,
            source_span_omitted_bytes: None,
            score,
        }
    }

    fn unit(hit: SymbolHit) -> EvidenceUnit {
        let qualified_name = format!("{}::{}", hit.file_path, hit.symbol_name);
        EvidenceUnit {
            hit,
            qualified_name,
            calls: Vec::new(),
            called_by: Vec::new(),
            contained_in: None,
        }
    }

    #[test]
    fn a_method_inside_a_shown_class_is_folded_into_it() {
        let mut units = vec![
            unit(hit("a.py", "Cache", (1, 10), "class Cache: ...", 0.9)),
            unit(hit("a.py", "get", (3, 5), "def get(self): ...", 0.8)),
        ];
        fold_nested(&mut units);
        assert_eq!(units[1].contained_in.as_deref(), Some("a.py::Cache"));
        assert!(units[1].hit.source_span.is_empty());
        assert!(units[0].contained_in.is_none());
    }

    #[test]
    fn a_capped_container_folds_nothing() {
        let mut outer = hit("a.py", "Cache", (1, 400), "class Cache: ...", 0.9);
        outer.source_span_omitted_bytes = Some(9_000);
        let mut units = vec![
            unit(outer),
            unit(hit("a.py", "get", (300, 310), "def get(self): ...", 0.8)),
        ];
        fold_nested(&mut units);
        assert!(units[1].contained_in.is_none(), "{:?}", units[1]);
        assert!(!units[1].hit.source_span.is_empty());
    }

    #[test]
    fn same_lines_in_another_file_do_not_fold() {
        let mut units = vec![
            unit(hit("a.py", "Cache", (1, 10), "class Cache: ...", 0.9)),
            unit(hit("b.py", "get", (3, 5), "def get(self): ...", 0.8)),
        ];
        fold_nested(&mut units);
        assert!(units[1].contained_in.is_none());
    }

    #[test]
    fn files_keep_rank_order_and_units_read_top_down() {
        let response = crate::engine::budget_take(
            vec![
                hit("src/b.py", "late", (20, 22), "def late(): ...", 0.9),
                hit(
                    "tests/test_b.py",
                    "test_late",
                    (1, 3),
                    "def test_late(): ...",
                    0.7,
                ),
                hit("src/b.py", "early", (1, 3), "def early(): ...", 0.5),
            ],
            10_000,
            |_| 1,
        );
        let qualified = vec![
            "b.late".to_string(),
            "test_b.test_late".to_string(),
            "b.early".to_string(),
        ];
        let pack = assemble(response, &qualified, None, 1.0);
        let paths: Vec<&str> = pack.files.iter().map(|f| f.file_path.as_str()).collect();
        assert_eq!(paths, ["src/b.py", "tests/test_b.py"]);
        assert_eq!(pack.files[0].role, EvidenceRole::Implementation);
        assert_eq!(pack.files[1].role, EvidenceRole::Test);
        assert_eq!(pack.files[0].score, 0.9);
        let names: Vec<&str> = pack.files[0]
            .units
            .iter()
            .map(|u| u.hit.symbol_name.as_str())
            .collect();
        assert_eq!(names, ["early", "late"]);
        assert_eq!(pack.shown, 3);
    }
}
