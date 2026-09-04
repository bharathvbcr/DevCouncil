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
    write_manifest_atomically, FreshnessInfo, Request, ResolutionAvailability, StampedFreshness,
    StoreQueryEngine, CODE_GRAPH_DEFAULT_OUTPUT,
};
use devmap_resolve::{Resolver, UnresolvedClass};
use devmap_serve::{default_ipc_path_for, Daemon};
use devmap_store::{
    current_git_head, extract_tree_cached_with_report, GenerationWriteOpts, Store,
    GENERATION_RETENTION,
};

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

/// `Some(value)` for a non-blank flag, `None` otherwise.
///
/// A flag passed as the empty string is a caller whose own computation failed,
/// not a caller reporting an empty digest. Treating the two the same would
/// stamp `""` into the artifact as if it were an answer.
fn non_empty(value: &Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// `devmap 0.1.0 (schema 12)` — package identity plus store compatibility.
///
/// K3: every build of this workspace reports `devmap 0.1.0`, so the package
/// version alone cannot tell a caller whether the binary in hand can open the
/// store in hand. The schema number is the part that answers that, and the only
/// other way to read it is to open a store — which is exactly what a caller
/// checking compatibility has not yet established it may do.
/// Built once into a process-lifetime `OnceLock` rather than formatted per
/// call: clap's `version` takes a `&'static str`, and the schema number is only
/// known at runtime because it lives in another crate's constant.
fn version_line() -> &'static str {
    static LINE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    LINE.get_or_init(|| {
        format!(
            "{} (schema {})",
            env!("CARGO_PKG_VERSION"),
            devmap_store::CURRENT_SCHEMA_VERSION
        )
    })
    .as_str()
}

#[derive(Parser)]
#[command(
    name = "devmap",
    author,
    version = version_line(),
    about = "DevCouncil code-intelligence system (Rust kernel)"
)]
struct Cli {
    /// Store this kernel reads and writes.
    ///
    /// `devmap.sqlite`, not `index.sqlite`. The latter is the *Python* engine's
    /// store — `user_version = 2`, a schema this binary has no migration for —
    /// so the old default aimed every un-flagged invocation at a database that
    /// could only be refused, while the Python seam
    /// (`devmap_engine.DEFAULT_DB_RELPATH`) had already moved here. Keep the two
    /// in step: this string and that constant name the same file.
    #[arg(short, long, default_value = ".devcouncil/codeintel/devmap.sqlite")]
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

/// One completed stage and the sub-phases that ran inside it.
struct StageTiming {
    label: String,
    seconds: f64,
    /// Sub-phases closed while this stage was open. Their durations are
    /// *included* in `seconds`; they break the stage down, they do not add to
    /// it. Summing both levels would double-count the build.
    sub: Vec<(String, f64)>,
}

/// The stage in flight: its label, when it began, and the sub-phases closed
/// inside it so far.
///
/// Named rather than written inline because the tuple appears in a field, a
/// borrow and two closures, and a reader meeting `(String, Instant, Vec<(String,
/// f64)>)` in any of them has to reconstruct which position means what.
type OpenStage = (String, Instant, Vec<(String, f64)>);

struct ProgressReporter {
    enabled: bool,
    started_at: Instant,
    /// The stage currently running: its label, when it began, and the
    /// sub-phases closed inside it so far.
    ///
    /// A stage's cost is recorded when the stage *ends*, against its own label.
    /// Attributing it at the next stage boundary — which is what this reporter
    /// used to do — shifts every measurement one position: on a 13.44 s
    /// scholarlm build, extraction's 7.25 s was printed beside the word
    /// "resolving" and extraction itself was reported as 42 ns. A profiler that
    /// names the wrong phase is worse than none, because the reader acts on it.
    ///
    /// A cumulative-only readout ("complete in 2.90s") is the other failure:
    /// it cannot tell an operator whether a slow build is parsing, resolving,
    /// or writing, and those have nothing in common as fixes.
    ///
    /// `RefCell` because `stage` takes `&self`: the reporter is shared by the
    /// whole pipeline and must not need a mutable borrow to print.
    open: std::cell::RefCell<Option<OpenStage>>,
    /// Stages that have closed, in the order they ran, for `--json` builds.
    /// Kept so a benchmark or a daemon can consume the breakdown without
    /// scraping stderr, which is formatted for humans and not a contract.
    timings: std::cell::RefCell<Vec<StageTiming>>,
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
            open: std::cell::RefCell::new(None),
            timings: std::cell::RefCell::new(Vec::new()),
        }
    }

    /// Close the running stage, recording its cost against its own label.
    fn close_open_stage(&self) {
        let Some((label, started, sub)) = self.open.borrow_mut().take() else {
            return;
        };
        let seconds = started.elapsed().as_secs_f64();
        if self.enabled {
            eprintln!("      {label} took {:.0}ms", seconds * 1000.0);
        }
        self.timings.borrow_mut().push(StageTiming {
            label,
            seconds,
            sub,
        });
    }

    /// Record and announce a stage boundary.
    ///
    /// Timings are recorded whether or not printing is enabled: `--progress
    /// never` is about keeping stderr clean, not about declining to measure,
    /// and the `--json` breakdown must not depend on the human output being on.
    fn stage(&self, current: usize, message: impl std::fmt::Display) {
        self.close_open_stage();
        let rendered = message.to_string();
        if self.enabled {
            eprintln!("[{current}/{}] {rendered}", Self::TOTAL_STAGES);
        }
        *self.open.borrow_mut() = Some((rendered, Instant::now(), Vec::new()));
    }

    /// Time one sub-phase, recording it under `label` without printing a stage
    /// header. Used to break a stage that is too coarse to act on into the
    /// parts that have different fixes.
    ///
    /// The result is returned untouched, including the error case: a phase that
    /// fails is still a phase that took time, and swallowing the error to keep
    /// the timing tidy would trade a correct build for a pretty number.
    fn timed<T, E>(
        &self,
        label: &str,
        work: impl FnOnce() -> std::result::Result<T, E>,
    ) -> std::result::Result<T, E> {
        let started = Instant::now();
        let outcome = work();
        let elapsed = started.elapsed().as_secs_f64();
        if self.enabled {
            eprintln!("      {label} (+{:.0}ms)", elapsed * 1000.0);
        }
        match self.open.borrow_mut().as_mut() {
            Some((_, _, sub)) => sub.push((label.to_string(), elapsed)),
            // A sub-phase outside any stage would otherwise be dropped
            // silently. Record it as a stage of its own rather than lose it.
            None => self.timings.borrow_mut().push(StageTiming {
                label: label.to_string(),
                seconds: elapsed,
                sub: Vec::new(),
            }),
        }
        outcome
    }

    /// An untimed detail line under the current stage. Does not disturb the
    /// stage clock, so a note between two phases cannot be mistaken for one.
    fn note(&self, message: impl std::fmt::Display) {
        if self.enabled {
            eprintln!("      {message}");
        }
    }

    /// Close the last stage and print the total.
    ///
    /// Deliberately not a `stage` call: completion is an instant, not a span,
    /// and opening a fifth stage here would leave it running forever and put a
    /// zero-length entry in the breakdown.
    fn complete(&self, generation_id: u32) {
        self.close_open_stage();
        if self.enabled {
            eprintln!(
                "[{}/{}] complete: generation #{generation_id} in {:.2}s",
                Self::TOTAL_STAGES,
                Self::TOTAL_STAGES,
                self.started_at.elapsed().as_secs_f64()
            );
        }
    }

    /// The recorded breakdown as `{stage_label: seconds}` plus the total, for
    /// embedding in a `--json` build result.
    /// The recorded breakdown, for embedding in a `--json` build result.
    ///
    /// Sub-phase seconds are nested inside their stage and are already counted
    /// in the stage's own `seconds`; a consumer sums one level, never both.
    /// A stage still running when this is called is reported with its elapsed
    /// time so far and `"open": true`, because omitting it would make the
    /// stages silently fail to account for the total.
    fn timings_json(&self) -> serde_json::Value {
        let render = |label: &str, secs: f64, sub: &[(String, f64)], open: bool| {
            let mut entry = serde_json::json!({"stage": label, "seconds": secs});
            if !sub.is_empty() {
                entry["sub"] = sub
                    .iter()
                    .map(|(l, s)| serde_json::json!({"stage": l, "seconds": s}))
                    .collect();
            }
            if open {
                entry["open"] = serde_json::Value::Bool(true);
            }
            entry
        };
        let mut stages: Vec<serde_json::Value> = self
            .timings
            .borrow()
            .iter()
            .map(|t| render(&t.label, t.seconds, &t.sub, false))
            .collect();
        if let Some((label, started, sub)) = self.open.borrow().as_ref() {
            stages.push(render(label, started.elapsed().as_secs_f64(), sub, true));
        }
        serde_json::json!({
            "stages": stages,
            "total_seconds": self.started_at.elapsed().as_secs_f64(),
        })
    }
}

