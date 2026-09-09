use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand, ValueEnum};
use tracing::Level;
use tracing_subscriber::FmtSubscriber;

mod claude;
mod progress;

use devmap_extract::collect_go_modules;
use devmap_query::freshness::{self, FreshnessDigests, InventoryLimits, InventorySource};
use devmap_query::{
    generate_code_graph_encodings, generate_manifest_with_edges, resolve_manifest_output,
    resolved_edge_from_stored, semantic_snapshots, write_code_graph_atomically,
    write_manifest_atomically, ArtifactStamp, FreshnessInfo, Request, ResolutionAvailability,
    StampedFreshness, StoreQueryEngine, CODE_GRAPH_SCHEMA_VERSION,
};
use devmap_resolve::{Resolver, UnresolvedClass};
use devmap_serve::{default_ipc_path_for, Daemon};
use devmap_store::{current_git_head, GenerationWriteOpts, Store, GENERATION_RETENTION};

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

/// `devmap 0.1.0 (store schema 13, code graph schema 2)` — package identity
/// plus both compatibility numbers, each said to be the one it is.
///
/// K3: every build of this workspace reports `devmap 0.1.0`, so the package
/// version alone cannot tell a caller whether the binary in hand can open the
/// store in hand. The schema number is the part that answers that, and the only
/// other way to read it is to open a store — which is exactly what a caller
/// checking compatibility has not yet established it may do.
///
/// There are *two* numbers called "schema" in this system and they are not
/// related: `devmap_store::CURRENT_SCHEMA_VERSION` is the SQLite
/// `user_version` that decides whether this binary can open a store, and
/// `CODE_GRAPH_SCHEMA_VERSION` is the `schema_version` field of the
/// `code_graph.json` this binary writes, which decides whether a Python
/// consumer can read the artifact. An unqualified "schema 13" beside an
/// artifact declaring `"schema_version": 2` reads as a contradiction, and the
/// only way to tell which was meant was to know the codebase.
///
/// The store number stays first. `devmap_health.probe` parses this line by
/// splitting on the first `"schema"` and taking the first integer after it
/// (`src/devcouncil/devmap_health.py`), so the store schema must remain the
/// first one named or that probe silently starts reporting the artifact
/// version as the store's.
///
/// Built once into a process-lifetime `OnceLock` rather than formatted per
/// call: clap's `version` takes a `&'static str`, and the schema number is only
/// known at runtime because it lives in another crate's constant.
fn version_line() -> &'static str {
    static LINE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    LINE.get_or_init(|| {
        format!(
            "{} (store schema {}, code graph schema {})",
            env!("CARGO_PKG_VERSION"),
            devmap_store::CURRENT_SCHEMA_VERSION,
            devmap_query::CODE_GRAPH_SCHEMA_VERSION
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
    /// so an un-flagged invocation must never aim at it.
    ///
    /// Unset, it is resolved by [`Cli::db`] rather than fixed to a literal:
    /// which state directory a repository uses is a property of the repository,
    /// not of this binary. See `devmap_extract::paths`.
    ///
    /// `global`, so it is accepted on either side of the subcommand. A flag that
    /// parses as `devmap --db X status` and fails as `devmap status --db X` is a
    /// papercut every caller hits once, and the generated agent guide hit it in
    /// writing: it told agents to run `devmap dead --json`, which did not parse.
    #[arg(short, long, global = true)]
    db: Option<PathBuf>,

    /// Machine-readable output. Global; see `--db`.
    #[arg(long, global = true, default_value_t = false)]
    json: bool,

    /// Build progress policy. Auto animates stages on interactive stderr;
    /// always also emits plain progress in logs. JSON stdout stays clean.
    #[arg(long, value_enum, global = true, default_value_t = ProgressMode::Auto)]
    progress: ProgressMode,

    /// Include build phase timings, reclaim details and resolution breakdowns.
    #[arg(long, short, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Commands,
}

impl Cli {
    /// The tree this invocation is about.
    ///
    /// Only the subcommands that actually name a repository root contribute
    /// one. `claude validate <path>` is deliberately absent: its argument is a
    /// *file* to check, and treating it as a root would resolve the store
    /// relative to a hooks manifest.
    fn root_hint(&self) -> PathBuf {
        match &self.command {
            Commands::Build { path, .. }
            | Commands::Manifest { path, .. }
            | Commands::Freshness { path, .. }
            | Commands::Serve { path, .. }
            | Commands::Html { path, .. }
            | Commands::Export { path, .. }
            | Commands::Routes { path, .. }
            | Commands::ShapeCheck { path, .. }
            | Commands::ApiImpact { path, .. }
            | Commands::Paths { path } => path.clone(),
            // Hook templates retain project-relative paths for the host that
            // will execute them; they are not a query against this checkout.
            Commands::Claude { .. } => PathBuf::from("."),
            _ => devmap_extract::git_worktree_root(Path::new("."))
                .unwrap_or_else(|| PathBuf::from(".")),
        }
    }

    /// The store this invocation reads and writes.
    ///
    /// An explicit `--db` is used as given — the Python seam passes one on
    /// every call, so DevCouncil's behaviour cannot change here. Otherwise the
    /// store is resolved against [`Self::root_hint`], which is what makes
    /// `devmap build /other/repo` index into *that* repository instead of
    /// creating a state directory under whatever the shell's working directory
    /// happened to be.
    fn db(&self) -> PathBuf {
        match &self.db {
            Some(explicit) => explicit.clone(),
            None => devmap_extract::paths::store_path(self.root_hint()),
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ProgressMode {
    Auto,
    Always,
    Never,
}

/// One completed span and the spans that ran inside it.
///
/// Recursive, because the breakdown is. A stage contains sub-phases, and a
/// sub-phase contains the split its own implementation measured — the
/// generation write reports what each relation cost, and only the store can,
/// since the node and full-text inserts are one interleaved loop. Two levels
/// were enough while `persist:write` was one number; a third would have needed
/// a second, near-identical struct, and the rule below is the same at every
/// depth.
struct StageTiming {
    label: String,
    seconds: f64,
    /// Spans closed while this one was open. Their durations are *included* in
    /// `seconds`; they break it down, they do not add to it. Summing two levels
    /// would double-count the build.
    sub: Vec<StageTiming>,
}

/// The stage in flight: its label, when it began, and the sub-phases closed
/// inside it so far.
///
/// Named rather than written inline because the tuple appears in a field, a
/// borrow and two closures, and a reader meeting `(String, Instant, Vec<(String,
/// f64)>)` in any of them has to reconstruct which position means what.
type OpenStage = (String, Instant, Vec<StageTiming>);

struct ProgressReporter {
    display: progress::Display,
    started_at: Instant,
    json: bool,
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
    const TOTAL_STAGES: usize = progress::TOTAL_STAGES;

    fn new(mode: ProgressMode, json: bool, verbose: bool) -> Self {
        Self {
            display: progress::Display::new(mode, json, verbose),
            started_at: Instant::now(),
            json,
            open: std::cell::RefCell::new(None),
            timings: std::cell::RefCell::new(Vec::new()),
        }
    }

    /// Close the running stage, recording its cost against its own label.
    fn close_open_stage(&self, succeeded: bool) {
        let Some((label, started, sub)) = self.open.borrow_mut().take() else {
            return;
        };
        let seconds = started.elapsed().as_secs_f64();
        self.display.closed(&label, seconds, succeeded);
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
        self.close_open_stage(true);
        let rendered = message.to_string();
        self.display.stage(current, &rendered);
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
        self.display.detail(label);
        let started = Instant::now();
        let outcome = work();
        let elapsed = started.elapsed().as_secs_f64();
        self.display.phase(label, elapsed);
        self.display.detail("");
        self.record(StageTiming {
            label: label.to_string(),
            seconds: elapsed,
            sub: Vec::new(),
        });
        outcome
    }

    /// Time one sub-phase whose implementation reports its own split.
    ///
    /// Some phases can only be broken down from the inside. `persist:write` is
    /// the case that forced this: it is 0.30 s of a 1.10 s one-file incremental
    /// build on this repository, the relations under it have nothing in common
    /// as fixes, and the node and full-text writes are one interleaved loop
    /// that nothing outside the store can separate. The store measures them and
    /// hands the labelled spans back here.
    ///
    /// The parts nest inside the sub-phase and are already counted in its
    /// `seconds`, exactly as sub-phases are counted in their stage's.
    fn timed_split<T, E>(
        &self,
        label: &str,
        work: impl FnOnce() -> std::result::Result<(T, Vec<(String, f64)>), E>,
    ) -> std::result::Result<T, E> {
        self.display.detail(label);
        let started = Instant::now();
        let outcome = work();
        let elapsed = started.elapsed().as_secs_f64();
        self.display.phase(label, elapsed);
        self.display.detail("");
        // An error path reports the phase with no split rather than no phase:
        // a write that failed halfway still took the time, and the parts it
        // managed to charge are not a breakdown of what it did.
        let parts = match &outcome {
            Ok((_, parts)) => parts.clone(),
            Err(_) => Vec::new(),
        };
        self.record(StageTiming {
            label: label.to_string(),
            seconds: elapsed,
            sub: parts
                .into_iter()
                .map(|(label, seconds)| StageTiming {
                    label,
                    seconds,
                    sub: Vec::new(),
                })
                .collect(),
        });
        outcome.map(|(value, _)| value)
    }

    /// File a closed span under the stage that was open when it ran.
    ///
    /// One owner for that decision, so `timed` and `timed_split` cannot come to
    /// disagree about where a sub-phase lands. A span closed outside any stage
    /// becomes a stage of its own rather than being dropped: silence there is
    /// how a phase goes unattributed and its time is charged to nothing.
    fn record(&self, timing: StageTiming) {
        match self.open.borrow_mut().as_mut() {
            Some((_, _, sub)) => sub.push(timing),
            None => self.timings.borrow_mut().push(timing),
        }
    }

    /// An untimed detail line under the current stage. Does not disturb the
    /// stage clock, so a note between two phases cannot be mistaken for one.
    fn note(&self, message: impl std::fmt::Display) {
        self.display.note(message);
    }

    /// Close the last stage and print the total.
    ///
    /// Deliberately not a `stage` call: completion is an instant, not a span,
    /// and opening a fifth stage here would leave it running forever and put a
    /// zero-length entry in the breakdown.
    fn complete(&self, generation_id: u32) {
        self.close_open_stage(true);
        if !self.json {
            self.display.finish("");
            return;
        }
        self.display.finish(format_args!(
            "[{}/{}] complete: generation #{generation_id} in {}",
            Self::TOTAL_STAGES,
            Self::TOTAL_STAGES,
            progress::duration(self.started_at.elapsed().as_secs_f64())
        ));
    }

    fn up_to_date(&self, generation: u32, files: usize) {
        self.close_open_stage(true);
        if !self.json {
            self.display.finish("");
            return;
        }
        self.display.finish(format_args!(
            "up to date: generation #{generation} · {} checked in {} · resolve/analyze/write skipped",
            progress::count(files, "file"),
            progress::duration(self.started_at.elapsed().as_secs_f64())
        ));
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
        fn render(label: &str, secs: f64, sub: &[StageTiming], open: bool) -> serde_json::Value {
            let mut entry = serde_json::json!({"stage": label, "seconds": secs});
            if !sub.is_empty() {
                entry["sub"] = sub
                    .iter()
                    .map(|t| render(&t.label, t.seconds, &t.sub, false))
                    .collect();
            }
            if open {
                entry["open"] = serde_json::Value::Bool(true);
            }
            entry
        }
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

/// Which graph the HTML view draws.
#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum HtmlLevel {
    /// Files and their imports.
    Files,
    /// Symbols and their calls, inheritance and named imports.
    Symbols,
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

/// Caller-computed freshness digests to stamp into both artifacts.
///
/// The kernel computes these itself now (see `devmap_query::freshness`), so
/// these flags are no longer how the values normally arrive. They stay because a
/// caller with its own inventory rules — a monorepo tool that indexes a subtree,
/// a test that wants a fixed stamp — is entitled to say what the digests are,
/// and because a value supplied here is used verbatim rather than recomputed.
///
/// Supplying *any* of the three switches the whole set to caller-supplied: a run
/// that mixed one caller digest with two of its own would stamp an identity no
/// single snapshot of the tree ever had.
#[derive(Debug, Clone, clap::Args)]
struct StampFlags {
    /// Caller-computed `generated_head` (`git rev-parse HEAD`).
    #[arg(long)]
    generated_head: Option<String>,
    /// Caller-computed `indexed_hash` (SHA-1 over the git file list).
    #[arg(long)]
    indexed_hash: Option<String>,
    /// Caller-computed `content_fingerprint` (scheme-prefixed SHA-1 over file bytes).
    #[arg(long)]
    content_fingerprint: Option<String>,
}

impl StampFlags {
    fn supplied(&self) -> Option<StampedFreshness> {
        let stamped = StampedFreshness {
            generated_head: non_empty(&self.generated_head),
            indexed_hash: non_empty(&self.indexed_hash),
            content_fingerprint: non_empty(&self.content_fingerprint),
        };
        let any = stamped.generated_head.is_some()
            || stamped.indexed_hash.is_some()
            || stamped.content_fingerprint.is_some();
        any.then_some(stamped)
    }
}

/// The two inventory rules `RepoMapper._inventory_limits` reads out of
/// `.devcouncil/config.yaml`.
///
/// Passed in rather than parsed here: they live in a YAML document holding the
/// whole project configuration, and a second reader for two scalars would
/// disagree with the real one in ways that surface as a silently different file
/// set — which is a *wrong digest*, not a missing one. The defaults match the
/// defaults `_inventory_limits` falls back to.
#[derive(Debug, Clone, Copy, clap::Args)]
struct InventoryFlags {
    /// Fingerprint tracked files only (`indexing.include_untracked: false`).
    #[arg(long)]
    no_untracked: bool,
    /// Inventory ceiling (`indexing.max_indexed_files`).
    #[arg(long, default_value_t = 50_000)]
    max_indexed_files: usize,
}

impl From<InventoryFlags> for InventoryLimits {
    fn from(flags: InventoryFlags) -> Self {
        Self {
            include_untracked: !flags.no_untracked,
            max_indexed_files: flags.max_indexed_files,
        }
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
        verify_rows: bool,
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
        manifest: bool,
        /// Unset, resolved against `path`'s state directory. See
        /// `devmap_extract::paths`.
        #[arg(long, requires = "manifest")]
        output: Option<PathBuf>,
        /// Unset, resolved against `path`'s state directory.
        #[arg(long, requires = "manifest")]
        graph_output: Option<PathBuf>,
        /// Also write the marker-guarded agent guides. See `manifest --guides`.
        #[arg(long, requires = "manifest")]
        guides: bool,
        /// Replace a Python-schema or otherwise foreign repo map / code graph.
        #[arg(long, requires = "manifest", default_value_t = false)]
        force: bool,
        #[command(flatten)]
        stamps: StampFlags,
        #[command(flatten)]
        inventory: InventoryFlags,
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
        /// Keep only edges at or above a named rung on the resolution ladder:
        /// `deterministic`, `high` or `speculative`. Omitted filters nothing.
        ///
        /// A name rather than a `--min-confidence` float, because the ladder
        /// has named rungs and a caller wanting deterministic edges should not
        /// have to know that means 1.0. The answer carries a `rungs` histogram
        /// of the population *before* the cut, so a short list is never
        /// mistaken for a sparse graph.
        #[arg(long)]
        min_rung: Option<String>,
    },
    Impact {
        target: String,
        #[arg(short, long, default_value_t = 2000)]
        budget: u32,
        #[arg(long, default_value_t = 3)]
        depth: usize,
        /// Keep only edges at or above a named rung on the resolution ladder:
        /// `deterministic`, `high` or `speculative`. Omitted filters nothing.
        ///
        /// A name rather than a `--min-confidence` float, because the ladder
        /// has named rungs and a caller wanting deterministic edges should not
        /// have to know that means 1.0. The answer carries a `rungs` histogram
        /// of the population *before* the cut, so a short list is never
        /// mistaken for a sparse graph.
        #[arg(long)]
        min_rung: Option<String>,
        /// Also band the reached symbols by distance from the target.
        ///
        /// The flat edge list says *what* reaches the target; it cannot say how
        /// far, because an edge does not carry the hop the walk found it at. A
        /// consumer that needs "3 call it directly and 39 are reached through
        /// those 3" gets it from the kernel here rather than inventing it.
        ///
        /// Refused together with `--min-rung`: the band walk filters on
        /// confidence and has no rung, so honouring one would narrow the edges
        /// and leave the bands wide.
        #[arg(long)]
        layers: bool,
    },
    /// Callers and callees for several targets in one invocation.
    ///
    /// Exists because the composed views above it were paying a process spawn
    /// per direction per target: a five-definition `graph_query` cost eleven
    /// `devmap` invocations, and the spawn — not the query — was the wall
    /// clock. More than `MAX_NEIGHBOR_TARGETS` targets is refused rather than
    /// trimmed, so a short answer is never mistaken for a complete one.
    Neighbors {
        #[arg(required = true, num_args = 1..)]
        targets: Vec<String>,
        #[arg(short, long, default_value_t = 2000)]
        budget: u32,
        #[arg(long, default_value_t = 1)]
        depth: usize,
        #[arg(long, default_value_t = 0.0)]
        min_confidence: f32,
        /// Keep only edges at or above a named rung on the resolution ladder:
        /// `deterministic`, `high` or `speculative`. Omitted filters nothing.
        ///
        /// Accepted here because this command *is* `impact` and `deps` composed,
        /// and both take the floor. A composition that drops a filter its parts
        /// accept answers the narrower question its caller did not ask.
        #[arg(long)]
        min_rung: Option<String>,
    },
    Trace {
        from: String,
        to: Option<String>,
        #[arg(short, long, default_value_t = 2000)]
        budget: u32,
        #[arg(long, default_value_t = 3)]
        depth: usize,
        /// Keep only edges at or above a named rung on the resolution ladder:
        /// `deterministic`, `high` or `speculative`. Omitted filters nothing.
        ///
        /// A name rather than a `--min-confidence` float, because the ladder
        /// has named rungs and a caller wanting deterministic edges should not
        /// have to know that means 1.0. The answer carries a `rungs` histogram
        /// of the population *before* the cut, so a short list is never
        /// mistaken for a sparse graph.
        #[arg(long)]
        min_rung: Option<String>,
    },
    Dead {
        #[arg(short, long, default_value_t = 2000)]
        budget: u32,
    },
    /// Definitions matching a query, with source, callers, callees and a
    /// layered blast radius — the whole neighbourhood in one invocation.
    ///
    /// Replaces the Python `CodeIntelQueryEngine.explore`, which loaded the
    /// entire graph into process memory to answer. The budget is divided across
    /// the four parts and the division is reported in `budget`, so a thin edge
    /// list is attributable to the allowance rather than mistaken for a symbol
    /// nothing calls.
    Explore {
        query: String,
        /// Definitions to consider. The budget decides how many are packed;
        /// both numbers are reported.
        #[arg(short, long, default_value_t = 20)]
        limit: usize,
        #[arg(short, long, default_value_t = devmap_query::Budget::EXPLORE)]
        budget: u32,
        #[arg(long, default_value_t = 3)]
        depth: usize,
        #[arg(long, default_value_t = 0.0)]
        min_confidence: f32,
    },
    /// Test files reachable through the inbound blast radius of some targets.
    ///
    /// Ranked nearest-first: a budget-trimmed list keeps the tests closest to
    /// the change. Targets that match nothing are named rather than dropped.
    Affected {
        #[arg(required = true, num_args = 1..)]
        targets: Vec<String>,
        #[arg(short, long, default_value_t = devmap_query::Budget::AFFECTED)]
        budget: u32,
        #[arg(long, default_value_t = 3)]
        depth: usize,
        #[arg(long, default_value_t = 0.0)]
        min_confidence: f32,
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
        /// Unset, resolved against `path`'s state directory. See
        /// `devmap_extract::paths`.
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Symbol-level graph companion artifact. Unset, resolved against
        /// `path`'s state directory.
        #[arg(long)]
        graph_output: Option<PathBuf>,
        /// Also write the interned encoding of the same graph here.
        ///
        /// Opt-in and additive: the verbose artifact above stays canonical and
        /// is written either way, because every existing consumer reads it.
        /// This form carries the identical model with each distinct string
        /// written once and referred to by index — on this repository
        /// 20,899,318 B becomes 4,951,872 B (-76.3%), `json.loads` 103.2 ms
        /// becomes 49.5 ms, write+fsync 10.7 ms becomes 4.2 ms. `source` and
        /// `target` alone were 52.6% of the verbose file: 14,324 distinct
        /// endpoint strings written 147,726 times.
        ///
        /// It does **not** make the graph readable by an agent — 5.2M tokens
        /// becomes 1.2M, which is still unopenable. It buys bytes, parse time
        /// and disk churn. Use `devmap search` / `impact` / `trace` to read the
        /// graph.
        #[arg(long)]
        compact_graph_output: Option<PathBuf>,
        /// Also write the marker-guarded agent guides, `AGENTS.md` and
        /// `CLAUDE.md`.
        ///
        /// Opt-in: creating two files in somebody's repository is not a thing a
        /// code index should do unasked. A guide that exists but carries no
        /// `Managed by devmap` marker is hand-written and is never touched.
        #[arg(long)]
        guides: bool,
        /// Replace a Python-schema or otherwise foreign repo map / code graph.
        #[arg(long, default_value_t = false)]
        force: bool,
        #[command(flatten)]
        stamps: StampFlags,
        #[command(flatten)]
        inventory: InventoryFlags,
    },
    /// Render the repo map as one self-contained, offline HTML page.
    ///
    /// Reads the `repo_map.json` `manifest` writes — no store, so it answers
    /// for any checkout that has a map, and never blocks on an indexing run.
    ///
    /// Supersedes the Python renderer at `src/devcouncil/indexing/map_viz.py`,
    /// which coloured nodes by hashing the area name and dropped the file
    /// inventory before it could say anything about language or coverage.
    MapHtml {
        /// Repository root; `--input` and `--output` resolve against it.
        #[arg(default_value = ".")]
        path: PathBuf,
        /// The repo map to render.
        #[arg(long, default_value = ".devcouncil/repo_map.json")]
        input: PathBuf,
        /// Where to write the page.
        #[arg(short, long, default_value = ".devcouncil/map.html")]
        output: PathBuf,
        /// Rewrite even when the existing page already carries this map's
        /// fingerprint.
        #[arg(long, default_value_t = false)]
        force: bool,
    },

    /// The three freshness digests for a working tree, and — with the
    /// `--expect-*` flags — whether a map stamped with given values is stale.
    ///
    /// This is `RepoMapper.map_is_stale`'s expensive half moved off Python: two
    /// `git ls-files` passes and a stat walk over the whole inventory, which
    /// cost ~150 ms per call in the interpreter and are asked on every
    /// `--if-stale`, every watch tick and every verify. It needs no store, so it
    /// answers for a repository that has never been indexed.
    ///
    /// The comparison is reported field by field: a caller is told *which* of
    /// head, inventory and content moved, not merely that something did.
    Freshness {
        #[arg(default_value = ".")]
        path: PathBuf,
        /// The `generated_head` a map carries. Compared, never written.
        #[arg(long)]
        expect_head: Option<String>,
        /// The `indexed_hash` a map carries.
        #[arg(long)]
        expect_indexed_hash: Option<String>,
        /// The `content_fingerprint` a map carries.
        #[arg(long)]
        expect_content_fingerprint: Option<String>,
        /// Read the digest memo but never write it, for callers that must not
        /// modify the project — the MCP freshness probe runs on tools annotated
        /// `readOnlyHint: true`, and that annotation is a promise.
        #[arg(long)]
        no_cache_write: bool,
        #[command(flatten)]
        inventory: InventoryFlags,
    },
    Status,
    /// Binary and store health for hosts that install or verify `devmap`.
    ///
    /// Emits the store schema on disk (if any), the schema this binary speaks,
    /// the code-graph artifact schema, how many tree-sitter grammars are linked
    /// into this build, and the resolved store path. GitPulse and similar hosts
    /// call `devmap doctor --json` rather than scraping `devmap --version`
    /// prose, so a vendored-vs-on-disk grammar or schema mismatch is a structured
    /// refusal rather than a parse of free text.
    Doctor,
    /// Where this repository's state lives — the state directory, the store, the
    /// artifacts, the workspace registry — resolved exactly as every other
    /// command resolves them, and reported without opening anything.
    ///
    /// The Python seam's state-directory resolver asked `status` for `db_path`
    /// once per process; `status` opens the store to count nodes, so that cost
    /// 29 ms against a 168 MB store to answer a question the kernel settles
    /// before it opens anything, and could not be answered at all for a store
    /// `status` cannot open. Existence is reported, never inferred: a resolved
    /// directory that is on disk is where the state actually is.
    Paths {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
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
        /// Rewrite the store at the current default page size.
        ///
        /// Page size is fixed when a database first gets content, so a store
        /// built before the default changed keeps its old one for life: the
        /// pragma is accepted and ignored on an existing database, and the
        /// daemon reopens whatever it finds. Nothing in the normal course of
        /// running converts one, which is why this is an explicit action —
        /// the rewrite takes an exclusive lock and leaves WAL for its duration.
        #[arg(long = "page-size")]
        page_size: bool,
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

    /// Speak the Model Context Protocol on stdin/stdout, for an agent host.
    ///
    /// This is the connection an agent actually makes. It answers `tools/call`
    /// from the store held open in this process, where the Python seam spawned
    /// a fresh `devmap` per request whenever no daemon socket was live.
    ///
    /// The store is opened on first use, not here: an agent host starts this
    /// server when its session begins, which on a fresh clone is before any
    /// index exists. Starting anyway means the agent sees the tools and a
    /// message naming the build command, rather than seeing no server at all.
    Mcp {
        /// Serve MCP 2.0 (protocol 2026-07-28) over HTTP on this address
        /// instead of speaking stdio.
        ///
        /// That revision is not reachable over stdio at all: its requests are
        /// self-contained POSTs carrying their own protocol version, and the
        /// `initialize` handshake every stdio client uses tops out at
        /// 2025-11-25. This flag is the only way to reach it.
        ///
        /// Defaults to loopback when given a bare port. A code index is a map of
        /// a private repository, so binding it to a routable interface publishes
        /// that map — do that deliberately or not at all.
        #[arg(long, value_name = "ADDR")]
        http: Option<String>,

        /// Print the MCP server entry an agent host needs, and exit.
        ///
        /// Creates nothing and starts nothing — the same contract as
        /// `serve --print-socket-path`, and for the same reason: a second
        /// implementation of "how do I reach this server" living in a host's
        /// config generator can disagree with this binary, and the disagreement
        /// is invisible from either side. This is the authority.
        ///
        /// The emitted `command` is this executable's own absolute path, not the
        /// bare name `devmap`. A host config that says `devmap` works only when
        /// something already put it on PATH, which on a fresh machine is exactly
        /// what has not happened.
        #[arg(long)]
        print_config: bool,
    },

    /// Control and data dependency graphs for the functions in a file.
    ///
    /// Intra-procedural: a closure's body is its own PDG, not part of its
    /// enclosing function's control flow.
    ///
    /// Python only. That is the language the analysis this ports covered, and
    /// claiming a language whose statement tree nothing produces would return an
    /// empty result that reads as "this file has no control flow".
    Pdg {
        /// File to analyse. Read from disk, not from the index, so it answers
        /// about the buffer on disk right now.
        file: PathBuf,
        /// Report only statements that reach a security-sensitive sink.
        ///
        /// A sink is evidence. Its *absence* is not a safety claim: the patterns
        /// are a heuristic list of well-known sinks, transcribed from the
        /// implementation this replaces.
        #[arg(long)]
        taint: bool,
    },

    /// Run a small openCypher subset over the graph.
    ///
    /// Supported: `MATCH (a)-[r:calls|imports|…]->(b) WHERE … RETURN a, b
    /// LIMIT n`, with `contains(a.name, '…')` and `starts with(b.path, '…')`
    /// joined by `AND`.
    ///
    /// Anything outside that is **refused**, never silently widened: a `WHERE`
    /// term this cannot evaluate would otherwise return every row in the graph
    /// under a successful status.
    Cypher {
        /// The query.
        query: String,
        /// Rows to return when the query states no `LIMIT`. A query's own
        /// `LIMIT` is a request; the server's ceiling still applies, and both
        /// numbers are reported.
        #[arg(short, long, default_value_t = 50)]
        limit: usize,
    },

    /// Find symbols by kind, language and name, from the parsed index.
    ///
    /// The structural counterpart to `search`: `search` ranks by name relevance
    /// and budgets its answer; this enumerates everything matching a filter and
    /// reports the exact total.
    Ast {
        /// Case-insensitive substring of the name or qualified name.
        #[arg(default_value = "")]
        query: String,
        /// Only this symbol kind. `--facets` lists the ones this index holds.
        #[arg(long)]
        kind: Option<String>,
        /// Only this language.
        #[arg(long)]
        language: Option<String>,
        /// Rows to return. The total is reported whatever this is.
        #[arg(short, long, default_value_t = 100)]
        limit: usize,
        /// List the kinds and languages this generation holds, and stop.
        #[arg(long)]
        facets: bool,
    },

    /// Write the graph as GraphML, for Gephi, yEd, Cytoscape or networkx.
    ///
    /// Attributed: every node carries its kind, path, area, community and its
    /// dead/unwired/unreachable flags; every edge its kind and confidence.
    Export {
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Where to write. Defaults to `<state dir>/graph.graphml`; `-` is stdout.
        #[arg(short, long)]
        out: Option<PathBuf>,
    },

    /// HTTP routes, their handlers, and the clients that call them.
    ///
    /// Routes come from `routes_to` edges, whose source the resolver writes as
    /// `"VERB /path"`. Client call sites are found by pattern, over a bounded
    /// walk of the files the graph names — so every answer carries what the
    /// scan read and whether it finished.
    Routes {
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Only routes matching this path or id.
        #[arg(long)]
        filter: Option<String>,
        /// Files the client scan may open before it stops.
        #[arg(long, default_value_t = 5_000)]
        max_files: usize,
        /// Largest file the client scan will read, in bytes.
        #[arg(long, default_value_t = 1 << 20)]
        max_file_bytes: u64,
    },

    /// Compare what a handler returns against what its callers read.
    #[command(name = "shape-check")]
    ShapeCheck {
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Only routes matching this path or id.
        #[arg(long)]
        filter: Option<String>,
        #[arg(long, default_value_t = 5_000)]
        max_files: usize,
        #[arg(long, default_value_t = 1 << 20)]
        max_file_bytes: u64,
    },

    /// What changing one route reaches: callers, shape, and a risk band.
    #[command(name = "api-impact")]
    ApiImpact {
        /// The route path or `"VERB /path"` id.
        route: String,
        #[arg(default_value = ".")]
        path: PathBuf,
        #[arg(long, default_value_t = 5_000)]
        max_files: usize,
        #[arg(long, default_value_t = 1 << 20)]
        max_file_bytes: u64,
    },

    /// Render the graph as one self-contained HTML file.
    ///
    /// No network and no build step: the renderer is embedded, so the page
    /// opens from a `file://` URL on a machine that has never seen a package
    /// manager. Capped by node count and honest about it — see `--max-nodes`.
    Html {
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Where to write. Defaults to `<state dir>/graph.html`.
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// What a node is: files and their imports, or symbols and their calls.
        ///
        /// The subsystem view is `devmap map-html`, which reads `repo_map.json`
        /// and colours by language.
        #[arg(long, value_enum, default_value_t = HtmlLevel::Files)]
        level: HtmlLevel,
        /// Most nodes to draw, ranked by degree so the hubs survive.
        ///
        /// A force layout stops converging in a browser tab well before a real
        /// repository's node count, so this is a cap rather than a preference.
        /// Whatever it cuts is stated in the payload *and* in the page header:
        /// "1,000 of 12,103 nodes", never "1,000 nodes".
        #[arg(long, default_value_t = 1_500)]
        max_nodes: usize,
    },

    /// Emit and check Dev Map's own Claude Code integration.
    ///
    /// Hook specs and a plugin manifest, built and validated here rather than
    /// by a generator living somewhere else: Claude Code drops configuration it
    /// cannot make sense of *quietly* — an unknown event name is ignored at
    /// runtime, a matcher on an event without matcher support is ignored, an
    /// `if` outside a tool event means the handler never runs — so a writer
    /// that only serializes reports the same success for a dead install as for
    /// a working one.
    Claude {
        #[command(subcommand)]
        action: ClaudeAction,
    },
}

#[derive(Subcommand)]
enum ClaudeAction {
    /// Print the `hooks` block for `.claude/settings.json`.
    ///
    /// Printed, not installed: a settings file is the user's, and merging into
    /// it is their edit to make. Everything needed to make it is here.
    Hooks {
        /// Command to write into the emitted handlers.
        ///
        /// Unset, `devmap` is emitted when that name on `PATH` resolves to this
        /// binary, and this binary's absolute path otherwise. Set it when the
        /// install location is known but not yet populated — packaging.
        #[arg(long)]
        binary: Option<PathBuf>,
    },

    /// List every documented hook event beside what Dev Map does about it.
    ///
    /// Coverage stated as a decision per event, so "not handled" is on the
    /// record with its reason rather than being an omission nobody counted.
    Events,

    /// Write the installable plugin bundle: marketplace, manifest, hooks, MCP.
    Plugin {
        /// Directory the bundle is written under.
        ///
        /// Unset, resolved to `<state dir>/devmap-plugin`. Deliberately *not*
        /// `<state dir>/claude-plugin`, which is DevCouncil's own bundle: the
        /// two emitters write a single-repo marketplace to the same
        /// `.claude-plugin/marketplace.json`, under different names
        /// (`devcouncil-local` and `devmap-local`), so sharing the directory
        /// meant whichever ran last silently replaced the other's registration.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Render to stdout instead of writing anything.
        #[arg(long)]
        dry_run: bool,
        /// Command to write into the emitted hooks and MCP entry. See
        /// `claude hooks --binary`.
        #[arg(long)]
        binary: Option<PathBuf>,
    },

    /// Check an existing hook config, plugin manifest, or marketplace file.
    Validate {
        path: PathBuf,
        /// Treat warnings as errors, as `claude plugin validate --strict` does.
        #[arg(long)]
        strict: bool,
    },
}

/// Everything one `manifest` write needs, whether it was asked for on its own
/// or fused onto the end of a build.
struct ManifestRequest<'a> {
    progress: Option<&'a progress::Display>,
    /// The tree the caller named. Used only when the store cannot say where its
    /// repository root is.
    path: &'a std::path::Path,
    db: &'a std::path::Path,
    output: &'a std::path::Path,
    graph_output: &'a std::path::Path,
    compact_graph_output: Option<&'a std::path::Path>,
    force: bool,
    stamps: &'a StampFlags,
    inventory: InventoryLimits,
    /// Write the marker-guarded agent guides from this generation.
    guides: bool,
}

/// What a `manifest` write did, for the caller's `--json` payload.
struct ManifestOutcome {
    /// One entry per guide file considered, or empty when guides were not
    /// requested. Reported in full — including the files left alone and why —
    /// because "the guide was not refreshed" and "the guide is hand-written and
    /// therefore ours to leave" are different facts and only one is a problem.
    guides: Vec<devmap_query::guides::GuideOutcome>,
    output: std::path::PathBuf,
    graph_output: std::path::PathBuf,
    compact_graph_output: Option<std::path::PathBuf>,
    generation_id: u32,
    /// True when the artifacts on disk were already exactly the ones this run
    /// would have written, and the store was therefore never read.
    artifacts_unchanged: bool,
    /// `caller` / `kernel` / `unavailable` — where the three stamps came from.
    freshness_source: &'static str,
    freshness_unavailable_reason: String,
}

/// `<db>.artifacts.json` — the stamp beside the store the artifacts came from.
///
/// Beside the store rather than beside the artifacts: it describes what *this*
/// store's current generation produced, and two stores pointed at one output
/// path must not share one stamp. It also keeps it out of the git inventory,
/// so it can never change the fingerprint it helps compute.
fn artifact_stamp_path(db: &std::path::Path) -> std::path::PathBuf {
    std::path::PathBuf::from(format!("{}.artifacts.json", db.display()))
}

/// Write `repo_map.json` and `code_graph.json` from the store's current
/// generation — or prove they are already written and touch nothing.
///
/// The proof is a sidecar naming the binary, every input the artifacts derive
/// from, and each output's `(len, mtime, inode)` as written. When it holds, the
/// generation is never read out of SQLite and nothing is serialized: the case a
/// watcher and the PostToolUse hook hit on almost every tick.
/// Write `AGENTS.md` / `CLAUDE.md` from this generation, when asked.
///
/// Returns an empty vector when guides were not requested, which is the one
/// case that costs nothing: the manifest generation this needs is skipped
/// entirely rather than computed and discarded.
///
/// A failure here is fatal rather than a warning. The guide is what points an
/// agent at the map; a run that silently failed to refresh it leaves the tree
/// with a guide describing a generation that no longer exists, and nothing said
/// so.
fn write_guides_if_requested(
    store: &Store,
    request: &ManifestRequest<'_>,
    tree: &std::path::Path,
) -> anyhow::Result<Vec<devmap_query::guides::GuideOutcome>> {
    if !request.guides {
        return Ok(Vec::new());
    }
    let extractions = store.latest_extractions()?;
    let analysis = store
        .latest_analysis()?
        .ok_or_else(|| anyhow::anyhow!("guides unavailable: build a persisted generation first"))?;
    let edges = store
        .latest_edges(0.0)?
        .into_iter()
        .map(resolved_edge_from_stored)
        .collect::<anyhow::Result<Vec<_>>>()?;
    // Stamps the guide never reads. Passing the real ones would mean computing
    // the digests first, which is the ordering this whole function exists to
    // avoid; passing placeholders is safe precisely because `guides.rs` reads
    // `subsystems`, `important_files` and `meta.devmap_rust` and nothing else.
    let placeholder = FreshnessInfo {
        head_sha: String::new(),
        generation_id: 0,
        pending_count: 0,
        stamped: StampedFreshness::default(),
    };
    let (_manifest, json_str) =
        generate_manifest_with_edges(&extractions, &analysis, placeholder, &edges, Some(tree));
    let map: serde_json::Value = serde_json::from_str(&json_str)?;

    let relative = |absolute: &std::path::Path| -> String {
        let text = absolute
            .strip_prefix(tree)
            .unwrap_or(absolute)
            .to_string_lossy()
            .replace('\\', "/");
        // `--db` defaults to a CWD-relative path, so stripping the tree prefix
        // can leave `./.devmap/...`. The guide is prose an agent reads and
        // copies; a stray `./` is noise in every line that quotes a path.
        text.strip_prefix("./").unwrap_or(&text).to_string()
    };
    Ok(devmap_query::guides::write_agent_guides(
        tree,
        &map,
        &relative(&devmap_extract::paths::repo_map_path(tree)),
        &relative(&devmap_extract::paths::code_graph_path(tree)),
        &relative(request.db),
    )?)
}

/// The graph model, read from the store, for a surface that only wants to look.
///
/// One owner for `html` and `cypher`: both project the same value the artifact
/// writer builds, so a picture and a query cannot describe different
/// generations — and neither pays to parse a 20 MB `code_graph.json` back off
/// disk to answer.
///
/// The freshness stamps are the store's own and are deliberately not computed
/// here. These surfaces describe the generation, not the working tree, and
/// digesting the tree would make rendering a picture cost a full rehash.
fn graph_value_for_read(store: &Store, db: &std::path::Path) -> anyhow::Result<serde_json::Value> {
    let gen_id = store
        .latest_generation_id()?
        .ok_or_else(|| anyhow::anyhow!("no committed generation: run `devmap build` first"))?;
    let extractions = store.latest_extractions()?;
    let analysis = store
        .latest_analysis()?
        .ok_or_else(|| anyhow::anyhow!("no committed generation: run `devmap build` first"))?;
    let edges = store
        .latest_edges(0.0)?
        .into_iter()
        .map(resolved_edge_from_stored)
        .collect::<anyhow::Result<Vec<_>>>()?;
    let freshness = FreshnessInfo {
        head_sha: store
            .latest_generation_head()?
            .unwrap_or_else(|| "unavailable".to_string()),
        generation_id: gen_id,
        pending_count: store.status(&db.display().to_string())?.pending_count,
        stamped: StampedFreshness::default(),
    };
    let repo_root = store.latest_repo_root()?;
    devmap_query::build_code_graph_value(
        &extractions,
        &analysis,
        &edges,
        &freshness,
        repo_root.as_deref(),
    )
}

/// The graph the read-only surfaces answer from: the artifact's `nodes` and
/// `edges` from the latest generation, and none of its panels — no `git log`,
/// no intel, no dead-code list, no freshness. `build_graph_core_value` says
/// what building the whole artifact cost these commands.
fn graph_core_for_read(store: &Store) -> anyhow::Result<serde_json::Value> {
    store
        .latest_generation_id()?
        .ok_or_else(|| anyhow::anyhow!("no committed generation: run `devmap build` first"))?;
    let extractions = store.latest_extractions()?;
    let analysis = store
        .latest_analysis()?
        .ok_or_else(|| anyhow::anyhow!("no committed generation: run `devmap build` first"))?;
    let edges = store
        .latest_edges(0.0)?
        .into_iter()
        .map(resolved_edge_from_stored)
        .collect::<anyhow::Result<Vec<_>>>()?;
    let repo_root = store.latest_repo_root()?;
    Ok(devmap_query::build_graph_core_value(
        &extractions,
        &analysis,
        &edges,
        repo_root.as_deref(),
    ))
}

fn write_consumer_artifacts(
    store: &Store,
    request: ManifestRequest<'_>,
) -> anyhow::Result<ManifestOutcome> {
    let gen_id = store.latest_generation_id()?.ok_or_else(|| {
        anyhow::anyhow!("manifest unavailable: build a persisted generation first")
    })?;
    let status = store.status(&request.db.display().to_string())?;
    let built_head = store
        .latest_generation_head()?
        .unwrap_or_else(|| "unavailable".to_string());
    let repo_root = store.latest_repo_root()?.or_else(|| {
        request
            .path
            .canonicalize()
            .ok()
            .map(|root| root.to_string_lossy().into_owned())
    });

    // The tree the digests describe is the one the store indexed. Falling back
    // to the caller's path only when the store cannot say keeps the stamps and
    // the generation talking about the same directory.
    let tree = repo_root
        .as_deref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| request.path.to_path_buf());

    // The guides go in **before** the digests are taken, not after.
    //
    // A guide this run creates or rewrites is a file in the tree, and unless the
    // repository ignores it, it is part of the inventory `freshness::compute`
    // hashes. Written afterwards it would move `content_fingerprint` the instant
    // it landed, and the map would report itself stale against a tree only it
    // had changed — a false staleness that costs a rebuild on every single run.
    //
    // The guide's text depends on the manifest's subsystems and provenance
    // markers but not on its freshness stamps, so generating the manifest early
    // to feed the guide and again afterwards with real stamps is well-founded:
    // the second generation cannot change what the first one said here. The
    // early generation is skipped entirely unless guides were asked for.
    let guides = write_guides_if_requested(store, &request, &tree)?;

    let (stamped, freshness_source, freshness_unavailable_reason) = match request.stamps.supplied()
    {
        Some(supplied) => (supplied, "caller", String::new()),
        None => {
            let digests = freshness::compute(&tree, request.inventory, true);
            let source = if digests.unavailable_reason.is_empty() {
                "kernel"
            } else {
                "unavailable"
            };
            (
                StampedFreshness {
                    generated_head: digests.generated_head,
                    indexed_hash: digests.indexed_hash,
                    content_fingerprint: digests.content_fingerprint,
                },
                source,
                digests.unavailable_reason,
            )
        }
    };

    let dest = resolve_manifest_output(repo_root.as_deref(), request.output);
    let graph_dest = resolve_manifest_output(repo_root.as_deref(), request.graph_output);
    let compact_dest = request
        .compact_graph_output
        .map(|destination| resolve_manifest_output(repo_root.as_deref(), destination));

    // Every input the artifacts' bytes derive from, as real JSON. These values
    // are compared for equality to decide a skip, and they are also the only
    // record of *why* a given set of artifacts exists, so a consumer has to be
    // able to read them. `serde_json::Value` keeps the property the previous
    // `{:?}` renderings were reaching for — `null` and `""` are different
    // values, so a digest that could not be computed can never compare equal to
    // one that came out empty — without the file being JSON in syntax only.
    let mut inputs: std::collections::BTreeMap<String, serde_json::Value> =
        std::collections::BTreeMap::new();
    inputs.insert("generation_id".into(), gen_id.into());
    inputs.insert("pending_count".into(), status.pending_count.into());
    inputs.insert("built_head".into(), built_head.clone().into());
    inputs.insert("repo_root".into(), repo_root.clone().into());
    inputs.insert(
        "generated_head".into(),
        stamped.generated_head.clone().into(),
    );
    inputs.insert("indexed_hash".into(), stamped.indexed_hash.clone().into());
    inputs.insert(
        "content_fingerprint".into(),
        stamped.content_fingerprint.clone().into(),
    );
    inputs.insert("code_graph_schema".into(), CODE_GRAPH_SCHEMA_VERSION.into());
    // The one input that moves with the clock rather than the tree: churn is
    // `git log --since=90.days`, relative to now, so the same repository on a
    // later day is a different window. Without this the artifacts of a quiet
    // repository matched every input for months while their hotspot counts
    // silently shrank. Day granularity: one regeneration per calendar day at
    // most, and only on a run that would otherwise have skipped.
    inputs.insert(
        "churn_window_day".into(),
        devmap_query::inventory::churn_window_day().into(),
    );
    inputs.insert(
        "compact".into(),
        match &compact_dest {
            Some(path) => path.to_string_lossy().into_owned().into(),
            None => serde_json::Value::Null,
        },
    );

    // Taken before `stamped` is consumed below; the stamp is written at the end
    // of the run, long after it has been moved into the manifest.
    let stamp_generated_head = stamped.generated_head.clone();

    let stamp_path = artifact_stamp_path(request.db);
    // Role, not position: the sidecar is read by consumers that cannot rebuild
    // the writer's spelling of these paths, so each output is named.
    let mut outputs: Vec<(&str, &std::path::Path)> = vec![
        ("repo_map", dest.as_path()),
        ("code_graph", graph_dest.as_path()),
    ];
    if let Some(compact) = &compact_dest {
        outputs.push(("compact_graph", compact.as_path()));
    }
    if ArtifactStamp::read(&stamp_path).is_some_and(|stamp| stamp.still_current(&inputs, &outputs))
    {
        return Ok(ManifestOutcome {
            output: dest,
            graph_output: graph_dest,
            compact_graph_output: compact_dest,
            generation_id: gen_id,
            artifacts_unchanged: true,
            guides,
            freshness_source,
            freshness_unavailable_reason,
        });
    }

    let extractions = store.latest_extractions()?;
    let analysis = store.latest_analysis()?.ok_or_else(|| {
        anyhow::anyhow!("manifest unavailable: build a persisted generation first")
    })?;
    let edges = store
        .latest_edges(0.0)?
        .into_iter()
        .map(resolved_edge_from_stored)
        .collect::<anyhow::Result<Vec<_>>>()?;
    // One freshness identity for both artifacts: a map and a graph stamped from
    // different generations is the drift the single command exists to prevent.
    let freshness = FreshnessInfo {
        head_sha: built_head,
        generation_id: gen_id,
        pending_count: status.pending_count,
        stamped,
    };
    let (_manifest, json_str) = generate_manifest_with_edges(
        &extractions,
        &analysis,
        freshness.clone(),
        &edges,
        Some(tree.as_path()),
    );
    let (graph_json, compact_graph_json) = generate_code_graph_encodings(
        &extractions,
        &analysis,
        &edges,
        &freshness,
        repo_root.as_deref(),
        compact_dest.is_some(),
    )?;

    ensure_parent(&dest)?;
    write_manifest_atomically(&dest, &json_str, request.force)?;
    ensure_parent(&graph_dest)?;
    write_code_graph_atomically(&graph_dest, &graph_json, request.force)?;
    // Written through the same clobber guard as the verbose artifact. A foreign
    // file at this path is refused for the same reason: the guard's question is
    // "did this kernel write what is already here", and the answer does not
    // depend on the encoding.
    if let (Some(destination), Some(json)) = (&compact_dest, &compact_graph_json) {
        ensure_parent(destination)?;
        write_code_graph_atomically(destination, json, request.force)?;
    }

    // The stamp last, and only after every write succeeded: a stamp claiming
    // artifacts that were never written is a skip that skips nothing real.
    // A stamp that cannot be written is not fatal — it costs the next run a
    // regeneration, which is the behaviour that existed before the stamp.
    let note = |message: String| {
        if let Some(progress) = request.progress {
            progress.diagnostic(message);
        } else {
            eprintln!("{message}");
        }
    };
    match ArtifactStamp::of(inputs, stamp_generated_head, &outputs) {
        Ok(stamp) => {
            if let Err(error) = stamp.write(&stamp_path) {
                note(format!(
                    "  note: could not record the artifact stamp at {} ({error}); \
                     the next manifest will regenerate rather than skip",
                    stamp_path.display()
                ));
            }
        }
        Err(error) => note(format!(
            "  note: could not stat the artifacts just written ({error}); \
             the next manifest will regenerate rather than skip"
        )),
    }

    Ok(ManifestOutcome {
        output: dest,
        graph_output: graph_dest,
        compact_graph_output: compact_dest,
        generation_id: gen_id,
        artifacts_unchanged: false,
        guides,
        freshness_source,
        freshness_unavailable_reason,
    })
}

/// The store fields `devmap status` reports, as one object.
///
/// One owner, because two callers ask for them now: `status` itself, and the
/// build that writes the artifacts, which embeds them so the seam does not have
/// to spawn a third process to learn what the store it just wrote looks like.
/// The schema keys are *not* here — they come from a probe `status` runs before
/// it opens the store at all, and a build has already opened it.
///
/// What this kernel can be asked to do, read out of its own parser.
///
/// The seam used to learn this by running `devmap manifest --help` and
/// `devmap build --help` and grepping the output — two extra process launches
/// (~140 ms each, measured) per `dev map`, on top of the `status` probe it
/// already runs to rank candidate binaries. `status` is the probe that has to
/// happen anyway, so it is the one that should answer.
///
/// Derived from clap's command tree rather than asserted, because a hand-written
/// `true` is a claim that drifts the moment a flag is renamed: this cannot
/// declare a flag the binary does not actually accept. A kernel too old to carry
/// this key declares nothing, and the seam falls back to the `--help` probe —
/// "no evidence" must not read as "does not support it".
fn kernel_capabilities() -> serde_json::Value {
    use clap::CommandFactory;
    let command = Cli::command();
    let accepts = |subcommand: &str, flag: &str| -> bool {
        command
            .get_subcommands()
            .find(|candidate| candidate.get_name() == subcommand)
            .is_some_and(|candidate| {
                candidate
                    .get_arguments()
                    .any(|argument| argument.get_long() == Some(flag))
            })
    };
    let has_command = |subcommand: &str| -> bool {
        command
            .get_subcommands()
            .any(|candidate| candidate.get_name() == subcommand)
    };
    // All three or none: a kernel accepting only some of the digests would need
    // the read-modify-write path for the rest, and running both is strictly
    // worse than running one.
    let stamp_flags = ["generated-head", "indexed-hash", "content-fingerprint"]
        .iter()
        .all(|flag| accepts("manifest", flag));
    serde_json::json!({
        "status": has_command("status"),
        "search": has_command("search"),
        "explore": has_command("explore"),
        "impact": has_command("impact"),
        "trace": has_command("trace"),
        "affected": has_command("affected"),
        "html": has_command("html"),
        "manifest_graph_output": accepts("manifest", "graph-output"),
        "manifest_stamp_flags": stamp_flags,
        "build_manifest": accepts("build", "manifest"),
    })
}

/// Stable compatibility fields for process hosts such as Manvi and GitPulse.
///
/// `status` intentionally exits successfully for incompatible stores so a host
/// can inspect this contract before deciding whether to invoke a query. These
/// fields therefore carry readiness explicitly rather than making exit status
/// stand in for schema negotiation.
fn host_contract_fields(
    stored_schema: Option<i32>,
    generation_id: Option<i64>,
) -> serde_json::Map<String, serde_json::Value> {
    let relation = match stored_schema {
        None => "missing",
        Some(version) if version == devmap_store::CURRENT_SCHEMA_VERSION => "current",
        Some(version) if version == devmap_store::PYTHON_INDEX_SCHEMA_VERSION => "foreign",
        Some(version) if version > devmap_store::CURRENT_SCHEMA_VERSION => "newer",
        Some(version) if Store::schema_is_migratable(version) => "upgradeable",
        Some(_) => "unsupported",
    };
    let reader_ready = relation == "current";
    serde_json::Map::from_iter([
        ("host_contract_version".into(), serde_json::json!(1)),
        (
            "binary_version".into(),
            serde_json::json!(env!("CARGO_PKG_VERSION")),
        ),
        ("schema_relation".into(), serde_json::json!(relation)),
        ("reader_ready".into(), serde_json::json!(reader_ready)),
        (
            "query_ready".into(),
            serde_json::json!(reader_ready && generation_id.is_some()),
        ),
    ])
}

fn extend_host_contract(
    value: &mut serde_json::Value,
    stored_schema: Option<i32>,
    generation_id: Option<i64>,
) {
    if let Some(fields) = value.as_object_mut() {
        fields.extend(host_contract_fields(stored_schema, generation_id));
    }
}

/// The structured answer `devmap doctor` emits.
///
/// One owner for the fields a host needs to verify a binary against a store
/// without scraping `--version` prose: the schema on disk (if any), the schema
/// this binary speaks, the code-graph artifact schema, how many grammars are
/// linked into this build, and the resolved store path. Absence of a store is
/// reported as `schema_version: null`, never invented.
fn doctor_report(db: &std::path::Path) -> anyhow::Result<serde_json::Value> {
    let schema_version = Store::stored_schema_version(db)?;
    Ok(serde_json::json!({
        "schema_version": schema_version,
        "expected_schema_version": devmap_store::CURRENT_SCHEMA_VERSION,
        "code_graph_schema_version": CODE_GRAPH_SCHEMA_VERSION,
        "linked_grammar_count": devmap_extract::linked_grammar_count(),
        "store_path": db.display().to_string(),
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

fn store_status_fields(
    store: &Store,
    db: &std::path::Path,
) -> anyhow::Result<serde_json::Map<String, serde_json::Value>> {
    let status = store.status(&db.display().to_string())?;
    // K-A2: the graph's own degradation belongs in the answer a health check
    // reads.
    //
    // `freshness_degraded_reason` describes the *index* — no generation
    // persisted, paths stuck in the retry queue — and said nothing about a
    // generation built from a corpus the extractor could not read in full. That
    // is how a repository whose only caller of a symbol was refused for being
    // oversized reported `degraded_reason: null` while both artifacts of the
    // same build carried `graph_degraded: true`. Both degradations can hold at
    // once and neither may shadow the other, so they are joined with
    // `devmap_analyze::combine_reasons`, the same joiner the analysis uses for
    // its own pair.
    let analysis_degraded = match store.latest_analysis_status()? {
        Some(devmap_analyze::model::AnalysisStatus::Ok) | None => None,
        Some(devmap_analyze::model::AnalysisStatus::Partial { reason }) => {
            Some(format!("partial: {reason}"))
        }
        Some(devmap_analyze::model::AnalysisStatus::Timeout { reason }) => {
            Some(format!("timeout: {reason}"))
        }
    };
    let degraded_reason = devmap_analyze::combine_reasons(
        devmap_serve::freshness_degraded_reason(&status),
        analysis_degraded,
    );
    let serde_json::Value::Object(fields) = serde_json::json!({
        "generation_id": status.latest_generation,
        "pending_count": status.pending_count,
        "node_count": status.node_count,
        "edge_count": status.edge_count,
        // K-A6: one owner for this rule, shared with the daemon's `status`.
        // Computing it here as `pending_count == 0` is what let a store with no
        // generation at all report as current.
        "is_fresh": devmap_serve::index_is_fresh(&status),
        "db_path": status.db_path,
        "degraded_reason": degraded_reason,
        "quarantined_count": status.quarantined_count,
        // K1(g): naming the stuck paths is what makes a degraded status
        // actionable — "64 path(s) exceeded the retry threshold" told an
        // operator nothing about which 64.
        "quarantined_paths": status.quarantined_paths,
        // The same argument, one surface over. `degraded_reason` has always
        // carried "2 file(s) failed to parse, 1 recovered by pattern, 1 refused
        // by discovery" and never a single path, so an operator could not tell
        // a correct refusal — this repository's is a 30.6 MB vendored
        // `parser.c` against a 1 MiB ceiling — from a broken one without
        // opening the database. Rendered by `devmap_serve::coverage_gaps_json`,
        // shared with the daemon's own `status`.
        "coverage_gaps": devmap_serve::coverage_gaps_json(&status),
        // Whether this generation's edges carry the evidence the resolver
        // recorded, or a reconstruction standing in for one it never stored.
        // `null` when there is no generation or it holds no edges — which is
        // "nothing to say", not "reconstructed".
        "edge_resolution_source": store
            .latest_edge_resolution_source()?
            .map(|source| source.label()),
        // The read-side half of the honesty invariant: stored edges whose
        // confidence contradicts the resolution kind recorded for them. On the
        // way in `ResolvedEdge::resolved` makes the two agree; this reads them
        // back separately and counts the rows that no longer do. Counted in
        // SQL because this is a fresh process per call and must not build the
        // edge index for one number. `null` with no generation; 0 is a
        // measurement, never a default.
        "edge_confidence_mismatches": store.edge_confidence_mismatches()?,
    }) else {
        unreachable!("json! of an object literal is an object")
    };
    Ok(fields)
}

/// The `manifest` result, as JSON or as the two human lines it always printed.
fn report_manifest(cli: &Cli, outcome: &ManifestOutcome) -> anyhow::Result<()> {
    if cli.json {
        return emit_json(cli, &manifest_json(outcome));
    }
    if outcome.artifacts_unchanged {
        println!(
            "Artifacts already current for generation #{} ({:?}, {:?}).",
            outcome.generation_id, outcome.output, outcome.graph_output
        );
    } else {
        println!("Manifest written to {:?}", outcome.output);
        println!("Code graph written to {:?}", outcome.graph_output);
        if let Some(destination) = &outcome.compact_graph_output {
            println!("Interned code graph written to {:?}", destination);
        }
    }
    for guide in &outcome.guides {
        let note = match guide.disposition {
            devmap_query::guides::GuideDisposition::Created => "created",
            devmap_query::guides::GuideDisposition::Updated => "updated",
            devmap_query::guides::GuideDisposition::Unchanged => "already current",
            devmap_query::guides::GuideDisposition::NotOurs => {
                "left alone (no `Managed by devmap` marker)"
            }
        };
        println!("  guide {}: {note}", guide.path.display());
    }
    if !outcome.freshness_unavailable_reason.is_empty() {
        println!(
            "  freshness stamps unavailable: {}",
            outcome.freshness_unavailable_reason
        );
    }
    Ok(())
}

fn manifest_json(outcome: &ManifestOutcome) -> serde_json::Value {
    serde_json::json!({
        "output": outcome.output,
        "graph_output": outcome.graph_output,
        // Absent, not empty, when no interned artifact was asked for: `""`
        // would read as a path that failed.
        "compact_graph_output": outcome.compact_graph_output,
        "generation_id": outcome.generation_id,
        // The artifacts on disk were already the ones this run would write, so
        // the generation was never read and nothing was serialized. Reported
        // rather than left silent: a caller timing this command needs to know
        // which of the two paths it measured.
        "artifacts_unchanged": outcome.artifacts_unchanged,
        "freshness_source": outcome.freshness_source,
        "freshness_unavailable_reason": outcome.freshness_unavailable_reason,
        // Every file considered, with what happened to it. A guide left alone
        // because it is hand-written is reported as such rather than omitted:
        // "not refreshed" and "not ours to refresh" are different facts, and a
        // caller that cannot tell them apart cannot tell a working install from
        // a guide that silently stopped tracking the map.
        "guides": outcome.guides.iter().map(|guide| serde_json::json!({
            "path": guide.path,
            "disposition": match guide.disposition {
                devmap_query::guides::GuideDisposition::Created => "created",
                devmap_query::guides::GuideDisposition::Updated => "updated",
                devmap_query::guides::GuideDisposition::Unchanged => "unchanged",
                devmap_query::guides::GuideDisposition::NotOurs => "not_ours",
            },
            "changed": guide.changed(),
        })).collect::<Vec<_>>(),
    })
}

/// Where `repo_map.json` goes for this invocation.
///
/// An explicit `--output` is used as given. Otherwise the artifact lands in
/// whichever state directory `root` resolves to, so the map is written beside
/// the store that produced it rather than into a directory chosen by the
/// caller's shell.
fn resolve_map_output(explicit: &Option<PathBuf>, root: &Path) -> PathBuf {
    explicit
        .clone()
        .unwrap_or_else(|| devmap_extract::paths::repo_map_path(root))
}

/// Where `code_graph.json` goes for this invocation. See [`resolve_map_output`].
fn resolve_graph_output(explicit: &Option<PathBuf>, root: &Path) -> PathBuf {
    explicit
        .clone()
        .unwrap_or_else(|| devmap_extract::paths::code_graph_path(root))
}

/// One line per symbol, then the counts — never a page length alone.
fn report_ast(answer: &serde_json::Value) {
    for hit in answer["matches"].as_array().into_iter().flatten() {
        println!(
            "{:<10} {:<12} {}  {}",
            hit["kind"].as_str().unwrap_or(""),
            hit["language"].as_str().unwrap_or(""),
            hit["qualified_name"].as_str().unwrap_or(""),
            hit["path"].as_str().unwrap_or(""),
        );
    }
    let shown = answer["shown"].as_u64().unwrap_or(0);
    let total = answer["total"].as_u64().unwrap_or(0);
    if answer["truncated"].as_bool().unwrap_or(false) {
        println!("{shown} of {total} match(es); raise --limit to see the rest");
    } else {
        println!("{total} match(es)");
    }
    // An empty answer because the filter names something the index does not
    // hold is a different problem from an empty answer because nothing matched.
    for unmatched in answer["unmatched_filters"].as_array().into_iter().flatten() {
        println!(
            "  --{} {:?}: {}",
            unmatched["filter"].as_str().unwrap_or(""),
            unmatched["value"].as_str().unwrap_or(""),
            unmatched["detail"].as_str().unwrap_or(""),
        );
    }
}

/// The client scan's bounds, from the flags.
fn scan_budget(max_files: usize, max_file_bytes: u64) -> devmap_query::api_routes::ScanBudget {
    devmap_query::api_routes::ScanBudget {
        max_files,
        max_file_bytes,
        ..Default::default()
    }
}

/// The tree the scan reads, which is the one the store was built from.
///
/// The path argument names a repository; the store records the root it indexed.
/// Those disagree when `--db` points elsewhere, and the file paths in the graph
/// are relative to the *store's* root — resolving them against the argument
/// would read a different tree, or nothing.
fn repo_root_for(store: &Store, path: &std::path::Path) -> anyhow::Result<PathBuf> {
    Ok(store
        .latest_repo_root()?
        .map(PathBuf::from)
        .unwrap_or_else(|| path.to_path_buf()))
}

/// Keep only routes matching the filter, leaving the scan report intact.
fn retain_matching_routes(mapped: &mut serde_json::Value, filter: &str) {
    let kept: Vec<serde_json::Value> = mapped["routes"]
        .as_array()
        .map(|routes| {
            routes
                .iter()
                .filter(|route| {
                    let path = route["path"].as_str().unwrap_or("");
                    let id = route["id"].as_str().unwrap_or("");
                    path.contains(filter)
                        || id.contains(filter)
                        || devmap_query::api_routes::paths_match(path, filter)
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    // `count` stays the number of routes the graph holds; `shown` is what the
    // filter kept. Overwriting `count` would make a filtered view read as the
    // whole surface.
    mapped["shown"] = serde_json::json!(kept.len());
    mapped["routes"] = serde_json::Value::Array(kept);
}

/// One line per route, then the scan's own limits.
fn report_routes(mapped: &serde_json::Value) {
    let routes = mapped["routes"].as_array().cloned().unwrap_or_default();
    if routes.is_empty() {
        println!("No routes in this generation.");
    }
    for route in &routes {
        let handlers: Vec<String> = route["handlers"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|h| match h["resolution"].as_str() {
                Some("ambiguous") => format!(
                    "{} (ambiguous: {} candidates)",
                    h["id"].as_str().unwrap_or("?"),
                    h["candidates"].as_array().map(Vec::len).unwrap_or(0)
                ),
                Some("unresolved") => format!("{} (unresolved)", h["id"].as_str().unwrap_or("?")),
                _ => h["id"].as_str().unwrap_or("?").to_string(),
            })
            .collect();
        println!(
            "{:<7} {}  -> {}",
            route["verb"].as_str().unwrap_or("ANY"),
            route["path"].as_str().unwrap_or(""),
            if handlers.is_empty() {
                "(no handler)".to_string()
            } else {
                handlers.join(", ")
            }
        );
        let consumers = route["consumers"].as_array().map(Vec::len).unwrap_or(0);
        if consumers > 0 {
            println!("        {consumers} client call site(s)");
        }
    }
    report_scan(mapped);
}

fn report_shape_check(checked: &serde_json::Value) {
    let checks = checked["checks"].as_array().cloned().unwrap_or_default();
    for check in &checks {
        let verdict = check["verdict"].as_str().unwrap_or("");
        println!(
            "{:<7} {}  {}",
            check["verb"].as_str().unwrap_or("ANY"),
            check["route"].as_str().unwrap_or(""),
            verdict
        );
        if verdict == "mismatch" {
            let missing: Vec<&str> = check["missing_in_handler"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|k| k.as_str())
                .collect();
            println!(
                "        consumers read, handler never returns: {}",
                missing.join(", ")
            );
        }
    }
    println!(
        "{} of {} route(s) mismatch",
        checked["mismatch_count"].as_u64().unwrap_or(0),
        checks.len()
    );
    report_scan(checked);
}

fn report_api_impact(impact: &serde_json::Value) {
    if impact["found"] != serde_json::json!(true) {
        println!(
            "No route matched {:?}.",
            impact["route"].as_str().unwrap_or("")
        );
        report_scan(impact);
        return;
    }
    println!(
        "{} {}",
        impact["verb"].as_str().unwrap_or("ANY"),
        impact["route"].as_str().unwrap_or("")
    );
    println!(
        "  risk: {} — {}",
        impact["risk"].as_str().unwrap_or("unknown"),
        impact["risk_reason"].as_str().unwrap_or("")
    );
    for consumer in impact["consumers"].as_array().into_iter().flatten() {
        println!(
            "  called from {}:{}",
            consumer["path"].as_str().unwrap_or(""),
            consumer["line"].as_u64().unwrap_or(0)
        );
    }
    for mismatch in impact["shape_mismatches"].as_array().into_iter().flatten() {
        let missing: Vec<&str> = mismatch["missing_in_handler"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|k| k.as_str())
            .collect();
        println!("  shape: consumers read {}", missing.join(", "));
    }
    report_scan(impact);
}

/// What the scan read, and what it did not. Printed whenever it did not finish,
/// because every count above it is then a lower bound.
fn report_scan(payload: &serde_json::Value) {
    let scan = &payload["scan"];
    if scan["complete"].as_bool().unwrap_or(true) {
        return;
    }
    println!(
        "  scan incomplete: read {} of {} file(s); {} skipped for budget, \
{} over size, {} unreadable. Counts above are lower bounds.",
        scan["files_read"].as_u64().unwrap_or(0),
        scan["files_eligible"].as_u64().unwrap_or(0),
        scan["files_skipped_budget"].as_u64().unwrap_or(0),
        scan["files_over_size"].as_u64().unwrap_or(0),
        scan["files_unreadable"].as_u64().unwrap_or(0),
    );
}

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

fn open_for_read(cli: &Cli) -> anyhow::Result<Store> {
    let db = cli.db();
    match Store::open_existing(&db)? {
        Some(store) => {
            if cli.db.is_none() {
                store.validate_repo_root(&cli.root_hint())?;
            }
            Ok(store)
        }
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
    // What a `--min-rung` floor cost, printed only when it cost something.
    //
    // Without it a narrowed answer is indistinguishable at the terminal from a
    // sparse graph, and the second reading is the one that gets a live symbol
    // deleted. Silent when nothing was filtered, so an unfiltered query reads
    // exactly as it did before the flag existed.
    if let Some(rungs) = &resp.rungs {
        if rungs.filtered_out > 0 {
            println!(
                "note: --min-rung hid {} of {} edges (deterministic {}, high {}, speculative {})",
                rungs.filtered_out,
                rungs.total(),
                rungs.deterministic,
                rungs.high,
                rungs.speculative
            );
        }
    }
    // Distinct from the truncation line, which describes the token budget. This
    // one says the walk that produced `items` stopped before the graph ran out,
    // so `total` is the size of a partial answer.
    if let Some(reason) = &resp.walk_incomplete {
        println!("warning: {reason}");
    }
}

fn emit_blast_radius(radius: &devmap_query::BlastRadius) {
    if !radius.unmatched_targets.is_empty() {
        println!(
            "warning: no indexed traversal start for: {}",
            radius.unmatched_targets.join(", ")
        );
    }
    if let ResolutionAvailability::Unavailable { reason } = &radius.layers.resolution {
        emit_unavailable(reason);
        return;
    }
    println!("blast radius: {} impacted", radius.total_impacted);
    for layer in &radius.layers.items {
        let confidence = layer
            .lowest_confidence
            .map(|value| format!("{value:.2}"))
            .unwrap_or_else(|| "-".to_string());
        println!(
            "  depth {}: {} nodes (lowest confidence {confidence}){}",
            layer.depth,
            layer.node_count,
            if layer.nodes_omitted > 0 {
                format!(", {} not listed", layer.nodes_omitted)
            } else {
                String::new()
            }
        );
        for node in &layer.nodes {
            println!("    {node}");
        }
    }
    emit_truncation(
        radius.layers.shown,
        radius.layers.hidden,
        radius.layers.total,
        radius.layers.truncated,
    );
    if let Some(reason) = &radius.layers.walk_incomplete {
        println!("warning: {reason}");
    }
}

fn emit_explore(report: &devmap_query::ExploreReport) {
    if let ResolutionAvailability::Unavailable { reason } = &report.definitions.resolution {
        emit_unavailable(reason);
        return;
    }
    for definition in &report.definitions.items {
        println!(
            "{}:{}-{}  {}  {}",
            definition.file_path,
            definition.span.0,
            definition.span.1,
            definition.kind,
            definition.id
        );
        // Class A: an unreadable file is reported as unread, never as a symbol
        // whose body happens to be empty.
        match &definition.source_unavailable_reason {
            Some(reason) => println!("  source unavailable: {reason}"),
            None => {
                for line in definition.source.lines() {
                    println!("  {line}");
                }
                if let Some(omitted) = definition.source_omitted_bytes {
                    println!("  ... {omitted} bytes omitted to fit the budget");
                }
            }
        }
        println!(
            "  callers: {} of {}{}   callees: {} of {}{}",
            definition.callers.shown,
            definition.callers.total,
            if definition.callers.truncated {
                " (truncated)"
            } else {
                ""
            },
            definition.callees.shown,
            definition.callees.total,
            if definition.callees.truncated {
                " (truncated)"
            } else {
                ""
            },
        );
    }
    emit_truncation(
        report.definitions.shown,
        report.definitions.hidden,
        report.definitions.total,
        report.definitions.truncated,
    );
    println!(
        "budget: {} total = {} definitions + {} per edge direction + {} blast radius",
        report.budget.total,
        report.budget.definitions,
        report.budget.edges_per_direction,
        report.budget.blast_radius
    );
    emit_blast_radius(&report.blast_radius);
}

fn emit_affected(report: &devmap_query::AffectedTestsReport) {
    if let ResolutionAvailability::Unavailable { reason } = &report.tests.resolution {
        emit_unavailable(reason);
        return;
    }
    for test in &report.tests.items {
        println!(
            "{}  depth {}  {} reached symbol(s)",
            test.path, test.depth, test.reached_symbols
        );
    }
    emit_truncation(
        report.tests.shown,
        report.tests.hidden,
        report.tests.total,
        report.tests.truncated,
    );
    if let Some(reason) = &report.tests.walk_incomplete {
        println!("warning: {reason}");
    }
    emit_blast_radius(&report.blast_radius);
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
    emit_dead_clusters(resp);
}

/// The abandoned cycles, printed beside the single-symbol list rather than
/// inside it.
///
/// One line per cluster with a member sample: a forty-symbol dead subsystem is
/// *one* thing a reader acts on, and forty rows would push real single-symbol
/// findings past the budget. That is the same argument `manifest.rs` already
/// makes for the artifact.
///
/// `None` and an empty list are printed differently on purpose. "No abandoned
/// cycles" is a finding; "this generation predates the pass" is not, and a
/// reader deciding whether to rebuild needs to know which they are looking at.
fn emit_dead_clusters(resp: &devmap_query::Response<devmap_analyze::DeadSymbolReport>) {
    for line in dead_cluster_lines(resp) {
        println!("{line}");
    }
}

/// The lines [`emit_dead_clusters`] prints, built rather than written straight
/// to stdout.
///
/// Split out so the branch order is assertable. The three outcomes read almost
/// alike to a human — "not computed", "not recorded", "none" — and the two that
/// differ by a single word give opposite advice, so which branch wins is the
/// property worth a test rather than a comment.
fn dead_cluster_lines(
    resp: &devmap_query::Response<devmap_analyze::DeadSymbolReport>,
) -> Vec<String> {
    // Order matters: a refusal is also an absence, and the rebuild advice below
    // is wrong for it — the next build walks the same oversized graph and
    // refuses again.
    if let Some(reason) = resp.dead_clusters_incomplete.as_ref() {
        return vec![format!("\nabandoned cycles: not computed — {reason}")];
    }
    let Some(clusters) = resp.dead_clusters.as_ref() else {
        return vec![
            "\nabandoned cycles: not recorded for this generation (rebuild with \
             `devmap build` to compute them)"
                .to_string(),
        ];
    };
    if clusters.is_empty() {
        return vec!["\nabandoned cycles: none".to_string()];
    }
    let mut lines = vec![format!(
        "\nabandoned cycles: {} component(s) nothing outside reaches",
        clusters.len()
    )];
    for cluster in clusters {
        let sample: Vec<&str> = cluster
            .members
            .iter()
            .take(DEAD_CLUSTER_SAMPLE)
            .map(String::as_str)
            .collect();
        let more = cluster.size.saturating_sub(sample.len());
        let tail = if more > 0 {
            format!(", +{more} more")
        } else {
            String::new()
        };
        lines.push(format!(
            "  {:.2}  {} symbols: {}{}",
            cluster.confidence,
            cluster.size,
            sample.join(", "),
            tail
        ));
    }
    if resp.dead_clusters_truncated > 0 {
        lines.push(format!(
            "  … {} further component(s) were found and not listed",
            resp.dead_clusters_truncated
        ));
    }
    lines
}

/// How many members of a cluster the human readout names before saying how many
/// more there are.
///
/// The producer already caps the stored list at `DEAD_CLUSTER_MEMBER_CAP`; this
/// is the *display* cap, and it is smaller because a terminal line is not a
/// place to read twenty-five qualified names. `cluster.size` is the real count
/// and is printed beside the sample, so a capped sample is never presented as a
/// whole membership.
const DEAD_CLUSTER_SAMPLE: usize = 4;

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

// The token-budget and traversal-depth ceilings are `devmap_query`'s — the
// engine applies them — and both transports import them, so a bound that
// holds over the socket also holds over argv without a second spelling.
use devmap_query::{MAX_TOKEN_BUDGET, MAX_TRAVERSAL_DEPTH};

/// One rendering of an error chain for the human line and the JSON line.
///
/// `{:#}` prints every link joined by `: `. The store carries its refusals in
/// a rusqlite variant that displays its boxed error *and* returns it as
/// `source()`, so the chain says the same sentence twice and every store
/// refusal read as two (measured: the index-gate refusal against this
/// repository's store, 2026-09-07). A link whose text equals the one before it
/// adds nothing and is dropped; every other link is kept in order.
fn render_error(error: &anyhow::Error) -> String {
    let mut rendered = String::new();
    let mut previous: Option<String> = None;
    for cause in error.chain() {
        let text = cause.to_string();
        if previous.as_deref() == Some(text.as_str()) {
            continue;
        }
        if !rendered.is_empty() {
            rendered.push_str(": ");
        }
        rendered.push_str(&text);
        previous = Some(text);
    }
    rendered
}

/// The path a root-taking subcommand names must be a directory that exists.
///
/// Measured through the release binary: `manifest <missing path>` created
/// `<missing path>/.devmap/` and wrote the artifacts of the store's *other*
/// repository into it; `routes`, `shape-check` and `api-impact` answered from
/// the store's recorded root and never said the path they were given does not
/// exist; `build <file>` walked the file as an empty tree. A path the caller
/// named and this binary could not examine is not a repository root, and
/// answering — or writing — as if it were is the check that could not run
/// reporting as one that ran. Only [`Cli::root_hint`]'s subcommands carry a
/// root, and the default `.` always exists, so only a path the caller actually
/// spelled can fail here.
fn validate_root(cli: &Cli) -> Result<(), String> {
    let root = cli.root_hint();
    match std::fs::metadata(&root) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(format!(
            "{}: not a directory; the path a subcommand names must be a repository root",
            root.display()
        )),
        Err(error) => Err(format!("{}: {error}", root.display())),
    }
}

/// Reject a numeric argument the engine cannot honour, before it reaches the
/// engine.
///
/// S-1 follow-up. `validate_request` bounds every one of these over the IPC
/// transport — finite confidence inside `[0, 1]`, a budget and a depth under
/// the ceilings — while argv reached `StoreQueryEngine` unchecked. The
/// asymmetry is not cosmetic: `--min-confidence nan` makes every `>=` comparison
/// in the edge filter false, so the answer is an empty edge list that reads
/// exactly like "this symbol has no callers"; `--budget 0` returns an empty
/// result with `truncated` unset for the same reason; and a depth past the
/// engine's clamp is silently rewritten.
///
/// Refused, never clamped. Clamping is what makes a capped answer
/// indistinguishable from a complete one, which is the failure this codebase
/// treats as worse than an error.
fn validate_limits(command: &Commands) -> Result<(), String> {
    let check_budget = |budget: u32| -> Result<(), String> {
        if budget == 0 {
            return Err(
                "--budget must be at least 1: a zero token budget returns an empty result \
                 that cannot be told apart from a complete one"
                    .to_string(),
            );
        }
        if budget > MAX_TOKEN_BUDGET {
            return Err(format!(
                "--budget must be at most {MAX_TOKEN_BUDGET}, got {budget}"
            ));
        }
        Ok(())
    };
    let check_depth = |depth: usize| -> Result<(), String> {
        if depth == 0 {
            return Err("--depth must be at least 1: depth 0 walks nothing".to_string());
        }
        if depth > MAX_TRAVERSAL_DEPTH {
            return Err(format!(
                "--depth must be at most {MAX_TRAVERSAL_DEPTH}, got {depth} — the engine \
                 clamps past that and would answer a question you did not ask"
            ));
        }
        Ok(())
    };
    let check_confidence = |value: f32| -> Result<(), String> {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(format!(
                "--min-confidence must be finite and within [0, 1], got {value}"
            ));
        }
        Ok(())
    };
    // Refused, never defaulted. A typo silently answered at full breadth is a
    // filtered answer the caller believes is narrow — worse than an error,
    // because they will act on the short list.
    let check_rung = |value: &Option<String>| -> Result<(), String> {
        match value.as_deref() {
            None => Ok(()),
            Some(name) if devmap_query::Rung::parse(name).is_some() => Ok(()),
            Some(name) => Err(format!(
                "--min-rung must be one of deterministic, high, speculative; got {name:?}"
            )),
        }
    };

    match command {
        Commands::Search { budget, .. }
        | Commands::Snapshots { budget, .. }
        | Commands::Savings { budget, .. } => check_budget(*budget),
        Commands::Dead { budget } => check_budget(*budget),
        Commands::Deps {
            budget,
            min_confidence,
            min_rung,
            ..
        } => {
            check_budget(*budget)?;
            check_rung(min_rung)?;
            check_confidence(*min_confidence)
        }
        Commands::Impact {
            budget,
            depth,
            min_rung,
            layers,
            ..
        } => {
            check_rung(min_rung)?;
            check_budget(*budget)?;
            // Refused rather than half-applied. The band walk filters on
            // confidence and knows nothing of rungs, so a request for both
            // would return edges cut to the floor beside bands that were not —
            // one answer whose two halves disagree about the question.
            if *layers && min_rung.is_some() {
                return Err(
                    "--layers cannot be combined with --min-rung: the distance bands are \
                     walked without a rung floor, so the two halves of the answer would \
                     describe different graphs"
                        .to_string(),
                );
            }
            check_depth(*depth)
        }
        Commands::Trace {
            budget,
            depth,
            min_rung,
            ..
        } => {
            check_rung(min_rung)?;
            check_budget(*budget)?;
            check_depth(*depth)
        }
        Commands::Neighbors {
            targets,
            budget,
            depth,
            min_confidence,
            min_rung,
        } => {
            check_rung(min_rung)?;
            // Refused, not trimmed, exactly as `validate_request` does over the
            // socket: answering the first sixteen of twenty hands back a short
            // list that reads like a complete one.
            if targets.len() > devmap_query::MAX_NEIGHBOR_TARGETS {
                return Err(format!(
                    "neighbors accepts at most {} targets, got {}",
                    devmap_query::MAX_NEIGHBOR_TARGETS,
                    targets.len()
                ));
            }
            check_budget(*budget)?;
            check_depth(*depth)?;
            check_confidence(*min_confidence)
        }
        Commands::Explore {
            limit,
            budget,
            depth,
            min_confidence,
            ..
        } => {
            if *limit == 0 {
                return Err("--limit must be at least 1".to_string());
            }
            check_budget(*budget)?;
            check_depth(*depth)?;
            check_confidence(*min_confidence)
        }
        Commands::Affected {
            budget,
            depth,
            min_confidence,
            ..
        } => {
            check_budget(*budget)?;
            check_depth(*depth)?;
            check_confidence(*min_confidence)
        }
        Commands::Preview {
            budget,
            min_confidence,
            ..
        } => {
            check_budget(*budget)?;
            check_confidence(*min_confidence)
        }
        Commands::Clones { budget, .. } => check_budget(*budget),
        Commands::History { last } => {
            if *last == 0 {
                return Err("--last must be at least 1".to_string());
            }
            Ok(())
        }
        // No numeric query arguments reach the engine from these.
        Commands::Build { .. }
        | Commands::Status
        | Commands::Doctor
        | Commands::Paths { .. }
        | Commands::Manifest { .. }
        | Commands::MapHtml { .. }
        | Commands::Freshness { .. }
        | Commands::Repair { .. }
        | Commands::Workspace { .. }
        | Commands::Serve { .. }
        | Commands::Mcp { .. }
        | Commands::Html { .. }
        | Commands::Cypher { .. }
        | Commands::Pdg { .. }
        | Commands::Ast { .. }
        | Commands::Export { .. }
        | Commands::Routes { .. }
        | Commands::ShapeCheck { .. }
        | Commands::ApiImpact { .. }
        | Commands::Claude { .. } => Ok(()),
    }
}

impl Commands {
    /// Whether this command stays up to answer other processes.
    ///
    /// The daemon and the MCP servers write to sockets and pipes whose peers
    /// come and go; for them a closed peer is an `EPIPE` to handle, not a
    /// reason to exit, and the ignored-`SIGPIPE` disposition Rust's runtime
    /// installs is the right one. Everything else is a one-shot command.
    fn serves(&self) -> bool {
        matches!(self, Commands::Serve { .. } | Commands::Mcp { .. })
    }
}

/// Let a one-shot command end the way every other CLI does when its reader
/// goes away.
///
/// Rust's runtime ignores `SIGPIPE` at startup so that a write to a closed
/// pipe surfaces as `EPIPE` — and `println!` answers `EPIPE` with a panic.
/// `devmap export -o - | head` therefore printed `failed printing to stdout:
/// Broken pipe` and a backtrace hint, where `git`, `sqlite3` and `rg` end
/// silently. Restoring the default disposition for one-shot commands makes
/// the kernel behave like them: the process is terminated by the signal the
/// moment the reader is gone, with nothing written after it and nothing left
/// half-done that a later run cannot recover (a build killed at any point
/// leaves the store consistent — `test_process_recovery` and the crash gate
/// are the evidence). Only one-shot commands: see [`Commands::serves`].
///
#[cfg(unix)]
fn restore_default_sigpipe() {
    // SAFETY: `signal(2)` with `SIG_DFL` installs the default action for a
    // signal this process is not otherwise handling; it is called once, on
    // the main thread, before any other thread exists.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

#[cfg(not(unix))]
fn restore_default_sigpipe() {}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let started = Instant::now();
    let matches = Cli::command().get_matches();
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|error| error.exit());
    // stderr, not the builder's default stdout. Every command that emits a
    // payload emits it on stdout — `emit_json` prints there, and `devmap mcp`
    // speaks JSON-RPC there — so a log line on stdout is not noise beside the
    // answer, it is a line *inside* the answer. `devmap search --json | jq`
    // fails on it, and an MCP client's next parse fails on it.
    let subscriber = FmtSubscriber::builder()
        .with_max_level(if cli.verbose {
            Level::DEBUG
        } else {
            Level::INFO
        })
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
        .with_writer(std::io::stderr)
        .finish();
    if let Err(error) = tracing::subscriber::set_global_default(subscriber) {
        eprintln!("DevMap logging unavailable: {error}");
    }

    if !cli.command.serves() {
        restore_default_sigpipe();
    }
    let progress = matches!(cli.command, Commands::Build { .. })
        .then(|| ProgressReporter::new(cli.progress, cli.json, cli.verbose));
    let outcome = match validate_limits(&cli.command).and_then(|()| validate_root(&cli)) {
        Ok(()) => run(&cli, progress.as_ref()).await,
        Err(message) => Err(anyhow::anyhow!(message)),
    };
    match outcome {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            // Capture the open stage before closing it as failed. Do not log
            // query arguments, preview buffers, or environment variables.
            let root = std::path::absolute(cli.root_hint())
                .unwrap_or_else(|_| cli.root_hint().to_path_buf());
            let db = std::path::absolute(cli.db()).unwrap_or_else(|_| cli.db());
            let context = serde_json::json!({
                "command": matches.subcommand_name(),
                "binary_version": version_line(),
                "binary_path": std::env::current_exe().ok().map(|p| p.to_string_lossy().into_owned()),
                "pid": std::process::id(),
                "unix_ms": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_millis()),
                "root": root.to_string_lossy(),
                "root_path_lossy": root.to_str().is_none(),
                "db_path": db.to_string_lossy(),
                "db_path_lossy": db.to_str().is_none(),
                "elapsed_ms": started.elapsed().as_millis(),
                "stage": progress.as_ref().and_then(|p| p.open.borrow().as_ref().map(|(label, _, _)| label.clone())),
            });
            // The human line always, on stderr where every other diagnostic
            // this binary writes goes. Under `--json`, the same failure *also*
            // goes out as one line of JSON on stdout, because that is what
            // `--json` promises on every exit and the failing paths are the
            // ones a caller most needs to handle: returning the error from
            // `main` left stdout empty, so a caller reading one line and
            // parsing it saw an empty string and could not tell a failure from
            // a command that answered nothing. Two channels, one message —
            // stdout stays exactly one JSON line either way.
            if let Some(progress) = progress.as_ref() {
                progress.close_open_stage(false);
                progress.display.diagnostic(format_args!(
                    "Error: {} — build stopped before completion; DevMap context: {context}",
                    render_error(&error)
                ));
                progress.display.finish("");
                if cli.json {
                    println!(
                        "{}",
                        serde_json::json!({ "error": render_error(&error), "diagnostic_context": context,
                        "timings": progress.timings_json(), "progress_output": progress.display.output_json() })
                    );
                }
            } else {
                eprintln!("Error: {}", render_error(&error));
                eprintln!("DevMap context: {context}");
                if cli.json {
                    println!(
                        "{}",
                        serde_json::json!({ "error": render_error(&error), "diagnostic_context": context })
                    );
                }
            }
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run(cli: &Cli, progress: Option<&ProgressReporter>) -> anyhow::Result<()> {
    match &cli.command {
        Commands::Build {
            path,
            affected: affected_flag,
            deleted,
            full,
            verify_rows,
            manifest: write_manifest,
            output,
            graph_output,
            guides,
            force,
            stamps,
            inventory,
        } => {
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
            progress.display.detail("waiting for writer lock");
            let _writer = Store::lock_writer_at(&cli.db(), Store::WRITER_LOCK_WAIT)?;
            progress.display.detail("opening index");
            let store = Store::open(cli.db())?;
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
            let reconciled = store.reconcile_pending_paths(path)?;
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
            let scan_progress =
                std::sync::Arc::new(devmap_extract::progress::FileProgress::default());
            progress
                .display
                .files("reading", std::sync::Arc::clone(&scan_progress));
            let scanned = devmap_extract::scan_tree_with_progress(path, Some(&scan_progress))?;
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
            let previous = store.latest_file_hashes()?;
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
                        println!(
                            "No source changes; generation #{generation} still current \
                             ({}) in {}.",
                            progress::count(file_count, "file"),
                            progress::duration(progress.started_at.elapsed().as_secs_f64())
                        );
                        progress.display.report_loss();
                        if cli.verbose {
                            if let Some(manifest) = &manifest {
                                report_manifest(cli, &manifest.outcome)?;
                            }
                        }
                        if cli.verbose && !progress.display.enabled() {
                            println!("  Reclaim: {}", reclaim_note(&vacuum));
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
            let extractions = if *full {
                let refs: Vec<devmap_extract::FileRef<'_>> = scanned
                    .sources
                    .iter()
                    .map(|(file, source)| devmap_extract::FileRef {
                        path: file.as_str(),
                        source: source.as_str(),
                    })
                    .collect();
                devmap_extract::extract_all_with_progress(&refs, Some(&extraction_progress))
            } else {
                devmap_store::extract_scanned_cached_with_progress(
                    &store,
                    &scanned,
                    Some(&extraction_progress),
                )?
            };
            let extraction_snapshot = extraction_progress.snapshot();
            // The corpus text is dead the moment extraction has consumed it,
            // but it is bound in this scope and would otherwise stay resident
            // through resolve, analyze and persist — the stages that set the
            // peak. It is the one cost the scan-before-extract split would
            // otherwise have added, and it is not hypothetical: measured A/B on
            // scholarlm (4,278 files), holding it cost 23 MiB of peak RSS.
            //
            // The *report* has to outlive it — `discovery_refusals` below turns
            // it into the analysis disclosure — so this destructures rather
            // than dropping the pair, and only the source text goes.
            let devmap_extract::ScannedTree {
                sources,
                report: discovery,
            } = scanned;
            drop(sources);

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
            progress.stage(
                2,
                format_args!("resolving {}", progress::count(extractions.len(), "file")),
            );
            let resolution = resolver.resolve_all(&extractions);
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
                        "unresolved_unattributed": unattributed_calls,
                        // The arithmetic over the six counters above, done
                        // once and published, rather than left to a reader who
                        // will not do it. `net` excludes the misses that are
                        // explained — a language builtin, a runtime global, a
                        // name an import proves is outside the corpus — and is
                        // the figure worth ratcheting.
                        "resolution_rate": analysis.resolution_rate,
                        // The per-stage breakdown, so a caller profiling a slow
                        // build reads it from the result rather than scraping
                        // the human progress lines off stderr.
                        "timings": progress.timings_json(),
                                "progress_output": progress.display.output_json(),
                        "manifest": manifest.as_ref().map(|manifest| &manifest.json),
                    }),
                )?;
            } else {
                println!(
                    "Built generation #{gen_id} · {} · {} · {} · {}",
                    progress::count(analysis.total_files, "file"),
                    progress::count(analysis.total_symbols, "symbol"),
                    progress::count(analysis.total_edges, "edge"),
                    progress::duration(progress.started_at.elapsed().as_secs_f64())
                );
                println!(
                    "  Changes: +{} ~{} -{} · {} unchanged · {} cached",
                    file_delta.added,
                    file_delta.changed,
                    file_delta.removed,
                    file_delta.unchanged,
                    extraction_snapshot.cache_hits
                );
                progress.display.report_loss();
                if cli.verbose {
                    println!("  Files indexed: {}", analysis.total_files);
                }
                if refused_count > 0 {
                    // Said here as well as on stderr: the count belongs beside
                    // the file total it is part of, or a reader takes the total
                    // for a count of files that were read.
                    println!(
                        "    of which refused by discovery: {refused_count} \
                         (recorded as lost coverage, not parsed)"
                    );
                }
                if !cli.verbose && (unattributed_calls > 0 || uninferred_receiver_calls > 0) {
                    println!("  Unresolved: {unattributed_calls} unattributed, {uninferred_receiver_calls} uninferred receivers (details: --verbose)");
                }
                if cli.verbose {
                    println!("  Symbols extracted: {}", analysis.total_symbols);
                    println!("  Edges resolved: {}", analysis.total_edges);
                    // R5: a call we could not attribute is reported, not dropped.
                    println!("  Unresolved calls: {}", analysis.unresolved_calls);
                    print_resolution_rate(&analysis.resolution_rate);
                    println!("    language builtins:  {builtin_calls}");
                    println!("    host globals:       {host_global_calls}");
                    println!("    local bindings:     {local_binding_calls}");
                    println!("    external imports:   {external_calls}");
                    println!("    uninferred receiver:{uninferred_receiver_calls}");
                    println!("    unattributed:       {unattributed_calls}");
                    if let Some(manifest) = &manifest {
                        report_manifest(cli, &manifest.outcome)?;
                    }
                }
            }
        }
        Commands::Search {
            query,
            budget,
            semantic,
        } => {
            let store = open_for_read(cli)?;
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
                emit_json(cli, &serde_json::to_value(&resp)?)?;
            } else {
                emit_search(&resp);
            }
        }
        Commands::Deps {
            file,
            budget,
            min_confidence,
            min_rung,
        } => {
            let store = open_for_read(cli)?;
            let engine = StoreQueryEngine::new(&store);
            // Both floors apply, at different places: `min_confidence` goes to
            // the store, which drops rows before the engine sees them, and the
            // rung is applied over what came back — so only the rung's cut is
            // countable in the histogram.
            let resp = engine.dependencies_at_rung(
                Request {
                    query: file.clone(),
                    token_budget: *budget,
                    min_confidence: *min_confidence,
                    max_depth: 1,
                },
                min_rung.as_deref().and_then(devmap_query::Rung::parse),
            )?;
            if cli.json {
                emit_json(cli, &serde_json::to_value(&resp)?)?;
            } else {
                emit_edges(&resp);
            }
        }
        Commands::Impact {
            target,
            budget,
            depth,
            min_rung,
            layers,
        } => {
            let store = open_for_read(cli)?;
            let engine = StoreQueryEngine::new(&store);
            let req = Request {
                query: target.clone(),
                token_budget: *budget,
                min_confidence: 0.0,
                max_depth: *depth,
            };
            if *layers {
                // `--min-rung` with `--layers` was refused in validation, so
                // dropping the floor here cannot silently widen an answer a
                // caller asked to narrow.
                let resp = engine.impact_layered(req)?;
                if cli.json {
                    emit_json(cli, &serde_json::to_value(&resp)?)?;
                } else {
                    emit_edges(&resp.edges);
                    emit_blast_radius(&resp.blast_radius);
                }
            } else {
                let resp = engine.impact_at_rung(
                    req,
                    // Already validated above, so `None` here means "none was
                    // asked for", never "one was asked for and did not parse".
                    min_rung.as_deref().and_then(devmap_query::Rung::parse),
                )?;
                if cli.json {
                    emit_json(cli, &serde_json::to_value(&resp)?)?;
                } else {
                    emit_edges(&resp);
                }
            }
        }
        Commands::Neighbors {
            targets,
            budget,
            depth,
            min_confidence,
            min_rung,
        } => {
            let store = open_for_read(cli)?;
            let engine = StoreQueryEngine::new(&store);
            // `check_rung` above has already refused an unparseable name, so a
            // `None` here means no floor was asked for and never that one was
            // asked for and dropped.
            let answers = engine.neighbors_at_rung(
                targets,
                *budget,
                *min_confidence,
                *depth,
                min_rung.as_deref().and_then(devmap_query::Rung::parse),
            )?;
            if cli.json {
                emit_json(cli, &serde_json::json!({ "neighbors": answers }))?;
            } else {
                for entry in &answers {
                    println!("{}", entry.target);
                    println!("  callers:");
                    emit_edges(&entry.callers);
                    println!("  callees:");
                    emit_edges(&entry.callees);
                }
            }
        }
        Commands::Trace {
            from,
            to,
            budget,
            depth,
            min_rung,
        } => {
            let store = open_for_read(cli)?;
            let engine = StoreQueryEngine::new(&store);
            let rung = min_rung.as_deref().and_then(devmap_query::Rung::parse);
            let resp = if let Some(destination) = to {
                // The path variant answers with one path rather than an edge
                // population, so there is nothing for a histogram to describe
                // and the floor goes to the walk as a confidence. Exact all the
                // same: `floor_millis / 1000.0` reconstructs the float the
                // confidence constants were built from, bit for bit.
                engine.trace_between(Request {
                    query: (from.clone(), destination.clone()),
                    token_budget: *budget,
                    min_confidence: rung.map_or(0.0, |r| r.floor_millis() as f32 / 1000.0),
                    max_depth: *depth,
                })?
            } else {
                engine.trace_at_rung(
                    Request {
                        query: from.clone(),
                        token_budget: *budget,
                        min_confidence: 0.0,
                        max_depth: *depth,
                    },
                    rung,
                )?
            };
            if cli.json {
                emit_json(cli, &serde_json::to_value(&resp)?)?;
            } else {
                emit_edges(&resp);
            }
        }
        Commands::Dead { budget } => {
            let store = open_for_read(cli)?;
            let payload = StoreQueryEngine::new(&store).dead_symbols(*budget)?;
            if cli.json {
                emit_json(cli, &serde_json::to_value(&payload)?)?;
            } else {
                emit_dead(&payload);
            }
        }
        Commands::Explore {
            query,
            limit,
            budget,
            depth,
            min_confidence,
        } => {
            let store = open_for_read(cli)?;
            let report = StoreQueryEngine::new(&store).explore(
                query,
                *limit,
                *budget,
                *min_confidence,
                *depth,
            )?;
            if cli.json {
                emit_json(cli, &serde_json::to_value(&report)?)?;
            } else {
                emit_explore(&report);
            }
        }
        Commands::Affected {
            targets,
            budget,
            depth,
            min_confidence,
        } => {
            let store = open_for_read(cli)?;
            let report = StoreQueryEngine::new(&store).affected_tests(
                targets,
                *budget,
                *min_confidence,
                *depth,
            )?;
            if cli.json {
                emit_json(cli, &serde_json::to_value(&report)?)?;
            } else {
                emit_affected(&report);
            }
        }
        Commands::Workspace { action } => {
            // Rooted at the store's repository when `--db` names a store in its
            // standard place, so `devmap --db X workspace` and `dev map workspace`
            // agree on where the registry lives. A store anywhere else — a
            // `$DEVMAP_HOME` layout, a scratch path — names no repository, and
            // the registry belongs to the repository this command ran in. The
            // inverse used to answer the grandparent of *any* path, and an
            // off-layout `--db` put the registry two directories above the
            // store, in a directory that was nobody's repository.
            let root = match &cli.db {
                Some(explicit) => devmap_extract::paths::repo_root_from_store(explicit)
                    .unwrap_or_else(|| cli.root_hint().to_path_buf()),
                None => cli.root_hint().to_path_buf(),
            };
            // Absolute in the answer: the registry records absolute roots, and
            // a `registry` of `./.devmap/workspace.json` tells a caller in
            // another directory nothing.
            let root = root.canonicalize().unwrap_or(root);
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
                    let (added, written) =
                        devmap_query::workspace::Workspace::update(&root, |workspace| {
                            workspace.add(label.clone(), canonical.clone())
                        })?;
                    let replaced = added?;
                    if cli.json {
                        emit_json(
                            cli,
                            &serde_json::json!({
                                "added": label,
                                "root": canonical,
                                "registry": written,
                                "replaced": replaced,
                            }),
                        )?;
                    } else {
                        println!(
                            "{} {label} -> {} ({})",
                            if replaced { "replaced" } else { "added" },
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
                        emit_json(cli, &serde_json::json!({"removed": removed, "name": name}))?;
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
                            cli,
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
                        emit_json(cli, &serde_json::to_value(&result)?)?;
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
                            cli,
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
            let store = open_for_read(cli)?;
            let report = StoreQueryEngine::new(&store).savings(query.as_deref(), *budget)?;
            if cli.json {
                emit_json(cli, &serde_json::to_value(&report)?)?;
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
                use std::io::Read;
                let mut buffer = Vec::new();
                std::io::stdin()
                    .lock()
                    .take(devmap_extract::MAX_SOURCE_BYTES + 1)
                    .read_to_end(&mut buffer)?;
                if buffer.len() as u64 > devmap_extract::MAX_SOURCE_BYTES {
                    anyhow::bail!("preview source exceeds the 1 MiB source ceiling");
                }
                String::from_utf8(buffer)?
            } else {
                devmap_extract::read_source(std::path::Path::new(content))
                    .map_err(|e| anyhow::anyhow!("cannot read {content}: {e}"))?
            };
            let store = open_for_read(cli)?;
            let report =
                StoreQueryEngine::new(&store).preview(file, &source, *budget, *min_confidence)?;
            if cli.json {
                emit_json(cli, &serde_json::to_value(&report)?)?;
            } else {
                emit_preview(&report);
            }
        }
        Commands::Clones {
            budget,
            kind,
            min_nodes,
        } => {
            let store = open_for_read(cli)?;
            // `value_parser` has already rejected anything but the two names,
            // so a `None` here can only be "no filter requested".
            let wanted = kind.as_deref().and_then(devmap_query::parse_clone_kind);
            let report = StoreQueryEngine::new(&store).clones(*budget, wanted, *min_nodes)?;
            if cli.json {
                emit_json(cli, &serde_json::to_value(&report)?)?;
            } else {
                emit_clones(&report);
            }
        }
        Commands::Manifest {
            path,
            output,
            graph_output,
            compact_graph_output,
            guides,
            force,
            stamps,
            inventory,
        } => {
            let store = open_for_read(cli)?;
            let outcome = write_consumer_artifacts(
                &store,
                ManifestRequest {
                    progress: None,
                    path,
                    db: &cli.db(),
                    output: &resolve_map_output(output, path),
                    graph_output: &resolve_graph_output(graph_output, path),
                    compact_graph_output: compact_graph_output.as_deref(),
                    force: *force,
                    stamps,
                    inventory: (*inventory).into(),
                    guides: *guides,
                },
            )?;
            report_manifest(cli, &outcome)?;
        }
        Commands::MapHtml {
            path,
            input,
            output,
            force,
        } => {
            let resolve = |p: &PathBuf| -> PathBuf {
                if p.is_absolute() {
                    p.clone()
                } else {
                    path.join(p)
                }
            };
            let map_path = resolve(input);
            let out_path = resolve(output);

            let text = std::fs::read_to_string(&map_path).map_err(|err| {
                anyhow::anyhow!(
                    "cannot read repo map at {}: {err} (run `devmap manifest` first)",
                    map_path.display()
                )
            })?;
            let repo_map: serde_json::Value = serde_json::from_str(&text).map_err(|err| {
                anyhow::anyhow!("{} is not valid JSON: {err}", map_path.display())
            })?;
            let fingerprint = devmap_query::fingerprint_for(&repo_map);

            // Skip an unchanged rewrite so a watch tick does not churn the file
            // — but only when the map carries stamps to fingerprint. An unstamped
            // map fingerprints to a constant, and skipping on that would pin the
            // page to whatever was rendered first.
            let stamped = !fingerprint.generated_head.is_empty();
            let regenerate =
                *force || !stamped || devmap_query::should_regenerate(&out_path, &fingerprint);
            if !regenerate {
                if cli.json {
                    println!(
                        "{}",
                        serde_json::json!({
                            "output": out_path.display().to_string(),
                            "written": false,
                            "reason": "unchanged",
                            "fingerprint": fingerprint.fingerprint,
                        })
                    );
                } else {
                    println!("{} is current", out_path.display());
                }
                return Ok(());
            }

            let html = devmap_query::render_map_preview_html(&repo_map, &fingerprint);
            devmap_query::write_atomic(&out_path, html.as_bytes())?;
            if cli.json {
                println!(
                    "{}",
                    serde_json::json!({
                        "output": out_path.display().to_string(),
                        "written": true,
                        "bytes": html.len(),
                        "fingerprint": fingerprint.fingerprint,
                    })
                );
            } else {
                println!("Wrote {} ({} bytes)", out_path.display(), html.len());
            }
        }
        Commands::Freshness {
            path,
            expect_head,
            expect_indexed_hash,
            expect_content_fingerprint,
            no_cache_write,
            inventory,
        } => {
            let limits: InventoryLimits = (*inventory).into();
            let listing = freshness::inventory(path, limits);
            let mut digests = match &listing.source {
                InventorySource::Unavailable(reason) => FreshnessDigests {
                    unavailable_reason: format!("git file inventory unavailable: {reason}"),
                    ..Default::default()
                },
                InventorySource::Git => FreshnessDigests {
                    generated_head: Some(freshness::git_head(path)).filter(|head| !head.is_empty()),
                    indexed_hash: Some(freshness::files_fingerprint(&listing.files)),
                    // Left for the short-circuit below to decide: hashing the
                    // whole inventory is the expensive part, and a caller whose
                    // head or file set has already moved has its answer.
                    content_fingerprint: None,
                    unavailable_reason: String::new(),
                },
            };

            // One field at a time, and only the fields the caller asked about.
            let compare = |expected: &Option<String>, actual: &Option<String>| {
                expected.as_deref().map(|expected| {
                    let actual = actual.clone().unwrap_or_default();
                    serde_json::json!({
                        "stored": expected,
                        "actual": actual,
                        "match": expected == actual,
                    })
                })
            };
            let head = compare(expect_head, &digests.generated_head);
            let files = compare(expect_indexed_hash, &digests.indexed_hash);
            let cheap_mismatch = [&head, &files]
                .into_iter()
                .flatten()
                .any(|checked| checked["match"] != serde_json::Value::Bool(true));
            // `RepoMapper.map_is_stale` computes the content fingerprint only
            // after head and inventory both match, and this has to cost what
            // that costs or delegating to it is a regression: hashing 1,385
            // files is ~110 ms against ~25 ms for the two `git ls-files` passes
            // that already answered. Computed anyway when nobody asked a
            // question, because then the digests *are* the answer.
            let content_wanted = !cheap_mismatch
                && (expect_content_fingerprint.is_some()
                    || (expect_head.is_none() && expect_indexed_hash.is_none()));
            if content_wanted && listing.is_available() {
                digests.content_fingerprint = Some(freshness::content_fingerprint(
                    path,
                    &listing.files,
                    !*no_cache_write,
                ));
            }
            let content = if content_wanted {
                compare(expect_content_fingerprint, &digests.content_fingerprint)
            } else {
                // Not "matched": *not checked*. A field whose check was skipped
                // must never report what a field that was checked and passed
                // reports, so it carries no `match` at all.
                expect_content_fingerprint.as_deref().map(|expected| {
                    serde_json::json!({
                        "stored": expected,
                        "checked": false,
                        "reason": "head or inventory already differ",
                    })
                })
            };
            let asked = head.is_some() || files.is_some() || content.is_some();
            let mismatched: Vec<&str> = [
                ("head", &head),
                ("inventory", &files),
                ("content", &content),
            ]
            .into_iter()
            .filter_map(|(name, checked)| {
                checked
                    .as_ref()
                    .filter(|value| value.get("match") == Some(&serde_json::Value::Bool(false)))
                    .map(|_| name)
            })
            .collect();

            // Fail closed. An inventory that could not be enumerated cannot
            // prove a map fresh, so the answer is "stale, and here is why it
            // could not be checked" — never "fresh" by absence of evidence.
            let (stale, reason) = if !digests.unavailable_reason.is_empty() {
                (asked.then_some(true), digests.unavailable_reason.clone())
            } else if !asked {
                (None, String::new())
            } else if mismatched.is_empty() {
                (Some(false), String::new())
            } else {
                (
                    Some(true),
                    format!(
                        "{} changed since the map was written",
                        mismatched.join(", ")
                    ),
                )
            };

            emit_json(
                cli,
                &serde_json::json!({
                    "path": path,
                    "source": match listing.source {
                        InventorySource::Git => "git",
                        InventorySource::Unavailable(_) => "unavailable",
                    },
                    "unavailable_reason": digests.unavailable_reason,
                    "generated_head": digests.generated_head,
                    "indexed_hash": digests.indexed_hash,
                    "content_fingerprint": digests.content_fingerprint,
                    "files": listing.files.len(),
                    // Class A: a capped inventory fingerprints a subset of the
                    // tree, and the number it was cut from travels with it.
                    "inventory_capped_from": listing.capped_from,
                    "stale": stale,
                    "reason": reason,
                    "checked": {
                        "head": head,
                        "inventory": files,
                        "content": content,
                    },
                }),
            )?;
        }
        Commands::Paths { path } => {
            // Absolute, so a caller in another directory can use every field
            // as given; `validate_root` has already checked the directory exists.
            let root = path.canonicalize()?;
            let state_dir = devmap_extract::paths::state_dir(&root);
            let db_path = cli.db();
            let db_path = if db_path.is_absolute() {
                db_path
            } else {
                root.join(db_path)
            };
            let payload = serde_json::json!({
                "root": root,
                "state_dir": state_dir,
                "state_dir_exists": state_dir.is_dir(),
                "db_path": db_path,
                "store_exists": db_path.is_file(),
                "repo_map": devmap_extract::paths::repo_map_path(&root),
                "code_graph": devmap_extract::paths::code_graph_path(&root),
                "workspace": devmap_extract::paths::workspace_path(&root),
                "plugin_dir": devmap_extract::paths::plugin_dir(&root),
            });
            if cli.json {
                emit_json(cli, &payload)?;
            } else {
                for key in [
                    "root",
                    "state_dir",
                    "db_path",
                    "repo_map",
                    "code_graph",
                    "workspace",
                    "plugin_dir",
                ] {
                    println!("{key:<12} {}", payload[key].as_str().unwrap_or(""));
                }
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
            let stored_schema = Store::stored_schema_version(cli.db())?;
            let Some(stored_schema) = stored_schema else {
                let mut payload = serde_json::json!({
                    "generation_id": serde_json::Value::Null,
                    "pending_count": 0,
                    "node_count": 0,
                    "edge_count": 0,
                    "is_fresh": false,
                    "db_path": cli.db().display().to_string(),
                    "degraded_reason": "no devmap store at this path (run `devmap build`)",
                    "quarantined_count": 0,
                    "quarantined_paths": Vec::<String>::new(),
                    // `null`, never three empty lists. Nothing was measured
                    // here — there is no store to measure — and an empty
                    // inventory is the answer of a build that read everything.
                    "coverage_gaps": serde_json::Value::Null,
                    "edge_resolution_source": serde_json::Value::Null,
                    "edge_confidence_mismatches": serde_json::Value::Null,
                    "schema_outdated": false,
                    "schema_version": serde_json::Value::Null,
                    "expected_schema_version": devmap_store::CURRENT_SCHEMA_VERSION,
                    // A property of the binary, not of the store — so it is
                    // answered even here, where there is no store. This is the
                    // exit the seam's own probe takes.
                    "capabilities": kernel_capabilities(),
                });
                extend_host_contract(&mut payload, None, None);
                // Through `emit_json` like every other exit from this command.
                // Printed pretty regardless of `--json`, this was the one
                // `--json` path in the binary that emitted a multi-line
                // document, so a caller reading a line at a time got a `{` and
                // a parse error out of the case it most needs to handle: no
                // store yet.
                emit_json(cli, &payload)?;
                return Ok(());
            };
            if stored_schema != devmap_store::CURRENT_SCHEMA_VERSION {
                let version = stored_schema;
                let mut payload = serde_json::json!({
                    "generation_id": serde_json::Value::Null,
                    "pending_count": 0,
                    "node_count": 0,
                    "edge_count": 0,
                    "is_fresh": false,
                    "db_path": cli.db().display().to_string(),
                    // `user_version = 2` is the Python engine's `index.sqlite`, a
                    // schema this kernel has no migration for. `devmap build`
                    // against it already refuses by name (the store's own
                    // message); telling `status` readers to run it is advice
                    // that cannot work, for a file the other command names.
                    "degraded_reason": if version == devmap_store::PYTHON_INDEX_SCHEMA_VERSION {
                        format!(
                            "store schema is {version}: this is the Python engine's database \
                             (`.devcouncil/codeintel/index.sqlite`), not a devmap store, and \
                             this kernel cannot convert it — point `--db` at `devmap.sqlite` \
                             (this binary speaks {})",
                            devmap_store::CURRENT_SCHEMA_VERSION
                        )
                    } else if version > devmap_store::CURRENT_SCHEMA_VERSION {
                        format!(
                            "store schema is {version}, newer than the {} this binary speaks; \
                             install a matching or newer devmap binary",
                            devmap_store::CURRENT_SCHEMA_VERSION
                        )
                    } else if Store::schema_is_migratable(version) {
                        format!(
                            "store schema is {version}, this binary speaks {}; \
                             run `devmap build` to migrate it",
                            devmap_store::CURRENT_SCHEMA_VERSION
                        )
                    } else {
                        format!(
                            "store schema is {version}, unsupported by this binary (which speaks {})",
                            devmap_store::CURRENT_SCHEMA_VERSION
                        )
                    },
                    "quarantined_count": 0,
                    "quarantined_paths": Vec::<String>::new(),
                    // Same reason as the no-store case: this binary refused to
                    // read the store, so it measured nothing.
                    "coverage_gaps": serde_json::Value::Null,
                    "edge_resolution_source": serde_json::Value::Null,
                    "edge_confidence_mismatches": serde_json::Value::Null,
                    "schema_outdated": true,
                    "schema_version": version,
                    "expected_schema_version": devmap_store::CURRENT_SCHEMA_VERSION,
                    "capabilities": kernel_capabilities(),
                });
                extend_host_contract(&mut payload, Some(version), None);
                emit_json(cli, &payload)?;
                return Ok(());
            }
            let Some(store) = Store::open_existing(cli.db())? else {
                anyhow::bail!(
                    "the devmap store at {} vanished between the schema probe and the read",
                    cli.db().display()
                );
            };
            if cli.db.is_none() {
                store.validate_repo_root(&cli.root_hint())?;
            }
            let mut payload = store_status_fields(&store, &cli.db())?;
            payload.insert("schema_outdated".into(), serde_json::json!(false));
            payload.insert("schema_version".into(), serde_json::json!(stored_schema));
            payload.insert(
                "expected_schema_version".into(),
                serde_json::json!(devmap_store::CURRENT_SCHEMA_VERSION),
            );
            payload.insert("capabilities".into(), kernel_capabilities());
            payload.extend(host_contract_fields(
                Some(stored_schema),
                payload
                    .get("generation_id")
                    .and_then(serde_json::Value::as_i64),
            ));
            emit_json(cli, &serde_json::Value::Object(payload))?;
        }
        Commands::Doctor => {
            // Never creates a store. The probe is what a host runs *before* it
            // trusts this binary against a tree it may not own; creating one
            // here would turn "is this binary usable?" into a write.
            let payload = doctor_report(&cli.db())?;
            emit_json(cli, &payload)?;
        }
        Commands::History { last } => {
            let store = open_for_read(cli)?;
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
                    cli,
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
        Commands::Repair {
            fts,
            pending,
            page_size,
        } => {
            // `cli.db()` rather than `cli.db`: the store path is resolved per
            // repository now, and `--db` is an `Option`.
            let store = open_for_read(cli)?;
            if !*fts && !*pending && !*page_size {
                anyhow::bail!("specify a repair target, e.g. --fts, --pending or --page-size");
            }
            if *page_size {
                let outcome = store.convert_page_size()?;
                if cli.json {
                    emit_json(
                        cli,
                        &serde_json::json!({
                            "page_size_before": outcome.before,
                            "page_size_after": outcome.after,
                            "converted": outcome.converted,
                        }),
                    )?;
                } else if outcome.converted {
                    println!(
                        "Store rewritten at {} byte pages (was {}).",
                        outcome.after, outcome.before
                    );
                } else {
                    println!("Store already uses {} byte pages.", outcome.after);
                }
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
                        cli,
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
            let store = open_for_read(cli)?;
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
            emit_json(cli, &serde_json::to_value(&resp)?)?;
        }
        Commands::Mcp { http, print_config } => {
            // Nothing but JSON-RPC frames may reach stdout on this transport.
            // `main` installs the tracing subscriber before this match, and
            // `FmtSubscriber::builder()` writes to stdout by default — see the
            // `.with_writer(std::io::stderr)` there, which this transport
            // depends on and which `--json` output depended on already.
            if *print_config {
                // Before the store is touched: this must create no file and open
                // no database, so it can be run against a repository that has
                // never been indexed — which is when a user configures a host.
                let executable = std::env::current_exe()?;
                // One owner for "how do I reach this server": the plugin bundle
                // calls the same builder, so a host configured from one cannot
                // point at a different server than a host configured from the
                // other. It also refuses a non-UTF-8 path rather than writing
                // `display()`'s replacement characters into a `command` that
                // then names no file on disk.
                let entry = claude::mcp_entry(&executable, &cli.db(), http.as_deref())?;
                emit_json(
                    cli,
                    &serde_json::json!({"mcpServers": {claude::MCP_SERVER_NAME: entry}}),
                )?;
                return Ok(());
            }

            let slot = std::sync::Arc::new(devmap_serve::StoreSlot::new(cli.db()));
            match http {
                Some(address) => {
                    // A bare port means loopback. Spelling the default out here
                    // rather than accepting "8080" as 0.0.0.0 is the difference
                    // between serving one machine and serving a network.
                    let address = claude::normalize_http_address(address);
                    let parsed: std::net::SocketAddr = address.parse().map_err(|err| {
                        anyhow::anyhow!("could not parse --http address '{address}': {err}")
                    })?;
                    devmap_serve::serve_http(slot, parsed).await?;
                }
                None => devmap_serve::serve_stdio(slot).await?,
            }
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
                    emit_json(cli, &serde_json::json!({"socket": ipc_path}))?;
                } else {
                    println!("{}", ipc_path.display());
                }
                return Ok(());
            }

            ensure_parent(&cli.db())?;
            let store = Store::open(cli.db())?;
            let daemon = Daemon::new(store, root)
                // So the daemon can notice its own store being deleted and
                // exit, instead of serving a removed inode until its idle bound
                // expires half an hour later.
                .with_store_path(cli.db())
                .with_ipc_path(ipc_path);
            daemon.run_loop().await?;
        }
        Commands::Pdg { file, taint } => {
            let language = devmap_extract::detect_language(file);
            if language != "python" {
                anyhow::bail!(
                    "pdg supports python; {} is {language}. Refusing rather than returning an \
empty graph, which would read as 'this file has no control flow'.",
                    file.display()
                );
            }
            let source = devmap_extract::read_source(file)
                .map_err(|error| anyhow::anyhow!("cannot read {}: {error}", file.display()))?;
            let qualifier = file.to_string_lossy().replace('\\', "/");
            let inputs = devmap_analyze::pdgsrc::python_function_pdgs(&source, &qualifier, 0, 0);

            let mut graphs = Vec::new();
            let mut refused = Vec::new();
            for input in &inputs {
                match devmap_analyze::pdg::build_function_pdg(input) {
                    Ok(graph) => graphs.push((input, graph)),
                    // A function the builder refuses is named with its reason
                    // rather than dropped: a short list that looks complete is
                    // the failure this whole analysis is careful about.
                    Err(error) => refused.push(serde_json::json!({
                        "function": input.function_name,
                        "reason": render_error(&error),
                    })),
                }
            }

            let sinks_of = |input: &devmap_analyze::pdg::FunctionPdgInput| -> Vec<String> {
                fn walk(statements: &[devmap_analyze::pdg::PdgStatement], out: &mut Vec<String>) {
                    use devmap_analyze::pdg::PdgStatementKind as K;
                    for statement in statements {
                        for sink in &statement.taint_sinks {
                            out.push(format!("{}:{}", statement.line, sink));
                        }
                        match &statement.kind {
                            K::Branch {
                                then_body,
                                else_body,
                            } => {
                                walk(then_body, out);
                                walk(else_body, out);
                            }
                            K::Loop { body } => walk(body, out),
                            K::Try {
                                body,
                                handlers,
                                finally_body,
                            } => {
                                walk(body, out);
                                for handler in handlers {
                                    walk(handler, out);
                                }
                                walk(finally_body, out);
                            }
                            K::Basic | K::Return | K::Raise => {}
                        }
                    }
                }
                let mut out = Vec::new();
                walk(&input.body, &mut out);
                out
            };

            let rows: Vec<serde_json::Value> = graphs
                .iter()
                .filter_map(|(input, graph)| {
                    let sinks = sinks_of(input);
                    if *taint && sinks.is_empty() {
                        return None;
                    }
                    Some(serde_json::json!({
                        "function": graph.function_name,
                        "start_line": input.start_line,
                        "end_line": input.end_line,
                        "params": input.params,
                        "nodes": graph.nodes.len(),
                        "edges": graph.edges.len(),
                        "taint_sinks": sinks,
                    }))
                })
                .collect();

            if cli.json {
                emit_json(
                    cli,
                    &serde_json::json!({
                        "file": file,
                        "language": language,
                        "functions": rows,
                        // Both numbers: a filtered view reported as a count
                        // reads as a total.
                        "shown": rows.len(),
                        "total": graphs.len(),
                        "refused": refused,
                    }),
                )?;
            } else {
                for row in &rows {
                    println!(
                        "{}  lines {}-{}  {} node(s), {} edge(s)",
                        row["function"].as_str().unwrap_or(""),
                        row["start_line"],
                        row["end_line"],
                        row["nodes"],
                        row["edges"]
                    );
                    for sink in row["taint_sinks"].as_array().into_iter().flatten() {
                        println!("    sink {}", sink.as_str().unwrap_or(""));
                    }
                }
                println!("  {} of {} function(s)", rows.len(), graphs.len());
                for entry in &refused {
                    println!(
                        "  refused {}: {}",
                        entry["function"].as_str().unwrap_or(""),
                        entry["reason"].as_str().unwrap_or("")
                    );
                }
            }
        }
        Commands::Cypher { query, limit } => {
            let store = open_for_read(cli)?;
            let graph = graph_core_for_read(&store)?;
            let result = devmap_query::cypher::run(&graph, query, *limit);
            if cli.json {
                emit_json(cli, &result)?;
            } else if result["ok"].as_bool() == Some(true) {
                for row in result["rows"].as_array().into_iter().flatten() {
                    match row.get("rel").and_then(serde_json::Value::as_str) {
                        Some(rel) => println!(
                            "{}  -[{rel}]->  {}",
                            row["a_id"].as_str().unwrap_or(""),
                            row["b_id"].as_str().unwrap_or("")
                        ),
                        None => println!("{}", row["a_id"].as_str().unwrap_or("")),
                    }
                }
                // Both numbers, always: a page reported as a count reads as a
                // total, and this surface exists to answer "how many".
                println!(
                    "  {} of {} row(s){}",
                    result["shown"].as_u64().unwrap_or(0),
                    result["total"].as_u64().unwrap_or(0),
                    if result["limit_capped"].as_bool() == Some(true) {
                        format!(
                            " (LIMIT {} capped to {})",
                            result["limit_requested"].as_u64().unwrap_or(0),
                            result["limit_applied"].as_u64().unwrap_or(0)
                        )
                    } else {
                        String::new()
                    }
                );
            } else {
                // A refusal is an error exit, not a zero-row success: a caller
                // that scripts this must be able to tell "your query was not
                // run" from "your query matched nothing".
                anyhow::bail!("{}", result["error"].as_str().unwrap_or("query refused"));
            }
        }
        Commands::Ast {
            query,
            kind,
            language,
            limit,
            facets,
        } => {
            let store = open_for_read(cli)?;
            if *facets {
                let facets = devmap_query::ast::ast_facets(&store)?;
                if cli.json {
                    emit_json(cli, &facets)?;
                } else {
                    println!("kinds:");
                    for (name, count) in facets["kinds"].as_object().into_iter().flatten() {
                        println!("  {name:<16} {count}");
                    }
                    println!("languages:");
                    for (name, count) in facets["languages"].as_object().into_iter().flatten() {
                        println!("  {name:<16} {count}");
                    }
                }
                return Ok(());
            }
            let filter = devmap_query::ast::AstFilter {
                query: query.clone(),
                kind: kind.clone(),
                language: language.clone(),
                limit: *limit,
            };
            let answer = devmap_query::ast::ast_query(&store, &filter)?;
            if cli.json {
                emit_json(cli, &answer)?;
            } else {
                report_ast(&answer);
            }
        }
        Commands::Export { path, out } => {
            let store = open_for_read(cli)?;
            let graph = graph_value_for_read(&store, &cli.db())?;
            let gen_id = store.latest_generation_id()?.unwrap_or(0);
            let (xml, report) = devmap_query::export::export_graphml(&graph);

            let to_stdout = out.as_deref() == Some(std::path::Path::new("-"));
            let destination = out
                .clone()
                .filter(|_| !to_stdout)
                .unwrap_or_else(|| devmap_extract::paths::state_dir(path).join("graph.graphml"));
            if to_stdout {
                print!("{xml}");
            } else {
                ensure_parent(&destination)?;
                std::fs::write(&destination, &xml)?;
            }

            if cli.json {
                emit_json(
                    cli,
                    &serde_json::json!({
                        "output": if to_stdout { serde_json::Value::Null }
                                  else { serde_json::json!(destination) },
                        "generation_id": gen_id,
                        "nodes": report.nodes,
                        "edges": report.edges,
                        // GraphML cannot express an edge to an undeclared node,
                        // and a symbol name can hold bytes XML forbids. Both are
                        // repairs, and a repair nobody is told about is a
                        // difference between the graph and its export.
                        "edges_dangling": report.edges_dangling,
                        "characters_replaced": report.characters_replaced,
                        "bytes": xml.len(),
                    }),
                )?;
            } else if !to_stdout {
                println!("Wrote {}", destination.display());
                println!("  {} nodes, {} edges", report.nodes, report.edges);
                if report.edges_dangling > 0 {
                    println!(
                        "  {} edge(s) omitted: an endpoint is not a declared node \
(GraphML cannot express one)",
                        report.edges_dangling
                    );
                }
                if report.characters_replaced > 0 {
                    println!(
                        "  {} character(s) replaced with U+FFFD: XML 1.0 cannot \
represent them",
                        report.characters_replaced
                    );
                }
            }
        }
        Commands::Routes {
            path,
            filter,
            max_files,
            max_file_bytes,
        } => {
            let store = open_for_read(cli)?;
            let graph = graph_core_for_read(&store)?;
            let budget = scan_budget(*max_files, *max_file_bytes);
            let root = repo_root_for(&store, path)?;
            let mut mapped = devmap_query::api_routes::route_map(&root, &graph, &budget);
            if let Some(filter) = filter {
                retain_matching_routes(&mut mapped, filter);
            }
            if cli.json {
                emit_json(cli, &mapped)?;
            } else {
                report_routes(&mapped);
            }
        }
        Commands::ShapeCheck {
            path,
            filter,
            max_files,
            max_file_bytes,
        } => {
            let store = open_for_read(cli)?;
            let graph = graph_core_for_read(&store)?;
            let budget = scan_budget(*max_files, *max_file_bytes);
            let root = repo_root_for(&store, path)?;
            let checked =
                devmap_query::api_routes::shape_check(&root, &graph, &budget, filter.as_deref());
            if cli.json {
                emit_json(cli, &checked)?;
            } else {
                report_shape_check(&checked);
            }
        }
        Commands::ApiImpact {
            route,
            path,
            max_files,
            max_file_bytes,
        } => {
            let store = open_for_read(cli)?;
            let graph = graph_core_for_read(&store)?;
            let budget = scan_budget(*max_files, *max_file_bytes);
            let root = repo_root_for(&store, path)?;
            let impact = devmap_query::api_routes::api_impact(&root, &graph, &budget, route);
            if cli.json {
                emit_json(cli, &impact)?;
            } else {
                report_api_impact(&impact);
            }
        }
        Commands::Html {
            path,
            out,
            level,
            max_nodes,
        } => {
            let store = open_for_read(cli)?;
            let gen_id = store.latest_generation_id()?.unwrap_or(0);
            let repo_root = store.latest_repo_root()?;

            let title = repo_root
                .as_deref()
                .and_then(|root| Path::new(root).file_name())
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Dev Map".to_string());

            let graph = graph_value_for_read(&store, &cli.db())?;
            let options = devmap_query::viz::VizOptions {
                symbols: *level == HtmlLevel::Symbols,
                max_nodes: *max_nodes,
                title,
            };
            let payload = devmap_query::viz::build_payload(&graph, &options);
            let html = devmap_query::viz::render_html(&graph, &options);

            let destination = out
                .clone()
                .unwrap_or_else(|| devmap_extract::paths::state_dir(path).join("graph.html"));
            ensure_parent(&destination)?;
            std::fs::write(&destination, &html)?;

            let counts = &payload["counts"];
            if cli.json {
                emit_json(
                    cli,
                    &serde_json::json!({
                        "output": destination,
                        "generation_id": gen_id,
                        "level": payload["level"],
                        // Both numbers travel with the answer, as they do in the
                        // page: a capped view reported as a node count is a
                        // capped view nobody knows is capped.
                        "counts": counts,
                        "bytes": html.len(),
                    }),
                )?;
            } else {
                println!("Wrote {}", destination.display());
                let shown = counts["nodes_shown"].as_u64().unwrap_or(0);
                let total = counts["nodes_total"].as_u64().unwrap_or(0);
                if counts["nodes_truncated"].as_bool().unwrap_or(false) {
                    println!(
                        "  {shown} of {total} nodes drawn (most connected first); \
raise --max-nodes to widen"
                    );
                } else {
                    println!("  {total} nodes drawn");
                }
            }
        }
        Commands::Claude { action } => run_claude(cli, action)?,
    }

    Ok(())
}

/// `devmap claude …` — emission and validation of Dev Map's Claude Code surface.
///
/// Nothing here opens the store or starts anything: a user configures a host
/// before the repository has ever been indexed, so every one of these must work
/// on a fresh clone. Same contract as `serve --print-socket-path` and
/// `mcp --print-config`.
fn run_claude(cli: &Cli, action: &ClaudeAction) -> anyhow::Result<()> {
    let subcommands = claude::known_subcommands::<Cli>();
    match action {
        ClaudeAction::Hooks { binary } => {
            let executable = claude::plugin_command(&std::env::current_exe()?, binary.as_deref());
            let block = claude::hooks_block(&executable, &cli.db(), &subcommands)?;
            emit_json(cli, &block)
        }
        ClaudeAction::Events => {
            let coverage = claude::event_coverage();
            if cli.json {
                let rows: Vec<serde_json::Value> = coverage
                    .iter()
                    .map(|(event, hook, reason)| {
                        serde_json::json!({
                            "event": event,
                            "handled": hook.is_some(),
                            // The authorization surface, named in the data so a
                            // consumer checking it reads the same list the
                            // writer enforces rather than a copy of it.
                            "decides_permission":
                                claude::PERMISSION_DECIDING_EVENTS.contains(event),
                            "matcher": hook.map(|h| h.matcher),
                            "subcommand": hook.map(|h| h.subcommand),
                            "reason": reason,
                        })
                    })
                    .collect();
                let handled = coverage.iter().filter(|(_, h, _)| h.is_some()).count();
                // Both numbers, always: "2 handled" beside a list of two reads
                // as complete coverage of a surface that has 33 events.
                emit_json(
                    cli,
                    &serde_json::json!({
                        "events_total": coverage.len(),
                        "events_handled": handled,
                        "events": rows,
                    }),
                )
            } else {
                for (event, hook, reason) in &coverage {
                    match hook {
                        Some(hook) => println!(
                            "{event:<20} handled   devmap {} (matcher {:?})\n{:22}{reason}",
                            hook.subcommand, hook.matcher, ""
                        ),
                        None => println!("{event:<20} -\n{:22}{reason}", ""),
                    }
                }
                let handled = coverage.iter().filter(|(_, h, _)| h.is_some()).count();
                println!("\n{handled} of {} events handled", coverage.len());
                Ok(())
            }
        }
        ClaudeAction::Plugin {
            out,
            dry_run,
            binary,
        } => {
            let executable = claude::plugin_command(&std::env::current_exe()?, binary.as_deref());
            let version = env!("CARGO_PKG_VERSION");
            let out = &out
                .clone()
                .unwrap_or_else(|| devmap_extract::paths::plugin_dir(Path::new(".")));
            if *dry_run {
                let rendered = claude::render_plugin_bundle(
                    &executable,
                    &cli.db(),
                    Some(version),
                    &subcommands,
                )?;
                let files: serde_json::Map<String, serde_json::Value> = rendered
                    .into_iter()
                    .map(|(path, json)| {
                        Ok((
                            path.to_str()
                                .ok_or_else(|| {
                                    anyhow::anyhow!("bundle path is not valid UTF-8: {path:?}")
                                })?
                                .to_string(),
                            serde_json::from_str::<serde_json::Value>(&json)?,
                        ))
                    })
                    .collect::<anyhow::Result<_>>()?;
                return emit_json(cli, &serde_json::Value::Object(files));
            }
            let written = claude::write_plugin_bundle(
                out,
                &executable,
                &cli.db(),
                Some(version),
                &subcommands,
            )?;
            let changed = written.iter().filter(|f| f.changed).count();
            if cli.json {
                emit_json(
                    cli,
                    &serde_json::json!({
                        "out": out,
                        "files": written.iter().map(|f| serde_json::json!({
                            "path": f.path,
                            "changed": f.changed,
                        })).collect::<Vec<_>>(),
                        "changed": changed,
                    }),
                )
            } else {
                for file in &written {
                    println!(
                        "{} {}",
                        if file.changed { "wrote  " } else { "current" },
                        file.path.display()
                    );
                }
                println!(
                    "\n{changed} of {} file(s) changed. Install with:\n  \
                     claude plugin marketplace add {}\n  claude plugin install {}@{}",
                    written.len(),
                    out.display(),
                    claude::PLUGIN_NAME,
                    claude::MARKETPLACE_NAME,
                );
                Ok(())
            }
        }
        ClaudeAction::Validate { path, strict } => {
            let report = claude::validate_file(path, *strict)?;
            if cli.json {
                emit_json(cli, &report.to_json())?;
            } else {
                for diagnostic in &report.diagnostics {
                    println!("{diagnostic}");
                }
                println!(
                    "{}: {} error(s), {} warning(s){}",
                    if report.ok() { "ok" } else { "FAILED" },
                    report.errors().count(),
                    report.warnings().count(),
                    if report.strict {
                        " (strict: warnings block)"
                    } else {
                        ""
                    },
                );
            }
            // A validator that exits 0 on a failed document is worse than none:
            // a CI step reads the code, not the prose.
            if !report.ok() {
                std::process::exit(1);
            }
            Ok(())
        }
    }
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
        println!("  Resolution rate: not measured (no attribution sites)");
        return;
    };
    println!(
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
            Some(net) => println!(
                "    {language:<12} {}.{}%  ({} resolved / {} unresolved)",
                net / 10,
                net % 10,
                row.resolved_sites,
                row.unresolved_sites
            ),
            None if !row.extracts_calls => {
                println!("    {language:<12} no call extractor in this build (0 attribution sites)")
            }
            None => println!("    {language:<12} not measured (no attribution sites)"),
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
            println!(
                "    {:<12} …and this build extracts no {} for it, so an empty \
                 answer there is a hole and not a finding",
                "",
                blind.join("/")
            );
        }
    }
    if rows.len() > RESOLUTION_RATE_LANGUAGES_SHOWN {
        println!(
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
        let reporter = ProgressReporter::new(ProgressMode::Never, true, false);

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
        let reporter = ProgressReporter::new(ProgressMode::Never, true, false);
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

    /// A cluster scan that refused must never print as a scan that found none,
    /// and must never print the rebuild advice.
    ///
    /// The scan has three outcomes and `Option<Vec<_>>` holds two: a graph past
    /// `DEAD_CLUSTER_MAX_NODES` comes back with an *empty* cluster list beside
    /// a refusal flag. Mapping it field-for-field renders "too large to walk"
    /// as "no abandoned subsystems", and the human line under it tells the
    /// reader to rebuild — which walks the same graph and refuses again.
    ///
    /// All four states are asserted together because what is being pinned is
    /// the *branch order*, and a test of one branch cannot see an ordering.
    #[test]
    fn a_refused_cluster_scan_prints_neither_none_nor_rebuild_advice() {
        fn response(
            clusters: Option<Vec<devmap_analyze::DeadClusterReport>>,
            incomplete: Option<&str>,
        ) -> devmap_query::Response<devmap_analyze::DeadSymbolReport> {
            devmap_query::Response {
                source_freshness: None,
                items: Vec::new(),
                shown: 0,
                hidden: 0,
                total: 0,
                truncated: false,
                tokens_used: 0,
                resolution: devmap_query::ResolutionAvailability::Available,
                walk_incomplete: None,
                rungs: None,
                dead_clusters: clusters,
                dead_clusters_truncated: 0,
                dead_clusters_incomplete: incomplete.map(str::to_string),
            }
        }
        let joined = |resp| dead_cluster_lines(&resp).join("\n");

        // 1. Refused: the reason, and neither of the other two readings.
        let refused = joined(response(
            None,
            Some("the call graph exceeded 400000 symbols"),
        ));
        assert!(
            refused.contains("not computed") && refused.contains("400000"),
            "a refusal must say so and say why: {refused:?}"
        );
        assert!(
            !refused.contains("none") && !refused.contains("rebuild"),
            "a refused scan is neither an empty finding nor a stale generation: {refused:?}"
        );

        // 2. Refused *and* holding a list — the shape the mapping is written to
        //    make impossible, asserted anyway, because the branch order is what
        //    keeps it impossible at the readout.
        let both = joined(response(Some(Vec::new()), Some("too large")));
        assert!(
            both.contains("not computed"),
            "the refusal wins over a list it also carries: {both:?}"
        );

        // 3. Absent: the generation predates the pass, and rebuilding does help.
        let absent = joined(response(None, None));
        assert!(
            absent.contains("not recorded") && absent.contains("rebuild"),
            "a generation with no scan is the one case rebuilding fixes: {absent:?}"
        );

        // 4. Ran and found none: a finding, and it must not read as either
        //    absence.
        let empty = joined(response(Some(Vec::new()), None));
        assert_eq!(
            empty.trim(),
            "abandoned cycles: none",
            "the pass ran; that is a result, not a caveat"
        );
    }
}
