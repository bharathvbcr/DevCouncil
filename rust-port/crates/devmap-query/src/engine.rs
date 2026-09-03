use devmap_analyze::clones::group_clones;
use devmap_analyze::traversal::{traverse_graph, TraversalOptions};
use devmap_extract::model::*;
use devmap_resolve::model::*;
use devmap_store::{Store, StoredEdge};

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

use crate::cancel::{Cancel, QueryCancelled};
use crate::model::*;

pub struct QueryEngine<'a> {
    extractions: &'a [Extraction],
    resolution: &'a ResolutionResult,
}

/// Query facade over the latest durable SQLite generation. Unlike
/// `QueryEngine`, this type never extracts or resolves source files.
pub struct StoreQueryEngine<'a> {
    store: &'a Store,
    /// Consulted inside the long loops. Default is a flag nobody sets, so a
    /// caller that has no way to give up (the CLI) behaves exactly as before.
    cancel: Cancel,
}

impl<'a> StoreQueryEngine<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self {
            store,
            cancel: Cancel::new(),
        }
    }

    /// Answer under a cancellation flag the caller can trip.
    ///
    /// The IPC layer bounds a query with a timeout that frees the connection
    /// but cannot abort the blocking task behind it, so without this the
    /// abandoned traversal or corpus scan runs to completion on a pool thread
    /// with nobody left to read it. See [`crate::cancel`].
    pub fn with_cancel(mut self, cancel: Cancel) -> Self {
        self.cancel = cancel;
        self
    }

    pub fn search(&self, req: Request<String>) -> anyhow::Result<Response<SymbolHit>> {
        if self.store.latest_generation_id()?.is_none() {
            return Ok(unavailable_response(ResolutionAvailability::Unavailable {
                reason: "no persisted generation is available".to_string(),
            }));
        }
        if req.query.trim().is_empty() {
            return Ok(budget_take(Vec::new(), req.token_budget, |_| 0));
        }
        let total = self.store.count_search_symbols(&req.query)?;
        let rows = self
            .store
            .search_symbols(&req.query, budget_page_size(req.token_budget))?;
        let repo_root = self.store.latest_repo_root()?;
        let query = req.query.to_lowercase();
        let mut hits = Vec::with_capacity(rows.len());
        for row in rows {
            let name = row.name.to_lowercase();
            let qualified = row.qualified_name.to_lowercase();
            let score = if name == query || qualified == query {
                1.0
            } else if name.starts_with(&query) {
                0.95
            } else {
                0.8
            };
            hits.push(hit_from_stored(
                row,
                repo_root.as_deref(),
                req.token_budget,
                score,
            ));
        }
        hits.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.file_path.cmp(&b.file_path))
                .then_with(|| a.symbol_name.cmp(&b.symbol_name))
                .then_with(|| a.span.cmp(&b.span))
        });
        let mut response = budget_take(hits, req.token_budget, search_hit_tokens);
        response.total = total;
        response.hidden = total.saturating_sub(response.shown);
        response.truncated = response.hidden > 0;
        Ok(response)
    }

    pub fn dependencies(&self, req: Request<String>) -> anyhow::Result<Response<ResolvedEdge>> {
        let Some(file) = self.store.latest_file(&req.query)? else {
            return Ok(unavailable_response(ResolutionAvailability::Unavailable {
                reason: format!("{} is not indexed", req.query),
            }));
        };
        if matches!(file.parse_outcome, ParseOutcome::Failed { .. }) {
            return Ok(unavailable_response(ResolutionAvailability::Unavailable {
                reason: format!("{} could not be parsed", req.query),
            }));
        }
        let rows = self
            .store
            .latest_edges_for_file(&req.query, req.min_confidence)?;
        let edges = rows
            .into_iter()
            .map(stored_edge_to_resolved)
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(budget_take(edges, req.token_budget, |_| 25))
    }

    pub fn impact(&self, req: Request<String>) -> anyhow::Result<Response<ResolvedEdge>> {
        self.traverse(req, true)
    }

    pub fn trace(&self, req: Request<String>) -> anyhow::Result<Response<ResolvedEdge>> {
        self.traverse(req, false)
    }

    /// Return one deterministic shortest path from `from` to `to`.
    ///
    /// A scoped trace is atomic under token budgeting: a prefix that does not
    /// reach the requested destination is not a valid answer, so an
    /// insufficient budget returns zero items with `truncated = true`.
    pub fn trace_between(
        &self,
        req: Request<(String, String)>,
    ) -> anyhow::Result<Response<ResolvedEdge>> {
        if self.store.latest_generation_id()?.is_none() {
            return Ok(unavailable_response(ResolutionAvailability::Unavailable {
                reason: "no persisted generation is available".to_string(),
            }));
        }
        let (from, to) = req.query;
        let from = from.trim();
        let to = to.trim();
        if from.is_empty() || to.is_empty() {
            return Ok(unavailable_response(ResolutionAvailability::Unavailable {
                reason: "scoped trace endpoints must not be empty".to_string(),
            }));
        }
        let edges = self.resolved_edges(req.min_confidence)?;
        let path = shortest_path(&edges, from, to, req.max_depth.min(64), 5_000, &self.cancel)?;
        let Some(path) = path else {
            return Ok(unavailable_response(ResolutionAvailability::Unavailable {
                reason: format!("no indexed path from {from:?} to {to:?}"),
            }));
        };
        Ok(atomic_budget_take(path, req.token_budget, |_| 25))
    }

    /// Every edge in the latest generation at or above `min_confidence`, in
    /// engine form.
    ///
    /// One owner for the conversion, and the one place the whole edge set is
    /// walked before any traversal starts — so it is where an abandoned
    /// request stops earliest. `latest_edges` takes and releases the store lock
    /// internally, so nothing here holds it across the walk that follows.
    fn resolved_edges(&self, min_confidence: f32) -> anyhow::Result<Vec<ResolvedEdge>> {
        let rows = self.store.latest_edges(min_confidence)?;
        let mut edges = Vec::with_capacity(rows.len());
        for (index, row) in rows.into_iter().enumerate() {
            self.cancel.check_every(index)?;
            edges.push(stored_edge_to_resolved(row)?);
        }
        Ok(edges)
    }

    fn traverse(
        &self,
        req: Request<String>,
        reverse: bool,
    ) -> anyhow::Result<Response<ResolvedEdge>> {
        // The store lock is released by `latest_edges` before it returns — it
        // locks, reads and drops — so nothing below this line holds it. An
        // abandoned traversal therefore cannot block the drain loop's writes
        // while it unwinds.
        let edges = self.resolved_edges(req.min_confidence)?;
        let target = req.query.trim();
        let start: Vec<String> = edges
            .iter()
            .filter(|edge| {
                if reverse {
                    crate::query_match::traversal_start_matches(
                        target,
                        &edge.target_symbol,
                        &edge.target_file,
                    )
                } else {
                    crate::query_match::traversal_start_matches(
                        target,
                        &edge.source_symbol,
                        &edge.source_file,
                    )
                }
            })
            .map(|edge| {
                if reverse {
                    edge.target_symbol.clone()
                } else {
                    edge.source_symbol.clone()
                }
            })
            .collect();
        if start.is_empty() {
            return Ok(unavailable_response(ResolutionAvailability::Unavailable {
                reason: format!("{target} has no indexed traversal start"),
            }));
        }
        // `traverse_graph` is bounded by `max_nodes`/`max_depth` and does not
        // itself consult the flag; checking on either side of it keeps an
        // abandoned request from paying for the sort and the budgeting that
        // follow.
        self.cancel.check()?;
        let walk = traverse_graph(
            &start,
            &edges,
            &TraversalOptions {
                max_depth: req.max_depth.min(64),
                max_nodes: 5_000,
                reverse,
            },
        );
        self.cancel.check()?;
        let mut traversed = traversed_resolution_edges(&walk, &edges, req.min_confidence);
        traversed.sort_by(|a, b| {
            b.confidence
                .0
                .total_cmp(&a.confidence.0)
                .then_with(|| a.source_file.cmp(&b.source_file))
                .then_with(|| a.target_file.cmp(&b.target_file))
                .then_with(|| a.source_symbol.cmp(&b.source_symbol))
        });
        Ok(budget_take(traversed, req.token_budget, |_| 25))
    }

    pub fn dead_symbols(
        &self,
        token_budget: u32,
    ) -> anyhow::Result<Response<devmap_analyze::DeadSymbolReport>> {
        if self.store.latest_generation_id()?.is_none() {
            return Ok(unavailable_response(ResolutionAvailability::Unavailable {
                reason: "no persisted generation is available".to_string(),
            }));
        }
        let dead = self
            .store
            .latest_dead_symbols()?
            .into_iter()
            .filter(|row| !row.is_exempt)
            .collect();
        Ok(budget_take(dead, token_budget, |_| 30))
    }

    /// Duplicate bodies in the latest generation.
    ///
    /// Grouping runs here rather than at build time. The signatures are stored
    /// per symbol, so the groups are derivable on demand and never go stale
    /// against the rows they came from — and a build does not pay for a report
    /// most builds have no reader for.
    /// `kind` and `min_nodes` narrow the report; `None` and `0` mean no filter.
    ///
    /// The filters are applied *before* the budget, and that ordering is the
    /// whole contract. Filtering afterwards would narrow a list the budget had
    /// already cut, so `--min-nodes` could never reach past the first page —
    /// and, because re-budgeting the survivors leaves `hidden` at zero, the
    /// subset would be returned as a complete answer. Measured on this
    /// repository: `--kind exact --min-nodes 100` under a 900-token budget
    /// reported "2 groups, not truncated" where the true answer was 29.
    pub fn clones(
        &self,
        token_budget: u32,
        kind: Option<devmap_analyze::CloneKind>,
        min_nodes: u32,
    ) -> anyhow::Result<CloneReport> {
        if self.store.latest_generation_id()?.is_none() {
            return Ok(CloneReport {
                groups: unavailable_response(ResolutionAvailability::Unavailable {
                    reason: "no persisted generation is available".to_string(),
                }),
                signed_symbols: 0,
                unsigned_symbols: 0,
            });
        }
        let (candidates, unsigned) = self.store.latest_clone_candidates()?;
        let summary = group_clones(&candidates, unsigned);
        let matching: Vec<_> = summary
            .groups
            .into_iter()
            .filter(|group| kind.is_none_or(|wanted| group.kind == wanted))
            .filter(|group| group.min_nodes >= min_nodes)
            .collect();
        Ok(CloneReport {
            groups: budget_take(matching, token_budget, clone_group_tokens),
            signed_symbols: summary.signed_symbols,
            unsigned_symbols: summary.unsigned_symbols,
        })
    }

    /// Rank symbols by TF-IDF similarity of their names to `query`.
    ///
    /// Complements `search`, which is FTS5 prefix matching: that finds symbols
    /// whose names *contain* the query, this finds symbols whose names are
    /// *about* it. `LLMCache` for "llm cache", `compute_freshness` for
    /// "freshness computation".
    ///
    /// Scores nothing when no symbol shares a term with the query, rather than
    /// returning the whole corpus ordered by a zero. "Nothing matched" is an
    /// answer.
    pub fn search_semantic(
        &self,
        query: &str,
        token_budget: u32,
    ) -> anyhow::Result<Response<SymbolHit>> {
        if self.store.latest_generation_id()?.is_none() {
            return Ok(unavailable_response(ResolutionAvailability::Unavailable {
                reason: "no persisted generation is available".to_string(),
            }));
        }
        let symbols = self.store.all_symbols()?;
        if symbols.is_empty() || query.trim().is_empty() {
            return Ok(budget_take(Vec::new(), token_budget, |_| 0));
        }
        // Both names, so a query can match either the bare symbol or the path
        // and type it sits under.
        let texts: Vec<String> = symbols
            .iter()
            .map(|s| format!("{} {}", s.name, s.qualified_name))
            .collect();
        let index = crate::semantic::SemanticIndex::build(&texts, &self.cancel)?;

        let repo_root = self.store.latest_repo_root()?;
        let scored = index.score(query, &self.cancel)?;
        let total = u32::try_from(scored.len()).unwrap_or(u32::MAX);
        // Materialise only as far down the ranking as the budget could reach.
        // Every scored symbol used to be turned into a `SymbolHit` first — one
        // `read_to_string` each — and budgeted afterwards, so a query matching
        // a common term opened every file it matched in order to discard almost
        // all of them. The ranking is already sorted, so the page bound is the
        // same one keyword search uses.
        let hits: Vec<SymbolHit> = scored
            .into_iter()
            .take(budget_page_size(token_budget))
            .map(|(position, score)| {
                hit_from_stored(
                    symbols[position].clone(),
                    repo_root.as_deref(),
                    token_budget,
                    score,
                )
            })
            .collect();
        // `total` is the whole ranked corpus, not the page: budgeting a page
        // and reporting its length as the total is how a capped sample comes
        // back labelled complete.
        let mut response = budget_take(hits, token_budget, search_hit_tokens);
        response.total = total;
        response.hidden = total.saturating_sub(response.shown);
        response.truncated = response.hidden > 0;
        Ok(response)
    }

    /// What the map cost against what reading files would have.
    ///
    /// Sizes come from the files on disk, resolved against the generation's
    /// `repo_root`. Files that cannot be read are counted separately rather
    /// than treated as zero bytes: a corpus figure that silently omits what it
    /// could not open understates the alternative and flatters the map.
    pub fn savings(&self, query: Option<&str>, token_budget: u32) -> anyhow::Result<SavingsReport> {
        let repo_root = self.store.latest_repo_root()?;
        let resolve = |path: &str| -> PathBuf {
            match &repo_root {
                Some(root) if !Path::new(path).is_absolute() => Path::new(root).join(path),
                _ => PathBuf::from(path),
            }
        };
        let size_of =
            |path: &str| -> Option<u64> { std::fs::metadata(resolve(path)).ok().map(|m| m.len()) };

        let indexed_paths = match self.store.latest_generation_id()? {
            Some(gen) => self.store.list_generation_paths(gen)?,
            None => Vec::new(),
        };
        let mut corpus_bytes = 0u64;
        let mut corpus_files_unreadable = 0usize;
        for path in &indexed_paths {
            match size_of(path) {
                Some(bytes) => corpus_bytes = corpus_bytes.saturating_add(bytes),
                None => corpus_files_unreadable += 1,
            }
        }

        let repo_map_bytes = repo_root
            .as_ref()
            .map(|root| Path::new(root).join(".devcouncil/repo_map.json"))
            .and_then(|path| std::fs::metadata(path).ok())
            .map(|m| m.len());

        let query_savings = match query {
            None => None,
            Some(text) => {
                let response = self.search(Request {
                    query: text.to_string(),
                    token_budget,
                    min_confidence: 0.0,
                    max_depth: 1,
                })?;
                let mut named: BTreeSet<&str> = BTreeSet::new();
                for hit in &response.items {
                    named.insert(hit.file_path.as_str());
                }
                let mut files_bytes = 0u64;
                let mut files_unreadable = 0usize;
                for path in &named {
                    match size_of(path) {
                        Some(bytes) => files_bytes = files_bytes.saturating_add(bytes),
                        None => files_unreadable += 1,
                    }
                }
                Some(QuerySavings {
                    query: text.to_string(),
                    hits: response.items.len(),
                    answer_tokens: response.tokens_used,
                    files_named: named.len(),
                    files_bytes,
                    files_unreadable,
                })
            }
        };

        Ok(SavingsReport {
            basis: format!("estimated as bytes / {BYTES_PER_TOKEN}; not a tokenizer count"),
            indexed_files: indexed_paths.len(),
            corpus_bytes,
            corpus_files_unreadable,
            repo_map_bytes,
            query: query_savings,
        })
    }

    /// What `new_source` would do to the graph if it were written to `path`.
    ///
    /// Nothing is written and no generation is created. The buffer is extracted
    /// in memory and diffed against the stored extraction for the same path.
    ///
    /// A buffer that fails to parse produces no symbols, and diffing that
    /// against a real file reports every symbol as removed and every caller as
    /// breaking. That output is indistinguishable from a genuine mass deletion
    /// and is the single most likely thing to be produced by a half-typed edit,
    /// which is exactly when an agent would be asking. So a failed parse
    /// returns `delta_available: false` and no symbols at all.
    ///
    /// Requires the `parse` feature. Every other query on this engine answers
    /// from a persisted map; this one has to parse a buffer that was never
    /// indexed, so it is the one surface that genuinely needs the grammars.
    #[cfg(feature = "parse")]
    pub fn preview(
        &self,
        path: &str,
        new_source: &str,
        token_budget: u32,
        min_confidence: f32,
    ) -> anyhow::Result<PreviewReport> {
        let candidate = devmap_extract::extract_file(path, new_source);
        let parse_status = parse_status_name(&candidate.parse_outcome).to_string();

        let empty_callers = || Response {
            items: Vec::new(),
            shown: 0,
            hidden: 0,
            total: 0,
            truncated: false,
            tokens_used: 0,
            resolution: ResolutionAvailability::Available,
        };

        if let ParseOutcome::Failed { reason } = &candidate.parse_outcome {
            return Ok(PreviewReport {
                file_path: path.to_string(),
                parse_status,
                delta_available: false,
                file_is_indexed: self.store.latest_extraction_for_path(path)?.is_some(),
                compared_against: "nothing".to_string(),
                degraded_reason: Some(format!(
                    "the buffer did not parse ({reason}); no delta is reported,                      because an unparsed file yields no symbols and would read                      as a deletion of every symbol in it"
                )),
                symbols: Vec::new(),
                bodies_not_compared: 0,
                ambiguous_callers: 0,
                broken_callers: empty_callers(),
            });
        }

        // The "before" side is extracted from the file on disk, not read back
        // from the store. Two reasons: the stored extraction has been through
        // `for_durable_store`, and more importantly the user is editing the
        // file that is on disk — diffing against a generation built from an
        // older commit would report their own already-saved work as part of the
        // candidate change. The store is still consulted, but only for the
        // caller graph, where being a generation behind is a known and stated
        // property rather than a wrong diff.
        let file_is_indexed = self.store.latest_extraction_for_path(path)?.is_some();
        // Resolved against the generation's own root, not the process's working
        // directory. Indexed paths are repository-relative, and the two callers
        // of this have different working directories: the CLI runs wherever the
        // user is, the daemon wherever it was spawned. Reading `path` directly
        // works for one of them and silently finds nothing for the other —
        // which reads as "no such file", i.e. every symbol added.
        //
        // Containment is enforced *before* the read: `path` arrives from an IPC
        // caller, and this is the only query that reads a file the caller
        // names. See `contained_repo_path`.
        let resolved = contained_repo_path(self.store.latest_repo_root()?.as_deref(), path)?;
        let on_disk = std::fs::read_to_string(&resolved).ok();
        let compared_against = if on_disk.is_some() { "disk" } else { "nothing" };
        let previous = on_disk
            .as_deref()
            .map(|source| devmap_extract::extract_file(path, source).symbols)
            .unwrap_or_default();

        let mut old_by_name: BTreeMap<&str, &ExtractedSymbol> = BTreeMap::new();
        for symbol in &previous {
            old_by_name.insert(symbol.qualified_name.as_str(), symbol);
        }
        let mut new_by_name: BTreeMap<&str, &ExtractedSymbol> = BTreeMap::new();
        for symbol in &candidate.symbols {
            new_by_name.insert(symbol.qualified_name.as_str(), symbol);
        }

        let mut symbols: Vec<PreviewSymbol> = Vec::new();
        let mut bodies_not_compared = 0usize;
        for (qualified, now) in &new_by_name {
            match old_by_name.get(qualified) {
                None => symbols.push(PreviewSymbol {
                    symbol_name: now.name.clone(),
                    qualified_name: now.qualified_name.clone(),
                    kind: now.kind.as_str().to_string(),
                    change: PreviewChange::Added,
                    was: None,
                    now: now.signature.clone(),
                }),
                Some(was) => {
                    // Declaration first: that is what a caller binds to, and a
                    // changed declaration outranks whatever the body did.
                    //
                    // `ExtractedSymbol::signature` is not used for this. It is
                    // populated by only one grammar in this workspace — 80 of
                    // ~2,300 sampled symbols, all Go — so comparing it makes
                    // every Python, Rust and TypeScript signature change look
                    // like a body change. The declaration hash is computed from
                    // the tree and works wherever the grammar names a body.
                    let declaration_moved = match (was.declaration_hash, now.declaration_hash) {
                        (Some(a), Some(b)) => Some(a != b),
                        _ => None,
                    };
                    let change = match declaration_moved {
                        Some(true) => PreviewChange::SignatureChanged,
                        Some(false) | None => match (was.body_signature, now.body_signature) {
                            (Some(a), Some(b)) if a.exact != b.exact => {
                                if declaration_moved.is_none() {
                                    // The body moved and nothing could tell
                                    // whether the declaration did. Reported as
                                    // the caller-affecting case, because
                                    // under-reporting a break is the costlier
                                    // error of the two.
                                    PreviewChange::Changed
                                } else {
                                    PreviewChange::BodyChanged
                                }
                            }
                            (Some(_), Some(_)) => continue,
                            // Bodies not comparable. With an unchanged
                            // declaration there is nothing to report; without
                            // one, nothing was compared at all.
                            _ => {
                                bodies_not_compared += 1;
                                continue;
                            }
                        },
                    };
                    symbols.push(PreviewSymbol {
                        symbol_name: now.name.clone(),
                        qualified_name: now.qualified_name.clone(),
                        kind: now.kind.as_str().to_string(),
                        change,
                        was: was.signature.clone(),
                        now: now.signature.clone(),
                    });
                }
            }
        }
        for (qualified, was) in &old_by_name {
            if new_by_name.contains_key(qualified) {
                continue;
            }
            symbols.push(PreviewSymbol {
                symbol_name: was.name.clone(),
                qualified_name: was.qualified_name.clone(),
                kind: was.kind.as_str().to_string(),
                change: PreviewChange::Removed,
                was: was.signature.clone(),
                now: None,
            });
        }
        symbols.sort_by(|a, b| {
            (a.change as u8, &a.qualified_name).cmp(&(b.change as u8, &b.qualified_name))
        });

        // Only removals and re-declarations can break a caller. A body change
        // moves behaviour without touching the call site, and listing its
        // callers would bury the cases that actually stop compiling.
        let at_risk: Vec<String> = symbols
            .iter()
            .filter(|s| {
                matches!(
                    s.change,
                    PreviewChange::Removed
                        | PreviewChange::SignatureChanged
                        | PreviewChange::Changed
                )
            })
            // Qualified, not bare. Every `target_symbol` in `generation_edges`
            // is `path::Name` — matching on the bare name finds nothing at all,
            // and "no calls are affected" is a perfectly plausible-looking way
            // for this feature to do nothing.
            .map(|s| s.qualified_name.clone())
            .collect();
        let callers: Vec<PreviewCaller> = self
            .store
            .callers_of(&at_risk, path, min_confidence)?
            .into_iter()
            .map(|edge| PreviewCaller {
                target_symbol: edge.target_symbol,
                caller_file: edge.source_file,
                caller_symbol: edge.source_symbol,
                confidence: edge.confidence,
            })
            .collect();
        // What the floor excluded. Reported rather than dropped: a symbol with
        // no confident callers and 900 ambiguous ones is not the same situation
        // as one nothing references, and the difference decides whether a
        // reader should go and look.
        let ambiguous_callers = self
            .store
            .callers_of(&at_risk, path, 0.0)?
            .len()
            .saturating_sub(callers.len());

        let degraded_reason = match &candidate.parse_outcome {
            ParseOutcome::Partial { .. } => Some(
                "the buffer parsed with errors; a symbol inside an error region \
                 is invisible to extraction and will appear here as removed"
                    .to_string(),
            ),
            ParseOutcome::Fallback { .. } => Some(
                "the buffer's language has no linked grammar, so declarations \
                 were recovered by pattern and carry no bodies; body changes \
                 cannot be detected"
                    .to_string(),
            ),
            _ => None,
        };

        Ok(PreviewReport {
            file_path: path.to_string(),
            parse_status,
            delta_available: true,
            file_is_indexed,
            compared_against: compared_against.to_string(),
            degraded_reason,
            symbols,
            bodies_not_compared,
            ambiguous_callers,
            broken_callers: budget_take(callers, token_budget, |_| PREVIEW_CALLER_TOKENS),
        })
    }
}

