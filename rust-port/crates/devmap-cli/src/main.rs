use std::io::IsTerminal;
use std::path::PathBuf;
use std::time::Instant;

use clap::{Parser, Subcommand, ValueEnum};
use tracing::Level;
use tracing_subscriber::FmtSubscriber;

use devmap_analyze::analyze;
use devmap_extract::collect_go_modules;
use devmap_query::{
    generate_code_graph_json, generate_manifest_with_edges, resolve_manifest_output,
    resolved_edge_from_stored, semantic_snapshots, write_code_graph_atomically,
    write_manifest_atomically, FreshnessInfo, Request, ResolutionAvailability, StoreQueryEngine,
    CODE_GRAPH_DEFAULT_OUTPUT,
};
use devmap_resolve::{Resolver, UnresolvedClass};
use devmap_serve::{default_ipc_path_for, Daemon};
use devmap_store::{
    current_git_head, extract_tree_cached_with_report, GenerationWriteOpts, Store,
    GENERATION_RETENTION,
};

#[derive(Parser)]
#[command(
    name = "devmap",
    author,
    version,
    about = "DevCouncil code-intelligence system (Rust kernel)"
)]
struct Cli {
    #[arg(short, long, default_value = ".devcouncil/codeintel/index.sqlite")]
    db: PathBuf,

    #[arg(long, default_value_t = false)]
    json: bool,

    /// Build progress policy. Auto writes progress to stderr only for an interactive terminal.
    #[arg(long, value_enum, default_value_t = ProgressMode::Auto)]
    progress: ProgressMode,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ProgressMode {
    Auto,
    Always,
    Never,
}

struct ProgressReporter {
    enabled: bool,
    started_at: Instant,
}

impl ProgressReporter {
    const TOTAL_STAGES: usize = 5;

    fn new(mode: ProgressMode, json: bool) -> Self {
        let enabled = match mode {
            ProgressMode::Auto => !json && std::io::stderr().is_terminal(),
            ProgressMode::Always => true,
            ProgressMode::Never => false,
        };
        Self {
            enabled,
            started_at: Instant::now(),
        }
    }

    fn stage(&self, current: usize, message: impl std::fmt::Display) {
        if self.enabled {
            eprintln!("[{current}/{}] {message}", Self::TOTAL_STAGES);
        }
    }

