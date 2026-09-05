use devmap_analyze::clones::group_clones;
use devmap_analyze::traversal::{
    traverse_graph, traverse_graph_indexed, AdjacencyIndex, TraversalLimits, TraversalOptions,
    TraversalStop,
};
use devmap_extract::model::*;
use devmap_resolve::model::*;
use devmap_store::{Store, StoredEdge, StoredSymbol};

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

use crate::cancel::{Cancel, QueryCancelled};
use crate::model::*;

/// Most targets one composed `neighbors` request may ask about.
///
/// Sized above the five definitions the `graph_query` view measures, with room
/// for a caller that wants a few more, and far below anything that would let
/// one request monopolise the daemon: the worst case is
/// `2 * MAX_NEIGHBOR_TARGETS` sub-queries, which a client could already issue
/// as separate calls. It adds no reach the client did not have — it makes that
/// reach cost one round trip instead of thirty-two.
pub const MAX_NEIGHBOR_TARGETS: usize = 16;

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
        if req.query.trim().is_empty() {
            return Ok(budget_take(Vec::new(), req.token_budget, |_| 0));
        }
        let page = budget_page_size(req.token_budget);
        let pool = search_rank_pool_size(req.token_budget);
        // One snapshot. The count, the rows and the root used to be three
        // independent reads, each resolving "the latest generation" for itself,
        // so a daemon commit landing between them produced an answer stitched
        // from two generations — `shown=40 hidden=0 total=1 truncated=false`
        // was measured. `shown + hidden == total` is the contract clients
        // enforce, and it cannot be honoured by numbers describing different
        // corpora.
        //
        // `neighbors` answers the same race by *detecting* a straddle and
        // disclosing it rather than locking, on the grounds that holding the
        // store lock across a whole fan-out blocks the writer for too long.
        // That trade is about fan-outs. This is a count, one limited select and
        // one row — so the exact answer is affordable here, and an exact answer
        // beats a disclosed approximation whenever it can be had.
        let Some(snapshot) = self.store.search_page(&req.query, pool)? else {
            return Ok(unavailable_response(ResolutionAvailability::Unavailable {
                reason: "no persisted generation is available".to_string(),
            }));
        };
        let total = snapshot.total;
        let rows = snapshot.rows;
        let repo_root = snapshot.repo_root;
        let query = req.query.to_lowercase();
        // Rank, then truncate — R7, and the reason the pool above is wider than
        // the page below. The store cuts its page with `ORDER BY bm25(...)` and
        // this function then re-scores what survived with a different ordering
        // function, so the key that decided which rows *exist* was not the key
        // that decides which rows *rank*. A symbol named exactly `alpha` that
        // bm25 puts 150th among 200 prefix matches never entered the page, and
        // the answer led with 100 worse matches under honest counts.
        //
        // Scoring happens on the stored row, before any hit is materialised:
        // the score reads `name`/`qualified_name` and nothing else, while
        // building a hit reads the file off disk. So a ten-times wider pool
        // costs ten times the string comparisons and not one extra file read —
        // `page`, not `pool`, bounds what is materialised.
        let mut ranked: Vec<(f32, devmap_store::StoredSymbol)> = rows
            .into_iter()
            .map(|row| (name_match_score(&row, &query), row))
            .collect();
        ranked.sort_by(|(left_score, left), (right_score, right)| {
            right_score
                .total_cmp(left_score)
                .then_with(|| left.path.cmp(&right.path))
                .then_with(|| left.name.cmp(&right.name))
                .then_with(|| left.span_start.cmp(&right.span_start))
                .then_with(|| left.span_end.cmp(&right.span_end))
        });
        let mut hits = Vec::with_capacity(ranked.len().min(page));
        for (score, row) in ranked.into_iter().take(page) {
            hits.push(hit_from_stored(
                row,
                repo_root.as_deref(),
                req.token_budget,
                score,
            ));
        }
        let mut response = budget_take(hits, req.token_budget, search_hit_tokens);
        response.total = total;
        response.hidden = total.saturating_sub(response.shown);
        response.truncated = response.hidden > 0;
        // The pool is bounded, so on a query that matches more than it holds
        // the ranking really is over a bm25-ordered prefix. `truncated` says
        // the *list* was cut, which a caller expects; this says the *ordering*
        // was computed over a sample, which it cannot otherwise know. It is
        // `None` whenever every match was ranked, which on any ordinary query
        // is every time.
        response.walk_incomplete = (total as usize > pool).then(|| {
            format!(
                "ranked the first {pool} of {total} matches, in the store's \
                 relevance order; a closer match may sit outside that page"
            )
        });
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

    /// Answer both call-graph directions for several targets in one pass.
    ///
    /// This is a composition, not new analysis: each target still gets exactly
    /// the [`Self::impact`] and [`Self::trace`] it would have got on its own,
    /// under the same budget and the same `min_confidence` — both directions,
    /// so the two halves of one answer cannot disagree about what the filter
    /// meant, nor about which file the target names. What it removes is the
    /// per-direction round trip — the caller pays one, not `2 * targets.len()`.
    ///
    /// The fan-out is bounded and the bound is *refused*, never silently
    /// applied: more than [`MAX_NEIGHBOR_TARGETS`] targets is an error, so
    /// nobody can read a truncated answer as a complete one. A caller that
    /// needs more must ask again, and know that it did.
    ///
    /// Cancellation is checked between targets, so a composed request cannot
    /// outlive its client by the whole fan-out.
    pub fn neighbors(
        &self,
        targets: &[String],
        token_budget: u32,
        min_confidence: f32,
        max_depth: usize,
    ) -> anyhow::Result<Vec<Neighbors>> {
        if targets.len() > MAX_NEIGHBOR_TARGETS {
            anyhow::bail!(
                "neighbors accepts at most {} targets, got {}",
                MAX_NEIGHBOR_TARGETS,
                targets.len()
            );
        }
        // A composed answer must come from one generation.
        //
        // The fan-out used to be `2 * targets.len()` sub-queries, each taking
        // and releasing the store lock on its own. A build committing
        // mid-fan-out left one answer describing two different snapshots of the
        // repository — measured at 25 of 62 composed answers under contention —
        // and nothing in the response disclosed it. That is not a regression
        // against the separate `impact`/`deps` calls this replaced, which
        // straddled the same way; but those were visibly separate exchanges and
        // this is sold as one.
        //
        // `neighbors_once` now reads the edge table once for the whole
        // fan-out, so the parts of one answer can no longer disagree with each
        // other about the graph. The check below stays because that read is
        // still not atomic with the two generation probes around it: a commit
        // landing between the first probe and the read produces an answer from
        // a generation the caller was not told about. One read narrows the
        // window; it does not close it, and an undisclosed straddle is the
        // defect either way.
        //
        // Detected rather than locked out: holding the store lock across the
        // whole fan-out would block the writer for the duration of a composed
        // query, which is a worse trade. Commits are rare, so one retry
        // resolves nearly all of them; a second straddle is reported on every
        // direction instead of being smoothed over, because a caller that
        // cannot tell is the actual defect.
        for attempt in 0..2 {
            let before = self.store.latest_generation_id()?;
            let mut answers =
                self.neighbors_once(targets, token_budget, min_confidence, max_depth)?;
            let after = self.store.latest_generation_id()?;
            if before == after {
                return Ok(answers);
            }
            if attempt == 1 {
                let note = format!(
                    "the index moved from generation {before:?} to {after:?} while this \
                     composed answer was being assembled, twice in a row; its parts may \
                     describe different snapshots"
                );
                for entry in &mut answers {
                    entry.callers.walk_incomplete = Some(note.clone());
                    entry.callees.walk_incomplete = Some(note.clone());
                }
                return Ok(answers);
            }
        }
        unreachable!("the loop returns on both attempts")
    }

    /// One pass of the composition. See [`Self::neighbors`] for the retry that
    /// keeps a composed answer inside a single generation.
    fn neighbors_once(
        &self,
        targets: &[String],
        token_budget: u32,
        min_confidence: f32,
        max_depth: usize,
    ) -> anyhow::Result<Vec<Neighbors>> {
        // Nothing asked, nothing read. Without this the hoisted load below
        // would pull the whole edge table to answer a request with no targets.
        if targets.is_empty() {
            return Ok(Vec::new());
        }
        // One edge load for the whole fan-out.
        //
        // `impact` and `trace` each begin by reading and converting every edge
        // in the generation, so a composed answer over N targets paid for that
        // 2N times — 16 full reads for the eight-target fan-out the daemon
        // sends, of a table that does not change between them. On a
        // 660,000-edge store the read is 38 ms and the conversion 33 ms, so
        // fifteen of those sixteen reads were 1.07 s of a 2.56 s answer.
        //
        // This is the same hoist `explore` already performs, for the same
        // reason and through the same seam: `traverse_over` *is* the body of
        // `impact`/`trace` once the edges are in hand, so every direction below
        // gets exactly the traversal, budget and `walk_incomplete` reason it
        // got before — the ownership of the load moved, nothing else. The
        // filter is `min_confidence`, which is one value for the whole request,
        // so a single load can serve every target and both directions.
        //
        // It also makes the composition *more* coherent than it was: the parts
        // of one answer now share one edge snapshot instead of racing each
        // other. The generation straddle check in [`Self::neighbors`] still
        // wraps this, because the load is not the only store read here.
        let edges = self.resolved_edges(min_confidence)?;
        // One index per direction for the whole fan-out, for the same reason
        // the edge load above is shared. Every walk below used to rebuild this
        // map over the entire generation first, so N targets x 2 directions
        // paid O(edges) 2N times over an edge slice that does not change
        // between them. There are only ever two directions, so there are only
        // ever two indexes.
        let inbound = AdjacencyIndex::build(&edges, true);
        let outbound = AdjacencyIndex::build(&edges, false);
        let mut answers = Vec::with_capacity(targets.len());
        for target in targets {
            // Plain `check`, not `check_every`: the latter consults the flag
            // once per CHECK_INTERVAL (512) iterations, and this loop runs at
            // most MAX_NEIGHBOR_TARGETS (16) times, so it would fire on target
            // 0 and never again.
            //
            // In practice cancellation already lands inside `traverse_over`,
            // which checks the flag on either side of the walk, so this is not
            // what rescues a cancelled request — measurement confirms a
            // composition stops partway through either way. It closes the gap
            // *between* sub-queries, and costs one relaxed load per target.
            self.cancel.check()?;
            // `min_confidence` applies to *both* directions. It was hardcoded to
            // 0.0 here, which silently discarded the caller's filter on the
            // inbound side: one composed answer would report every caller while
            // reporting only the callees that cleared the threshold, so its two
            // halves disagreed about what the filter meant. It is now applied
            // once, in the shared `resolved_edges(min_confidence)` above, and
            // again inside `traverse_over` — so the two halves cannot diverge
            // by construction rather than by both remembering to pass it.
            let callers = self.traverse_over(
                &edges,
                &inbound,
                Request {
                    query: target.clone(),
                    token_budget,
                    min_confidence,
                    max_depth,
                },
            )?;
            // Outbound edges come from whichever query can actually answer
            // for this target's shape.
            //
            // `dependencies` resolves a *file* path, so for a symbol id it
            // returned `Unavailable: … is not indexed` — every time. The
            // composed `query` view therefore reported its callees as unknown
            // for every symbol-shaped query, which is honest but useless, and
            // the earlier attempt to fix it by asking about the containing file
            // was worse: a function's "callees" became the whole file's
            // outbound edges, so symbols appeared to call themselves.
            //
            // The forward traversal is symbol-scoped and answers exactly the
            // question. `latest_file` — the store's own notion of what is a
            // file — picks between them, rather than sniffing for `::`.
            // Outbound edges come from the forward traversal, for every target
            // shape — not from `dependencies`.
            //
            // `dependencies` resolves a *file* path, so for a symbol id it
            // always answered `Unavailable: … is not indexed` and `callees`
            // never carried anything for a symbol query. For a file it answered,
            // but wrongly for this field: its SQL matches
            // `sp.path = ?2 OR tp.path = ?2`, so "outbound edges" included
            // edges pointing *into* the file, and a file appeared in its own
            // callee list — the same "symbols appeared to call themselves"
            // shape, surviving on the file path.
            //
            // Worse, mixing the two resolvers made one answer incoherent:
            // `impact` matches by suffix (`path_matches`: `ends_with("/{query}")`)
            // while `dependencies` matches exactly, so with both `core.py` and
            // `pkg/core.py` indexed, the callers described one file and the
            // callees another. One resolver for both directions removes that by
            // construction.
            //
            // `deps` the command is unchanged; only this composition's `callees`
            // tightened to what the field has always claimed to be.
            let callees = self.traverse_over(
                &edges,
                &outbound,
                Request {
                    query: target.clone(),
                    token_budget,
                    min_confidence,
                    max_depth: 1,
                },
            )?;
            answers.push(Neighbors {
                target: target.clone(),
                callers,
                callees,
            });
        }
        Ok(answers)
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
        let path = match shortest_path(
            &edges,
            from,
            to,
            req.max_depth.min(64),
            5_000,
            &self.cancel,
        )? {
            PathSearch::Found(path) => path,
            // The only outcome that is a claim about the graph.
            PathSearch::NoPath => {
                return Ok(unavailable_response(ResolutionAvailability::Unavailable {
                    reason: format!("no indexed path from {from:?} to {to:?}"),
                }))
            }
            // A limit stopped the walk, so the graph was never asked. Saying
            // "no path" here is how an agent concludes two symbols are
            // unrelated when the path is merely longer than `--depth`.
            PathSearch::Exhausted {
                depth_capped,
                node_capped,
                visited,
                max_depth,
                max_nodes,
            } => {
                let mut limits = Vec::new();
                if depth_capped {
                    limits.push(format!("depth {max_depth}"));
                }
                if node_capped {
                    limits.push(format!("{max_nodes} nodes"));
                }
                let limits = if limits.is_empty() {
                    "its budget".to_string()
                } else {
                    limits.join(" and ")
                };
                return Ok(unavailable_response(ResolutionAvailability::Unavailable {
                    reason: format!(
                        "search from {from:?} to {to:?} stopped at {limits} after                          visiting {visited} nodes without reaching the target;                          whether a path exists is unknown — retry with a larger                          --depth"
                    ),
                }));
            }
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
        self.traverse_over(&edges, &AdjacencyIndex::build(&edges, reverse), req)
    }

    /// The traversal itself, over an edge set the caller already holds.
    ///
    /// Split out of [`Self::traverse`] because `explore` needs `2 * n + 1`
    /// walks for one answer and every one of them used to re-read and re-convert
    /// the whole generation's edge table — 71,195 rows on this repository. The
    /// walk is unchanged; only the ownership of the edge load moved up, so a
    /// composed query pays for it once. Every caller still gets exactly the
    /// traversal `impact`/`trace` performs, including the `walk_incomplete`
    /// reason, because there is only one implementation of it.
    fn traverse_over(
        &self,
        edges: &[ResolvedEdge],
        index: &AdjacencyIndex<'_>,
        req: Request<String>,
    ) -> anyhow::Result<Response<ResolvedEdge>> {
        // The direction is the index's, not a second argument that could
        // disagree with it. A reversed walk over a forward index answers
        // plausibly and wrongly rather than failing, so the two are not
        // separable here.
        let reverse = index.reverse();
        let target = req.query.trim();
        let start: Vec<String> = traversal_starts(edges, target, reverse)
            .into_iter()
            .map(|(symbol, _)| symbol)
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
        let max_depth = req.max_depth.min(64);
        let max_nodes = TRAVERSAL_MAX_NODES;
        let walk = traverse_graph_indexed(
            &start,
            index,
            TraversalLimits {
                max_depth,
                max_nodes,
            },
        );
        self.cancel.check()?;
        let mut traversed = traversed_resolution_edges(&walk, edges, req.min_confidence);
        traversed.sort_by(|a, b| {
            b.confidence
                .0
                .total_cmp(&a.confidence.0)
                .then_with(|| a.source_file.cmp(&b.source_file))
                .then_with(|| a.target_file.cmp(&b.target_file))
                .then_with(|| a.source_symbol.cmp(&b.source_symbol))
        });
        let mut response = budget_take(traversed, req.token_budget, |_| EDGE_TOKENS);
        // The budgeter counts what it received. When the walk itself stopped
        // early, `total` is the size of a partial answer and `truncated: false`
        // is a claim the walk never earned — this is where `impact` said "here
        // is the blast radius" after visiting three levels of a deeper graph.
        response.walk_incomplete = walk.stop.reason(max_depth, max_nodes);
        Ok(response)
    }

    /// Definitions matching `query`, each with its source, both call-graph
    /// directions, and one layered blast radius over all of them.
    ///
    /// Replaces the Python `CodeIntelQueryEngine.explore`, which loaded the
    /// whole graph into process memory and walked it there. Three contract
    /// repairs came with the move, all of them in the direction of not
    /// overclaiming:
    ///
    /// * **Ranked before truncated (R7).** Python concatenated exact and
    ///   partial name matches and sliced `[:limit]`, so which definitions
    ///   survived depended on node order in the store rather than on relevance.
    ///   Here the FTS hits are scored and sorted first, and the cut is the
    ///   budgeter's.
    /// * **A snippet that could not be read is not an empty snippet (Class A).**
    ///   Python returned `""` for a file it could not open and `""` for a
    ///   zero-length span. `source_unavailable_reason` separates them.
    /// * **One edge load, not `2n + 1`.** See [`Self::traverse_over`].
    ///
    /// `limit` caps the definitions considered; the budget decides how many of
    /// those are actually packed. Both are reported, and `definitions.total` is
    /// the measured index-wide match count either way.
    pub fn explore(
        &self,
        query: &str,
        limit: usize,
        token_budget: u32,
        min_confidence: f32,
        max_depth: usize,
    ) -> anyhow::Result<ExploreReport> {
        let budget = explore_budget(token_budget);
        let empty = |reason: String| ExploreReport {
            query: query.to_string(),
            definitions: unavailable_response(ResolutionAvailability::Unavailable { reason }),
            limit: u32::try_from(limit).unwrap_or(u32::MAX),
            blast_radius: BlastRadius {
                seeds: Vec::new(),
                unmatched_targets: Vec::new(),
                layers: budget_take(Vec::new(), budget.blast_radius, blast_layer_tokens),
                total_impacted: 0,
            },
            budget,
        };
        if self.store.latest_generation_id()?.is_none() {
            return Ok(empty("no persisted generation is available".to_string()));
        }
        if query.trim().is_empty() {
            return Ok(empty("explore requires a non-empty query".to_string()));
        }

        // Rank first. `count_search_symbols` measures the whole index, so
        // `total` below describes what matched rather than what fit.
        let total = self.store.count_search_symbols(query)?;
        let page = budget_page_size(budget.definitions).max(limit);
        let rows = self.store.search_symbols(query, page)?;
        let repo_root = self.store.latest_repo_root()?;
        let lowered = query.to_lowercase();
        // Rank the *rows*, then read files for the survivors only.
        //
        // Scoring needs `name` and `qualified_name`, both already in the row;
        // `hit_from_stored` is what opens a file. Materialising first and
        // cutting afterwards would open one file per candidate — 401 of them at
        // the default budget — in order to keep `limit` of them, which is the
        // amplification `search_semantic` was repaired for.
        let mut scored: Vec<(f32, StoredSymbol)> = rows
            .into_iter()
            .map(|row| (name_match_score(&row, &lowered), row))
            .collect();
        scored.sort_by(|(left_score, left), (right_score, right)| {
            right_score
                .total_cmp(left_score)
                .then_with(|| left.path.cmp(&right.path))
                .then_with(|| left.name.cmp(&right.name))
                .then_with(|| {
                    (left.span_start, left.span_end).cmp(&(right.span_start, right.span_end))
                })
        });
        scored.truncate(limit);
        // `qualified_name` is carried out of the row before `hit_from_stored`
        // consumes it: the hit keeps only the bare name, and a definition that
        // reported no qualified name would be indistinguishable from one whose
        // language has none.
        let ranked: Vec<(String, SymbolHit)> = scored
            .into_iter()
            .map(|(score, row)| {
                let qualified = row.qualified_name.clone();
                (
                    qualified,
                    hit_from_stored(row, repo_root.as_deref(), budget.definitions, score),
                )
            })
            .collect();

        // Pack the definitions before any edge work: a definition the budget
        // cannot admit must not cost two traversals.
        let shells: Vec<ExploreDefinition> = ranked
            .into_iter()
            .map(|(qualified_name, hit)| ExploreDefinition {
                // `file::name` — the identity every devmap traversal surface
                // already resolves, and the one `graph_query` sends today.
                id: node_id_of(&hit.file_path, &hit.symbol_name),
                qualified_name,
                symbol_name: hit.symbol_name,
                file_path: hit.file_path,
                kind: hit.kind,
                language: None,
                span: hit.span,
                source: hit.source_span,
                source_unavailable_reason: hit.source_unavailable_reason,
                source_omitted_bytes: hit.source_span_omitted_bytes,
                score: hit.score,
                callers: budget_take(Vec::new(), 0, |_| EDGE_TOKENS),
                callees: budget_take(Vec::new(), 0, |_| EDGE_TOKENS),
            })
            .collect();
        let mut definitions = budget_take(shells, budget.definitions, explore_definition_tokens);
        // `budget_take` counts the page it was handed; the index-wide count is
        // the honest denominator, exactly as `search` reports it.
        definitions.total = total.max(definitions.shown);
        definitions.hidden = definitions.total.saturating_sub(definitions.shown);
        definitions.truncated = definitions.hidden > 0;
        // Looked up only for the definitions that survived the budget, and left
        // `None` when the generation holds no row for the file — a definition
        // whose language was never recorded must not be labelled with a guess.
        for definition in &mut definitions.items {
            definition.language = self
                .store
                .latest_file(&definition.file_path)?
                .map(|file| file.language);
        }

        self.cancel.check()?;
        let edges = self.resolved_edges(min_confidence)?;
        // One index per direction for the whole fan-out, for the same reason
        // the edge load above is shared. Every walk below used to rebuild this
        // map over the entire generation first, so N targets x 2 directions
        // paid O(edges) 2N times over an edge slice that does not change
        // between them. There are only ever two directions, so there are only
        // ever two indexes.
        let inbound = AdjacencyIndex::build(&edges, true);
        let outbound = AdjacencyIndex::build(&edges, false);
        let per_direction = edges_per_direction(&budget, definitions.shown);
        let mut budget = budget;
        budget.edges_per_direction = per_direction;
        for definition in &mut definitions.items {
            self.cancel.check()?;
            definition.callers = self.traverse_over(
                &edges,
                &inbound,
                Request {
                    query: definition.id.clone(),
                    token_budget: per_direction,
                    min_confidence,
                    max_depth: 1,
                },
            )?;
            definition.callees = self.traverse_over(
                &edges,
                &outbound,
                Request {
                    query: definition.id.clone(),
                    token_budget: per_direction,
                    min_confidence,
                    max_depth: 1,
                },
            )?;
        }

        let seeds: Vec<String> = definitions
            .items
            .iter()
            .map(|definition| definition.id.clone())
            .collect();
        let blast_radius = self
            .blast_walk(&edges, &seeds, max_depth, min_confidence)?
            .into_radius(budget.blast_radius);
        Ok(ExploreReport {
            query: query.to_string(),
            definitions,
            limit: u32::try_from(limit).unwrap_or(u32::MAX),
            blast_radius,
            budget,
        })
    }

    /// Test files reachable through the inbound blast radius of `targets`.
    ///
    /// Replaces the Python `CodeIntelQueryEngine.affected_tests`. Two things
    /// changed with the move. Each test file carries the **distance** at which
    /// the walk first reached it, and the list is ranked nearest-first before
    /// truncation, so a budget-trimmed answer keeps the tests most likely to
    /// break; Python sorted alphabetically and had no cap at all. And a target
    /// that matched nothing is named in `blast_radius.unmatched_targets`
    /// instead of silently contributing no seeds — a typo used to come back as
    /// "no affected tests", which is the flattering reading of "we did not
    /// look".
    pub fn affected_tests(
        &self,
        targets: &[String],
        token_budget: u32,
        min_confidence: f32,
        max_depth: usize,
    ) -> anyhow::Result<AffectedTestsReport> {
        let layer_budget = token_budget / 2;
        let list_budget = token_budget.saturating_sub(layer_budget);
        let empty_report = |reason: String| AffectedTestsReport {
            targets: targets.to_vec(),
            tests: unavailable_response(ResolutionAvailability::Unavailable { reason }),
            blast_radius: BlastRadius {
                seeds: Vec::new(),
                unmatched_targets: targets.to_vec(),
                layers: budget_take(Vec::new(), layer_budget, blast_layer_tokens),
                total_impacted: 0,
            },
        };
        if self.store.latest_generation_id()?.is_none() {
            return Ok(empty_report(
                "no persisted generation is available".to_string(),
            ));
        }
        if targets.is_empty() {
            return Ok(empty_report(
                "affected_tests requires at least one target".to_string(),
            ));
        }
        if targets.len() > MAX_NEIGHBOR_TARGETS {
            anyhow::bail!(
                "affected accepts at most {} targets, got {}",
                MAX_NEIGHBOR_TARGETS,
                targets.len()
            );
        }

        let edges = self.resolved_edges(min_confidence)?;
        let walk = self.blast_walk(&edges, targets, max_depth, min_confidence)?;

        // Derived from the *complete* walk, never from the budgeted layers.
        // Reading the presentation back would drop every test whose band the
        // token budget trimmed, and the shortfall would be invisible: the test
        // list's own counters would report a complete answer over a set that
        // had already been cut. The bands are also sampled for display at
        // `BLAST_LAYER_NODE_SAMPLE`, which would hide the 51st caller in a band
        // for the same reason.
        //
        // Depth 0 is the seed band: a target that is itself in a test file
        // counts as an affected test.
        let mut nearest: BTreeMap<String, (usize, BTreeSet<String>)> = BTreeMap::new();
        for (symbol, file) in &walk.seeds {
            record_test_hit(&mut nearest, symbol, file, 0);
        }
        for band in &walk.bands {
            for (symbol, file) in &band.members {
                record_test_hit(&mut nearest, symbol, file, band.depth);
            }
        }

        let mut tests: Vec<AffectedTest> = nearest
            .into_iter()
            .map(|(path, (depth, symbols))| AffectedTest {
                path,
                depth,
                reached_symbols: u32::try_from(symbols.len()).unwrap_or(u32::MAX),
                symbols: symbols.into_iter().take(AFFECTED_SYMBOL_SAMPLE).collect(),
            })
            .collect();
        // Nearest first, then alphabetically — ranked before the budgeter cuts.
        tests.sort_by(|a, b| a.depth.cmp(&b.depth).then_with(|| a.path.cmp(&b.path)));
        let mut response = budget_take(tests, list_budget, affected_test_tokens);
        // A test list derived from a walk that stopped early is a lower bound,
        // and the counters above cannot say so — they describe the budget.
        response.walk_incomplete = walk.incomplete_reason();
        Ok(AffectedTestsReport {
            targets: targets.to_vec(),
            tests: response,
            blast_radius: walk.into_radius(layer_budget),
        })
    }

    /// Inbound reachability from `targets`, banded by distance.
    ///
    /// [`traverse_graph`] answers *what* is reachable and [`Response`] carries
    /// how much of that fit; neither carries *how far*, and `TraversalResult`
    /// does not expose per-node depth. So the banding is done here, over the
    /// same edge set, under the same `max_nodes` bound, and it reports the same
    /// kind of incompleteness reason — a blast radius that stopped at the cap
    /// must not read like one that ran out of graph.
    ///
    /// Returns the **complete** walk. Sampling and token budgeting happen in
    /// [`BlastWalk::into_radius`], at the presentation boundary, so anything
    /// derived from the walk — the affected-test list — sees everything the
    /// walk reached rather than what a budget left of it.
    fn blast_walk(
        &self,
        edges: &[ResolvedEdge],
        targets: &[String],
        max_depth: usize,
        min_confidence: f32,
    ) -> anyhow::Result<BlastWalk> {
        let depth_cap = max_depth.clamp(1, 64);
        let mut seed_set: BTreeSet<(String, String)> = BTreeSet::new();
        let mut unmatched: Vec<String> = Vec::new();
        for target in targets {
            let matched = traversal_starts(edges, target.trim(), true);
            if matched.is_empty() {
                unmatched.push(target.clone());
                continue;
            }
            seed_set.extend(matched);
        }
        let seeds: Vec<(String, String)> = seed_set.into_iter().collect();
        let mut walk = BlastWalk {
            seeds,
            unmatched,
            bands: Vec::new(),
            total_impacted: 0,
            stop: TraversalStop::default(),
            depth_cap,
            unresolved_seeds: false,
        };
        if walk.seeds.is_empty() {
            walk.unresolved_seeds = true;
            return Ok(walk);
        }

        let mut inbound: BTreeMap<&str, Vec<&ResolvedEdge>> = BTreeMap::new();
        for (index, edge) in edges.iter().enumerate() {
            self.cancel.check_every(index)?;
            if edge.confidence.0 < min_confidence {
                continue;
            }
            inbound.entry(&edge.target_symbol).or_default().push(edge);
        }

        let mut visited: BTreeSet<String> = walk
            .seeds
            .iter()
            .map(|(symbol, _)| symbol.clone())
            .collect();
        let mut frontier: Vec<String> = visited.iter().cloned().collect();
        for depth in 1..=depth_cap {
            self.cancel.check()?;
            let mut members: BTreeSet<(String, String)> = BTreeSet::new();
            let mut seen: BTreeSet<String> = BTreeSet::new();
            let mut lowest: Option<f32> = None;
            for node in &frontier {
                for edge in inbound.get(node.as_str()).into_iter().flatten() {
                    if visited.contains(edge.source_symbol.as_str())
                        || seen.contains(edge.source_symbol.as_str())
                    {
                        continue;
                    }
                    // The cap is a *withholding*, recorded as one. A radius
                    // that stopped at 5,000 nodes must not be readable as one
                    // that ran out of graph.
                    if visited.len() + seen.len() >= TRAVERSAL_MAX_NODES {
                        walk.stop.node_capped = true;
                        continue;
                    }
                    seen.insert(edge.source_symbol.clone());
                    // The file comes from the edge that actually reached this
                    // node, not from a global symbol-to-file guess: the same
                    // qualified name can appear in two files, and attributing a
                    // reached symbol to the wrong one puts the wrong test in
                    // the answer.
                    members.insert((edge.source_symbol.clone(), edge.source_file.clone()));
                    lowest = Some(match lowest {
                        Some(current) => current.min(edge.confidence.0),
                        None => edge.confidence.0,
                    });
                }
            }
            if seen.is_empty() {
                break;
            }
            walk.total_impacted = walk
                .total_impacted
                .saturating_add(u32::try_from(seen.len()).unwrap_or(u32::MAX));
            walk.bands.push(BlastBand {
                depth,
                members,
                lowest_confidence: lowest,
                node_count: u32::try_from(seen.len()).unwrap_or(u32::MAX),
            });
            visited.extend(seen.iter().cloned());
            frontier = seen.into_iter().collect();
            if depth == depth_cap {
                // Something was still expanding when the depth bound stopped
                // it. Left unsaid, a capped radius reads as a complete one.
                walk.stop.depth_capped = frontier.iter().any(|node| {
                    inbound
                        .get(node.as_str())
                        .into_iter()
                        .flatten()
                        .any(|edge| !visited.contains(edge.source_symbol.as_str()))
                });
            }
        }
        Ok(walk)
    }

    /// Symbols the latest generation found nothing calling.
    ///
    /// The answer carries the coverage it was computed over. This list is read
    /// as "delete these", and without a denominator a generation with 4,242
    /// unattributed calls answered in exactly the shape of one with none —
    /// `resolution: Available`, `truncated: false`, and not a word about
    /// either `AnalysisSummary::status` or `unresolved_calls`, the field whose
    /// own documentation says it exists so a reader can tell "nothing calls
    /// this" from "we could not work out what this calls".
    ///
    /// It rides on `walk_incomplete` rather than a wrapper struct because that
    /// is the field this crate already has for "the producer of these items
    /// did not see everything", it is already rendered by the CLI and already
    /// read by `DevMapClient._budgeted`, and a second shape for the same
    /// statement is a second thing for a consumer to miss.
    pub fn dead_symbols(
        &self,
        token_budget: u32,
    ) -> anyhow::Result<Response<devmap_analyze::DeadSymbolReport>> {
        // One snapshot. This resolved the generation three times — an existence
        // check, the analysis, then the rows — so the coverage disclosure could
        // describe a different generation than the findings it was attached to.
        // That combination is what promotes a row from "look at this" to "safe
        // to delete": a disclosure saying the corpus was fully covered, over
        // rows from a generation where it was not.
        //
        // Bounded by the answer, not by the corpus. The exempt filter and the
        // cut both run in SQL now; this used to materialise every dead row of
        // the generation and drop almost all of them here — 80,000 read to show
        // 66 on the benchmark corpus. One more row than the budget can seat is
        // read on purpose, so `budget_take` still sees something it cannot fit
        // and reports `truncated` for the right reason.
        let limit = (token_budget / DEAD_SYMBOL_TOKENS) as usize + 1;
        let Some(page) = self.store.dead_page(limit)? else {
            return Ok(unavailable_response(ResolutionAvailability::Unavailable {
                reason: "no persisted generation is available".to_string(),
            }));
        };
        let mut response = budget_take(page.rows, token_budget, |_| DEAD_SYMBOL_TOKENS);
        // `budget_take` counts the page it was handed, and the page is now a
        // bounded read — so the generation-wide count has to be restored as the
        // denominator, exactly as `explore` does for definitions. Without this
        // a capped list would report itself as the whole truth.
        response.total = u32::try_from(page.total_non_exempt)
            .unwrap_or(u32::MAX)
            .max(response.shown);
        response.hidden = response.total.saturating_sub(response.shown);
        response.truncated = response.hidden > 0;
        response.walk_incomplete = dead_symbol_coverage_gap(page.analysis.as_ref());
        Ok(response)
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
            walk_incomplete: None,
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
        // `.ok()` here used to collapse two different facts into one. "There is
        // no such file" and "the file is there and I could not read it"
        // (non-UTF-8, EACCES, EISDIR) both became `None`, and `None` means
        // `compared_against: "nothing"` — documented as *no such file, so every
        // symbol is an addition*.
        //
        // The consequence is the worst shape this repository has: a genuine
        // removal **disappears**. Same edit, same file — readable, the report
        // says `symbols: ["beta:Removed"]`; unreadable, it says
        // `["mod.py:Added", "alpha:Added"]` with `degraded_reason: null` and
        // `delta_available: true`. The caller is handed a clean bill of health
        // by a comparison that never ran.
        let (on_disk, read_failure) = match std::fs::read_to_string(&resolved) {
            Ok(source) => (Some(source), None),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => (None, None),
            Err(err) => (None, Some(err)),
        };
        let compared_against = match (&on_disk, &read_failure) {
            (Some(_), _) => "disk",
            // A third value, never a reuse of `nothing`. A consumer keying off
            // `nothing` to mean "new file" must not be handed this case.
            (None, Some(_)) => "unreadable",
            (None, None) => "nothing",
        };
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
        // One snapshot for the list and the denominator it is measured
        // against. Read separately they could describe two generations, and the
        // `saturating_sub` below turns that into silence: when the newer
        // generation holds fewer callers the difference clamps to zero and this
        // reports that the confidence floor hid nothing. Drawn from one
        // generation the floored set is a subset of the unfiltered one, so the
        // subtraction cannot underflow at all.
        let page = self.store.callers_page(&at_risk, path, min_confidence)?;
        let (caller_edges, total_unfiltered) = match page {
            Some(page) => (page.callers, page.total_unfiltered),
            None => (Vec::new(), 0),
        };
        let callers: Vec<PreviewCaller> = caller_edges
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
        // Counted, not built. This asked for every caller edge at floor 0.0 —
        // six `String` allocations per row — solely to take `.len()`. On this
        // repository the busiest symbol has 918 callers, so previewing a file
        // that declares one materialised ~1,836 rows and kept none of them.
        let ambiguous_callers = total_unfiltered.saturating_sub(callers.len());

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

        // A read that failed outranks a parse note: without the previous
        // content there is no comparison at all, so the delta is not merely
        // "to be read with care", it is absent. `delta_available: false` is the
        // gate the model documents for exactly this, and stating the errno is
        // what lets a caller tell a permissions problem from a binary file.
        let (delta_available, degraded_reason) = match read_failure {
            Some(err) => (
                false,
                Some(format!(
                    "the file on disk could not be read ({err}), so the buffer was compared \
against nothing and no symbol can be reported as removed; this is not a clean delta"
                )),
            ),
            None => (true, degraded_reason),
        };

        Ok(PreviewReport {
            file_path: path.to_string(),
            parse_status,
            delta_available,
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

#[cfg(test)]
mod span_line_range_tests {
    use super::byte_span_to_line_range;
    use devmap_extract::model::Span;

    /// Multi-byte source must not abort the process.
    ///
    /// This function counted newlines with `source[..start]` — slicing a `&str`
    /// at an index that is not a character boundary, which panics. Spans are
    /// byte offsets recorded at extraction time while the source is re-read
    /// from disk when the graph is exported, so an offset lands mid-character
    /// whenever a multi-byte character was inserted before it. One emoji added
    /// to a file aborted `dev map manifest`, and the release profile is
    /// `panic = "abort"`, so nothing recovered.
    ///
    /// Exhaustive over every offset pair rather than sampled: the failure is
    /// per-offset, and testing only the boundaries would pass against exactly
    /// the code that panicked, because boundaries were always the safe case.
    #[test]
    fn every_offset_into_multibyte_source_is_answered_rather_than_panicked_on() {
        let source = "fn a() {}\n// \u{1F980} ferris r\u{e9}\nfn b() {}\n";
        for start in 0..=source.len() {
            for end in 0..=source.len() {
                let span = Span {
                    start_byte: start,
                    end_byte: end,
                };
                let (first, last) = byte_span_to_line_range(source, &span);
                assert!(first >= 1, "lines are one-based, got {first}");
                assert!(
                    last >= first,
                    "end line {last} precedes start line {first} for {start}..{end}"
                );
            }
        }
    }

    /// The line numbers must be right, not merely non-panicking.
    ///
    /// A fix that clamped every offset to zero would satisfy the test above.
    #[test]
    fn line_numbers_are_correct_across_a_multibyte_character() {
        let source = "alpha\n\u{1F980}beta\ngamma\n";
        let crab = source
            .find('\u{1F980}')
            .expect("fixture contains the emoji");

        let at_emoji = Span {
            start_byte: crab,
            end_byte: crab,
        };
        assert_eq!(byte_span_to_line_range(source, &at_emoji), (2, 2));

        // Strictly inside the four-byte emoji — the exact index that panicked.
        let inside = Span {
            start_byte: crab + 1,
            end_byte: crab + 2,
        };
        assert_eq!(byte_span_to_line_range(source, &inside), (2, 2));

        let whole = Span {
            start_byte: 0,
            end_byte: source.len(),
        };
        assert_eq!(byte_span_to_line_range(source, &whole), (1, 4));
    }

    /// A stored span outliving the file it points into is the everyday case
    /// after an edit, not a hostile one.
    #[test]
    fn offsets_beyond_the_source_are_clamped() {
        let source = "one\ntwo\n";
        let span = Span {
            start_byte: 10_000,
            end_byte: 20_000,
        };
        assert_eq!(byte_span_to_line_range(source, &span), (3, 3));
    }
}

/// Token cost of one caller line: two paths and a symbol name.
#[cfg(feature = "parse")]
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
#[cfg(feature = "parse")]
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
/// Why a scoped trace ended, so a caller can tell an answer from a decline.
///
/// A bare `Option<Vec<_>>` made four outcomes one value: a zero budget, a
/// frontier pruned at `max_depth`, the node cap stopping the walk, and the
/// reachable set genuinely not containing the target. Only the last of those
/// licenses the sentence `trace_between` was printing — "no indexed path from
/// X to Y" — and an agent that reads it concludes two symbols are unrelated.
#[derive(Debug)]
pub(crate) enum PathSearch {
    /// The target was reached; these are the edges, source-first.
    Found(Vec<ResolvedEdge>),
    /// The reachable set from `from` was explored to exhaustion without
    /// reaching `to`. This is the only outcome that is a fact about the graph.
    NoPath,
    /// A limit stopped the walk before it could answer. `depth_capped` means a
    /// node with unexplored successors sat at `max_depth`; `node_capped` means
    /// the frontier hit `max_nodes`. Both can be true.
    Exhausted {
        depth_capped: bool,
        node_capped: bool,
        visited: usize,
        max_depth: usize,
        max_nodes: usize,
    },
}

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
) -> Result<PathSearch, QueryCancelled> {
    if max_depth == 0 || max_nodes == 0 {
        return Ok(PathSearch::Exhausted {
            depth_capped: max_depth == 0,
            node_capped: max_nodes == 0,
            visited: 0,
            max_depth,
            max_nodes,
        });
    }
    // Set the instant a limit actually costs the walk a successor it would
    // otherwise have expanded. Reaching a cap with nothing left to explore is
    // not a decline, so neither flag is raised for it.
    let mut depth_capped = false;
    let mut node_capped = false;
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
            return Ok(PathSearch::Found(vec![(*edge).clone()]));
        }
        if visited.len() >= max_nodes {
            node_capped = true;
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
            // Only a pruned node that *had* somewhere to go cost us anything.
            // A leaf at `max_depth` is fully explored, and counting it would
            // make every trace on a bounded graph report itself uncertain.
            if outgoing.contains_key(&reached[entry].node) {
                depth_capped = true;
            }
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
                return Ok(PathSearch::Found(path_to(
                    &reached,
                    &ordered,
                    reached.len() - 1,
                )));
            }
            if visited.len() >= max_nodes {
                return Ok(PathSearch::Exhausted {
                    depth_capped,
                    node_capped: true,
                    visited: visited.len(),
                    max_depth,
                    max_nodes,
                });
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
    if depth_capped || node_capped {
        return Ok(PathSearch::Exhausted {
            depth_capped,
            node_capped,
            visited: visited.len(),
            max_depth,
            max_nodes,
        });
    }
    Ok(PathSearch::NoPath)
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
#[cfg(feature = "parse")]
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
                walk_incomplete: None,
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
        let mut response = budget_take(inbound, req.token_budget, |_| 25);
        // The walk's own "I stopped looking" signal, carried the way
        // `StoreQueryEngine::traverse` carries it (engine.rs, `traverse`).
        // Discarding it published a depth-capped walk as a complete answer:
        // over a four-hop chain at depth 2 the traversal computes *"stopped at
        // depth 2; the result is a lower bound, not the full blast radius"* and
        // the response said `truncated: false, walk_incomplete: None`. For
        // `impact` in particular that is the reading that gets a live symbol
        // deleted — an incomplete blast radius is indistinguishable from a small
        // one.
        response.walk_incomplete = walk.stop.reason(opts.max_depth, opts.max_nodes);
        response
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
        let mut response = budget_take(outbound, req.token_budget, |_| 25);
        // Same signal, same reason as `impact` above.
        response.walk_incomplete = walk.stop.reason(opts.max_depth, opts.max_nodes);
        response
    }
}

/// The traversed identities, mapped back to the full edges they name.
///
/// Public so `examples/query_bench.rs` can time this phase of an `impact`
/// call against the real function rather than against a copy of it that
/// could drift from it.
pub fn traversed_resolution_edges(
    traversal: &devmap_analyze::traversal::TraversalResult,
    edges: &[ResolvedEdge],
    min_confidence: f32,
) -> Vec<ResolvedEdge> {
    // Borrowed keys. The set is built from the walk, which outlives this call,
    // and probed with slices of `edges`, which the caller owns — so the scan
    // allocates nothing. The previous spelling built an owned `(String, String,
    // String)` key for *every edge in the generation* purely to ask a question
    // and then dropped it: three allocations per edge, so ~2.0M per `impact`
    // on a 660,000-edge store, measured at 42.9 ms against 11.2 ms here
    // (`examples/query_phase_ab.rs`, hypothesis H2).
    //
    // `edge_kind_name` has to spell a kind exactly as `traverse_graph` recorded
    // it — that side uses `format!("{:?}", kind)`. If the two ever diverge this
    // filter matches nothing and `impact` answers "no callers" from a
    // comparison that never ran, which is the Class A failure this codebase
    // treats as worse than a visible gap. Both directions are pinned by
    // `edge_kind_name_is_the_spelling_traverse_graph_records`.
    let traversed: std::collections::BTreeSet<(&str, &str, &str)> = traversal
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
                    edge_kind_name(edge.edge_kind),
                ))
        })
        .cloned()
        .collect()
}

