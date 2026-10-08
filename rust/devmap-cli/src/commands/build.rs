use std::path::PathBuf;

use devmap_extract::collect_project_manifests;
use devmap_resolve::{Resolver, UnresolvedClass};
use devmap_store::{current_git_head, GenerationWriteOpts, Store, GENERATION_RETENTION};

use crate::cli::{default_root_hint, Cli, InventoryFlags, StampFlags};
use crate::commands::manifest::{
    manifest_json, report_manifest, resolve_graph_output, resolve_map_output,
    write_consumer_artifacts, ManifestOutcome, ManifestRequest,
};
use crate::commands::status::store_status_fields;
use crate::output::{emit_json, ensure_parent};
use crate::progress;
use crate::reporter::ProgressReporter;

#[derive(clap::Args)]
#[group(id = "Build")]
pub(crate) struct Args {
    #[arg(default_value_os_t = default_root_hint())]
    pub(crate) path: PathBuf,
    #[arg(long)]
    pub(crate) affected: Option<String>,
    #[arg(long)]
    pub(crate) deleted: Option<String>,
    /// Force a cold rebuild: ignore the unchanged early-return, re-parse
    /// every source instead of reading the extraction cache, and write a
    /// full generation.
    ///
    /// K4: the Python CLI has exposed `dev map --full` all along, but the
    /// kernel had no way to honour it — the unchanged check and the cache
    /// both applied unconditionally, so the only recovery from a store an
    /// operator distrusted was to delete the database. `--affected` cannot
    /// stand in: it narrows the write, it does not widen the read.
    #[arg(long)]
    pub(crate) full: bool,
    /// Re-derive the validity of every stored edge and unresolved call,
    /// instead of comparing only the files whose freshly resolved rows
    /// disagree with the digest schema 19 recorded beside the previous
    /// generation.
    ///
    /// The cheap half of `--full`. `--full` re-parses every source, which
    /// on a large repository is minutes; this keeps the incremental
    /// extraction and widens only the comparison the *write* makes, which
    /// is the ~200 ms the scoping saves. It is the recovery for a store
    /// whose digests an operator distrusts, and it is what
    /// `digest_scoped_delta.rs` compares the scoped path against.
    ///
    /// `--full` implies it: an empty affected set is the full-rewrite
    /// signal, and a full rewrite never scopes.
    #[arg(long)]
    pub(crate) verify_rows: bool,
    /// Also write `repo_map.json` and `code_graph.json` from the generation
    /// this build leaves current, in this same process.
    ///
    /// The seam ran `build` and then `manifest` as two invocations, which
    /// meant two process launches, two store opens, and — because the
    /// second process cannot see what the first decided — a full
    /// re-serialization of a 22 MB code graph on every tick where nothing
    /// had changed. Fused, the unchanged case is one open and one stat of
    /// each artifact.
    ///
    /// `devmap manifest` stays a command of its own: writing the artifacts
    /// from a store somebody else built is a real request, and a caller
    /// that wants it should not have to run a build to get it.
    #[arg(long)]
    pub(crate) manifest: bool,
    /// Unset, resolved against `path`'s state directory. See
    /// `devmap_extract::paths`.
    #[arg(long, requires = "manifest")]
    pub(crate) output: Option<PathBuf>,
    /// Unset, resolved against `path`'s state directory.
    #[arg(long, requires = "manifest")]
    pub(crate) graph_output: Option<PathBuf>,
    /// Also write the marker-guarded agent guides. See `manifest --guides`.
    #[arg(long, requires = "manifest")]
    pub(crate) guides: bool,
    /// Replace a Python-schema or otherwise foreign repo map / code graph.
    #[arg(long, requires = "manifest", default_value_t = false)]
    pub(crate) force: bool,
    /// Opt-in language-server edges for sites the syntax resolver left
    /// unresolved. Off unless passed. Servers on PATH (rust-analyzer,
    /// gopls, pyright, typescript-language-server) may add
    /// `LanguageServer` / `LanguageServerDispatch` edges; a missing or
    /// hung server is reported as did-not-run and never as zero edges
    /// from a run. Unresolved sites stay in the ledger unless a server
    /// actually names a target inside the repository — there is no
    /// bare-name fallback.
    #[arg(long)]
    pub(crate) lsp: bool,
    #[command(flatten)]
    pub(crate) stamps: StampFlags,
    #[command(flatten)]
    pub(crate) inventory: InventoryFlags,
}