/// Search every repository in a workspace, labelling each hit with its origin.
///
/// Repositories are queried in registry order and the budget is spent across
/// the union, so a large first repository can exhaust it before a later one is
/// reached. That is reported — `truncated` and `hidden` cover the whole
/// workspace, not one repository — rather than papered over by giving each
/// repository an equal slice, which would silently drop the best matches in a
/// large repository to make room for weak ones in a small one.
pub fn workspace_search(
    workspace: &crate::workspace::Workspace,
    query: &str,
    token_budget: u32,
    semantic: bool,
) -> anyhow::Result<crate::workspace::FederatedSearch> {
    use crate::workspace::{FederatedHit, FederatedSearch, RepoUnavailable};

    let mut all: Vec<FederatedHit> = Vec::new();
    let mut unavailable: Vec<RepoUnavailable> = Vec::new();
    let mut queried = 0usize;
    // Matches across the workspace *before* any budget was applied. Counting
    // the union of the returned items instead — which this did — counts what
    // each repository could afford to send, and every repository has already
    // spent its budget by then. A repository with 500 matches that fitted four
    // contributed four, and the federated answer called that the total and set
    // `truncated: false`.
    let mut matched_total = 0u32;

    for repo in &workspace.repos {
        let db = repo.db_path();
        let store = match devmap_store::Store::open_existing(&db) {
            Ok(Some(store)) => store,
            Ok(None) => {
                unavailable.push(RepoUnavailable {
                    repo: repo.name.clone(),
                    reason: format!("no store at {}", db.display()),
                });
                continue;
            }
            Err(error) => {
                unavailable.push(RepoUnavailable {
                    repo: repo.name.clone(),
                    reason: format!("store at {} could not be opened: {error}", db.display()),
                });
                continue;
            }
        };
        let engine = StoreQueryEngine::new(&store);
        // Each repository is asked for the *whole* budget's worth of hits; the
        // union is trimmed once at the end. Asking each for a slice would rank
        // within repositories instead of across them.
        let response = if semantic {
            engine.search_semantic(query, token_budget)?
        } else {
            engine.search(Request {
                query: query.to_string(),
                token_budget,
                min_confidence: 0.0,
                max_depth: 1,
            })?
        };
        if let ResolutionAvailability::Unavailable { reason } = &response.resolution {
            unavailable.push(RepoUnavailable {
                repo: repo.name.clone(),
                reason: reason.clone(),
            });
            continue;
        }
        queried += 1;
        matched_total = matched_total.saturating_add(response.total);
        for hit in response.items {
            all.push(FederatedHit {
                repo: repo.name.clone(),
                hit,
            });
        }
    }

    // One ranking across the workspace. Ties break on repository then path so
    // the order is total and identical on every run.
    all.sort_by(|a, b| {
        b.hit
            .score
            .total_cmp(&a.hit.score)
            .then_with(|| a.repo.cmp(&b.repo))
            .then_with(|| a.hit.file_path.cmp(&b.hit.file_path))
            .then_with(|| a.hit.symbol_name.cmp(&b.hit.symbol_name))
    });

    let budgeted = budget_take(all, token_budget, |entry| search_hit_tokens(&entry.hit));
    // `matched_total` counts every match each repository found, so it is never
    // below what was shown; the saturating subtraction is belt-and-braces
    // against a store that miscounts rather than a state this can reach.
    let hidden = matched_total.saturating_sub(budgeted.shown);
    Ok(FederatedSearch {
        items: budgeted.items,
        repos_queried: queried,
        unavailable,
        total: matched_total,
        shown: budgeted.shown,
        hidden,
        truncated: hidden > 0,
    })
}