/// Ceiling on nodes any one walk in this engine may visit.
///
/// Named because three surfaces now share it — `impact`, `trace` and the blast
/// radius — and a second literal would let one of them cap somewhere else while
/// reporting the first number in its `walk_incomplete` sentence.
const TRAVERSAL_MAX_NODES: usize = 5_000;

/// Token cost the budgeter charges for one graph edge, everywhere.
const EDGE_TOKENS: u32 = 25;
/// Token cost of one dead-symbol row, and so the divisor that turns a budget
/// into how many rows are worth reading.
const DEAD_SYMBOL_TOKENS: u32 = 30;

/// Node ids listed per blast-radius band. The band's exact size travels in
/// `node_count` regardless, so this trims the listing, never the count.
const BLAST_LAYER_NODE_SAMPLE: usize = 50;

/// Reached symbols listed per affected test file; `reached_symbols` stays exact.
const AFFECTED_SYMBOL_SAMPLE: usize = 8;

/// Fixed per-definition cost in `explore`'s packer: identity, kind, span, score
/// and the two edge-response envelopes, before any source text.
const EXPLORE_DEFINITION_OVERHEAD_TOKENS: u32 = 40;

/// The `file::symbol` identity every traversal surface resolves.
fn node_id_of(file_path: &str, symbol_name: &str) -> String {
    if file_path.is_empty() {
        return symbol_name.to_string();
    }
    if symbol_name.is_empty() {
        return file_path.to_string();
    }
    format!("{file_path}::{symbol_name}")
}