pub(crate) fn run(
    cli: &Cli,
    progress: Option<&ProgressReporter>,
    args: &Args,
) -> anyhow::Result<()> {
    let Args {
        // Read through `cli.root_hint()` below, which already falls back to
        // this positional when `--root` is absent. Binding it here as well
        // is what let the two disagree.
        path: _,
        affected: affected_flag,
        deleted,
        full,
        verify_rows,
        manifest: write_manifest,
        output,
        graph_output,
        guides,
        force,
        lsp,
        stamps,
        inventory,
    } = args;
    // `--root` outranks the positional path, for the same reason it
    // does in `paths`: `cli.db()` already resolves the store from
    // `--root`, so reading the repository from `path` made one build
    // name two repositories and abort with "DevMap store belongs to
    // worktree X, not Y" before extracting anything.
    //
    // The hook is the caller that breaks on it. `detach_build` spawns
    // `devmap --root <project> build` from whatever directory the agent
    // happens to be in, with stdout and stderr on /dev/null — so on
    // every edit outside the repository root the rebuild exited 1 into
    // nothing, the lock was taken and released, and the index silently
    // stopped following the tree while every hook still reported
    // success.
    let path = &cli.root_hint();
    let progress = progress.expect("main supplies a build reporter");
    let build_started = std::time::Instant::now();
    progress.stage(
        1,
        format_args!("scanning and extracting {}", path.display()),
    );
    ensure_parent(&cli.db())?;
    // K13: take the cross-process writer lock *before* extraction.
    //
    // There was no such lock, so two builds — or a build and the
    // daemon's drain — raced on SQLite's five-second `busy_timeout`
    // alone and the loser surfaced `database is locked` only at the
    // persist, having already paid for the whole extract and resolve.
    // Taking it first means the loser waits for the winner and then
    // does useful work, or fails immediately with a message naming the
    // pid that holds the store.
    let _writer = progress.timed("writer:wait", || {
        Store::lock_writer_at(&cli.db(), Store::WRITER_LOCK_WAIT)
    })?;
    let store = progress.timed("store:open", || Store::open(cli.db()))?;
    store.bind_repo_root(path)?;
    // A store this process can only read opens fine — queries need it
    // to — and would otherwise fail at the first write with a bare
    // SQLite code, after paying for the whole scan. Refuse before the
    // scan, in the store's own words.
    if store.is_read_only() {
        anyhow::bail!(
            "devmap store {} is read-only: the file or its directory is not writable \
             by this process, so it can be queried but not rebuilt",
            cli.db().display()
        );
    }
    // Capture the durable queue boundary before discovery.
    //
    // A build that walks the whole tree answers every request queued at
    // or before this instant, whatever that request named — which is
    // the only rule that retires a row naming a *directory*. Taken
    // before the walk, never after: an event that arrives while this
    // build is extracting may describe an edit it did not see, and that
    // row has to survive.
    let build_start = store.pending_watermark()?;

    // K7: refuse an `--affected` path inside a tagged build cache.
    //
    // Discovery no longer walks these directories, so such a path can
    // only mean the caller computed the wrong change set — a watcher or
    // hook that saw cargo write into its own output tree. Failing loud
    // is the point: silently narrowing to nothing, or silently indexing
    // a `.fingerprint/*.json`, is how 1,041 of 2,363 indexed files came
    // to be build artifacts.
    let mut caches = devmap_extract::CacheDirectoryCache::default();
    for candidate in split_csv(affected_flag) {
        match caches.tagged_ancestor(path, &candidate) {
            devmap_extract::CacheVerdict::Inside(cache) => anyhow::bail!(
                "--affected names {candidate}, which is inside {cache} — a build \
                 cache marked with CACHEDIR.TAG. devmap does not index build \
                 caches; drop it from the change set."
            ),
            // Raw command-line input, so this is the one caller that
            // can actually be handed `../x` or `/abs/x`. Such a path
            // cannot be checked for a tagged ancestor at all, and it
            // cannot name a file this build would index either, so
            // failing loud beats narrowing the change set in silence.
            devmap_extract::CacheVerdict::NotRepoRelative(why) => anyhow::bail!(
                "--affected names {candidate}, which {why}. Paths must be \
                 relative to the repository root {}; drop it from the change set.",
                path.display()
            ),
            devmap_extract::CacheVerdict::Unreadable { directory, reason } => anyhow::bail!(
                "--affected names {candidate}, whose cache marker {directory}/CACHEDIR.TAG could not be examined: {reason}"
            ),
            devmap_extract::CacheVerdict::Outside => {}
        }
    }

    // K1(e): drop pending rows no drain could ever process, before
    // deciding anything else. A queue full of paths under a previous
    // location of the repository, directories, and files over the size
    // ceiling held `devmap status` at `is_fresh=false` permanently —
    // and the build, which is the one command that could know better,
    // did not touch the queue at all. This runs on every build
    // including the unchanged early return below, because a store whose
    // sources have not moved is exactly where a stale queue hides.
    let reconciled = progress.timed("pending:reconcile", || store.reconcile_pending_paths(path))?;
    if !reconciled.dropped.is_empty() {
        progress.display.diagnostic(format_args!(
            "  pending queue: dropped {} unprocessable row(s):",
            reconciled.dropped.len()
        ));
        for (dropped, reason) in reconciled.dropped.iter().take(20) {
            progress
                .display
                .diagnostic(format_args!("    {dropped}: {reason}"));
        }
        if reconciled.dropped.len() > 20 {
            progress.display.diagnostic(format_args!(
                "    … and {} more",
                reconciled.dropped.len() - 20
            ));
        }
    }
    if !reconciled.rewritten.is_empty() {
        progress.display.diagnostic(format_args!(
            "  pending queue: normalized {} row(s) to repo-relative paths",
            reconciled.rewritten.len()
        ));
    }

    // Discovery, once, before anything decides whether to extract.
    //
    // The unchanged check below needs only `(path, content_hash)`, and
    // that is a pure function of the bytes discovery already read — so
    // scanning first lets a no-change build answer without paying for
    // an extraction round-trip per file (measured on this repository:
    // 213–254 ms of a ~300 ms no-op scan, every byte of it discarded).
    // `--full` reuses the same scan rather than walking and reading the
    // corpus a second time.
    progress.display.detail("discovering source files");
    let scan_progress = std::sync::Arc::new(devmap_extract::progress::FileProgress::default());
    progress
        .display
        .files("reading", std::sync::Arc::clone(&scan_progress));
    let scanned = progress.timed("scan:read", || {
        devmap_extract::scan_tree_with_progress(path, Some(&scan_progress))
    })?;
    let scan_snapshot = scan_progress.snapshot();
    progress.display.detail("checking content hashes");
    // Report what discovery refused. A file dropped for being oversized
    // or unreadable used to vanish with no record: `repo_map.json` would
    // say five files while two more existed, and nothing distinguished
    // "not in this repository" from "refused by the indexer". Which
    // skips count as loss is decided by `DiscoverySkipReason::is_refusal`
    // and nowhere else — the daemon reads the same report and must reach
    // the same verdict, and it cannot do that against a copy of the rule.
    let refused: Vec<&(String, devmap_extract::model::DiscoverySkipReason)> =
        scanned.report.refusals().collect();
    if !refused.is_empty() {
        // Both numbers in the header. A bare list of twenty under a
        // count of two hundred is a capped sample presented as the set,
        // which is the one thing this codebase never lets a report do.
        let shown = refused.len().min(REFUSAL_SAMPLE);
        progress.display.diagnostic(format_args!(
            "  discovery refused {} file(s) — these are absent from the graph \
             (showing {shown} of {}):",
            refused.len(),
            refused.len()
        ));
        for (path, reason) in refused.iter().take(REFUSAL_SAMPLE) {
            progress
                .display
                .diagnostic(format_args!("    {path}: {reason:?}"));
        }
        if refused.len() > REFUSAL_SAMPLE {
            progress.display.diagnostic(format_args!(
                "    … and {} more",
                refused.len() - REFUSAL_SAMPLE
            ));
        }
    }
    let refused_count = refused.len();

    // B3/SC2: if the tree that was just scanned is byte-for-byte the one
    // already committed, the graph it would produce is the graph that is
    // already stored — the determinism gate guarantees identical inputs
    // give an identical graph. Resolving and analysing it again costs
    // 54% of the build (measured: resolve 2,145 ms + analyze 835 ms of a
    // 5.5 s rebuild on 1,610 files) to arrive at what is already there.
    // This is the case a watcher hits on every tick where nothing
    // relevant changed.
    //
    // "Identical inputs give an identical graph" holds for one kernel,
    // not across two. An upgraded extractor reads the same bytes and
    // produces a different graph — that is what an extraction schema
    // bump *is* — so content hashes alone would report "still current"
    // over a generation this kernel would never have written. Measured
    // on DevCouncil: the first `dev map` after two schema bumps printed
    // "No source changes; generation #412 still current (1,152 files)"
    // while every row in it came from `extract-v23`.
    //
    // The comparison itself is made against the scan rather than
    // against extractions. `ScannedTree::matches_file_hashes` compares
    // the same `(path, content_hash)` pairs the extractions carry —
    // every `Extraction` is built with `content_hash(source)` over the
    // bytes discovery read, and a cached payload is only ever served
    // for a key built from those same bytes and a matching `file_path`
    // — so the verdict is the one extraction would have produced, for
    // the cost of an FNV pass instead of 1,311 store round-trips.
    let previous = progress.timed("scan:previous_hashes", || store.latest_file_hashes())?;
    let file_delta = scanned.file_delta(&previous);
    if !*full
        && !previous.is_empty()
        && previous.len() == scanned.sources.len()
        && store.latest_generation_payload_is_current()?
    {
        let unchanged = file_delta.is_unchanged();
        if unchanged {
            let file_count = scanned.sources.len();
            progress.stage(
                2,
                format_args!("{} unchanged", progress::count(file_count, "file")),
            );
            let generation = store.latest_generation_id()?.unwrap_or(0);
            // K2: reclaim runs on the warm path too.
            //
            // This return used to jump past prune and vacuum entirely,
            // so a store that had accumulated a large freelist stayed
            // that way through every no-change build — and a no-change
            // build is the common case for a watcher-driven repository.
            // `vacuum_if_needed` declines below the threshold on its
            // own, so the warm path pays nothing when there is nothing
            // to reclaim. Generations are *not* pruned here: no
            // generation was written, so there is nothing new to prune,
            // and pruning on a read-shaped path would delete history a
            // caller did not ask to lose.
            let vacuum = progress.timed("persist:vacuum", || store.vacuum_if_needed())?;
            if vacuum
                .checkpoint
                .is_none_or(|checkpoint| checkpoint.busy != 0)
            {
                progress
                    .display
                    .diagnostic(format_args!("reclaim: {}", reclaim_note(&vacuum)));
            } else {
                progress.note(format_args!("reclaim: {}", reclaim_note(&vacuum)));
            }
            // K1(e2): the unchanged check compared *every* file in the
            // tree against the stored generation and found them equal.
            // That is the same proof a fresh whole-tree build gives —
            // the graph on disk already describes this tree — so the
            // requests queued before this build started are answered,
            // even though no new generation was written. Without this a
            // repository that is already current keeps a stale queue,
            // and `status` reports NOT FRESH indefinitely.
            let retired = store.clear_pending_superseded(
                devmap_store::PendingSupersede::WholeTreeThrough(&build_start),
            )?;
            if !retired.is_empty() {
                progress.note(format_args!(
                    "pending queue: retired {} row(s) the current generation \
                     already answers",
                    retired.len()
                ));
            }
            // Provenance, not a new graph. The hashes above proved this
            // generation still describes the tree; HEAD may have moved
            // (empty commit, identical-tree checkout) while no file
            // did. Leaving the stamp behind is what made `status`
            // report NOT FRESH after a skip that had nothing to do.
            let head = current_git_head(path).unwrap_or_else(|_| "unavailable".to_string());
            store.restamp_latest_head(&head)?;
            // The artifacts, from the generation this build just
            // proved current. On this path the stamp almost always
            // holds, so nothing is read out of the store and nothing is
            // written — which is the entire saving: `manifest` used to
            // re-serialize a 22 MB code graph here to produce bytes
            // identical to the ones already on disk.
            if *write_manifest {
                progress.display.detail("checking consumer artifacts");
            }
            let manifest = build_manifest_payload(
                cli,
                &progress.display,
                &store,
                *write_manifest,
                path,
                &resolve_map_output(output, path),
                &resolve_graph_output(graph_output, path),
                *guides,
                *force,
                stamps,
                *inventory,
            )?;
            drop(_writer);
            progress.up_to_date(generation, file_count);
            if cli.json {
                // Built through `serde_json` and carrying `timings`,
                // like every other build result.
                //
                // This was a hand-written format string with no
                // timings key at all — so the *most frequent* build in
                // the system, the one a watcher runs on almost every
                // tick, was the one build shape a profiler could not
                // see. It is not an empty truth either: this path
                // hashes every file in the tree to prove nothing
                // changed, and it runs the reclaim decision, both of
                // which are already timed stages.
                emit_json(
                    cli,
                    &serde_json::json!({
                        "unchanged": true,
                        "file_progress": { "scan": scan_snapshot, "extraction": null, "delta": file_delta },
                        "files": file_count,
                        // Recomputed by this scan, not carried over: a
                        // build that proves nothing changed has just
                        // re-asked discovery the same question, and the
                        // answer is part of what it proved.
                        "discovery_refused_files": refused_count,
                        "generation": generation,
                        "reclaim": reclaim_note(&vacuum),
                        "timings": progress.timings_json(),
                        "progress_output": progress.display.output_json(),
                        // `null` when `--manifest` was not asked for,
                        // never an empty object: a caller must be able
                        // to tell "not requested" from "wrote nothing".
                        "manifest": manifest.as_ref().map(|manifest| &manifest.json),
                    }),
                )?;
            } else {
                progress.display.summary(
                    "Already mapped",
                    &[format!(
                        "No source changes; generation #{generation} still current \
                     ({}) in {}.",
                        progress::count(file_count, "file"),
                        progress::duration(progress.started_at.elapsed().as_secs_f64())
                    )],
                );
                progress.display.report_loss();
                if cli.verbose {
                    if let Some(manifest) = &manifest {
                        report_manifest(cli, &manifest.outcome)?;
                    }
                }
                if cli.verbose && !progress.display.enabled() {
                    outln!("  Reclaim: {}", reclaim_note(&vacuum));
                }
            }
            return Ok(());
        }
    }

    // Only now, with the tree known to have moved, is extraction worth
    // its cost.
    //
    // K4: `--full` re-parses rather than consulting the extraction
    // cache. Reading the cache would defeat the point — a cache hit
    // returns the payload this build is trying to reproduce from
    // source, so a "full" rebuild that used it would recommit exactly
    // the rows the operator is asking to replace.
    progress.display.detail(&format!(
        "extracting {}",
        progress::count(scanned.sources.len(), "file")
    ));
    let extraction_progress =
        std::sync::Arc::new(devmap_extract::progress::FileProgress::default());
    progress
        .display
        .files("extracting", std::sync::Arc::clone(&extraction_progress));
    let extractions = progress.timed("extract:files", || {
        if *full {
            let refs: Vec<devmap_extract::FileRef<'_>> = scanned
                .sources
                .iter()
                .map(|(file, source)| devmap_extract::FileRef {
                    path: file.as_str(),
                    source: source.as_str(),
                })
                .collect();
            Ok(devmap_extract::extract_all_with_progress(
                &refs,
                Some(&extraction_progress),
            ))
        } else {
            devmap_store::extract_scanned_for_generation(
                &store,
                &scanned,
                Some(&extraction_progress),
            )
        }
    })?;
    let extraction_snapshot = extraction_progress.snapshot();
    // The corpus text is dead the moment extraction has consumed it,
    // but it is bound in this scope and would otherwise stay resident
    // through resolve, analyze and persist — the stages that set the
    // peak. It is the one cost the scan-before-extract split would
    // otherwise have added, and it is not hypothetical: measured A/B on
    // scholarlm (4,278 files), holding it cost 23 MiB of peak RSS.
    //
    // `--lsp` is the exception: converting an unresolved call span into
    // an LSP position needs the source bytes, so they are held through
    // the opt-in pass and dropped immediately after.
    //
    // The *report* has to outlive it — `discovery_refusals` below turns
    // it into the analysis disclosure — so this destructures rather
    // than dropping the pair, and only the source text goes.
    let devmap_extract::ScannedTree {
        sources,
        report: discovery,
    } = scanned;
    let lsp_sources = if *lsp {
        Some(sources)
    } else {
        drop(sources);
        None
    };

    // B3/SC2: `affected` narrows what this generation *writes*. It no
    // longer narrows what is *resolved*.
    //
    // Resolution reads two genuinely global maps — the symbol index and
    // the type/method index — and both are keyed by symbol *name*. So a
    // file's edges can only change if its own source changed, or if a
    // name it mentions was defined or removed somewhere else. That is
    // the whole dependency surface, and it makes the affected set
    // computable: the changed files, plus every file mentioning a name
    // whose definition moved. The store partitions edges by source file
    // and carries the rest forward, so narrowing the write stays sound
    // and unaffected extractions are copied rather than re-serialized.
    //
    // Narrowing the *resolution* was not sound. `analyze` receives
    // whatever this produces, and liveness and community detection are
    // global by nature: they answer "does anything call this symbol"
    // and "what clusters with what", questions no subset of the edges
    // can answer. Measured on a 155-file fixture, one edited file
    // handed the analyser 63 edges instead of 15,017, and the
    // generation was committed with 433 dead-code candidates instead of
    // 14 and 138 communities instead of 17 — `CharClass.contains`,
    // `match_from` and `parse_class`, all plainly called, recorded as
    // callerless. `devmap dead` then reports them to whoever asks.
    //
    // The stored *edges* were correct throughout, which is why
    // `an_incremental_build_equals_a_cold_build` stayed green: it
    // compared the graph, and the graph was never the part that broke.
    // It now compares the generation.
    //
    // Resolving the whole tree costs what B3 saved on a changed build.
    // That is the price of an analysis that means the same thing on
    // both paths, and the no-change tick B3 was written for still
    // returns above without reaching here.
    // K4: `--full` writes a full generation. An empty affected set *is*
    // the full-rewrite signal to the store, so the closure — whose only
    // job is to narrow the write — is not computed at all.
    let affected = if *full {
        None
    } else {
        progress.timed("affected:closure", || {
            affected_closure(&store, &extractions)
        })?
    };

    let mut resolver = Resolver::new();
    let manifests = progress.timed("discovering project manifests", || {
        collect_project_manifests(path)
    })?;
    progress.timed("resolver:index", || {
        resolver.index_project_manifests(&manifests);
        resolver.index_extractions(&extractions);
        // After `index_extractions`, which resets the set this extends.
        resolver.mark_go_dirs_incomplete(devmap_extract::go_dirs_with_unindexed_files(
            path,
            extractions.iter().map(|ext| ext.file_path.as_str()),
        ));
        Ok::<(), std::convert::Infallible>(())
    })?;
    progress.stage(
        2,
        format_args!("resolving {}", progress::count(extractions.len(), "file")),
    );
    let mut resolution = resolver.resolve_all(&extractions)?;
    let mut lsp_report = None;
    if *lsp {
        progress.display.detail("language-server pass (opt-in)");
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let sources_vec = lsp_sources.expect("--lsp retained the scanned sources");
        let sources: std::collections::BTreeMap<String, String> = sources_vec.into_iter().collect();
        let report = progress.timed("resolve:lsp", || {
            Ok::<_, anyhow::Error>(
                devmap_resolve::lsp::enrich_with_language_servers_with_sources(
                    path,
                    &extractions,
                    &sources,
                    &mut resolution,
                    &cancel,
                ),
            )
        })?;
        for line in report.status_lines() {
            progress.display.diagnostic(format_args!("{line}"));
        }
        lsp_report = Some(report);
        drop(sources);
    }
    progress.stage(
        3,
        format_args!(
            "analyzing {}",
            progress::count(resolution.edges.len(), "resolved edge")
        ),
    );
    // The refusal count reaches the analysis, not just stderr. A file
    // discovery turned away has no `Extraction`, so nothing computed
    // from that slice can see it — and the one thing that most needs to
    // is the dead-code pass, because the file never read may hold the
    // only call to a symbol this build is about to call dead. The
    // persisted `AnalysisSummary` carries the degraded status onward, so
    // a later `devmap manifest` reading the store inherits it rather
    // than recomputing a clean answer.
    // The inventory, not just its size. `discovery_refused_files` is
    // `COUNT(*)` over these rows once they are persisted, and the
    // daemon's drain carries them forward path by path rather than
    // carrying a number it can only ever raise.
    let refusal_inventory = devmap_store::discovery_refusals(&discovery);
    let analysis = devmap_analyze::analyze_with_discovery(
        &extractions,
        &resolution,
        devmap_analyze::DiscoveryCoverage::refused(refusal_inventory.len()),
    );

    let opts = GenerationWriteOpts {
        affected_paths: match (&affected, split_csv(affected_flag)) {
            // `--full` overrides both: an empty list is the
            // full-rewrite signal and must not be narrowed by a stale
            // `--affected` from the caller.
            _ if *full => Vec::new(),
            // An explicit --affected list wins; otherwise use the
            // computed closure. Empty means a full rewrite.
            (_, explicit) if !explicit.is_empty() => explicit,
            (Some(set), _) => set.iter().cloned().collect(),
            (None, _) => Vec::new(),
        },
        deleted_paths: split_csv(deleted),
        // Canonical, so a query process resolves node paths against a
        // real absolute root rather than whatever `.` meant at build time.
        repo_root: path
            .canonicalize()
            .ok()
            .map(|root| root.to_string_lossy().into_owned()),
        build_started: Some(build_started),
        discovery_refusals: Some(refusal_inventory),
        verify_every_row: *verify_rows,
    };
    let head_sha = current_git_head(path).unwrap_or_else(|_| "unavailable".to_string());
    progress.stage(
        4,
        format_args!(
            "persisting {} symbols and {} edges",
            analysis.total_symbols, analysis.total_edges
        ),
    );
    // Persistence is timed in four parts rather than one. It is the
    // third-largest phase of a cold build after extraction and
    // resolution (28% on a 4,089-file corpus), but it is larger on an
    // *incremental* build than on a cold one — 2,448 ms against
    // 1,490 ms on this repository — which is backwards on its face and
    // is the visible edge of B3. A single "persisting" span could not
    // say which of the write, the two prunes, or the VACUUM was
    // responsible, and they have nothing in common as fixes.
    //
    // These four sub-phases were the only trustworthy part of the old
    // breakdown: they are measured by `timed`, which brackets its own
    // work, while the top-level stages were shifted by one until K8.
    // An earlier version of this comment called persistence "the
    // largest phase of a build" — that was read off the shifted
    // attribution and was never true.
    // Split by relation, because the phase as one number cannot be
    // acted on: v18 put the edges and the unresolved ledger on validity
    // ranges and left the nodes, the full-text map, the file rows, the
    // dead symbols and the coverage gaps as full per-generation copies,
    // and those have nothing in common as fixes. The store measures the
    // split — the node and full-text inserts are one interleaved loop,
    // so nothing out here can separate them.
    let gen_id = progress.timed_split("persist:write", || {
        store
            .save_generation_timed(&extractions, &resolution, &analysis, opts, &head_sha)
            .map(|(gen_id, spent)| {
                let parts = spent
                    .parts()
                    .into_iter()
                    .map(|(label, seconds)| (label.to_string(), seconds))
                    .collect();
                (gen_id, parts)
            })
    })?;

    // Every generation carries a full carry-forward copy of the
    // repository. Without this the store grows by O(repository size)
    // per build forever (SC1) — the daemon and the CLI both commit
    // generations, so both must bound retention. Reclaim afterwards:
    // deleting rows returns pages to the freelist, not to the
    // filesystem, so a pruned database otherwise never shrinks.
    progress.timed("persist:prune_generations", || {
        store.prune_generations_except_latest(GENERATION_RETENTION)
    })?;
    // After the generations go, drop cached extractions none of the
    // survivors reference (SC7). Order matters: this reads
    // generation_files, so it must see the pruned set.
    progress.timed("persist:prune_extractions", || {
        store.prune_extraction_cache()
    })?;

    // K1(e): the generation is committed, so the queued requests to
    // re-read the files it covers are answered. Leaving them queued made
    // `devmap status` report the store as stale immediately after a
    // successful build, and made the next drain resolve the whole
    // repository again to reproduce rows that already existed.
    // A build narrowed by an explicit `--affected` list read only what
    // it was told to; it cannot claim to have answered anything else.
    let indexed: Vec<String> = extractions
        .iter()
        .map(|extraction| extraction.file_path.clone())
        .collect();
    let narrowed = !*full && !split_csv(affected_flag).is_empty();
    let retired = store.clear_pending_superseded(if narrowed {
        devmap_store::PendingSupersede::IndexedPathsThrough(&indexed, &build_start)
    } else {
        devmap_store::PendingSupersede::WholeTreeThrough(&build_start)
    })?;
    if !retired.is_empty() {
        progress.note(format_args!(
            "pending queue: retired {} row(s) this generation supersedes",
            retired.len()
        ));
    }
    let vacuum = progress.timed("persist:vacuum", || store.vacuum_if_needed())?;
    // Report what the reclaim decided, not just how long it took. A
    // decline and a reclaim-that-reclaimed-nothing both take ~0 ms and
    // leave the same file behind, so the duration alone cannot tell a
    // healthy store from one growing without bound.
    if vacuum
        .checkpoint
        .is_none_or(|checkpoint| checkpoint.busy != 0)
    {
        progress
            .display
            .diagnostic(format_args!("reclaim: {}", reclaim_note(&vacuum)));
    } else {
        progress.note(format_args!("reclaim: {}", reclaim_note(&vacuum)));
    }

    // SC18: report the tiers separately. One undifferentiated count
    // made 380k structurally-unresolvable calls — language builtins,
    // runtime-supplied globals, values the calling function declares
    // itself, and names an import proves are outside the corpus —
    // indistinguishable from the failures that indicate a real defect.
    // Only `unattributed` is worth acting on.
    let mut builtin_calls = 0usize;
    let mut host_global_calls = 0usize;
    let mut local_binding_calls = 0usize;
    let mut external_calls = 0usize;
    let mut uninferred_receiver_calls = 0usize;
    let mut no_namesake_calls = 0usize;
    let mut module_path_calls = 0usize;
    let mut unattributed_calls = 0usize;
    for reference in &resolution.unresolved {
        match reference.class {
            UnresolvedClass::Builtin => builtin_calls += 1,
            UnresolvedClass::HostGlobal { .. } => host_global_calls += 1,
            UnresolvedClass::LocalBinding => local_binding_calls += 1,
            UnresolvedClass::External { .. } => external_calls += 1,
            UnresolvedClass::UninferredReceiver => uninferred_receiver_calls += 1,
            UnresolvedClass::NoNamesake => no_namesake_calls += 1,
            UnresolvedClass::ModulePath => module_path_calls += 1,
            UnresolvedClass::Unresolved => unattributed_calls += 1,
        }
    }

    if *write_manifest {
        progress.stage(5, "writing consumer artifacts");
    }
    let manifest = build_manifest_payload(
        cli,
        &progress.display,
        &store,
        *write_manifest,
        path,
        &resolve_map_output(output, path),
        &resolve_graph_output(graph_output, path),
        *guides,
        *force,
        stamps,
        *inventory,
    )?;
    drop(_writer);
    progress.complete(gen_id);
    if cli.json {
        emit_json(
            cli,
            &serde_json::json!({
                "generation_id": gen_id,
                "file_progress": { "scan": scan_snapshot, "extraction": extraction_snapshot, "delta": file_delta },
                "files_indexed": analysis.total_files,
                // Its own number, never folded into the parse-failure
                // count: a refused file is fixed by making it smaller or
                // readable, a parse failure by a grammar, and an operator
                // reading one total cannot tell which they have.
                "discovery_refused_files": refused_count,
                "symbols": analysis.total_symbols,
                "edges": analysis.total_edges,
                "dead_candidates": analysis.dead_symbols.iter().filter(|d| !d.is_exempt).count(),
                "communities": analysis.communities.len(),
                "unresolved_calls": analysis.unresolved_calls,
                "unresolved_builtin": builtin_calls,
                "unresolved_host_global": host_global_calls,
                "unresolved_local_binding": local_binding_calls,
                "unresolved_external": external_calls,
                "unresolved_uninferred_receiver": uninferred_receiver_calls,
                "unresolved_no_namesake": no_namesake_calls,
                "unresolved_module_path": module_path_calls,
                "unresolved_unattributed": unattributed_calls,
                // The arithmetic over the counters above, done
                // once and published, rather than left to a reader who
                // will not do it. `net` excludes the misses that are
                // explained — a language builtin, a runtime global, a
                // name an import proves is outside the corpus, a bare
                // name with no corpus namesake, or a local module path —
                // and is the figure worth ratcheting.
                "resolution_rate": analysis.resolution_rate,
                // The per-stage breakdown, so a caller profiling a slow
                // build reads it from the result rather than scraping
                // the human progress lines off stderr.
                "timings": progress.timings_json(),
                "progress_output": progress.display.output_json(),
                // `null` when `--lsp` was not asked for: a caller must
                // tell "not requested" from "ran and added zero edges".
                "lsp": lsp_report.as_ref().map(|report| {
                    serde_json::json!({
                        "edges_added": report.edges_added,
                        "sites_resolved": report.sites_resolved,
                        "sites_left_unresolved": report.sites_left_unresolved,
                        "servers": report.servers.iter().map(|server| {
                            serde_json::json!({
                                "binary": server.binary,
                                "version": server.version,
                                "ran": server.ran,
                                "did_not_run": server.did_not_run.as_ref().map(|reason| reason.label()),
                                "files_considered": server.files_considered,
                                "files_finished": server.files_finished,
                                "edges_added": server.edges_added,
                            })
                        }).collect::<Vec<_>>(),
                    })
                }),
                "manifest": manifest.as_ref().map(|manifest| &manifest.json),
            }),
        )?;
    } else {
        let mut summary = vec![
            format!(
                "Built generation #{gen_id} · {} · {} · {} · {}",
                progress::count(analysis.total_files, "file"),
                progress::count(analysis.total_symbols, "symbol"),
                progress::count(analysis.total_edges, "edge"),
                progress::duration(progress.started_at.elapsed().as_secs_f64())
            ),
            format!(
                "  Changes: +{} ~{} -{} · {} unchanged · {} cached",
                file_delta.added,
                file_delta.changed,
                file_delta.removed,
                file_delta.unchanged,
                extraction_snapshot.cache_hits
            ),
        ];
        if refused_count > 0 {
            summary.push(format!(
                "    of which refused by discovery: {refused_count} \
                 (recorded as lost coverage, not parsed)"
            ));
        }
        if !cli.verbose && (unattributed_calls > 0 || uninferred_receiver_calls > 0) {
            summary.push(format!("  Unresolved: {unattributed_calls} unattributed, {uninferred_receiver_calls} uninferred receivers (details: --verbose)"));
        }
        progress.display.summary("Map ready", &summary);
        progress.display.report_loss();
        if cli.verbose {
            outln!("  Files indexed: {}", analysis.total_files);
        }
        if cli.verbose {
            outln!("  Symbols extracted: {}", analysis.total_symbols);
            outln!("  Edges resolved: {}", analysis.total_edges);
            // R5: a call we could not attribute is reported, not dropped.
            outln!("  Unresolved calls: {}", analysis.unresolved_calls);
            print_resolution_rate(&analysis.resolution_rate);
            outln!("    language builtins:  {builtin_calls}");
            outln!("    host globals:       {host_global_calls}");
            outln!("    local bindings:     {local_binding_calls}");
            outln!("    external imports:   {external_calls}");
            outln!("    uninferred receiver:{uninferred_receiver_calls}");
            outln!("    no namesake:        {no_namesake_calls}");
            outln!("    module path:        {module_path_calls}");
            outln!("    unattributed:       {unattributed_calls}");
            if let Some(manifest) = &manifest {
                report_manifest(cli, &manifest.outcome)?;
            }
        }
    }
    // The answer is out and this process exits next. Freeing the build's
    // heap graph one allocation at a time is what a caller waiting on the
    // exit was waiting for: a teardown probe on DevCouncil (1,310 files)
    // timed the drops after the JSON line at `resolution` 48–598 ms,
    // `extractions` 14–304 ms and the resolver index 12–67 ms (min–max,
    // nine edits, loaded host), against 1–19 ms for closing the store.
    // These four own memory and nothing else — no file, lock, thread or
    // `Drop` with an effect — so the OS reclaims them at exit instead.
    // The store still drops normally: closing it releases its locks and
    // checkpoints the WAL. A/B and probe in BENCHMARK_HARDENING_AUDIT.md.
    std::mem::forget((resolution, resolver, extractions, analysis));
    Ok(())
}