#[derive(Subcommand)]
enum WorkspaceAction {
    /// Register a repository in this workspace.
    Add {
        /// Repository root. Stored canonicalised, so the registry survives a
        /// caller with a different working directory.
        path: PathBuf,
        /// Label for results. Defaults to the directory name.
        #[arg(long)]
        name: Option<String>,
    },
    /// Remove a repository by name.
    Remove { name: String },
    /// List registered repositories and whether each has a readable store.
    List,
    /// Search every registered repository at once.
    Search {
        query: String,
        #[arg(short, long, default_value_t = 2000)]
        budget: u32,
        #[arg(long)]
        semantic: bool,
    },
    /// Imports in one repository that another repository declares the module
    /// for. Candidates with evidence, not resolved edges.
    Links,
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
        full: bool,
    },
    Search {
        query: String,
        #[arg(short, long, default_value_t = 2000)]
        budget: u32,
        /// Rank by TF-IDF similarity of symbol names instead of FTS5 prefix
        /// matching. Finds symbols whose names are *about* the query rather
        /// than ones that contain it.
        #[arg(long)]
        semantic: bool,
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
    /// Ask what an unsaved edit would do to the graph, without writing it.
    ///
    /// Reads the candidate content from `--content` (a file, or `-` for stdin)
    /// and diffs it against the indexed version of `--file`, reporting symbols
    /// added, removed and re-declared, plus the calls from other files that a
    /// removal or re-declaration would break.
    Preview {
        /// Path the buffer would be written to. Resolved against the index as
        /// given, so it must match the indexed path.
        #[arg(long)]
        file: String,
        /// Where the candidate content comes from: a path, or `-` for stdin.
        #[arg(long, default_value = "-")]
        content: String,
        #[arg(short, long, default_value_t = 2000)]
        budget: u32,
        /// Confidence a call edge needs to be listed as affected. The default
        /// excludes the resolver's name-only tier, whose edges are counted
        /// separately rather than shown.
        #[arg(long, default_value_t = devmap_query::PREVIEW_CALLER_MIN_CONFIDENCE)]
        min_confidence: f32,
    },
    /// Manage the workspace registry and query across every repository in it.
    ///
    /// `devmap serve` indexes one root, which is the right unit for a build and
    /// the wrong one for a question: "who calls this" does not stop at a
    /// repository boundary when the caller is a sibling service.
    Workspace {
        #[command(subcommand)]
        action: WorkspaceAction,
    },
    /// Report what the map cost against what reading files would have.
    ///
    /// Every figure is bytes divided by 4, which is an estimate and is labelled
    /// one. With `--query`, also reports one search's actual token spend beside
    /// the size of the files that search pointed into.
    Savings {
        /// Optional search to account for, in addition to the corpus figures.
        #[arg(long)]
        query: Option<String>,
        #[arg(short, long, default_value_t = 2000)]
        budget: u32,
    },
    /// Report duplicated symbol bodies in the latest generation.
    ///
    /// `exact` groups are the same code modulo formatting and comments;
    /// `structural` groups are the same shape under renaming, and are limited
    /// to callables.
    Clones {
        #[arg(short, long, default_value_t = 2000)]
        budget: u32,
        /// Report only one kind. Both are reported by default.
        #[arg(long, value_parser = ["exact", "structural"])]
        kind: Option<String>,
        /// Drop groups whose smallest body is under this many parse nodes.
        /// Raises the floor for this query only; it cannot lower it below the
        /// one signatures were computed with.
        #[arg(long, default_value_t = 0)]
        min_nodes: u32,
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
        /// Caller-computed `generated_head` to stamp into both artifacts.
        ///
        /// The three stamp flags exist because the kernel cannot compute these
        /// values and the caller can. `devcouncil.devmap_engine.stamp_freshness`
        /// used to add them by reading each finished artifact back, parsing it,
        /// setting three scalars and re-serializing the whole thing — 1.68 s of
        /// a 2.72 s `dev map` on this repository, almost all of it Python
        /// re-encoding a 26 MB graph the kernel had just encoded.
        ///
        /// Passing them in means they are written once, by the writer, at
        /// generation time. Omitted, the artifacts carry the empty values and
        /// their `meta.devmap_rust.unavailable` markers exactly as before: a
        /// value is stamped only when a caller supplies a real one, and never
        /// invented here.
        #[arg(long)]
        generated_head: Option<String>,
        /// Caller-computed `indexed_hash` (SHA-1 over the git file list).
        #[arg(long)]
        indexed_hash: Option<String>,
        /// Caller-computed `content_fingerprint` (scheme-prefixed SHA-1 over file bytes).
        #[arg(long)]
        content_fingerprint: Option<String>,
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
        /// Drop pending-queue rows that no drain can ever process: quarantined
        /// rows, paths outside the repository, directories and files that are
        /// gone, oversized sources, and non-source files.
        ///
        /// K1(f): the queue had no operator-facing repair at all. A store whose
        /// checkout had moved carried 64 rows naming the old location, every
        /// one of them quarantined, and the only way out was to open the
        /// database by hand or delete it.
        #[arg(long)]
        pending: bool,
    },
    Snapshots {
        #[arg(default_value = "")]
        file: String,
        #[arg(short, long, default_value_t = 2000)]
        budget: u32,
    },
    /// Serve queries over IPC, watching the tree for changes.
    ///
    /// The daemon retires itself after 30 minutes with no IPC request, no
    /// pending work and no watcher event; the next client respawns it against
    /// the current kernel. Set DEVMAP_MAX_IDLE_SECS (seconds) to change the
    /// bound, or to 0 to keep serving forever.
    Serve {
        #[arg(default_value = ".")]
        path: PathBuf,
        #[arg(long)]
        socket: Option<PathBuf>,
        /// Print the IPC endpoint this repository would be served on and exit,
        /// without starting a daemon, opening a store, or creating any file.
        ///
        /// Exists so a second implementation of the socket-path formula can be
        /// checked against this one. The Python client derives the same path
        /// without spawning anything, and when the two disagree each side
        /// starts its own daemon against one store — a divergence that is
        /// invisible from either side.
        #[arg(long)]
        print_socket_path: bool,
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

fn emit_savings(report: &devmap_query::SavingsReport) {
    let tokens = |bytes: u64| bytes / u64::from(devmap_query::BYTES_PER_TOKEN);
    println!("basis: {}", report.basis);
    println!(
        "corpus:   {} files, {} bytes  (~{} tokens to read in full)",
        report.indexed_files,
        report.corpus_bytes,
        tokens(report.corpus_bytes)
    );
    if report.corpus_files_unreadable > 0 {
        // Named, not folded into the total as zero: an unread file makes the
        // corpus look smaller, which makes the map look better.
        println!(
            "          {} indexed file(s) could not be read and are excluded from that total",
            report.corpus_files_unreadable
        );
    }
    match report.repo_map_bytes {
        Some(bytes) => println!("repo_map: {} bytes  (~{} tokens)", bytes, tokens(bytes)),
        None => println!("repo_map: not written yet"),
    }
    let Some(query) = &report.query else {
        println!("(pass --query to account for one search)");
        return;
    };
    println!(
        "query {:?}: {} hit(s) across {} file(s)",
        query.query, query.hits, query.files_named
    );
    println!("  map answer cost:        {} tokens", query.answer_tokens);
    println!(
        "  reading those files:    ~{} tokens ({} bytes)",
        tokens(query.files_bytes),
        query.files_bytes
    );
    if query.files_unreadable > 0 {
        println!(
            "  {} named file(s) could not be read and are excluded",
            query.files_unreadable
        );
    }
    let alternative = tokens(query.files_bytes);
    if alternative > u64::from(query.answer_tokens) {
        println!(
            "  floor on the saving:    ~{} tokens, and only for a reader who already \
             knew which files to open",
            alternative - u64::from(query.answer_tokens)
        );
    } else {
        // Said plainly rather than suppressed. On a tiny corpus, or a query
        // whose hits live in small files, the map is not cheaper — and a
        // savings report that can only ever report a saving is advertising.
        println!("  no saving on this query: the files are smaller than the answer");
    }
}

fn emit_preview(report: &devmap_query::PreviewReport) {
    println!("{}  parse={}", report.file_path, report.parse_status);
    if report.compared_against == "nothing" {
        println!("note: no file at this path; every symbol reads as added");
    }
    if !report.file_is_indexed {
        println!("note: this file is not in the index; no caller graph is available for it");
    }
    if let Some(reason) = &report.degraded_reason {
        println!("note: {reason}");
    }
    if !report.delta_available {
        // The reason above already says why. Printing an empty symbol list
        // underneath it would read as "no changes".
        return;
    }
    for symbol in &report.symbols {
        let change = match symbol.change {
            devmap_query::PreviewChange::Added => "added",
            devmap_query::PreviewChange::Removed => "removed",
            devmap_query::PreviewChange::SignatureChanged => "signature",
            devmap_query::PreviewChange::BodyChanged => "body",
            devmap_query::PreviewChange::Changed => "changed",
        };
        println!("{change:<11} {} ({})", symbol.qualified_name, symbol.kind);
    }
    if report.symbols.is_empty() {
        println!("no symbol-level change");
    }
    if report.bodies_not_compared > 0 {
        println!(
            "{} symbol(s) declared identically but not body-compared \
             (below the signature size floor, or no grammar)",
            report.bodies_not_compared
        );
    }
    for caller in &report.broken_callers.items {
        // `caller_symbol` and `target_symbol` are already `path::Name`, so the
        // file is not printed again beside them.
        println!(
            "  affects  {}  ->  {}  ({:.2})",
            caller.caller_symbol, caller.target_symbol, caller.confidence
        );
    }
    if report.broken_callers.total == 0 {
        println!("no calls from other files are affected");
    }
    if report.ambiguous_callers > 0 {
        println!(
            "{} further call edge(s) fell below the confidence floor and are not \
             listed (usually a bare method name matching many definitions); \
             pass --min-confidence 0 to see them",
            report.ambiguous_callers
        );
    }
    emit_truncation(
        report.broken_callers.shown,
        report.broken_callers.hidden,
        report.broken_callers.total,
        report.broken_callers.truncated,
    );
}

fn emit_clones(report: &devmap_query::CloneReport) {
    if let ResolutionAvailability::Unavailable { reason } = &report.groups.resolution {
        emit_unavailable(reason);
        return;
    }
    for group in &report.groups.items {
        let kind = match group.kind {
            devmap_analyze::CloneKind::Exact => "exact",
            devmap_analyze::CloneKind::Structural => "structural",
        };
        println!(
            "{kind}  {} members  {} nodes  #{:016x}",
            group.members.len(),
            group.min_nodes,
            group.signature
        );
        for member in &group.members {
            println!(
                "    {}:{}  {}",
                member.file_path, member.span_start, member.symbol_name
            );
        }
        if group.members_omitted > 0 {
            println!("    ... {} more members not listed", group.members_omitted);
        }
    }
    // Always printed, including when nothing was found: "no duplicates" and
    // "nothing was examined" are different answers and must not print the same.
    println!(
        "coverage: {} symbols signed, {} unsigned",
        report.signed_symbols, report.unsigned_symbols
    );
    emit_truncation(
        report.groups.shown,
        report.groups.hidden,
        report.groups.total,
        report.groups.truncated,
    );
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
            full,
        } => {
            let progress = ProgressReporter::new(cli.progress, cli.json);
            let build_started = std::time::Instant::now();
            progress.stage(
                1,
                format_args!("scanning and extracting {}", path.display()),
            );
            ensure_parent(&cli.db)?;
            // K13: take the cross-process writer lock *before* extraction.
            //
            // There was no such lock, so two builds — or a build and the
            // daemon's drain — raced on SQLite's five-second `busy_timeout`
            // alone and the loser surfaced `database is locked` only at the
            // persist, having already paid for the whole extract and resolve.
            // Taking it first means the loser waits for the winner and then
            // does useful work, or fails immediately with a message naming the
            // pid that holds the store.
            let _writer = Store::lock_writer_at(&cli.db, Store::WRITER_LOCK_WAIT)?;
            let store = Store::open(&cli.db)?;
            // K1(e2): stamped before discovery, on the queue's own wall clock.
            //
            // A build that walks the whole tree answers every request queued at
            // or before this instant, whatever that request named — which is
            // the only rule that retires a row naming a *directory*. Taken
            // before the walk, never after: an event that arrives while this
            // build is extracting may describe an edit it did not see, and that
            // row has to survive.
            let build_start = Store::queue_clock_now();

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
                if let Some(cache) = caches.tagged_ancestor(path, &candidate) {
                    anyhow::bail!(
                        "--affected names {candidate}, which is inside {cache} — a build \
                         cache marked with CACHEDIR.TAG. devmap does not index build \
                         caches; drop it from the change set."
                    );
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
            let reconciled = store.reconcile_pending_paths(path)?;
            if !reconciled.dropped.is_empty() {
                eprintln!(
                    "  pending queue: dropped {} unprocessable row(s):",
                    reconciled.dropped.len()
                );
                for (dropped, reason) in reconciled.dropped.iter().take(20) {
                    eprintln!("    {dropped}: {reason}");
                }
                if reconciled.dropped.len() > 20 {
                    eprintln!("    … and {} more", reconciled.dropped.len() - 20);
                }
            }
            if !reconciled.rewritten.is_empty() {
                eprintln!(
                    "  pending queue: normalized {} row(s) to repo-relative paths",
                    reconciled.rewritten.len()
                );
            }

            // K4: `--full` re-parses rather than consulting the extraction
            // cache. Reading the cache would defeat the point — a cache hit
            // returns the payload this build is trying to reproduce from
            // source, so a "full" rebuild that used it would recommit exactly
            // the rows the operator is asking to replace.
            let (extractions, discovery) = if *full {
                let (sources, report) = devmap_extract::collect_sources_with_report(path)?;
                let refs: Vec<devmap_extract::FileRef<'_>> = sources
                    .iter()
                    .map(|(file, source)| devmap_extract::FileRef {
                        path: file.as_str(),
                        source: source.as_str(),
                    })
                    .collect();
                (devmap_extract::extract_all(&refs), report)
            } else {
                extract_tree_cached_with_report(&store, path)?
            };
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
            //
            // "Identical inputs give an identical graph" holds for one kernel,
            // not across two. An upgraded extractor reads the same bytes and
            // produces a different graph — that is what an extraction schema
            // bump *is* — so content hashes alone would report "still current"
            // over a generation this kernel would never have written. Measured
            // on DevCouncil: the first `dev map` after two schema bumps printed
            // "No source changes; generation #412 still current (1,152 files)"
            // while every row in it came from `extract-v23`.
            let previous = store.latest_file_hashes()?;
            if !*full
                && !previous.is_empty()
                && previous.len() == extractions.len()
                && store.latest_generation_payload_is_current()?
            {
                let unchanged = extractions.iter().all(|extraction| {
                    previous
                        .get(&extraction.file_path)
                        .is_some_and(|hash| *hash == extraction.content_hash)
                });
                if unchanged {
                    progress.stage(2, format_args!("{} files unchanged", extractions.len()));
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
                    progress.note(format_args!("reclaim: {}", reclaim_note(&vacuum)));
                    // K1(e2): the unchanged check compared *every* file in the
                    // tree against the stored generation and found them equal.
                    // That is the same proof a fresh whole-tree build gives —
                    // the graph on disk already describes this tree — so the
                    // requests queued before this build started are answered,
                    // even though no new generation was written. Without this a
                    // repository that is already current keeps a stale queue,
                    // and `status` reports NOT FRESH indefinitely.
                    let retired = store.clear_pending_superseded(
                        devmap_store::PendingSupersede::WholeTreeBuiltAt(build_start),
                    )?;
                    if !retired.is_empty() {
                        progress.note(format_args!(
                            "pending queue: retired {} row(s) the current generation \
                             already answers",
                            retired.len()
                        ));
                    }
                    if cli.json {
                        println!(
                            "{{\"unchanged\":true,\"files\":{},\"generation\":{},\"reclaim\":{}}}",
                            extractions.len(),
                            generation,
                            serde_json::to_string(&reclaim_note(&vacuum))?
                        );
                    } else {
                        println!(
                            "No source changes; generation #{} still current ({} files).",
                            generation,
                            extractions.len()
                        );
                        println!("  Reclaim: {}", reclaim_note(&vacuum));
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
            // K4: `--full` writes a full generation. An empty affected set *is*
            // the full-rewrite signal to the store, so the closure — whose only
            // job is to narrow the write — is not computed at all.
            let affected = if *full {
                None
            } else {
                affected_closure(&store, &extractions)?
            };

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
            let gen_id = progress.timed("persist:write", || {
                store.save_generation_with_metadata(
                    &extractions,
                    &resolution,
                    &analysis,
                    opts,
                    &head_sha,
                )
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
                devmap_store::PendingSupersede::IndexedPaths(&indexed)
            } else {
                devmap_store::PendingSupersede::WholeTreeBuiltAt(build_start)
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
            progress.note(format_args!("reclaim: {}", reclaim_note(&vacuum)));

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
                        // The per-stage breakdown, so a caller profiling a slow
                        // build reads it from the result rather than scraping
                        // the human progress lines off stderr.
                        "timings": progress.timings_json(),
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
        Commands::Search {
            query,
            budget,
            semantic,
        } => {
            let store = open_for_read(&cli.db)?;
            let engine = StoreQueryEngine::new(&store);
            let resp = if *semantic {
                engine.search_semantic(query, *budget)?
            } else {
                engine.search(Request {
                    query: query.clone(),
                    token_budget: *budget,
                    min_confidence: 0.0,
                    max_depth: 1,
                })?
            };
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
        Commands::Workspace { action } => {
            // Rooted at the store's repository, so `devmap --db X workspace` and
            // `dev map workspace` agree on where the registry lives.
            let root = cli
                .db
                .parent()
                .and_then(|dir| dir.parent())
                .and_then(|dir| dir.parent())
                .map(|dir| dir.to_path_buf())
                .unwrap_or_else(|| PathBuf::from("."));
            // Mutating actions go through `Workspace::update`, which holds an
            // advisory lock across the read and the write. Loading here and
            // saving later — which is what this did — let two concurrent
            // registrations each read the same registry and write their own
            // entry over the other's.
            match action {
                WorkspaceAction::Add { path, name } => {
                    let canonical = path.canonicalize().map_err(|error| {
                        anyhow::anyhow!("cannot resolve {}: {error}", path.display())
                    })?;
                    let label = name
                        .clone()
                        .unwrap_or_else(|| devmap_query::workspace::name_for(&canonical));
                    let (_, written) =
                        devmap_query::workspace::Workspace::update(&root, |workspace| {
                            workspace.add(label.clone(), canonical.clone());
                        })?;
                    if cli.json {
                        emit_json(
                            &cli,
                            &serde_json::json!({
                                "added": label,
                                "root": canonical,
                                "registry": written,
                            }),
                        )?;
                    } else {
                        println!(
                            "added {label} -> {} ({})",
                            canonical.display(),
                            written.display()
                        );
                    }
                }
                WorkspaceAction::Remove { name } => {
                    // Distinguished from success: a caller retrying a removal
                    // should learn the name was never registered.
                    //
                    // The registry is rewritten unconditionally rather than
                    // only when something was removed: the write is a rename of
                    // identical bytes when nothing changed, and skipping it
                    // would mean the "nothing to do" path took a different
                    // route through the lock than the mutating one.
                    let (removed, _) =
                        devmap_query::workspace::Workspace::update(&root, |workspace| {
                            workspace.remove(name)
                        })?;
                    if cli.json {
                        emit_json(&cli, &serde_json::json!({"removed": removed, "name": name}))?;
                    } else if removed {
                        println!("removed {name}");
                    } else {
                        println!("{name} is not registered");
                    }
                }
                WorkspaceAction::List => {
                    let workspace = devmap_query::workspace::Workspace::load(&root)?;
                    if cli.json {
                        let repos: Vec<serde_json::Value> = workspace
                            .repos
                            .iter()
                            .map(|repo| {
                                let db = repo.db_path();
                                let status = devmap_store::Store::open_existing(&db)
                                    .ok()
                                    .flatten()
                                    .and_then(|store| store.status(&db.display().to_string()).ok());
                                serde_json::json!({
                                    "name": repo.name,
                                    "root": repo.root,
                                    "db": db,
                                    // Null rather than zero when there is no
                                    // generation: "never indexed" and "indexed
                                    // and empty" are different states, and one
                                    // of the repositories here is the second.
                                    "generation": status.as_ref().and_then(|s| s.latest_generation),
                                    "symbols": status.as_ref().map(|s| s.node_count),
                                    "edges": status.as_ref().map(|s| s.edge_count),
                                })
                            })
                            .collect();
                        emit_json(
                            &cli,
                            &serde_json::json!({"repos": repos, "registry_root": root}),
                        )?;
                        return Ok(());
                    }
                    if workspace.repos.is_empty() {
                        println!("no repositories registered ({})", root.display());
                    }
                    for repo in &workspace.repos {
                        // The store *file* existing says nothing about whether
                        // it holds anything. One of these repositories has two
                        // generations whose latest contains zero symbols, and
                        // reporting that as "indexed" because the file is on
                        // disk is the same error as a check that could not run
                        // reporting as one that passed.
                        let db = repo.db_path();
                        let state = match devmap_store::Store::open_existing(&db) {
                            Ok(None) => "no store".to_string(),
                            Err(error) => format!("unreadable: {error}"),
                            Ok(Some(store)) => match store.status(&db.display().to_string()) {
                                Err(error) => format!("unreadable: {error}"),
                                Ok(status) => match status.latest_generation {
                                    None => "no generation".to_string(),
                                    Some(gen) => format!(
                                        "gen {gen}, {} symbols, {} edges",
                                        status.node_count, status.edge_count
                                    ),
                                },
                            },
                        };
                        println!("{:<16} {:<34} {}", repo.name, state, repo.root.display());
                    }
                }
                WorkspaceAction::Search {
                    query,
                    budget,
                    semantic,
                } => {
                    let workspace = devmap_query::workspace::Workspace::load(&root)?;
                    let result =
                        devmap_query::workspace_search(&workspace, query, *budget, *semantic)?;
                    if cli.json {
                        emit_json(&cli, &serde_json::to_value(&result)?)?;
                    } else {
                        for entry in &result.items {
                            println!(
                                "[{}] {}:{}  {}",
                                entry.repo,
                                entry.hit.file_path,
                                entry.hit.span.0,
                                entry.hit.symbol_name
                            );
                        }
                        println!(
                            "{} repo(s) queried, {} shown of {}",
                            result.repos_queried, result.shown, result.total
                        );
                        // Never silent. A workspace answer assembled from a
                        // subset is not a workspace answer.
                        for missing in &result.unavailable {
                            println!("  unavailable: {} — {}", missing.repo, missing.reason);
                        }
                    }
                }
                WorkspaceAction::Links => {
                    let workspace = devmap_query::workspace::Workspace::load(&root)?;
                    let links = devmap_query::link_candidates(&workspace)?;
                    if cli.json {
                        // An object, not a bare array: the count and the set of
                        // repositories considered are part of the answer, and a
                        // reader seeing `[]` should be able to tell "no links"
                        // from "no repositories were examined".
                        emit_json(
                            &cli,
                            &serde_json::json!({
                                "links": links,
                                "count": links.len(),
                                "repos_considered": workspace
                                    .repos
                                    .iter()
                                    .map(|repo| repo.name.as_str())
                                    .collect::<Vec<_>>(),
                            }),
                        )?;
                    } else {
                        for link in &links {
                            println!(
                                "{} {} -> {}  ({}; {})",
                                link.from_repo,
                                link.module_specifier,
                                link.to_repo,
                                link.from_file,
                                link.evidence
                            );
                        }
                        println!("{} candidate link(s)", links.len());
                    }
                }
            }
        }
        Commands::Savings { query, budget } => {
            let store = open_for_read(&cli.db)?;
            let report = StoreQueryEngine::new(&store).savings(query.as_deref(), *budget)?;
            if cli.json {
                emit_json(&cli, &serde_json::to_value(&report)?)?;
            } else {
                emit_savings(&report);
            }
        }
        Commands::Preview {
            file,
            content,
            budget,
            min_confidence,
        } => {
            let source = if content == "-" {
                let mut buffer = String::new();
                std::io::Read::read_to_string(&mut std::io::stdin(), &mut buffer)?;
                buffer
            } else {
                std::fs::read_to_string(content)
                    .map_err(|e| anyhow::anyhow!("cannot read {content}: {e}"))?
            };
            let store = open_for_read(&cli.db)?;
            let report =
                StoreQueryEngine::new(&store).preview(file, &source, *budget, *min_confidence)?;
            if cli.json {
                emit_json(&cli, &serde_json::to_value(&report)?)?;
            } else {
                emit_preview(&report);
            }
        }
        Commands::Clones {
            budget,
            kind,
            min_nodes,
        } => {
            let store = open_for_read(&cli.db)?;
            // `value_parser` has already rejected anything but the two names,
            // so a `None` here can only be "no filter requested".
            let wanted = kind.as_deref().and_then(devmap_query::parse_clone_kind);
            let report = StoreQueryEngine::new(&store).clones(*budget, wanted, *min_nodes)?;
            if cli.json {
                emit_json(&cli, &serde_json::to_value(&report)?)?;
            } else {
                emit_clones(&report);
            }
        }
        Commands::Manifest {
            path,
            output,
            graph_output,
            force,
            generated_head,
            indexed_hash,
            content_fingerprint,
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
                // Blank flags are treated as absent. An empty `--indexed-hash`
                // is a caller whose digest computation failed, and stamping ""
                // as though it were a result is the exact confusion the
                // "unavailable" markers exist to prevent.
                stamped: StampedFreshness {
                    generated_head: non_empty(generated_head),
                    indexed_hash: non_empty(indexed_hash),
                    content_fingerprint: non_empty(content_fingerprint),
                },
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
            //
            // K3: the schema is probed *before* the store is opened, because
            // `Store::open` runs the migration chain under an exclusive
            // transaction from every open. This command used to rewrite the
            // schema of a store it was only asked to describe, silently, on the
            // one command a health check runs against a store it does not own.
            // Migrating is `build`'s job, where the caller asked for a write.
            let stored_schema = Store::stored_schema_version(&cli.db)?;
            let Some(stored_schema) = stored_schema else {
                let payload = serde_json::json!({
                    "generation_id": serde_json::Value::Null,
                    "pending_count": 0,
                    "node_count": 0,
                    "edge_count": 0,
                    "is_fresh": false,
                    "db_path": cli.db.display().to_string(),
                    "degraded_reason": "no devmap store at this path (run `devmap build`)",
                    "quarantined_count": 0,
                    "quarantined_paths": Vec::<String>::new(),
                    "schema_outdated": false,
                    "schema_version": serde_json::Value::Null,
                    "expected_schema_version": devmap_store::CURRENT_SCHEMA_VERSION,
                });
                println!("{}", serde_json::to_string_pretty(&payload)?);
                return Ok(());
            };
            if stored_schema != devmap_store::CURRENT_SCHEMA_VERSION {
                let version = stored_schema;
                let payload = serde_json::json!({
                    "generation_id": serde_json::Value::Null,
                    "pending_count": 0,
                    "node_count": 0,
                    "edge_count": 0,
                    "is_fresh": false,
                    "db_path": cli.db.display().to_string(),
                    "degraded_reason": format!(
                        "store schema is {version}, this binary speaks {}; \
                         run `devmap build` to migrate it",
                        devmap_store::CURRENT_SCHEMA_VERSION
                    ),
                    "quarantined_count": 0,
                    "quarantined_paths": Vec::<String>::new(),
                    "schema_outdated": true,
                    "schema_version": version,
                    "expected_schema_version": devmap_store::CURRENT_SCHEMA_VERSION,
                });
                emit_json(&cli, &payload)?;
                return Ok(());
            }
            let Some(store) = Store::open_existing(&cli.db)? else {
                anyhow::bail!(
                    "the devmap store at {} vanished between the schema probe and the read",
                    cli.db.display()
                );
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
                // K1(g): naming the stuck paths is what makes a degraded
                // status actionable — "64 path(s) exceeded the retry
                // threshold" told an operator nothing about which 64.
                "quarantined_paths": status.quarantined_paths,
                "schema_outdated": false,
                "schema_version": stored_schema,
                "expected_schema_version": devmap_store::CURRENT_SCHEMA_VERSION,
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
        Commands::Repair { fts, pending } => {
            let store = open_for_read(&cli.db)?;
            if !*fts && !*pending {
                anyhow::bail!("specify a repair target, e.g. --fts or --pending");
            }
            if *fts {
                store.repair_fts()?;
                if !cli.json {
                    println!("FTS search index repaired.");
                }
            }
            if *pending {
                // K1(f): report what was dropped, per row, with the reason.
                // A repair that says "done" is indistinguishable from one that
                // found nothing, and the whole point of this command is that
                // the operator could not see what was stuck.
                //
                // The structural pass needs a root. The store records the one
                // the latest generation was built from; without a generation
                // there is nothing to compare a path against, so only the
                // quarantined rows are dropped and the payload says so.
                let root = store.latest_repo_root()?.map(PathBuf::from);
                let structural = match &root {
                    Some(root) => store.reconcile_pending_paths(root)?,
                    None => devmap_store::PendingReconcile::default(),
                };
                let quarantined = store.drop_quarantined_pending_paths()?;

                if cli.json {
                    emit_json(
                        &cli,
                        &serde_json::json!({
                            "repo_root": root.as_ref().map(|root| root.display().to_string()),
                            "structural_pass_ran": root.is_some(),
                            "dropped_unprocessable": structural.dropped
                                .iter()
                                .map(|(path, reason)| serde_json::json!({
                                    "path": path, "reason": reason,
                                }))
                                .collect::<Vec<_>>(),
                            "normalized": structural.rewritten
                                .iter()
                                .map(|(from, to)| serde_json::json!({ "from": from, "to": to }))
                                .collect::<Vec<_>>(),
                            "dropped_quarantined": quarantined,
                            "retained": structural.retained.saturating_sub(quarantined.len()),
                        }),
                    )?;
                } else {
                    if root.is_none() {
                        println!(
                            "No generation yet, so no repository root to check paths against; \
                             dropping quarantined rows only."
                        );
                    }
                    for (dropped, reason) in &structural.dropped {
                        println!("dropped {dropped}: {reason}");
                    }
                    for (from, to) in &structural.rewritten {
                        println!("normalized {from} -> {to}");
                    }
                    for dropped in &quarantined {
                        println!(
                            "dropped {dropped}: exceeded {} retry attempts",
                            devmap_store::MAX_PENDING_ATTEMPTS
                        );
                    }
                    println!(
                        "Pending queue repaired: {} unprocessable, {} quarantined, \
                         {} normalized.",
                        structural.dropped.len(),
                        quarantined.len(),
                        structural.rewritten.len(),
                    );
                }
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
        Commands::Serve {
            path,
            socket,
            print_socket_path,
        } => {
            // Canonicalize once, here: the daemon's watcher, reconcile sweep
            // and path-containment guards each canonicalized independently
            // before, and a non-canonical root (a symlinked tmpdir, `.`) made
            // the IPC identity hash — and therefore the socket path — differ
            // between invocations of the same repository.
            //
            // `default_ipc_path_for` canonicalizes too, so the two agree; this
            // one is what the daemon is *rooted* at.
            let root = path.canonicalize()?;
            let ipc_path = socket
                .clone()
                .unwrap_or_else(|| default_ipc_path_for(&root));

            // Before the store is touched: `--print-socket-path` must create
            // nothing, and `Store::open` creates the database file.
            if *print_socket_path {
                if cli.json {
                    emit_json(&cli, &serde_json::json!({"socket": ipc_path}))?;
                } else {
                    println!("{}", ipc_path.display());
                }
                return Ok(());
            }

            ensure_parent(&cli.db)?;
            let store = Store::open(&cli.db)?;
            let daemon = Daemon::new(store, root)
                // So the daemon can notice its own store being deleted and
                // exit, instead of serving a removed inode until its idle bound
                // expires half an hour later.
                .with_store_path(cli.db.clone())
                .with_ipc_path(ipc_path);
            daemon.run_loop().await?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Class E (PLAN.md §3.1): a measurement is attributed to what incurred it.
    ///
    /// K8 recorded time-since-previous-announcement against the *next* stage's
    /// label, so a 13.44 s build reported extraction's 7.25 s beside the word
    /// "resolving" and extraction itself as 42 nanoseconds. The output was
    /// plausible — real phase names, real durations, summing to the real total
    /// — and pointed at the wrong one, which is what makes this class corrosive
    /// rather than merely wrong.
    ///
    /// The three properties below are what a consumer needs in order to trust
    /// the breakdown, and none of them held before the fix.
    #[test]
    fn stage_timings_are_attributed_to_the_stage_that_incurred_them() {
        let reporter = ProgressReporter::new(ProgressMode::Never, true);

        reporter.stage(1, "alpha");
        std::thread::sleep(std::time::Duration::from_millis(20));
        reporter.stage(2, "beta");
        let _: std::result::Result<(), ()> = reporter.timed("beta:inner", || {
            std::thread::sleep(std::time::Duration::from_millis(20));
            Ok(())
        });
        reporter.complete(1);

        let json = reporter.timings_json();
        let stages = json["stages"].as_array().expect("stages array");
        assert_eq!(stages.len(), 2, "two stages ran: {stages:?}");

        // 1. The stage that slept is the stage that reports the time. Under the
        //    off-by-one, `alpha`'s 20 ms was reported against `beta`.
        assert_eq!(stages[0]["stage"], "alpha");
        let alpha = stages[0]["seconds"].as_f64().expect("alpha seconds");
        assert!(
            alpha >= 0.015,
            "alpha slept 20ms and must report it, got {alpha}s"
        );

        // 2. Sub-phases nest inside their parent and are included in its total,
        //    never listed beside it — summing both levels would double-count.
        let beta = stages[1]["seconds"].as_f64().expect("beta seconds");
        let sub = stages[1]["sub"].as_array().expect("beta sub-phases");
        assert_eq!(sub.len(), 1, "one sub-phase ran inside beta: {sub:?}");
        assert_eq!(sub[0]["stage"], "beta:inner");
        let inner = sub[0]["seconds"].as_f64().expect("inner seconds");
        assert!(
            inner <= beta + 1e-6,
            "a sub-phase cannot exceed the stage containing it: {inner}s in {beta}s"
        );

        // 3. The stages account for the whole build. A phase that went
        //    unattributed would leave a gap here, which is exactly how a
        //    42-nanosecond extraction went unnoticed.
        let total = json["total_seconds"].as_f64().expect("total");
        assert!(
            alpha + beta <= total + 1e-6 && alpha + beta >= total * 0.5,
            "stages ({alpha}s + {beta}s) must account for the total ({total}s)"
        );
    }

    /// A stage still running is reported as open, never omitted.
    ///
    /// Class A applied to the profiler itself: if an unfinished stage were
    /// simply left out, the breakdown would silently fail to account for the
    /// total and a reader would attribute the missing time to nothing at all.
    #[test]
    fn an_unfinished_stage_is_reported_rather_than_dropped() {
        let reporter = ProgressReporter::new(ProgressMode::Never, true);
        reporter.stage(1, "still running");

        let json = reporter.timings_json();
        let stages = json["stages"].as_array().expect("stages array");
        assert_eq!(stages.len(), 1, "the open stage must appear: {stages:?}");
        assert_eq!(stages[0]["stage"], "still running");
        assert_eq!(
            stages[0]["open"],
            serde_json::Value::Bool(true),
            "an unfinished stage must be marked open so its time is not read as final"
        );
    }

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