/// Nodes a walk from `target` would start at, in the given direction.
///
/// Lifted out of `traverse` so the blast radius resolves its seeds through the
/// same matcher the traversal does. Resolving them two ways is how a radius
/// ends up seeded from a symbol the trace never visits.
pub fn traversal_starts(
    edges: &[ResolvedEdge],
    target: &str,
    reverse: bool,
) -> Vec<(String, String)> {
    edges
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
                (edge.target_symbol.clone(), edge.target_file.clone())
            } else {
                (edge.source_symbol.clone(), edge.source_file.clone())
            }
        })
        .collect()
}

/// One distance band of a [`BlastWalk`], before sampling or budgeting.
///
/// `members` pairs each reached symbol with the file the reaching edge named,
/// so a derived answer never has to guess which file a qualified name lives in.
struct BlastBand {
    depth: usize,
    members: BTreeSet<(String, String)>,
    lowest_confidence: Option<f32>,
    node_count: u32,
}

/// The complete result of an inbound walk: every band, unsampled, unbudgeted.
///
/// Separated from [`BlastRadius`] because two consumers want different things
/// from it. `affected_tests` needs everything the walk reached — deriving its
/// answer from a trimmed list would drop tests without any counter saying so.
/// `explore` needs something that fits a token budget. Presentation is
/// [`Self::into_radius`]; derivation reads the bands directly.
struct BlastWalk {
    seeds: Vec<(String, String)>,
    unmatched: Vec<String>,
    bands: Vec<BlastBand>,
    total_impacted: u32,
    stop: TraversalStop,
    depth_cap: usize,
    /// True when no target resolved to a traversal start at all — an answer of
    /// "nothing is impacted" that nothing actually looked for.
    unresolved_seeds: bool,
}