/// One line describing what a reclaim decided, did, and whether it landed.
///
/// K2: the reclaim note used to report the decision and the page accounting and
/// stop there, while `vacuum_if_needed` discarded its checkpoint result with
/// `let _ =`. In WAL mode that checkpoint is what moves a truncation from the
/// log into the file, so "reclaimed 50,000 pages" and "reclaimed 50,000 pages
/// and the file is exactly as large as it was" printed identically — which is
/// how a store sat at 295 MB across eight builds that each reported success.
fn reclaim_note(vacuum: &devmap_store::VacuumOutcome) -> String {
    let base = format!(
        "{} freed {} page(s) at {:.1}% free ({} of {} pages)",
        vacuum.action,
        vacuum.pages_freed,
        vacuum.freelist_ratio() * 100.0,
        vacuum.freelist_before,
        vacuum.page_count_before,
    );
    match vacuum.checkpoint {
        None => format!("{base}; WAL checkpoint could not be run — freed pages stay in the log"),
        Some(checkpoint) if checkpoint.busy != 0 => format!(
            "{base}; WAL checkpoint busy after the {:?} fallback ({} of {} frames) — \
             a reader is pinning the log, so the file has not shrunk yet",
            checkpoint.mode, checkpoint.checkpointed_frames, checkpoint.log_frames
        ),
        Some(checkpoint) => format!(
            "{base}; WAL {:?} checkpoint folded {} of {} frames back",
            checkpoint.mode, checkpoint.checkpointed_frames, checkpoint.log_frames
        ),
    }
}