/// Modules each repository *provides*, as `(specifier prefix, evidence)`.
///
/// Two sources, both declarations rather than inferences:
///
/// - Go: every `module` line in every `go.mod`. `import "manvi/dc/store"`
///   resolving to the repository whose `go.mod` says `module manvi` is not a
///   guess, it is how the toolchain resolves it.
/// - Python and JavaScript: top-level package directories — a directory
///   directly under the root containing `__init__.py`, or a `package.json`
///   `name`. Weaker than Go's, and labelled as the directory it came from.
///
/// Deliberately not included: matching on symbol names. Two repositories both
/// declaring `Client` is not a link, and asserting one would produce edges at a
/// rate that buries the real ones.
fn provided_modules(root: &std::path::Path) -> Vec<(String, String)> {
    let mut provided: Vec<(String, String)> = Vec::new();

    if let Ok(modules) = devmap_extract::collect_go_modules(root) {
        for module in modules {
            if module.prefix.is_empty() {
                continue;
            }
            let where_from = if module.dir.is_empty() {
                "go.mod".to_string()
            } else {
                format!("{}/go.mod", module.dir)
            };
            provided.push((
                module.prefix.clone(),
                format!("{where_from} declares `module {}`", module.prefix),
            ));
        }
    }

    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || name == "node_modules" || name == "target" {
                continue;
            }
            if entry.path().join("__init__.py").is_file() {
                provided.push((
                    name.clone(),
                    format!("{name}/__init__.py declares a Python package"),
                ));
            }
        }
    }
    // A `src/` layout puts the package one level down, which is where this
    // repository's own `devcouncil` package lives.
    if let Ok(entries) = std::fs::read_dir(root.join("src")) {
        for entry in entries.flatten() {
            if entry.path().join("__init__.py").is_file() {
                let name = entry.file_name().to_string_lossy().into_owned();
                provided.push((
                    name.clone(),
                    format!("src/{name}/__init__.py declares a Python package"),
                ));
            }
        }
    }

    provided.sort();
    provided.dedup();
    provided
}