impl BlastWalk {
    /// Why the walk is a lower bound, or `None` when it ran to completion.
    fn incomplete_reason(&self) -> Option<String> {
        self.stop.reason(self.depth_cap, TRAVERSAL_MAX_NODES)
    }

    /// Sample each band and pack the bands into `token_budget`.
    ///
    /// The two trims are reported separately and neither touches a count:
    /// `nodes_omitted` per band, `hidden`/`truncated` for the band list.
    fn into_radius(self, token_budget: u32) -> BlastRadius {
        let incomplete = self.incomplete_reason();
        let layers: Vec<BlastLayer> = self
            .bands
            .into_iter()
            .map(|band| {
                let nodes: Vec<String> = band
                    .members
                    .iter()
                    .take(BLAST_LAYER_NODE_SAMPLE)
                    .map(|(symbol, _)| symbol.clone())
                    .collect();
                BlastLayer {
                    depth: band.depth,
                    nodes_omitted: band
                        .node_count
                        .saturating_sub(u32::try_from(nodes.len()).unwrap_or(u32::MAX)),
                    nodes,
                    node_count: band.node_count,
                    lowest_confidence: band.lowest_confidence,
                }
            })
            .collect();
        let mut response = budget_take(layers, token_budget, blast_layer_tokens);
        if self.unresolved_seeds {
            response.resolution = ResolutionAvailability::Unavailable {
                reason: "no target matched an indexed traversal start".to_string(),
            };
        }
        response.walk_incomplete = incomplete;
        BlastRadius {
            seeds: self.seeds.into_iter().map(|(symbol, _)| symbol).collect(),
            unmatched_targets: self.unmatched,
            layers: response,
            total_impacted: self.total_impacted,
        }
    }
}