/// How many refused paths a build names on stderr before eliding the rest.
///
/// A sample, and said to be one: the header carries `shown` and the true total
/// so a reader can never mistake the list for the set. See
/// `StoreStatus::quarantined_paths`, which caps the same way for the same
/// reason.
const REFUSAL_SAMPLE: usize = 20;

/// The `--manifest` half of a build: the artifacts and the store's own status,
/// as one JSON object, or `None` when the build was not asked to write them.
///
/// Both are computed from the store this build already has open. That is the
/// whole point of the flag: the seam used to run `build`, then `manifest`, then
/// `status` — three processes, three store opens — to answer one question about
/// one generation.
struct BuildManifestPayload {
    json: serde_json::Value,
    outcome: ManifestOutcome,
}

#[allow(clippy::too_many_arguments)]
fn build_manifest_payload(
    cli: &Cli,
    progress: &progress::Display,
    store: &Store,
    enabled: bool,
    path: &std::path::Path,
    output: &std::path::Path,
    graph_output: &std::path::Path,
    guides: bool,
    force: bool,
    stamps: &StampFlags,
    inventory: InventoryFlags,
) -> anyhow::Result<Option<BuildManifestPayload>> {
    if !enabled {
        return Ok(None);
    }
    let outcome = write_consumer_artifacts(
        store,
        ManifestRequest {
            progress: Some(progress),
            path,
            db: &cli.db(),
            output,
            graph_output,
            compact_graph_output: None,
            force,
            stamps,
            inventory: inventory.into(),
            guides,
        },
    )?;
    let mut payload = manifest_json(&outcome);
    // The store's own view, so a caller does not need a third process to learn
    // whether the generation it just built is fresh, degraded or backed up
    // behind a pending queue.
    payload["status"] = serde_json::Value::Object(store_status_fields(store, &cli.db())?);
    Ok(Some(BuildManifestPayload {
        json: payload,
        outcome,
    }))
}