    fn complete(&self, generation_id: u32) {
        self.stage(
            Self::TOTAL_STAGES,
            format_args!(
                "complete: generation #{generation_id} in {:.2}s",
                self.started_at.elapsed().as_secs_f64()
            ),
        );
    }
}

#[derive(Subcommand)]
enum Commands {
    /// Cold or incremental build of the code-intelligence graph
    Build {
        #[arg(default_value = ".")]
        path: PathBuf,
        #[arg(long)]
        affected: Option<String>,
        #[arg(long)]
        deleted: Option<String>,
    },
    Search {
        query: String,
        #[arg(short, long, default_value_t = 2000)]
        budget: u32,
    },
    Deps {
        file: String,
        #[arg(short, long, default_value_t = 2000)]
        budget: u32,
        #[arg(long, default_value_t = 0.0)]
        min_confidence: f32,
    },
    Impact {
        target: String,
        #[arg(short, long, default_value_t = 2000)]
        budget: u32,
        #[arg(long, default_value_t = 3)]
        depth: usize,
    },
    Trace {
        from: String,
        to: Option<String>,
        #[arg(short, long, default_value_t = 2000)]
        budget: u32,
        #[arg(long, default_value_t = 3)]
        depth: usize,
    },
    Dead {
        #[arg(short, long, default_value_t = 2000)]
        budget: u32,
    },
    /// Write the consumer artifacts: `repo_map.json` and its symbol-level
    /// companion `code_graph.json`.
    ///
    /// Both come from one invocation because that is how consumers get them
    /// from `dev map`: eleven modules under `src/devcouncil/` read the graph,
    /// and a second subcommand would let a repository sit with a fresh map
    /// beside a stale graph built from a different generation.
    Manifest {
        #[arg(default_value = ".")]
        path: PathBuf,
        #[arg(short, long, default_value = ".devcouncil/repo_map.json")]
        output: PathBuf,
        /// Symbol-level graph companion artifact.
        #[arg(long, default_value = CODE_GRAPH_DEFAULT_OUTPUT)]
        graph_output: PathBuf,
        /// Replace a Python-schema or otherwise foreign repo map / code graph.
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    Status,
    /// Longitudinal view: how the map has moved across recent builds.
    History {
        #[arg(short, long, default_value_t = 10)]
        last: usize,
    },
    Repair {
        #[arg(long)]
        fts: bool,
    },
    Snapshots {
        #[arg(default_value = "")]
        file: String,
        #[arg(short, long, default_value_t = 2000)]
        budget: u32,
    },
    Serve {
        #[arg(default_value = ".")]
        path: PathBuf,
        #[arg(long)]
        socket: Option<PathBuf>,
    },
}

fn open_for_read(db: &std::path::Path) -> anyhow::Result<Store> {
    match Store::open_existing(db)? {
        Some(store) => Ok(store),
        None => Err(anyhow::anyhow!(
            "no devmap store at {} — run `devmap build` first",
            db.display()
        )),
    }
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

fn ensure_parent(path: &std::path::Path) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

fn emit_json(cli: &Cli, payload: &serde_json::Value) -> anyhow::Result<()> {
    if cli.json {
        println!("{}", serde_json::to_string(payload)?);
    } else {
        println!("{}", serde_json::to_string_pretty(payload)?);
    }
    Ok(())
}

fn emit_unavailable(reason: &str) {
    println!("unavailable: {reason}");
}

/// The line a truncated result must carry, if any.
///
/// Split from the printing so the *policy* can be asserted directly. Mutation
/// testing replaced the whole emitter with `()` and flipped every comparison
/// in its condition without a failure, because nothing observed stdout — and
/// this line is the only thing telling a caller their result was capped.
/// PHASE1_CONTRACT.md:17 is explicit: never present a capped sample as
/// complete coverage. A silent emitter does exactly that.
fn truncation_line(shown: u32, hidden: u32, total: u32, truncated: bool) -> Option<String> {
    (truncated || hidden > 0).then(|| format!("shown {shown} of {total} ({hidden} hidden)"))
}

fn emit_truncation(shown: u32, hidden: u32, total: u32, truncated: bool) {
    if let Some(line) = truncation_line(shown, hidden, total, truncated) {
        println!("{line}");
    }
}

fn emit_search(resp: &devmap_query::Response<devmap_query::SymbolHit>) {
    if let ResolutionAvailability::Unavailable { reason } = &resp.resolution {
        emit_unavailable(reason);
        return;
    }
    for hit in &resp.items {
        println!(
            "{}:{}-{}  {}  {}",
            hit.file_path, hit.span.0, hit.span.1, hit.kind, hit.symbol_name
        );
    }
    emit_truncation(resp.shown, resp.hidden, resp.total, resp.truncated);
}

fn emit_edges(resp: &devmap_query::Response<devmap_resolve::ResolvedEdge>) {
    if let ResolutionAvailability::Unavailable { reason } = &resp.resolution {
        emit_unavailable(reason);
        return;
    }
    for edge in &resp.items {
        println!(
            "{}::{}  --{:?}-->  {}::{}",
            edge.source_file,
            edge.source_symbol,
            edge.edge_kind,
            edge.target_file,
            edge.target_symbol
        );
    }
    emit_truncation(resp.shown, resp.hidden, resp.total, resp.truncated);
}

fn emit_dead(resp: &devmap_query::Response<devmap_analyze::DeadSymbolReport>) {
    if let ResolutionAvailability::Unavailable { reason } = &resp.resolution {
        emit_unavailable(reason);
        return;
    }
    for row in &resp.items {
        println!(
            "{:.2}  {}::{}",
            row.confidence, row.file_path, row.symbol_name
        );
    }
    emit_truncation(resp.shown, resp.hidden, resp.total, resp.truncated);
}

/// Files whose edges a change can reach, or `None` for a full resolve.
///
/// Resolution reads two global maps — the symbol index and the type/method
/// index — and both are keyed by symbol *name*. A file's edges can therefore
/// only change if its own content changed, or if a name it mentions was
/// defined or removed elsewhere. The closure is the changed files plus every
/// file mentioning such a name.
///
/// Returns `None` (meaning "resolve everything") whenever the cheap, safe
/// answer is unavailable: no previous generation, a file added or deleted, or
/// a closure so large that narrowing it saves nothing. Falling back to a full
/// resolve is always correct; the danger is only ever narrowing too far.
fn affected_closure(
    store: &Store,
    extractions: &[devmap_extract::model::Extraction],
) -> anyhow::Result<Option<std::collections::BTreeSet<String>>> {
    use std::collections::{BTreeMap, BTreeSet};

    let previous_hashes = store.latest_file_hashes()?;
    if previous_hashes.is_empty() {
        return Ok(None); // No prior generation: this is a cold build.
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber).ok();

    let cli = Cli::parse();

    match &cli.command {
        Commands::Build {
            path,
            affected: affected_flag,
            deleted,
        } => {
            let progress = ProgressReporter::new(cli.progress, cli.json);
            let build_started = std::time::Instant::now();
            progress.stage(
                1,
                format_args!("scanning and extracting {}", path.display()),
            );
            ensure_parent(&cli.db)?;
            let store = Store::open(&cli.db)?;
            let (extractions, discovery) = extract_tree_cached_with_report(&store, path)?;
            // Report what discovery refused. A file dropped for being oversized
            // or unreadable used to vanish with no record: `repo_map.json` would
            // say five files while two more existed, and nothing distinguished
            // "not in this repository" from "refused by the indexer". Only
            // genuine refusals are counted — `NonSource` is the ordinary case of
            // a README next to the code, not a gap in coverage.
            let refused: Vec<&(String, devmap_extract::model::DiscoverySkipReason)> = discovery
                .skipped_paths
                .iter()
                .filter(|(_, reason)| {
                    !matches!(
                        reason,
                        devmap_extract::model::DiscoverySkipReason::NonSource
                    )
                })
                .collect();
            if !refused.is_empty() {
                eprintln!(
                    "  discovery refused {} file(s) — these are absent from the graph:",
                    refused.len()
                );
                for (path, reason) in refused.iter().take(20) {
                    eprintln!("    {path}: {reason:?}");
                }
                if refused.len() > 20 {
                    eprintln!("    … and {} more", refused.len() - 20);
                }
            }

            // B3/SC2: if the tree that was just scanned is byte-for-byte the one
            // already committed, the graph it would produce is the graph that is
            // already stored — the determinism gate guarantees identical inputs
            // give an identical graph. Resolving and analysing it again costs
            // 54% of the build (measured: resolve 2,145 ms + analyze 835 ms of a
            // 5.5 s rebuild on 1,610 files) to arrive at what is already there.
            // This is the case a watcher hits on every tick where nothing
            // relevant changed.
            let previous = store.latest_file_hashes()?;
            if !previous.is_empty() && previous.len() == extractions.len() {
                let unchanged = extractions.iter().all(|extraction| {
                    previous
                        .get(&extraction.file_path)
                        .is_some_and(|hash| *hash == extraction.content_hash)
                });
                if unchanged {
                    progress.stage(2, format_args!("{} files unchanged", extractions.len()));
                    let generation = store.latest_generation_id()?.unwrap_or(0);
                    if cli.json {
                        println!(
                            "{{\"unchanged\":true,\"files\":{},\"generation\":{}}}",
                            extractions.len(),
                            generation
                        );
                    } else {
                        println!(
                            "No source changes; generation #{} still current ({} files).",
                            generation,
                            extractions.len()
                        );
                    }
                    return Ok(());
                }
            }

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
            let affected = affected_closure(&store, &extractions)?;

            let mut resolver = Resolver::new();
            resolver.index_go_modules(&collect_go_modules(path)?);
            resolver.index_extractions(&extractions);
            progress.stage(2, format_args!("resolving {} files", extractions.len()));
            let resolution = resolver.resolve_all(&extractions);
            progress.stage(
                3,
                format_args!("analyzing {} resolved edges", resolution.edges.len()),
            );
            let analysis = analyze(&extractions, &resolution);

            let opts = GenerationWriteOpts {
                affected_paths: match (&affected, split_csv(affected_flag)) {
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
            };
            let head_sha = current_git_head(path).unwrap_or_else(|_| "unavailable".to_string());
            progress.stage(
                4,
                format_args!(
                    "persisting {} symbols and {} edges",
                    analysis.total_symbols, analysis.total_edges
                ),
            );
            let gen_id = store.save_generation_with_metadata(
                &extractions,
                &resolution,
                &analysis,
                opts,
                &head_sha,
            )?;

            // Every generation carries a full carry-forward copy of the
            // repository. Without this the store grows by O(repository size)
            // per build forever (SC1) — the daemon and the CLI both commit
            // generations, so both must bound retention. Reclaim afterwards:
            // deleting rows returns pages to the freelist, not to the
            // filesystem, so a pruned database otherwise never shrinks.
            store.prune_generations_except_latest(GENERATION_RETENTION)?;
            // After the generations go, drop cached extractions none of the
            // survivors reference (SC7). Order matters: this reads
            // generation_files, so it must see the pruned set.
            store.prune_extraction_cache()?;
            store.vacuum_if_needed()?;

            progress.complete(gen_id);

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
            let mut unattributed_calls = 0usize;
            for reference in &resolution.unresolved {
                match reference.class {
                    UnresolvedClass::Builtin => builtin_calls += 1,
                    UnresolvedClass::HostGlobal { .. } => host_global_calls += 1,
                    UnresolvedClass::LocalBinding => local_binding_calls += 1,
                    UnresolvedClass::External { .. } => external_calls += 1,
                    UnresolvedClass::UninferredReceiver => uninferred_receiver_calls += 1,
                    UnresolvedClass::Unresolved => unattributed_calls += 1,
                }
            }

            if cli.json {
                emit_json(
                    &cli,
                    &serde_json::json!({
                        "generation_id": gen_id,
                        "files_indexed": analysis.total_files,
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
                        "unresolved_unattributed": unattributed_calls,
                    }),
                )?;
            } else {
                println!("Successfully built generation #{gen_id}");
                println!("  Files indexed: {}", analysis.total_files);
                println!("  Symbols extracted: {}", analysis.total_symbols);
                println!("  Edges resolved: {}", analysis.total_edges);
                // R5: a call we could not attribute is reported, not dropped.
                println!("  Unresolved calls: {}", analysis.unresolved_calls);
                println!("    language builtins:  {builtin_calls}");
                println!("    host globals:       {host_global_calls}");
                println!("    local bindings:     {local_binding_calls}");
                println!("    external imports:   {external_calls}");
                println!("    uninferred receiver:{uninferred_receiver_calls}");
                println!("    unattributed:       {unattributed_calls}");
            }
        }
        Commands::Search { query, budget } => {
            let store = open_for_read(&cli.db)?;
            let engine = StoreQueryEngine::new(&store);
            let resp = engine.search(Request {
                query: query.clone(),
                token_budget: *budget,
                min_confidence: 0.0,
                max_depth: 1,
            })?;
            if cli.json {
                emit_json(&cli, &serde_json::to_value(&resp)?)?;
            } else {
                emit_search(&resp);
            }
        }
        Commands::Deps {
            file,
            budget,
            min_confidence,
        } => {
            let store = open_for_read(&cli.db)?;
            let engine = StoreQueryEngine::new(&store);
            let resp = engine.dependencies(Request {
                query: file.clone(),
                token_budget: *budget,
                min_confidence: *min_confidence,
                max_depth: 1,
            })?;
            if cli.json {
                emit_json(&cli, &serde_json::to_value(&resp)?)?;
            } else {
                emit_edges(&resp);
            }
        }
        Commands::Impact {
            target,
            budget,
            depth,
        } => {
            let store = open_for_read(&cli.db)?;
            let engine = StoreQueryEngine::new(&store);
            let resp = engine.impact(Request {
                query: target.clone(),
                token_budget: *budget,
                min_confidence: 0.0,
                max_depth: *depth,
            })?;
            if cli.json {
                emit_json(&cli, &serde_json::to_value(&resp)?)?;
            } else {
                emit_edges(&resp);
            }
        }
        Commands::Trace {
            from,
            to,
            budget,
            depth,
        } => {
            let store = open_for_read(&cli.db)?;
            let engine = StoreQueryEngine::new(&store);
            let resp = if let Some(destination) = to {
                engine.trace_between(Request {
                    query: (from.clone(), destination.clone()),
                    token_budget: *budget,
                    min_confidence: 0.0,
                    max_depth: *depth,
                })?
            } else {
                engine.trace(Request {
                    query: from.clone(),
                    token_budget: *budget,
                    min_confidence: 0.0,
                    max_depth: *depth,
                })?
            };
            if cli.json {
                emit_json(&cli, &serde_json::to_value(&resp)?)?;
            } else {
                emit_edges(&resp);
            }
        }
        Commands::Dead { budget } => {
            let store = open_for_read(&cli.db)?;
            let payload = StoreQueryEngine::new(&store).dead_symbols(*budget)?;
            if cli.json {
                emit_json(&cli, &serde_json::to_value(&payload)?)?;
            } else {
                emit_dead(&payload);
            }
        }
        Commands::Manifest {
            path,
            output,
            graph_output,
            force,
        } => {
            let store = open_for_read(&cli.db)?;
            let extractions = store.latest_extractions()?;
            let analysis = store.latest_analysis()?.ok_or_else(|| {
                anyhow::anyhow!("manifest unavailable: build a persisted generation first")
            })?;
            let gen_id = store.latest_generation_id()?.ok_or_else(|| {
                anyhow::anyhow!("manifest unavailable: build a persisted generation first")
            })?;
            let status = store.status(&cli.db.display().to_string())?;
            let built_head = store
                .latest_generation_head()?
                .unwrap_or_else(|| "unavailable".to_string());
            let edges = store
                .latest_edges(0.0)?
                .into_iter()
                .map(resolved_edge_from_stored)
                .collect::<anyhow::Result<Vec<_>>>()?;
            // One freshness identity for both artifacts: a map and a graph
            // stamped from different generations is the drift the single
            // command exists to prevent.
            let freshness = FreshnessInfo {
                head_sha: built_head,
                generation_id: gen_id,
                pending_count: status.pending_count,
            };
            let (_manifest, json_str) =
                generate_manifest_with_edges(&extractions, &analysis, freshness.clone(), &edges);
            let repo_root = store.latest_repo_root()?.or_else(|| {
                path.canonicalize()
                    .ok()
                    .map(|root| root.to_string_lossy().into_owned())
            });
            let graph_json = generate_code_graph_json(
                &extractions,
                &analysis,
                &edges,
                &freshness,
                repo_root.as_deref(),
            )?;

            let dest = resolve_manifest_output(repo_root.as_deref(), output);
            ensure_parent(&dest)?;
            write_manifest_atomically(&dest, &json_str, *force)?;

            let graph_dest = resolve_manifest_output(repo_root.as_deref(), graph_output);
            ensure_parent(&graph_dest)?;
            write_code_graph_atomically(&graph_dest, &graph_json, *force)?;

            if !cli.json {
                println!("Manifest written to {:?}", dest);
                println!("Code graph written to {:?}", graph_dest);
            } else {
                emit_json(
                    &cli,
                    &serde_json::json!({
                        "output": dest,
                        "graph_output": graph_dest,
                        "generation_id": gen_id,
                    }),
                )?;
            }
        }
        Commands::Status => {
            // Answers even with no store, but never creates one. The client
            // treats a missing store as "not built yet"; creating it here made
            // that a race (see `Store::open_existing`).
            let Some(store) = Store::open_existing(&cli.db)? else {
                let payload = serde_json::json!({
                    "generation_id": serde_json::Value::Null,
                    "pending_count": 0,
                    "node_count": 0,
                    "edge_count": 0,
                    "is_fresh": false,
                    "db_path": cli.db.display().to_string(),
                    "degraded_reason": "no devmap store at this path (run `devmap build`)",
                    "quarantined_count": 0,
                });
                println!("{}", serde_json::to_string_pretty(&payload)?);
                return Ok(());
            };
            let status = store.status(&cli.db.display().to_string())?;
            let payload = serde_json::json!({
                "generation_id": status.latest_generation,
                "pending_count": status.pending_count,
                "node_count": status.node_count,
                "edge_count": status.edge_count,
                "is_fresh": status.pending_count == 0,
                "db_path": status.db_path,
                "degraded_reason": status.degraded_reason,
                "quarantined_count": status.quarantined_count,
            });
            emit_json(&cli, &payload)?;
        }
        Commands::History { last } => {
            let store = open_for_read(&cli.db)?;
            let rows = store.build_history(*last)?;

            if cli.json {
                // Deltas are reported against the next-older row, so the oldest
                // row in the window carries none rather than a fabricated zero.
                let entries: Vec<serde_json::Value> = rows
                    .iter()
                    .enumerate()
                    .map(|(index, row)| {
                        let previous = rows.get(index + 1);
                        serde_json::json!({
                            "generation_id": row.generation_id,
                            "built_at": row.built_at,
                            "head_sha": row.head_sha,
                            "files": row.files,
                            "symbols": row.symbols,
                            "edges": row.edges,
                            "dead_confident": row.dead_confident,
                            "dead_ambiguous": row.dead_ambiguous,
                            "parse_failed": row.parse_failed,
                            "languages_covered": row.languages_covered,
                            "build_ms": row.build_ms,
                            "db_bytes": row.db_bytes,
                            "delta": previous.map(|prev| serde_json::json!({
                                "symbols": row.symbols as i64 - prev.symbols as i64,
                                "edges": row.edges as i64 - prev.edges as i64,
                                "dead_confident": row.dead_confident as i64 - prev.dead_confident as i64,
                                "build_ms": match (row.build_ms, prev.build_ms) {
                                    (Some(current), Some(previous)) => Some(current as i128 - previous as i128),
                                    _ => None,
                                },
                            })),
                        })
                    })
                    .collect();
                emit_json(
                    &cli,
                    &serde_json::json!({ "shown": rows.len(), "history": entries }),
                )?;
            } else if rows.is_empty() {
                println!("No build history yet — run `devmap build` first.");
            } else {
                println!(
                    "{:>5}  {:<12} {:>7} {:>8} {:>8} {:>6} {:>9} {:>8}",
                    "gen", "head", "files", "symbols", "edges", "dead", "build_ms", "db_MiB"
                );
                for (index, row) in rows.iter().enumerate() {
                    let head: String = row.head_sha.chars().take(12).collect();
                    let delta = rows
                        .get(index + 1)
                        .map(|prev| {
                            format!(
                                "  ({:+} sym, {:+} edge, {:+} dead)",
                                row.symbols as i64 - prev.symbols as i64,
                                row.edges as i64 - prev.edges as i64,
                                row.dead_confident as i64 - prev.dead_confident as i64,
                            )
                        })
                        .unwrap_or_default();
                    println!(
                        "{:>5}  {:<12} {:>7} {:>8} {:>8} {:>6} {:>9} {:>8.2}{}",
                        row.generation_id,
                        head,
                        row.files,
                        row.symbols,
                        row.edges,
                        row.dead_confident,
                        row.build_ms
                            .map_or_else(|| "-".to_string(), |value| value.to_string()),
                        row.db_bytes as f64 / (1024.0 * 1024.0),
                        delta
                    );
                }
            }
        }
        Commands::Repair { fts } => {
            let store = open_for_read(&cli.db)?;
            if *fts {
                store.repair_fts()?;
                if !cli.json {
                    println!("FTS search index repaired.");
                }
            } else {
                anyhow::bail!("specify a repair target, e.g. --fts");
            }
        }
        Commands::Snapshots { file, budget } => {
            let store = open_for_read(&cli.db)?;
            let extractions = store.latest_extractions()?;
            let resp = semantic_snapshots(
                &extractions,
                Request {
                    query: file.clone(),
                    token_budget: *budget,
                    min_confidence: 0.0,
                    max_depth: 1,
                },
            );
            emit_json(&cli, &serde_json::to_value(&resp)?)?;
        }
        Commands::Serve { path, socket } => {
            ensure_parent(&cli.db)?;
            let store = Store::open(&cli.db)?;
            let ipc_path = socket.clone().unwrap_or_else(|| default_ipc_path_for(path));
            let daemon = Daemon::new(store, path.clone()).with_ipc_path(ipc_path);
            daemon.run_loop().await?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A capped result always says so; a complete one stays quiet.
    ///
    /// Both halves of the condition matter. `truncated` alone covers a budget
    /// stop that happened to hide nothing; `hidden > 0` covers a result the
    /// budget did not flag but which dropped rows anyway. Collapsing them to
    /// `&&` silences the first, and any comparison that never fires silences
    /// both — leaving a partial answer indistinguishable from a full one.
    #[test]
    fn a_capped_result_is_always_reported_as_capped() {
        // Complete: nothing hidden, not flagged.
        assert_eq!(truncation_line(10, 0, 10, false), None);

        // Flagged by the budget even though nothing is hidden.
        assert!(
            truncation_line(10, 0, 10, true).is_some(),
            "a budget-truncated result must say so even with nothing hidden"
        );

        // Rows dropped without the flag.
        let line = truncation_line(3, 7, 10, false)
            .expect("hidden rows must be reported even when not flagged");
        assert!(
            line.contains('3') && line.contains("10") && line.contains('7'),
            "{line}"
        );

        // Both.
        assert!(truncation_line(3, 7, 10, true).is_some());
    }

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

    /// `ensure_parent` actually creates the directory it promises.
    ///
    /// It was replaceable with `Ok(())`, which defers the failure to whatever
    /// tries to write the file — reporting a path error instead of a missing
    /// directory.
    #[test]
    fn ensure_parent_creates_the_directory() {
        let dir = std::env::temp_dir().join(format!(
            "devmap-ensure-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let target = dir.join("nested/deeper/out.json");
        assert!(!dir.exists());
        ensure_parent(&target).unwrap();
        assert!(
            target.parent().unwrap().is_dir(),
            "ensure_parent must create the full parent chain"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