/// Divide one caller-supplied budget across `explore`'s four parts.
///
/// Halves and quarters, computed before any work, so the split is deterministic
/// and reportable. `edges_per_direction` is filled in later — it cannot be
/// known until the packer has decided how many definitions there are to divide
/// the edge pool between.
fn explore_budget(total: u32) -> ExploreBudget {
    let definitions = total / 2;
    let blast_radius = total / 4;
    ExploreBudget {
        total,
        definitions,
        edges_per_direction: 0,
        blast_radius,
    }
}

/// Split the edge pool evenly across every direction of every definition shown.
///
/// Deliberately not floored at one edge's worth: a floor would let a large
/// answer exceed the budget the caller set, and `DevMapClient._budgeted` treats
/// the budget as a hard contract. A direction that gets nothing still reports
/// `total` — "0 shown of 42 callers" is a complete answer to "how many", which
/// is the question the count exists for.
fn edges_per_direction(budget: &ExploreBudget, shown: u32) -> u32 {
    let pool = budget
        .total
        .saturating_sub(budget.definitions)
        .saturating_sub(budget.blast_radius);
    let directions = shown.saturating_mul(2);
    if directions == 0 {
        return 0;
    }
    pool / directions
}

/// Token cost of one packed definition: its source span plus a fixed overhead,
/// charged at the same [`BYTES_PER_TOKEN`] every other surface uses.
fn explore_definition_tokens(definition: &ExploreDefinition) -> u32 {
    u32::try_from(definition.source.len() / BYTES_PER_TOKEN as usize)
        .unwrap_or(u32::MAX)
        .saturating_add(EXPLORE_DEFINITION_OVERHEAD_TOKENS)
}