/// Whether `specifier` is satisfied by a module named `prefix`.
///
/// Exact, or a path segment beneath it. `manvi/dc/store` is provided by
/// `manvi`; `manvibench` is not, and matching on a bare `starts_with` would
/// claim it is.
fn specifier_matches(specifier: &str, prefix: &str) -> bool {
    if specifier == prefix {
        return true;
    }
    specifier
        .strip_prefix(prefix)
        .is_some_and(|rest| rest.starts_with('/') || rest.starts_with('.'))
}

/// Imports in one repository that another repository declares the module for.
///
/// Reported as *candidates*. A matching module path is strong evidence — for Go
/// it is how the compiler resolves the import — but this does not verify that
/// the imported symbol exists in the target, and it cannot tell a local
/// checkout from a published copy at a different version. Calling these
/// resolved edges would put an unverified claim in the graph beside verified
/// ones.
pub fn link_candidates(
    workspace: &crate::workspace::Workspace,
) -> anyhow::Result<Vec<crate::workspace::LinkCandidate>> {
    use crate::workspace::LinkCandidate;

    // What each repository provides.
    let mut providers: Vec<(&str, Vec<(String, String)>)> = Vec::new();
    for repo in &workspace.repos {
        providers.push((repo.name.as_str(), provided_modules(&repo.root)));
    }

    let mut candidates: Vec<LinkCandidate> = Vec::new();
    for repo in &workspace.repos {
        let Ok(Some(store)) = devmap_store::Store::open_existing(repo.db_path()) else {
            continue;
        };
        let extractions = store.latest_extractions()?;
        for extraction in &extractions {
            for import in &extraction.imports {
                let specifier = import.module_specifier.trim();
                if specifier.is_empty() || specifier.starts_with('.') {
                    continue;
                }
                for (provider_name, provided) in &providers {
                    // A repository importing its own module is not a
                    // cross-repository link.
                    if *provider_name == repo.name {
                        continue;
                    }
                    for (prefix, evidence) in provided {
                        if specifier_matches(specifier, prefix) {
                            candidates.push(LinkCandidate {
                                from_repo: repo.name.clone(),
                                from_file: extraction.file_path.clone(),
                                module_specifier: specifier.to_string(),
                                to_repo: (*provider_name).to_string(),
                                evidence: evidence.clone(),
                            });
                        }
                    }
                }
            }
        }
    }
    candidates.sort_by(|a, b| {
        (&a.from_repo, &a.from_file, &a.module_specifier, &a.to_repo).cmp(&(
            &b.from_repo,
            &b.from_file,
            &b.module_specifier,
            &b.to_repo,
        ))
    });
    candidates.dedup_by(|a, b| {
        a.from_repo == b.from_repo
            && a.from_file == b.from_file
            && a.module_specifier == b.module_specifier
            && a.to_repo == b.to_repo
    });
    Ok(candidates)
}

#[cfg(test)]
mod specifier_tests {
    use super::specifier_matches;

    /// A module prefix matches its own path and anything beneath it — and
    /// nothing that merely starts with the same letters. `manvibench` sharing a
    /// prefix with `manvi` is not an import of it, and a bare `starts_with`
    /// would claim it is.
    #[test]
    fn a_prefix_matches_only_on_a_segment_boundary() {
        assert!(specifier_matches("example.com/libb", "example.com/libb"));
        assert!(specifier_matches(
            "example.com/libb/store",
            "example.com/libb"
        ));
        assert!(specifier_matches("devcouncil.app.config", "devcouncil"));

        assert!(!specifier_matches(
            "example.com/libbeta",
            "example.com/libb"
        ));
        assert!(!specifier_matches("manvibench", "manvi"));
        assert!(!specifier_matches("libb", "example.com/libb"));
    }
}

/// Token cost of one caller line: two paths and a symbol name.
const PREVIEW_CALLER_TOKENS: u32 = 25;

/// Confidence a call edge needs before `preview` will call it a caller.
///
/// The resolver publishes three tiers on this workspace's 50,533 call edges:
/// 1.0 (19,134 edges), 0.9 (4,800) and 0.2 (26,599). The 0.2 tier is
/// name-only attribution, and on a common method name it is not close to
/// right: every `dict.get(...)` in the tree resolves to
/// `LLMCache.get`, giving that one method 921 edges — all of them at 0.2, none
/// of them real. Listing those under "this change breaks" would send a reader
/// to `benchmarks/map_bench.py` to fix a call it does not contain.
///
/// 0.5 sits in the empty band between 0.2 and 0.9, so it separates the tiers
/// rather than cutting through one. Edges below it are counted, not discarded —
/// see `PreviewReport::ambiguous_callers`.
pub const PREVIEW_CALLER_MIN_CONFIDENCE: f32 = 0.5;

/// Describe a parse outcome in one word, for a report a human reads.
fn parse_status_name(outcome: &ParseOutcome) -> &'static str {
    match outcome {
        ParseOutcome::Clean => "clean",
        ParseOutcome::Partial { .. } => "partial",
        ParseOutcome::Fallback { .. } => "fallback",
        ParseOutcome::Failed { .. } => "failed",
    }
}

/// Parse a clone kind from a caller-supplied string.
///
/// `None` for an unrecognised name rather than a default, so a caller can
/// reject a typo instead of answering it with an unfiltered report.
pub fn parse_clone_kind(name: &str) -> Option<devmap_analyze::CloneKind> {
    match name {
        "exact" => Some(devmap_analyze::CloneKind::Exact),
        "structural" => Some(devmap_analyze::CloneKind::Structural),
        _ => None,
    }
}

/// Token cost of one clone group: a header line plus one line per member.
///
/// Public because a caller that filters groups has to re-take the budget over
/// what survives, and it must charge the same rate this engine did — two copies
/// of the arithmetic is how a response comes to report a token count it did not
/// spend.
pub fn clone_group_tokens(group: &devmap_analyze::CloneGroup) -> u32 {
    /// A group's header line.
    const HEADER_TOKENS: u32 = 12;
    /// One member line: a path, a symbol name, and a span.
    const MEMBER_TOKENS: u32 = 25;
    HEADER_TOKENS + group.members.len() as u32 * MEMBER_TOKENS
}

fn edge_node_matches(file: &str, symbol: &str, query: &str) -> bool {
    crate::query_match::traversal_start_matches(query, symbol, file)
}

/// Stable name of an edge kind, for tie-breaking.
///
/// The comparator used `format!("{:?}", kind)`, which allocated two `String`s
/// per comparison inside a sort over every edge in the generation. These are
/// the same names `Debug` derives, so the ordering is unchanged and no
/// allocation happens.
fn edge_kind_name(kind: EdgeKind) -> &'static str {
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

/// One node reached by the scoped-trace walk, and the edge that reached it.
///
/// The walk carries a parent pointer rather than a copy of the path so far.
/// Cloning the whole path once per *edge considered* made the walk quadratic in
/// its own output on top of being quadratic in the graph.
struct Reached {
    /// Index into the confidence-ordered edge list.
    edge: usize,
    /// The entry this one extends, or `None` for a first hop.
    parent: Option<usize>,
    node: (String, String),
    /// Number of edges from the origin, i.e. the length of the path to here.
    depth: usize,
}

/// Walk back from `entry` to the origin, producing the path in forward order.
fn path_to(reached: &[Reached], ordered: &[&ResolvedEdge], entry: usize) -> Vec<ResolvedEdge> {
    let mut path = Vec::with_capacity(reached[entry].depth);
    let mut cursor = Some(entry);
    while let Some(index) = cursor {
        path.push(ordered[reached[index].edge].clone());
        cursor = reached[index].parent;
    }
    path.reverse();
    path
}