fn split_csv(raw: &Option<String>) -> Vec<String> {
    raw.as_ref()
        .map(|s| {
            s.split(',')
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Suggested source-file write set, or `None` for a full generation write.
///
/// Resolution reads two global maps — the symbol index and the type/method
/// index — and both are keyed by symbol *name*. A file's edges can therefore
/// only change if its own content changed, or if a name it mentions was
/// defined or removed elsewhere. The closure is the changed files plus every
/// file mentioning such a name.
///
/// Resolution and analysis always receive the whole repository. This set only
/// narrows persistence; the store independently checks content and row digests
/// before carrying anything forward.
///
/// Returns `None` (meaning "write everything") whenever the cheap, safe
/// answer is unavailable: no previous generation, a file added or deleted, or
/// a closure so large that narrowing it saves nothing. Falling back to a full
/// write is always correct; the danger is only ever narrowing too far.
fn affected_closure(
    store: &Store,
    extractions: &[devmap_extract::model::Extraction],
) -> anyhow::Result<Option<std::collections::BTreeSet<String>>> {
    use std::collections::{BTreeMap, BTreeSet};

    let previous_hashes = store.latest_file_hashes()?;
    if previous_hashes.is_empty() {
        return Ok(None); // No prior generation: this is a cold build.
    }
    // Nothing stored may be reused when the kernel that stored it is not the
    // kernel running now. Content hashes are unchanged across an extractor
    // upgrade — that is exactly the case this catches — so asking them first
    // would report "nothing changed" over payloads that are entirely stale.
    if !store.latest_generation_payload_is_current()? {
        return Ok(None);
    }
    // An added or removed file changes the file set itself; take the full path
    // rather than reason about it.
    if previous_hashes.len() != extractions.len() {
        return Ok(None);
    }

    let mut changed: BTreeSet<String> = BTreeSet::new();
    for extraction in extractions {
        match previous_hashes.get(&extraction.file_path) {
            Some(hash) if *hash == extraction.content_hash => {}
            Some(_) => {
                changed.insert(extraction.file_path.clone());
            }
            // A path present now but not before is an addition.
            None => return Ok(None),
        }
    }
    if changed.is_empty() {
        return Ok(Some(BTreeSet::new()));
    }

    // Names the changed files define now, against the names they defined
    // before. The symmetric difference is every name whose definition moved.
    let previous_symbols = store.latest_symbol_names_by_file()?;
    let mut changed_names: BTreeSet<String> = BTreeSet::new();
    let mut current_symbols: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for extraction in extractions {
        current_symbols.insert(
            extraction.file_path.as_str(),
            extraction
                .symbols
                .iter()
                .map(|symbol| symbol.name.as_str())
                .collect(),
        );
    }
    for file in &changed {
        let empty = BTreeSet::new();
        let before = previous_symbols.get(file).unwrap_or(&empty);
        let after = current_symbols
            .get(file.as_str())
            .cloned()
            .unwrap_or_default();
        for name in before {
            if !after.contains(name.as_str()) {
                changed_names.insert(name.clone());
            }
        }
        for name in after {
            if !before.contains(name) {
                changed_names.insert(name.to_string());
            }
        }
    }

    let mut affected = changed;
    if !changed_names.is_empty() {
        for extraction in extractions {
            if affected.contains(&extraction.file_path) {
                continue;
            }
            let mentions = extraction
                .calls
                .iter()
                .any(|call| changed_names.contains(&call.callee_name))
                || extraction
                    .references
                    .iter()
                    .any(|reference| changed_names.contains(&reference.name))
                || extraction.imports.iter().any(|import| {
                    import
                        .imported_names
                        .iter()
                        .any(|name| changed_names.contains(name))
                });
            if mentions {
                affected.insert(extraction.file_path.clone());
            }
        }
    }

    // Narrowing only pays when it actually narrows.
    if affected.len() * 2 >= extractions.len() {
        return Ok(None);
    }
    Ok(Some(affected))
}

/// Render the resolution rate under the human build summary.
///
/// Per language and sorted worst-first, because the corpus figure is not
/// actionable and the ordering is the whole point: a language sitting at zero
/// is a missing extractor, and it should be the first line a reader sees rather
/// than one they have to find. This is the readout that would have surfaced
/// W0.2's bug class — CFML and Terraform contributing no call edges while
/// reporting complete coverage — without anyone going looking for it.
fn print_resolution_rate(rate: &devmap_analyze::ResolutionRate) {
    let Some(net) = rate.net_permille else {
        // No site was attempted. Saying "0.0%" here would report a failure that
        // never happened; the `Option` exists precisely to keep the two apart.
        outln!("  Resolution rate: not measured (no attribution sites)");
        return;
    };
    outln!(
        "  Resolution rate: {}.{}% net, {}.{}% gross ({} resolved / {} unresolved, {} explained)",
        net / 10,
        net % 10,
        rate.gross_permille.map(|g| g / 10).unwrap_or(0),
        rate.gross_permille.map(|g| g % 10).unwrap_or(0),
        rate.resolved_sites,
        rate.unresolved_sites,
        rate.explained_sites,
    );

    let mut rows: Vec<(&String, &devmap_analyze::LanguageResolution)> =
        rate.by_language.iter().collect();
    // Worst first; a language that attempted nothing sorts last rather than
    // first, because `None` is "not measured" and not "measured at zero".
    rows.sort_by_key(|(language, row)| (row.net_permille.unwrap_or(u32::MAX), (*language).clone()));
    for (language, row) in rows.iter().take(RESOLUTION_RATE_LANGUAGES_SHOWN) {
        match row.net_permille {
            Some(net) => outln!(
                "    {language:<12} {}.{}%  ({} resolved / {} unresolved)",
                net / 10,
                net % 10,
                row.resolved_sites,
                row.unresolved_sites
            ),
            None if !row.extracts_calls => {
                outln!("    {language:<12} no call extractor in this build (0 attribution sites)")
            }
            None => outln!("    {language:<12} not measured (no attribution sites)"),
        }
        // Printed beside a *measured* row too, and that is the point: a language
        // can attribute calls perfectly and still have no heritage extractor, so
        // "no Extends edges in this corpus" reads as a fact about the code when
        // it is a fact about the build. `extracts_calls` already made this
        // sentence for one bit; the other three had no reader at all.
        let blind: Vec<&str> = row
            .blind_to
            .iter()
            .map(String::as_str)
            .filter(|name| *name != "calls")
            .collect();
        if !blind.is_empty() {
            outln!(
                "    {:<12} …and this build extracts no {} for it, so an empty \
                 answer there is a hole and not a finding",
                "",
                blind.join("/")
            );
        }
    }
    if rows.len() > RESOLUTION_RATE_LANGUAGES_SHOWN {
        outln!(
            "    … {} more language(s); the full breakdown is in `--json`",
            rows.len() - RESOLUTION_RATE_LANGUAGES_SHOWN
        );
    }
}

/// How many languages the human readout names before deferring to `--json`.
///
/// Capped because a 35-language corpus would otherwise bury the build summary,
/// and the truncation is *stated* rather than silent — a capped list presented
/// as a whole one is the same error this work order exists to correct, one
/// level down.
const RESOLUTION_RATE_LANGUAGES_SHOWN: usize = 8;

#[cfg(test)]
mod tests {
    use super::*;

    /// CSV path lists drop blanks and keep every real entry.
    ///
    /// `split_csv` feeds `--affected` and `--deleted`, which decide what a
    /// differential build rewrites. Returning an empty vec makes every build
    /// look like nothing changed; inverting the emptiness filter keeps only
    /// the blanks.
    #[test]
    fn csv_paths_are_trimmed_and_blanks_dropped() {
        assert_eq!(split_csv(&None), Vec::<String>::new());
        assert_eq!(split_csv(&Some(String::new())), Vec::<String>::new());
        assert_eq!(
            split_csv(&Some("a.py, b.py ,, c.py".to_string())),
            vec!["a.py".to_string(), "b.py".to_string(), "c.py".to_string()],
            "entries are trimmed, blanks dropped, and every real path kept"
        );
        assert_eq!(
            split_csv(&Some("only.py".to_string())),
            vec!["only.py".to_string()],
            "a single entry with no comma must survive"
        );
    }
}