/// Token cost of one blast-radius band: its listed node ids plus a small header.
fn blast_layer_tokens(layer: &BlastLayer) -> u32 {
    let bytes: usize = layer.nodes.iter().map(|node| node.len() + 1).sum();
    u32::try_from(bytes / BYTES_PER_TOKEN as usize)
        .unwrap_or(u32::MAX)
        .saturating_add(10)
}

/// Token cost of one affected-test row: path, listed symbols, and a header.
fn affected_test_tokens(test: &AffectedTest) -> u32 {
    let bytes: usize = test.path.len()
        + test
            .symbols
            .iter()
            .map(|symbol| symbol.len() + 1)
            .sum::<usize>();
    u32::try_from(bytes / BYTES_PER_TOKEN as usize)
        .unwrap_or(u32::MAX)
        .saturating_add(10)
}

/// Whether `path` names a test file.
///
/// Deliberately stricter than the Python predicate it replaces, which asked
/// `"/test" in "/" + path` and so counted `src/testing_utils.py`,
/// `lib/latest/mod.rs` and any path containing the substring anywhere as tests.
/// Here a directory must be *exactly* a test directory, and a file must carry a
/// recognised test affix. Recorded in `DIVERGENCES.md`: the affected-test list
/// gets shorter and the entries that leave were never tests.
pub fn is_test_path(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    let Some((directories, file_name)) = normalized.rsplit_once('/') else {
        return is_test_file_name(&normalized);
    };
    let in_test_directory = directories.split('/').any(|segment| {
        matches!(
            segment.to_lowercase().as_str(),
            "test" | "tests" | "spec" | "specs" | "__tests__" | "testing" | "e2e"
        )
    });
    in_test_directory || is_test_file_name(file_name)
}