/// One deterministic shortest path from `from` to `to`, breadth-first.
///
/// The graph is indexed by source node once, up front. The previous
/// implementation filtered the *entire* edge list for every node it dequeued,
/// so a trace across a repository-sized generation cost `frontier × edges` —
/// with the frontier bounded at 5,000 and generations running to tens of
/// thousands of edges, that is ~10^8 string comparisons per request, each one
/// also cloning the path built so far. Indexed, the walk touches each edge at
/// most once and the whole call is `O(E log E)`, dominated by the ordering
/// sort.
///
/// Ordering, and therefore *which* shortest path is returned, is unchanged:
/// edges are considered in descending confidence with a total tie-break, and
/// the frontier is explored in the same first-in-first-out order.
fn shortest_path(
    edges: &[ResolvedEdge],
    from: &str,
    to: &str,
    max_depth: usize,
    max_nodes: usize,
    cancel: &Cancel,
) -> Result<Option<Vec<ResolvedEdge>>, QueryCancelled> {
    if max_depth == 0 || max_nodes == 0 {
        return Ok(None);
    }
    let mut ordered: Vec<&ResolvedEdge> = edges.iter().collect();
    ordered.sort_by(|a, b| {
        b.confidence
            .0
            .total_cmp(&a.confidence.0)
            .then_with(|| a.source_file.cmp(&b.source_file))
            .then_with(|| a.source_symbol.cmp(&b.source_symbol))
            .then_with(|| a.target_file.cmp(&b.target_file))
            .then_with(|| a.target_symbol.cmp(&b.target_symbol))
            .then_with(|| edge_kind_name(a.edge_kind).cmp(edge_kind_name(b.edge_kind)))
    });

    // Source node -> its outgoing edges, in the order above. Built once; every
    // expansion below is a map lookup instead of a scan of the whole graph.
    let mut outgoing: BTreeMap<(String, String), Vec<usize>> = BTreeMap::new();
    for (index, edge) in ordered.iter().enumerate() {
        cancel.check_every(index)?;
        outgoing
            .entry((edge.source_file.clone(), edge.source_symbol.clone()))
            .or_default()
            .push(index);
    }

    let mut reached: Vec<Reached> = Vec::new();
    let mut queue: VecDeque<usize> = VecDeque::new();
    let mut visited: BTreeSet<(String, String)> = BTreeSet::new();

    for (index, edge) in ordered.iter().enumerate() {
        cancel.check_every(index)?;
        if !edge_node_matches(&edge.source_file, &edge.source_symbol, from) {
            continue;
        }
        let node = (edge.target_file.clone(), edge.target_symbol.clone());
        if edge_node_matches(&node.0, &node.1, to) {
            return Ok(Some(vec![(*edge).clone()]));
        }
        if visited.len() >= max_nodes {
            break;
        }
        if visited.insert(node.clone()) {
            reached.push(Reached {
                edge: index,
                parent: None,
                node,
                depth: 1,
            });
            queue.push_back(reached.len() - 1);
        }
    }

    let mut dequeued = 0usize;
    while let Some(entry) = queue.pop_front() {
        cancel.check_every(dequeued)?;
        dequeued += 1;
        let depth = reached[entry].depth;
        if depth >= max_depth {
            continue;
        }
        let Some(candidates) = outgoing.get(&reached[entry].node) else {
            continue;
        };
        // Copied so the borrow of `reached` ends before it is extended below;
        // an adjacency list is a handful of entries, not the whole graph.
        let candidates = candidates.clone();
        for index in candidates {
            let edge = ordered[index];
            let next = (edge.target_file.clone(), edge.target_symbol.clone());
            if edge_node_matches(&next.0, &next.1, to) {
                reached.push(Reached {
                    edge: index,
                    parent: Some(entry),
                    node: next,
                    depth: depth + 1,
                });
                return Ok(Some(path_to(&reached, &ordered, reached.len() - 1)));
            }
            if visited.len() >= max_nodes {
                return Ok(None);
            }
            if visited.insert(next.clone()) {
                reached.push(Reached {
                    edge: index,
                    parent: Some(entry),
                    node: next,
                    depth: depth + 1,
                });
                queue.push_back(reached.len() - 1);
            }
        }
    }
    Ok(None)
}

/// A caller-supplied path that does not resolve inside the indexed repository.
///
/// A distinct type rather than a bare `anyhow!` so the IPC layer can answer it
/// as a rejected *parameter* instead of an internal failure: the caller asked
/// for something it is not allowed to ask for, and "the request was invalid" is
/// a different fact from "the query broke".
#[derive(Debug)]
pub struct PathOutsideRepoRoot {
    /// The path exactly as the caller supplied it.
    pub requested: String,
    /// Why it was refused.
    pub reason: String,
}

impl std::fmt::Display for PathOutsideRepoRoot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{:?} {}", self.requested, self.reason)
    }
}

impl std::error::Error for PathOutsideRepoRoot {}

/// Resolve a caller-supplied path against the indexed repository root, or
/// refuse it.
///
/// `preview` is the one query that reads a file the *caller* names, and the
/// name arrives over IPC from any process that can reach the socket. Joined
/// onto the root unchecked, `../../etc/passwd` reads outside the repository;
/// used verbatim when absolute, any path at all does. What came back was not a
/// diff but a listing of every symbol and signature in the target file.
///
/// The rule is `daemon.rs::collect_pending_path`'s, which already guards
/// watcher paths, applied to the same question:
///
/// 1. A `..` component is refused outright — a repository-relative path never
///    needs one, and normalising it away would silently accept the escape.
/// 2. An absolute path is accepted only when it lies under the root, and is
///    then treated as the relative path it denotes. This is the daemon's own
///    convention for watcher paths, so refusing it outright would split the
///    two surfaces.
/// 3. The resolved candidate must be lexically under the root, and — when it
///    exists — its *canonical* form must be too, which is what catches a
///    symlink whose every component sits inside the repository.
///
/// With no recorded root (a pre-v7 generation, or an in-memory store) there is
/// nothing to contain against, so an absolute path cannot be shown to be
/// inside the repository and is refused. A relative path is resolved against
/// the process's working directory exactly as before, which rule 1 keeps from
/// climbing out of it.
pub(crate) fn contained_repo_path(
    repo_root: Option<&str>,
    path: &str,
) -> Result<PathBuf, PathOutsideRepoRoot> {
    let refuse = |reason: &str| PathOutsideRepoRoot {
        requested: path.to_string(),
        reason: reason.to_string(),
    };
    if path.is_empty() {
        return Err(refuse("is empty"));
    }
    let raw = Path::new(path);
    if raw
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(refuse("contains a parent traversal component"));
    }

    let Some(root) = repo_root else {
        if raw.is_absolute() {
            return Err(refuse(
                "is absolute, and this generation records no repository root to \
                 contain it within",
            ));
        }
        return Ok(PathBuf::from(path));
    };
    let root = Path::new(root);

    let candidate = if raw.is_absolute() {
        match raw.strip_prefix(root) {
            Ok(relative) => root.join(relative),
            Err(_) => return Err(refuse("is outside the indexed repository root")),
        }
    } else {
        root.join(raw)
    };
    if !candidate.starts_with(root) {
        return Err(refuse("is outside the indexed repository root"));
    }

    // Only a path that exists can be canonicalized, and a preview of a file
    // that does not exist yet is a legitimate request — it is how a new file is
    // previewed. Rules 1 and 3 already bound where a non-existent path could
    // point.
    if candidate.exists() {
        let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        match candidate.canonicalize() {
            Ok(canonical) if !canonical.starts_with(&canonical_root) => {
                return Err(refuse("resolves outside the indexed repository root"));
            }
            Ok(_) => {}
            Err(_) => return Err(refuse("could not be resolved for containment checking")),
        }
    }
    Ok(candidate)
}

/// Stored node paths are repo-relative. Resolving them against the recorded
/// build root — rather than the query process's working directory — is what
/// lets `devmap search` return real source spans from anywhere on the machine.
/// With no recorded root the relative path is used unchanged, which keeps the
/// pre-v7 behaviour for generations built before the root was captured.
pub(crate) fn resolve_source_path(repo_root: &Option<String>, path: &str) -> std::path::PathBuf {
    match repo_root {
        Some(root) => std::path::Path::new(root).join(path),
        None => std::path::PathBuf::from(path),
    }
}

pub fn resolved_edge_from_stored(edge: StoredEdge) -> anyhow::Result<ResolvedEdge> {
    stored_edge_to_resolved(edge)
}

fn stored_edge_to_resolved(edge: StoredEdge) -> anyhow::Result<ResolvedEdge> {
    let edge_kind = match edge.edge_kind.as_str() {
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
        other => anyhow::bail!("stored generation has unknown edge kind {other:?}"),
    };
    Ok(ResolvedEdge {
        source_file: edge.source_file,
        target_file: edge.target_file,
        source_symbol: edge.source_symbol,
        target_symbol: edge.target_symbol,
        edge_kind,
        confidence: Confidence(edge.confidence),
        resolution: None,
        details: None,
    })
}

impl<'a> QueryEngine<'a> {
    pub fn new(extractions: &'a [Extraction], resolution: &'a ResolutionResult) -> Self {
        Self {
            extractions,
            resolution,
        }
    }