/// Whether a bare file name carries a test affix.
///
/// Affixes only, never bare substrings, and the boundary is the point: `test`
/// as a suffix of a *word* (`contest`, `attestation`, `latest`) is not a test
/// affix, and `testing_utils.py` is not `test_utils.py`. The camel-case arm
/// reads the original casing, so `FooTest.java` is recognised while `contest`
/// is not — which is why the name is not lower-cased wholesale first.
fn is_test_file_name(file_name: &str) -> bool {
    let lowered = file_name.to_lowercase();
    let stem = file_name.split('.').next().unwrap_or(file_name);
    let lowered_stem = lowered.split('.').next().unwrap_or(&lowered);
    lowered.starts_with("test_")
        || lowered.starts_with("spec_")
        || lowered.contains(".test.")
        || lowered.contains(".spec.")
        || lowered.contains("_test.")
        || lowered.contains("_spec.")
        || lowered.contains("-test.")
        || lowered.contains("-spec.")
        || matches!(lowered_stem, "test" | "tests" | "spec" | "specs")
        || stem.ends_with("Test")
        || stem.ends_with("Tests")
        || stem.ends_with("Spec")
        || stem.ends_with("Specs")
}

/// Fold one reached symbol into the nearest-test table.
///
/// A free function rather than a closure so the table can be read back in the
/// same scope it is built in.
fn record_test_hit(
    nearest: &mut BTreeMap<String, (usize, BTreeSet<String>)>,
    symbol: &str,
    file: &str,
    depth: usize,
) {
    if !is_test_path(file) {
        return;
    }
    let entry = nearest
        .entry(file.to_string())
        .or_insert((depth, BTreeSet::new()));
    entry.0 = entry.0.min(depth);
    entry.1.insert(symbol.to_string());
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
        walk_incomplete: None,
    }
}

pub(crate) fn byte_span_to_line_range(source: &str, span: &Span) -> (u32, u32) {
    // Delegates the counting to `Span::line_range`, which is the canonical
    // owner and is already UTF-8-safe.
    //
    // This function used to count newlines itself with `source[..start]` —
    // slicing a `&str`, which **panics** on an index that is not a character
    // boundary. Spans are byte offsets recorded at extraction time while
    // `source` is re-read from disk when the graph is exported, so any
    // multi-byte character inserted before an indexed symbol's end offset put
    // the offset mid-character: one emoji added to a file aborted
    // `dev map manifest` outright, and the release profile is `panic = "abort"`,
    // so there was no recovery.
    //
    // Two copies of one computation existed and only one was safe. The wrapper
    // survives for the single thing it adds beyond the canonical version — the
    // `max(start)` below — and no longer restates the arithmetic.
    let clamped = Span {
        start_byte: span.start_byte.min(source.len()),
        // A stored span whose end precedes its start would otherwise report an
        // end line above its start line. Clamping keeps the range orderable for
        // the consumers that render it as `line..end_line`.
        end_byte: span
            .end_byte
            .min(source.len())
            .max(span.start_byte.min(source.len())),
    };
    clamped.line_range(source)
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
/// Both search paths use this as the ceiling on *materialised* hits — the ones
/// whose source is read off disk. Keyword search draws its candidates from a
/// wider page ([`search_rank_pool_size`]) and cuts to this after ranking;
/// semantic search bounds how far down its ranking it materialises hits before
/// budgeting them. Two copies of the arithmetic would let the same budget mean
/// different page sizes depending on which command asked.
fn budget_page_size(token_budget: u32) -> usize {
    (token_budget / SEARCH_HIT_OVERHEAD_TOKENS)
        .saturating_add(1)
        .max(1) as usize
}

/// How many candidates keyword search pulls from the store before ranking them.
///
/// The store orders its page by bm25 and this crate ranks by exact/prefix/other
/// match, so the page has to be wider than the answer or the second ranking
/// only ever sees what the first one liked. Ten times is comfortably past the
/// gap the audit measured (an exact match 100 rows below the cut on a 200-match
/// query) without being a licence to walk the corpus: the pool is a hard
/// ceiling, and when the match set outruns it `search` says so on
/// `walk_incomplete` rather than presenting a sample's best as the corpus's.
const SEARCH_RANK_OVERSAMPLE: usize = 10;

/// Hard ceiling on that pool, whatever the budget asks for.
///
/// Every pooled row costs a `String` comparison and no file read, so 2,000 is
/// cheap; it is here so one query can never scan an unbounded number of FTS
/// rows on a corpus where the query matches everything.
const SEARCH_RANK_POOL_MAX: usize = 2_000;

fn search_rank_pool_size(token_budget: u32) -> usize {
    let page = budget_page_size(token_budget);
    // Never below the page: a pool smaller than what the budget could show
    // would drop results the caller has already paid for.
    page.saturating_mul(SEARCH_RANK_OVERSAMPLE)
        .min(SEARCH_RANK_POOL_MAX)
        .max(page)
}

/// Why a dead-symbol list is a lower bound, or `None` when it is not.
///
/// Two independent reasons, joined rather than ranked — a reader deciding
/// whether to act on "delete this" needs every qualification the run holds, not
/// the first one that fired. `None` on a converged analysis with every call
/// attributed is the load-bearing case: a marker that appears on every answer
/// leaves a caller exactly where it started.
fn dead_symbol_coverage_gap(
    analysis: Option<&devmap_analyze::model::AnalysisDisclosure>,
) -> Option<String> {
    use devmap_analyze::model::AnalysisStatus;
    // A generation exists but its analysis blob does not read back. That is a
    // check that could not run, and it must not answer like one that ran.
    let Some(analysis) = analysis else {
        return Some(
            "the analysis summary for this generation could not be read, so the coverage \
             behind these findings is unknown"
                .to_string(),
        );
    };
    let status = match &analysis.status {
        AnalysisStatus::Ok => None,
        AnalysisStatus::Partial { reason } => Some(format!("the analysis is partial: {reason}")),
        AnalysisStatus::Timeout { reason } => Some(format!("the analysis timed out: {reason}")),
    };
    let unresolved = (analysis.unresolved_calls > 0).then(|| {
        format!(
            "{} call(s) in this generation are unattributed: any of them could be the \
             caller of a symbol listed here, so this list is a lower bound",
            analysis.unresolved_calls
        )
    });
    devmap_analyze::combine_reasons(status, unresolved)
}

/// Rank of one stored symbol against an already-lowercased query.
///
/// The single owner of the ordering, for `search` and for `explore` alike: two
/// copies would let the same query return a different "best match" depending on
/// which command asked. It runs on the stored row rather than on a built
/// [`SymbolHit`] precisely so that ranking can happen before the file reads do
/// — which is what lets the candidate pool be wider than the answer without
/// costing the caller anything.
fn name_match_score(row: &devmap_store::StoredSymbol, query_lower: &str) -> f32 {
    let name = row.name.to_lowercase();
    if name == query_lower || row.qualified_name.to_lowercase() == query_lower {
        1.0
    } else if name.starts_with(query_lower) {
        0.95
    } else {
        0.8
    }
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
        walk_incomplete: None,
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
            walk_incomplete: None,
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
        walk_incomplete: None,
    }
}

#[cfg(all(test, feature = "parse"))]
mod tests {
    use super::*;
    use devmap_extract::extract_file;
    use devmap_resolve::Resolver;

    /// Every `EdgeKind`, so a variant added to the enum cannot slip past the
    /// two spellings below without failing here.
    ///
    /// `edge_kind_name`'s own `match` has no wildcard arm, so a new variant is
    /// a compile error there; this array is what stops a new variant from being
    /// *added to the enum and to that match* while going untested. The length
    /// assertion below is what stops the array itself from silently shrinking.
    const ALL_EDGE_KINDS: [EdgeKind; 14] = [
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
    ];

    /// `edge_kind_name` must spell a kind exactly as `traverse_graph` records
    /// it, and exactly as `stored_edge_to_resolved` parses it back.
    ///
    /// This is load-bearing, not cosmetic. `traversed_resolution_edges` selects
    /// the walk's edges out of the generation by comparing
    /// `edge_kind_name(kind)` against the `format!("{:?}", kind)` string
    /// `traverse_graph` put in `EdgeIdentity::edge_kind`. If those two ever
    /// disagree for one variant, every edge of that kind silently fails the
    /// membership test and `impact` reports "no callers" — a positive claim
    /// produced by a comparison that never matched, which is exactly the
    /// failure mode this codebase treats as worse than an empty answer. There
    /// is no output to observe it in: the answer is well-formed and wrong.
    ///
    /// Three spellings are tied together here — the `Debug` derive, the
    /// `edge_kind_name` table, and the store's string parser — so no two of
    /// them can drift apart unnoticed.
    #[test]
    fn edge_kind_name_is_the_spelling_traverse_graph_records() {
        let mut seen = std::collections::BTreeSet::new();
        for kind in ALL_EDGE_KINDS {
            let name = edge_kind_name(kind);
            assert_eq!(
                name,
                format!("{kind:?}"),
                "edge_kind_name disagrees with the Debug spelling traverse_graph \
                 records in EdgeIdentity::edge_kind; traversed_resolution_edges \
                 would drop every {kind:?} edge and report an empty blast radius"
            );
            let round_tripped = stored_edge_to_resolved(StoredEdge {
                source_file: "a.py".to_string(),
                target_file: "b.py".to_string(),
                source_symbol: "a.py::from".to_string(),
                target_symbol: "b.py::to".to_string(),
                edge_kind: name.to_string(),
                confidence: 1.0,
            })
            .expect("the store must parse back the name this table emits");
            assert_eq!(
                round_tripped.edge_kind, kind,
                "the store parses {name:?} as a different kind than it names"
            );
            assert!(seen.insert(name), "two kinds share the name {name:?}");
        }
        assert_eq!(
            seen.len(),
            ALL_EDGE_KINDS.len(),
            "the kind table must hold one distinct name per variant"
        );
        assert_eq!(ALL_EDGE_KINDS.len(), 14, "a variant was added or removed");
    }

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

    /// A depth-capped walk must not be reported as proof that no path exists.
    ///
    /// `shortest_path` returned a bare `Option`, so "the frontier was pruned at
    /// `max_depth`", "the node cap stopped the walk", "the budget was zero" and
    /// "the reachable set really does not contain the target" were one value.
    /// `trace_between` then stated the strongest of those as fact —
    /// `no indexed path from {from} to {to}` — and an agent concluded two
    /// symbols were unrelated when the path was simply longer than `--depth`.
    #[test]
    fn a_depth_capped_trace_is_not_reported_as_proof_that_no_path_exists() {
        // a -> b -> c -> d is three hops; ask for two.
        let chain = vec![
            path_edge("a", "b", 0.9),
            path_edge("b", "c", 0.9),
            path_edge("c", "d", 0.9),
        ];
        match shortest_path(&chain, "a", "d", 2, 5_000, &Cancel::new()).expect("uncancelled") {
            PathSearch::Exhausted { depth_capped, .. } => {
                assert!(depth_capped, "the depth cap is what stopped this walk");
            }
            other => panic!("a depth-capped walk must report Exhausted, got {other:?}"),
        }

        // With the same graph and enough depth, the answer is the path.
        match shortest_path(&chain, "a", "d", 3, 5_000, &Cancel::new()).expect("uncancelled") {
            PathSearch::Found(path) => assert_eq!(path.len(), 3),
            other => panic!("the path is reachable at depth 3, got {other:?}"),
        }

        // A target that genuinely is not in the reachable set is NoPath, and
        // must stay distinguishable from the capped case above.
        let disjoint = vec![path_edge("a", "b", 0.9), path_edge("y", "z", 0.9)];
        match shortest_path(&disjoint, "a", "z", 64, 5_000, &Cancel::new()).expect("uncancelled") {
            PathSearch::NoPath => {}
            other => panic!("an exhausted reachable set is NoPath, got {other:?}"),
        }

        // The node cap is its own reason, not the depth cap's.
        match shortest_path(&chain, "a", "d", 64, 1, &Cancel::new()).expect("uncancelled") {
            PathSearch::Exhausted { node_capped, .. } => {
                assert!(node_capped, "the node cap is what stopped this walk");
            }
            other => panic!("a node-capped walk must report Exhausted, got {other:?}"),
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
        let PathSearch::Found(path) =
            shortest_path(&edges, "a", "d", 2, 5_000, &Cancel::new()).expect("uncancelled")
        else {
            panic!("two-hop path");
        };
        assert_eq!(
            path.iter()
                .map(|edge| edge.target_symbol.as_str())
                .collect::<Vec<_>>(),
            ["c", "d"]
        );
        assert!(
            matches!(
                shortest_path(&edges, "a", "d", 1, 5_000, &Cancel::new()).expect("uncancelled"),
                PathSearch::Exhausted {
                    depth_capped: true,
                    ..
                }
            ),
            "depth 1 cannot reach a two-hop target, and that is a cap, not a fact \
             about the graph"
        );

        let mut with_direct = edges;
        with_direct.push(path_edge("a", "d", 0.1));
        let PathSearch::Found(path) =
            shortest_path(&with_direct, "a", "d", 2, 5_000, &Cancel::new()).expect("uncancelled")
        else {
            panic!("direct path");
        };
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

        assert!(
            !matches!(found, PathSearch::Found(_)),
            "the destination is not in the graph, got {found:?}"
        );
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
        let PathSearch::Found(first) =
            shortest_path(&edges, "a", "d", 2, 5_000, &Cancel::new()).expect("uncancelled")
        else {
            panic!("two-hop path");
        };
        for _ in 0..8 {
            let PathSearch::Found(again) =
                shortest_path(&edges, "a", "d", 2, 5_000, &Cancel::new()).expect("uncancelled")
            else {
                panic!("two-hop path");
            };
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
    /// `explore` opens one file per definition it returns, not per candidate.
    ///
    /// The candidate pool is `budget_page_size(definition_budget)` rows — 100
    /// at the default 8,000-token budget — and a `SymbolHit` is what reads a
    /// file. Materialising the pool and cutting it to `limit` afterwards would
    /// open a hundred files to keep three, which is the amplification
    /// `search_semantic` was repaired for. The ranking runs on the stored rows,
    /// which already carry both names it scores.
    #[test]
    fn explore_reads_one_file_per_definition_it_returns_not_per_candidate() {
        use devmap_analyze::analyze;
        use devmap_store::Store;

        const SYMBOLS: usize = 200;
        const LIMIT: usize = 3;

        let mut source = String::new();
        for index in 0..SYMBOLS {
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
        let report = StoreQueryEngine::new(&store)
            .explore("widget", LIMIT, 8_000, 0.0, 1)
            .expect("explore");
        let reads = SOURCE_SPAN_READS.with(|reads| reads.get());

        assert_eq!(
            report.definitions.total, SYMBOLS as u32,
            "every match must still be counted in `total`"
        );
        assert!(
            report.definitions.shown <= LIMIT as u32,
            "the limit must bound what is returned, got {}",
            report.definitions.shown
        );
        assert!(
            reads <= LIMIT,
            "explore opened {reads} files to return {} definitions",
            report.definitions.shown
        );
    }

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

    /// Widening the candidate pool must cost string comparisons, not file
    /// reads.
    ///
    /// Keyword search now draws `SEARCH_RANK_OVERSAMPLE` times the page from
    /// the store so its ranking is not confined to what bm25 liked. That is
    /// only free because ranking runs on the stored row and the cut to
    /// `budget_page_size` happens *before* anything is materialised. Score the
    /// pool after building the hits instead — the obvious refactor — and this
    /// query opens ten times as many files to discard nine tenths of them.
    #[test]
    fn keyword_search_reads_only_as_many_files_as_the_budget_could_show() {
        use devmap_analyze::analyze;
        use devmap_store::Store;

        const SYMBOLS: usize = 500;
        const BUDGET: u32 = 200;

        let mut source = String::new();
        for index in 0..SYMBOLS {
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

        let pool = search_rank_pool_size(BUDGET);
        assert!(
            pool > budget_page_size(BUDGET),
            "the point of the pool is that it is wider than the page"
        );

        SOURCE_SPAN_READS.with(|reads| reads.set(0));
        let response = StoreQueryEngine::new(&store)
            .search(Request {
                query: "widget".to_string(),
                token_budget: BUDGET,
                min_confidence: 0.0,
                max_depth: 1,
            })
            .expect("keyword search");
        let reads = SOURCE_SPAN_READS.with(|reads| reads.get());

        assert_eq!(response.total, SYMBOLS as u32);
        assert!(response.shown > 0);
        assert!(
            reads <= budget_page_size(BUDGET),
            "ranked a pool of {pool} and read {reads} files for a page of {}",
            budget_page_size(BUDGET)
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