    pub fn search(&self, req: Request<String>) -> Response<SymbolHit> {
        if req.query.trim().is_empty() {
            return Response {
                items: Vec::new(),
                shown: 0,
                hidden: 0,
                total: 0,
                truncated: false,
                tokens_used: 0,
                resolution: ResolutionAvailability::Available,
            };
        }
        let q_lower = req.query.to_lowercase();
        let mut hits = Vec::new();

        for ext in self.extractions {
            for sym in &ext.symbols {
                let name_l = sym.name.to_lowercase();
                let qn_l = sym.qualified_name.to_lowercase();
                if !(name_l.contains(&q_lower) || qn_l.contains(&q_lower)) {
                    continue;
                }
                let score = if name_l == q_lower || qn_l == q_lower {
                    1.0
                } else if name_l.starts_with(&q_lower) {
                    0.95
                } else {
                    0.8
                };
                let disk_content = if ext.source_code.is_none() {
                    // In-memory engine: extractions are supplied by the caller,
                    // who is already running at the repo root, so the stored
                    // relative path is correct here.
                    std::fs::read_to_string(&ext.file_path).ok()
                } else {
                    None
                };
                let source_unavailable_reason = (ext.source_code.is_none()
                    && disk_content.is_none())
                .then(|| format!("source unavailable at query time for {:?}", ext.file_path));
                let code_str = ext
                    .source_code
                    .as_deref()
                    .or(disk_content.as_deref())
                    .unwrap_or("");
                let source_span = code_str
                    .get(sym.span.start_byte..sym.span.end_byte)
                    .unwrap_or("")
                    .to_string();
                let line_span = byte_span_to_line_range(code_str, &sym.span);
                // Capped for the same reason as the store-backed search above:
                // this engine shares the cost function, so it shares the bug.
                let (source_span, source_span_omitted_bytes) =
                    cap_source_span(source_span, req.token_budget);

                hits.push(SymbolHit {
                    symbol_name: sym.name.clone(),
                    file_path: ext.file_path.clone(),
                    kind: format!("{:?}", sym.kind),
                    span: line_span,
                    source_span,
                    source_unavailable_reason,
                    source_span_omitted_bytes,
                    score,
                });
            }
        }

        // Rank before truncating (T1–T4).
        hits.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.file_path.cmp(&b.file_path))
                .then_with(|| a.symbol_name.cmp(&b.symbol_name))
                .then_with(|| a.span.cmp(&b.span))
        });

        budget_take(hits, req.token_budget, |hit| {
            u32::try_from(hit.source_span.len() / 4)
                .unwrap_or(u32::MAX)
                .saturating_add(20)
        })
    }

    pub fn dependencies(&self, req: Request<String>) -> Response<ResolvedEdge> {
        let file_path = &req.query;
        let availability = match self
            .extractions
            .iter()
            .find(|extraction| &extraction.file_path == file_path)
        {
            None => ResolutionAvailability::Unavailable {
                reason: format!("{file_path} is not indexed"),
            },
            Some(extraction) if matches!(extraction.parse_outcome, ParseOutcome::Failed { .. }) => {
                ResolutionAvailability::Unavailable {
                    reason: format!("{file_path} could not be parsed"),
                }
            }
            Some(_) => ResolutionAvailability::Available,
        };
        if !matches!(availability, ResolutionAvailability::Available) {
            return unavailable_response(availability);
        }
        let mut deps = Vec::new();

        for edge in &self.resolution.edges {
            if (&edge.source_file == file_path || &edge.target_file == file_path)
                && edge.confidence.0 >= req.min_confidence
            {
                deps.push(edge.clone());
            }
        }

        deps.sort_by(|a, b| {
            b.confidence
                .0
                .total_cmp(&a.confidence.0)
                .then_with(|| a.source_file.cmp(&b.source_file))
                .then_with(|| a.target_file.cmp(&b.target_file))
                .then_with(|| a.target_symbol.cmp(&b.target_symbol))
        });

        budget_take(deps, req.token_budget, |_| 25)
    }

    /// Inbound blast radius (impact) with parametric depth (closes G8).
    pub fn impact(&self, req: Request<String>) -> Response<ResolvedEdge> {
        let target = req.query.trim();
        let start: Vec<String> = self
            .resolution
            .edges
            .iter()
            .filter(|edge| {
                crate::query_match::traversal_start_matches(
                    target,
                    &edge.target_symbol,
                    &edge.target_file,
                )
            })
            .map(|edge| edge.target_symbol.clone())
            .collect();
        if start.is_empty() {
            return unavailable_response(ResolutionAvailability::Unavailable {
                reason: format!("{target} has no indexed inbound target"),
            });
        }
        let opts = TraversalOptions {
            max_depth: req.max_depth,
            max_nodes: 5000,
            reverse: true,
        };
        let walk = traverse_graph(&start, &self.resolution.edges, &opts);
        let mut inbound =
            traversed_resolution_edges(&walk, &self.resolution.edges, req.min_confidence);
        inbound.sort_by(|a, b| {
            b.confidence
                .0
                .total_cmp(&a.confidence.0)
                .then_with(|| a.source_file.cmp(&b.source_file))
                .then_with(|| a.target_file.cmp(&b.target_file))
                .then_with(|| a.source_symbol.cmp(&b.source_symbol))
        });
        budget_take(inbound, req.token_budget, |_| 25)
    }

    /// Outbound trace with parametric depth (closes G8).
    pub fn trace(&self, req: Request<String>) -> Response<ResolvedEdge> {
        let target = req.query.trim();
        let start: Vec<String> = self
            .resolution
            .edges
            .iter()
            .filter(|edge| {
                crate::query_match::traversal_start_matches(
                    target,
                    &edge.source_symbol,
                    &edge.source_file,
                )
            })
            .map(|edge| edge.source_symbol.clone())
            .collect();
        if start.is_empty() {
            return unavailable_response(ResolutionAvailability::Unavailable {
                reason: format!("{target} has no indexed outbound source"),
            });
        }
        let opts = TraversalOptions {
            max_depth: req.max_depth,
            max_nodes: 5000,
            reverse: false,
        };
        let walk = traverse_graph(&start, &self.resolution.edges, &opts);
        let mut outbound =
            traversed_resolution_edges(&walk, &self.resolution.edges, req.min_confidence);
        outbound.sort_by(|a, b| {
            b.confidence
                .0
                .total_cmp(&a.confidence.0)
                .then_with(|| a.source_file.cmp(&b.source_file))
                .then_with(|| a.target_file.cmp(&b.target_file))
                .then_with(|| a.source_symbol.cmp(&b.source_symbol))
        });
        budget_take(outbound, req.token_budget, |_| 25)
    }
}

fn traversed_resolution_edges(
    traversal: &devmap_analyze::traversal::TraversalResult,
    edges: &[ResolvedEdge],
    min_confidence: f32,
) -> Vec<ResolvedEdge> {
    let traversed: std::collections::BTreeSet<(String, String, String)> = traversal
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

fn unavailable_response<T>(resolution: ResolutionAvailability) -> Response<T> {
    Response {
        items: Vec::new(),
        shown: 0,
        hidden: 0,
        total: 0,
        truncated: false,
        tokens_used: 0,
        resolution,
    }
}

pub(crate) fn byte_span_to_line_range(source: &str, span: &Span) -> (u32, u32) {
    let start = span.start_byte.min(source.len());
    let end = span.end_byte.min(source.len()).max(start);
    let start_line = source[..start]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count() as u32
        + 1;
    let end_line = source[..end].bytes().filter(|byte| *byte == b'\n').count() as u32 + 1;
    (start_line, end_line)
}

/// Per-hit token overhead in [`StoreQueryEngine::search`]'s cost function.
/// Kept next to [`cap_source_span`] because the cap must invert the same
/// arithmetic the packer uses, and a drift between the two reintroduces the
/// oversized-hit bug in a form no test names.
const SEARCH_HIT_OVERHEAD_TOKENS: u32 = 20;

/// Bytes of source per token, matching the `len / 4` estimate in the search
/// cost function.
pub const BYTES_PER_TOKEN: u32 = 4;

/// How many ranked candidates a budget could conceivably show.
///
/// A hit costs at least [`SEARCH_HIT_OVERHEAD_TOKENS`], so `budget / overhead`
/// bounds how many can fit; the `+ 1` keeps the page from cutting a hit the
/// packer would still have admitted, and the floor of 1 keeps a zero budget
/// from asking for an empty page and reporting "nothing matched".
///
/// Both search paths use this. Keyword search pages the FTS query with it;
/// semantic search bounds how far down its ranking it materialises hits — and
/// therefore how many files it reads — before budgeting them. Two copies of the
/// arithmetic would let the same budget mean different page sizes depending on
/// which command asked.
fn budget_page_size(token_budget: u32) -> usize {
    (token_budget / SEARCH_HIT_OVERHEAD_TOKENS)
        .saturating_add(1)
        .max(1) as usize
}

/// Token cost of one search hit: its source span plus a fixed per-row overhead.
///
/// Shared by keyword and semantic search so the two spend the budget at the
/// same rate; two copies of this arithmetic would let the same result cost
/// different amounts depending on which command asked for it.
fn search_hit_tokens(hit: &SymbolHit) -> u32 {
    u32::try_from(hit.source_span.len() / BYTES_PER_TOKEN as usize)
        .unwrap_or(u32::MAX)
        .saturating_add(SEARCH_HIT_OVERHEAD_TOKENS)
}

#[cfg(test)]
thread_local! {
    /// Source-span reads attempted on this thread.
    ///
    /// Reading a file per scored symbol is the cost `search_semantic` used to
    /// pay for the entire corpus before the budget was applied, and "how many
    /// files did this query open" is not observable from the response. Counted
    /// per thread rather than globally so tests running in parallel in one
    /// binary cannot contaminate each other's count.
    pub(crate) static SOURCE_SPAN_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Build a hit from a stored symbol row, reading its source span from disk.
///
/// One owner for the read, the line-range conversion, the span cap and the
/// unavailability reason. When the file cannot be read the reason is recorded
/// on the hit rather than dropped, so an empty `source_span` is never mistaken
/// for a symbol with no body.
fn hit_from_stored(
    row: devmap_store::StoredSymbol,
    repo_root: Option<&str>,
    token_budget: u32,
    score: f32,
) -> SymbolHit {
    let owned_root = repo_root.map(str::to_string);
    #[cfg(test)]
    SOURCE_SPAN_READS.with(|reads| reads.set(reads.get().saturating_add(1)));
    let source_result = std::fs::read_to_string(resolve_source_path(&owned_root, &row.path));
    let source_unavailable_reason = source_result.as_ref().err().map(|error| {
        format!(
            "source unavailable at query time for {:?}: {error}",
            row.path
        )
    });
    let source = source_result.ok();
    let source_span = source
        .as_deref()
        .and_then(|text| text.get(row.span_start..row.span_end))
        .unwrap_or("")
        .to_string();
    let span = source
        .as_deref()
        .map(|text| {
            Span {
                start_byte: row.span_start,
                end_byte: row.span_end,
            }
            .line_range(text)
        })
        .unwrap_or((0, 0));
    let (source_span, source_span_omitted_bytes) = cap_source_span(source_span, token_budget);
    SymbolHit {
        symbol_name: row.name,
        file_path: row.path,
        kind: row.kind,
        span,
        source_span,
        source_unavailable_reason,
        source_span_omitted_bytes,
        score,
    }
}

/// Cap a hit's source span so one hit can never exceed the whole token budget.
///
/// A search hit costs `source_span.len() / 4 + 20` tokens, and `source_span` is
/// the symbol's entire body. One 8 KB function therefore outweighed the 2,000
/// token default on its own, and the caller enforces the budget as a hard
/// contract — `DevMapClient._budgeted` raises on an over-budget response — so
/// an uncapped hit is not merely large, it is unreturnable. `devmap search
/// "resolve calls"` on this repository matched exactly one symbol,
/// `resolve_calls`, and answered with nothing.
///
/// Returns the (possibly capped) span and the number of bytes dropped, which
/// the caller records in `source_span_omitted_bytes`. A capped span is never
/// passed off as the verbatim body R2 promises.
/// Largest share of a request's budget one hit's source span may take.
///
/// Capping at the *whole* budget — which this did — is enough to keep a single
/// oversized item from being withheld, but it lets that item crowd out every
/// other result. A `File` symbol's span is its entire file, so a search whose
/// best matches are files returned two hits against a 4,000-token budget and
/// reported 512 more withheld. A quarter guarantees at least three results
/// survive alongside any one of them.
const MAX_HIT_BUDGET_SHARE: u32 = 4;

fn cap_source_span(source_span: String, token_budget: u32) -> (String, Option<u32>) {
    let max_bytes = (token_budget / MAX_HIT_BUDGET_SHARE)
        .saturating_sub(SEARCH_HIT_OVERHEAD_TOKENS)
        .saturating_mul(BYTES_PER_TOKEN) as usize;
    if source_span.len() <= max_bytes {
        return (source_span, None);
    }
    // Truncate on a char boundary: `String` slicing panics mid-codepoint, and
    // source files contain non-ASCII in strings, comments and identifiers.
    let mut end = max_bytes;
    while end > 0 && !source_span.is_char_boundary(end) {
        end -= 1;
    }
    let omitted = u32::try_from(source_span.len() - end).unwrap_or(u32::MAX);
    let mut capped = source_span;
    capped.truncate(end);
    (capped, Some(omitted))
}

pub fn budget_take<T, F>(items: Vec<T>, token_budget: u32, cost_of: F) -> Response<T>
where
    F: Fn(&T) -> u32,
{
    let total = items.len() as u32;
    let mut out = Vec::new();
    let mut current_tokens = 0u32;
    let mut truncated = false;

    // The budget is hard: an item that does not fit is withheld, never emitted
    // over budget. `test_search_never_exceeds_hard_token_budget` pins this, and
    // `DevMapClient._budgeted` raises on any response that breaks it, so a
    // packer that "made progress" by exceeding the budget would turn a thin
    // result into a client-side error.
    //
    // Which is why an oversized *item* is bounded where it is built rather than
    // waved through here — see `cap_source_span`.
    for item in items {
        let cost = cost_of(&item);
        if cost > token_budget.saturating_sub(current_tokens) {
            truncated = true;
            break;
        }
        current_tokens += cost;
        out.push(item);
    }

    Response {
        shown: out.len() as u32,
        hidden: total.saturating_sub(out.len() as u32),
        total,
        truncated,
        tokens_used: current_tokens,
        items: out,
        resolution: ResolutionAvailability::Available,
    }
}

fn atomic_budget_take<T, F>(items: Vec<T>, token_budget: u32, cost_of: F) -> Response<T>
where
    F: Fn(&T) -> u32,
{
    let total = u32::try_from(items.len()).unwrap_or(u32::MAX);
    let required = items
        .iter()
        .fold(0u32, |sum, item| sum.saturating_add(cost_of(item)));
    if required > token_budget {
        return Response {
            items: Vec::new(),
            shown: 0,
            hidden: total,
            total,
            truncated: total > 0,
            tokens_used: 0,
            resolution: ResolutionAvailability::Available,
        };
    }
    Response {
        shown: total,
        hidden: 0,
        total,
        truncated: false,
        tokens_used: required,
        items,
        resolution: ResolutionAvailability::Available,
    }
}

#[cfg(all(test, feature = "parse"))]
mod tests {
    use super::*;
    use devmap_extract::extract_file;
    use devmap_resolve::Resolver;

    /// A symbol too large for the budget is returned capped, and says so.
    ///
    /// The whole point: a hit costs `len / 4 + 20` tokens against a 2,000-token
    /// default, so one 8 KB function was unreturnable — the packer dropped it
    /// and `DevMapClient._budgeted` would reject it even if the packer had not.
    /// The cap must leave the hit inside the budget *and* record what it
    /// dropped, because `source_span` is contractually the verbatim body.
    #[test]
    fn an_oversized_source_span_is_capped_within_budget_and_reports_the_omission() {
        let budget = 2_000u32;
        let huge = "x".repeat(40_000);
        let (capped, omitted) = cap_source_span(huge.clone(), budget);

        let cost = (capped.len() as u32) / BYTES_PER_TOKEN + SEARCH_HIT_OVERHEAD_TOKENS;
        assert!(
            cost <= budget,
            "capped hit still costs {cost} tokens against a {budget} budget"
        );
        let omitted = omitted.expect("a capped span must report what it dropped");
        assert_eq!(
            capped.len() as u32 + omitted,
            huge.len() as u32,
            "kept + omitted must account for every byte of the original"
        );
    }

    /// A span that already fits is returned untouched and unmarked.
    ///
    /// `source_span_omitted_bytes` must mean "this was capped" and nothing
    /// else; a `Some(0)` on every hit would make the signal useless.
    #[test]
    fn a_span_within_budget_is_left_verbatim() {
        let small = "def f():\n    return 1\n".to_string();
        let (kept, omitted) = cap_source_span(small.clone(), 2_000);
        assert_eq!(kept, small);
        assert_eq!(omitted, None);
    }

    /// Capping never splits a UTF-8 codepoint.
    ///
    /// `String::truncate` panics on a non-boundary index, so a source file with
    /// non-ASCII in a comment or string literal would crash the query rather
    /// than answer it.
    #[test]
    fn capping_a_span_full_of_multibyte_characters_does_not_panic() {
        // 4-byte codepoints, so most byte offsets are not char boundaries.
        let emoji_source = "🦀".repeat(4_000);
        let (capped, omitted) = cap_source_span(emoji_source.clone(), 200);
        assert!(capped.len() < emoji_source.len());
        assert!(omitted.is_some());
        // Round-trips as valid UTF-8 precisely because it stopped on a boundary.
        assert!(capped.chars().all(|c| c == '🦀'));
    }

    #[test]
    fn search_reports_shown_and_total_when_truncated() {
        let mut src = String::new();
        for i in 0..50 {
            src.push_str(&format!("def fn_{i}():\n    return {i}\n"));
        }
        let ext = extract_file("mod.py", &src);
        let mut resolver = Resolver::new();
        resolver.index_extractions(std::slice::from_ref(&ext));
        let resolution = resolver.resolve_all(std::slice::from_ref(&ext));
        let exts = [ext];
        let engine = QueryEngine::new(&exts, &resolution);
        let resp = engine.search(Request {
            query: "fn_".into(),
            token_budget: 80,
            min_confidence: 0.0,
            max_depth: 1,
        });
        assert!(resp.total > resp.shown);
        assert!(resp.truncated);
        assert_eq!(resp.shown, resp.items.len() as u32);
    }

    #[test]
    fn persisted_engine_answers_without_reextracting_sources() {
        use devmap_analyze::analyze;
        use devmap_store::Store;

        let target = extract_file("missing/target.py", "def target():\n    return 1\n");
        let caller = extract_file(
            "missing/caller.py",
            "from target import target\n\ndef caller():\n    return target()\n",
        );
        let mut resolver = Resolver::new();
        resolver.index_extractions(&[target.clone(), caller.clone()]);
        let resolution = resolver.resolve_all(&[target.clone(), caller.clone()]);
        let analysis = analyze(&[target.clone(), caller.clone()], &resolution);
        let store = Store::open_in_memory().unwrap();
        store
            .save_generation(&[target, caller], &resolution, &analysis)
            .unwrap();

        let engine = StoreQueryEngine::new(&store);
        let search = engine
            .search(Request {
                query: "target".into(),
                token_budget: 2_000,
                min_confidence: 0.0,
                max_depth: 1,
            })
            .unwrap();
        assert!(search
            .items
            .iter()
            .any(|item| item.file_path == "missing/target.py"));
        let target_hit = search
            .items
            .iter()
            .find(|item| item.file_path == "missing/target.py")
            .expect("persisted target hit");
        assert!(target_hit.source_span.is_empty());
        assert!(target_hit.source_unavailable_reason.is_some());

        let deps = engine
            .dependencies(Request {
                query: "missing/caller.py".into(),
                token_budget: 2_000,
                min_confidence: 0.0,
                max_depth: 1,
            })
            .unwrap();
        assert!(matches!(deps.resolution, ResolutionAvailability::Available));
        assert!(deps.items.iter().any(|edge| {
            edge.source_file == "missing/caller.py" && edge.target_file == "missing/target.py"
        }));
    }

    #[test]
    fn persisted_engine_preserves_unavailable_and_dead_states() {
        use devmap_analyze::analyze;
        use devmap_store::Store;

        let ext = extract_file("dead.py", "def abandoned():\n    return 1\n");
        let mut resolver = Resolver::new();
        resolver.index_extractions(std::slice::from_ref(&ext));
        let resolution = resolver.resolve_all(std::slice::from_ref(&ext));
        let analysis = analyze(std::slice::from_ref(&ext), &resolution);
        let store = Store::open_in_memory().unwrap();
        store
            .save_generation(std::slice::from_ref(&ext), &resolution, &analysis)
            .unwrap();

        let engine = StoreQueryEngine::new(&store);
        let missing = engine
            .dependencies(Request {
                query: "absent.py".into(),
                token_budget: 2_000,
                min_confidence: 0.0,
                max_depth: 1,
            })
            .unwrap();
        assert!(matches!(
            missing.resolution,
            ResolutionAvailability::Unavailable { .. }
        ));
        let dead = engine.dead_symbols(2_000).unwrap();
        assert!(dead
            .items
            .iter()
            .any(|item| item.symbol_name == "abandoned"));
    }

    fn path_edge(source: &str, target: &str, confidence: f32) -> ResolvedEdge {
        ResolvedEdge {
            source_file: format!("{source}.py"),
            target_file: format!("{target}.py"),
            source_symbol: source.to_string(),
            target_symbol: target.to_string(),
            edge_kind: EdgeKind::Calls,
            confidence: Confidence(confidence),
            resolution: None,
            details: None,
        }
    }

    #[test]
    fn scoped_trace_is_shortest_bounded_and_deterministic() {
        let edges = vec![
            path_edge("a", "b", 0.7),
            path_edge("b", "d", 0.7),
            path_edge("a", "c", 0.9),
            path_edge("c", "d", 0.9),
            path_edge("d", "a", 1.0),
        ];
        let path = shortest_path(&edges, "a", "d", 2, 5_000, &Cancel::new())
            .expect("uncancelled")
            .expect("two-hop path");
        assert_eq!(
            path.iter()
                .map(|edge| edge.target_symbol.as_str())
                .collect::<Vec<_>>(),
            ["c", "d"]
        );
        assert!(shortest_path(&edges, "a", "d", 1, 5_000, &Cancel::new())
            .expect("uncancelled")
            .is_none());

        let mut with_direct = edges;
        with_direct.push(path_edge("a", "d", 0.1));
        let path = shortest_path(&with_direct, "a", "d", 2, 5_000, &Cancel::new())
            .expect("uncancelled")
            .expect("direct path");
        assert_eq!(
            path.len(),
            1,
            "edge count, not confidence, defines shortest"
        );
        assert_eq!(path[0].target_symbol, "d");
    }

    /// A scoped trace over a real-sized graph must finish in a moment.
    ///
    /// The search rescanned *every* edge for every node it dequeued and cloned
    /// the whole path vector once per edge, so the cost was
    /// `frontier × edges`. This fixture is shaped to reach both production
    /// bounds at once — 64 levels deep (`max_depth`) and just under the
    /// 5,000-node frontier (`max_nodes`) — across ~20,000 edges, which is
    /// 4,900 × 19,656 ≈ 96 million string comparisons. Measured pre-fix: 5.9 s.
    /// An adjacency index built once, plus parent pointers instead of path
    /// clones, makes the walk linear in the edges: ~40 ms.
    ///
    /// One second is deliberately loose — it must not flake on a loaded machine
    /// — and still an order of magnitude below the behaviour it guards against.
    #[test]
    fn a_scoped_trace_over_twenty_thousand_edges_finishes_promptly() {
        const LEVELS: usize = 64;
        const WIDTH: usize = 78;
        const FANOUT: usize = 4;

        // A layered DAG: every node points at four nodes one level down. Wide
        // enough to fill the frontier, shallow enough that the depth bound
        // never cuts the walk short, and acyclic so the node count is exact.
        let mut edges = Vec::with_capacity((LEVELS - 1) * WIDTH * FANOUT);
        for level in 0..LEVELS - 1 {
            for column in 0..WIDTH {
                for step in 0..FANOUT {
                    let next = (column * FANOUT + step) % WIDTH;
                    edges.push(path_edge(
                        &format!("L{level:02}W{column:03}"),
                        &format!("L{:02}W{next:03}", level + 1),
                        0.9,
                    ));
                }
            }
        }
        assert_eq!(edges.len(), (LEVELS - 1) * WIDTH * FANOUT);

        // A destination no node carries, so the search must exhaust the
        // frontier instead of returning on an early hit.
        let started = std::time::Instant::now();
        let found = shortest_path(
            &edges,
            "L00W000",
            "absent_destination",
            LEVELS,
            5_000,
            &Cancel::new(),
        )
        .expect("uncancelled");
        let elapsed = started.elapsed();

        assert!(found.is_none(), "the destination is not in the graph");
        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "exhausting a {}-edge graph took {elapsed:?}; the search is rescanning \
             every edge per dequeued node",
            edges.len()
        );
    }

    /// The index must not change which path is returned. Same fixture as
    /// `scoped_trace_is_shortest_bounded_and_deterministic`, asserted across
    /// repeated runs so a map iteration order could not make it wobble.
    #[test]
    fn the_scoped_trace_path_is_identical_across_runs() {
        let edges = vec![
            path_edge("a", "b", 0.7),
            path_edge("b", "d", 0.7),
            path_edge("a", "c", 0.9),
            path_edge("c", "d", 0.9),
            path_edge("d", "a", 1.0),
        ];
        let first = shortest_path(&edges, "a", "d", 2, 5_000, &Cancel::new())
            .expect("uncancelled")
            .expect("two-hop path");
        for _ in 0..8 {
            let again = shortest_path(&edges, "a", "d", 2, 5_000, &Cancel::new())
                .expect("uncancelled")
                .expect("two-hop path");
            assert_eq!(
                again
                    .iter()
                    .map(|edge| (edge.source_symbol.clone(), edge.target_symbol.clone()))
                    .collect::<Vec<_>>(),
                first
                    .iter()
                    .map(|edge| (edge.source_symbol.clone(), edge.target_symbol.clone()))
                    .collect::<Vec<_>>(),
                "the scoped trace answered differently on a repeat run"
            );
        }
    }

    /// Semantic search must not read the whole corpus to answer with a page of
    /// it.
    ///
    /// Every scored symbol was materialised into a `SymbolHit` — one
    /// `read_to_string` each — and only then handed to the budget, so a query
    /// matching a common term opened every file it matched in order to throw
    /// almost all of them away. Scoring already yields a ranked list, so the
    /// bound is the same page size keyword search uses: a hit costs at least
    /// `SEARCH_HIT_OVERHEAD_TOKENS`, so no more than `budget / overhead` of them
    /// can ever be shown.
    #[test]
    fn semantic_search_reads_only_as_many_files_as_the_budget_could_show() {
        use devmap_analyze::analyze;
        use devmap_store::Store;

        const SYMBOLS: usize = 500;
        const BUDGET: u32 = 200;

        let mut source = String::new();
        for index in 0..SYMBOLS {
            // `widget_NNNN` tokenizes to `widget` + `NNNN`, so every symbol
            // shares the query's single term and the whole corpus scores.
            source.push_str(&format!("def widget_{index:04}():\n    return {index}\n"));
        }
        let ext = extract_file("things.py", &source);
        let mut resolver = Resolver::new();
        resolver.index_extractions(std::slice::from_ref(&ext));
        let resolution = resolver.resolve_all(std::slice::from_ref(&ext));
        let analysis = analyze(std::slice::from_ref(&ext), &resolution);
        let store = Store::open_in_memory().unwrap();
        store
            .save_generation(std::slice::from_ref(&ext), &resolution, &analysis)
            .unwrap();

        SOURCE_SPAN_READS.with(|reads| reads.set(0));
        let response = StoreQueryEngine::new(&store)
            .search_semantic("widget", BUDGET)
            .expect("semantic search");
        let reads = SOURCE_SPAN_READS.with(|reads| reads.get());

        assert_eq!(
            response.total, SYMBOLS as u32,
            "every scored symbol must still be counted in `total`"
        );
        assert!(
            response.shown > 0,
            "a {BUDGET}-token budget must show something"
        );
        assert!(
            response.truncated && response.hidden == response.total - response.shown,
            "the withheld matches must be reported: shown={} hidden={} total={}",
            response.shown,
            response.hidden,
            response.total
        );

        let ceiling = (BUDGET / SEARCH_HIT_OVERHEAD_TOKENS) as usize + 1;
        assert!(
            reads <= ceiling,
            "scored {SYMBOLS} symbols and read {reads} files for a budget that can \
             show at most {ceiling}"
        );
        assert!(
            reads >= response.shown as usize,
            "every shown hit needs its source read: reads={reads} shown={}",
            response.shown
        );
    }

    #[test]
    fn scoped_trace_budget_never_returns_a_misleading_prefix() {
        let path = vec![path_edge("a", "b", 1.0), path_edge("b", "c", 1.0)];
        let response = atomic_budget_take(path, 25, |_| 25);
        assert!(response.items.is_empty());
        assert_eq!(response.total, 2);
        assert_eq!(response.hidden, 2);
        assert!(response.truncated);
        assert_eq!(response.tokens_used, 0);
    }
}
