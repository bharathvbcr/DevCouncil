//! The `devmap` argument surface: global flags, the subcommand enum, and the
//! limits every subcommand's arguments are validated against before it runs.

use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand, ValueEnum};
use devmap_query::freshness::InventoryLimits;
use devmap_query::StampedFreshness;
// The token-budget and traversal-depth ceilings are `devmap_query`'s — the
// engine applies them — and both transports import them, so a bound that
// holds over the socket also holds over argv without a second spelling.
use devmap_query::{MAX_TOKEN_BUDGET, MAX_TRAVERSAL_DEPTH};

use crate::commands;

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
pub(crate) fn version_line() -> &'static str {
    static LINE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    LINE.get_or_init(|| {
        format!(
            "{} (store schema {}, code graph schema {}, build {})",
            env!("CARGO_PKG_VERSION"),
            devmap_store::CURRENT_SCHEMA_VERSION,
            devmap_query::CODE_GRAPH_SCHEMA_VERSION,
            env!("DEVMAP_BUILD_ID")
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
pub(crate) struct Cli {
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
    pub(crate) db: Option<PathBuf>,

    /// The repository this invocation is about, when the working directory is
    /// not it.
    ///
    /// Resolves the store the same way every other command does — through
    /// `devmap_extract::paths` against this root — rather than naming a store
    /// path. That is the difference a host hook needs: a hook runs in the
    /// agent's current directory, which after a `cd` or a worktree entry is not
    /// the repository the index belongs to, and baking
    /// `--db ${CLAUDE_PROJECT_DIR}/.devcouncil/codeintel/devmap.sqlite` into a
    /// hook fixes the state layout at the moment the hook was written. Which
    /// state directory a repository uses is a property of the repository.
    ///
    /// `--db` still wins where both are given: it names a store outright.
    #[arg(long, global = true)]
    pub(crate) root: Option<PathBuf>,

    /// Machine-readable output. Global; see `--db`.
    #[arg(long, global = true, default_value_t = false)]
    pub(crate) json: bool,

    /// Command progress policy. Auto animates work on interactive stderr;
    /// always also emits plain progress in logs. JSON stdout stays clean.
    #[arg(long, value_enum, global = true, default_value_t = ProgressMode::Auto)]
    pub(crate) progress: ProgressMode,

    /// Include build phase timings, reclaim details and resolution breakdowns.
    #[arg(long, short, global = true)]
    pub(crate) verbose: bool,

    #[command(subcommand)]
    pub(crate) command: Commands,
}

impl Cli {
    /// The tree this invocation is about.
    ///
    /// Only the subcommands that actually name a repository root contribute
    /// one. `claude validate <path>` is deliberately absent: its argument is a
    /// *file* to check, and treating it as a root would resolve the store
    /// relative to a hooks manifest.
    pub(crate) fn root_hint(&self) -> PathBuf {
        // An explicit `--root` is the caller stating which repository this is
        // about, and it outranks every per-subcommand inference below.
        if let Some(root) = &self.root {
            return root.clone();
        }
        match &self.command {
            Commands::Build(commands::build::Args { path, .. })
            | Commands::Manifest(commands::manifest::Args { path, .. })
            | Commands::MapHtml(commands::map_html::Args { path, .. })
            | Commands::Freshness(commands::freshness::Args { path, .. })
            | Commands::Serve(commands::serve::Args { path, .. })
            | Commands::Html(commands::html::Args { path, .. })
            | Commands::Export(commands::export::Args { path, .. })
            | Commands::Routes(commands::routes::Args { path, .. })
            | Commands::ShapeCheck(commands::shape_check::Args { path, .. })
            | Commands::ApiImpact(commands::api_impact::Args { path, .. })
            | Commands::Paths(commands::paths::Args { path }) => path.clone(),
            // Hook templates retain project-relative paths for the host that
            // will execute them; they are not a query against this checkout.
            Commands::Claude(_) | Commands::Skills(_) | Commands::Hook(_) => PathBuf::from("."),
            Commands::Integrate(commands::integrate::Args { project_root, .. }) => {
                project_root.clone()
            }
            _ => default_root_hint(),
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
    pub(crate) fn db(&self) -> PathBuf {
        match &self.db {
            Some(explicit) => explicit.clone(),
            None => devmap_extract::paths::store_path(self.root_hint()),
        }
    }
}

/// Omitted roots agree with rootless queries. An explicit `.` still scopes a
/// build to the current directory; no other worktree's store is inherited.
pub(crate) fn default_root_hint() -> PathBuf {
    devmap_extract::git_worktree_root(Path::new(".")).unwrap_or_else(|| PathBuf::from("."))
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum ProgressMode {
    Auto,
    Always,
    Never,
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
pub(crate) struct StampFlags {
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
    pub(crate) fn supplied(&self) -> Option<StampedFreshness> {
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
pub(crate) struct InventoryFlags {
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
pub(crate) enum Commands {
    /// Cold or incremental build of the code-intelligence graph
    Build(commands::build::Args),
    Search(commands::search::Args),
    /// Plain-language find over names, docstrings, and the call graph.
    ///
    /// Seeds with TF-IDF over symbol names plus docstrings when present, then
    /// re-ranks with personalized PageRank over stored call edges. Distinct
    /// from `--semantic` search, which ranks names only.
    Ask(commands::ask::Args),
    Deps(commands::deps::Args),
    Impact(commands::impact::Args),
    /// Callers and callees for several targets in one invocation.
    ///
    /// Exists because the composed views above it were paying a process spawn
    /// per direction per target: a five-definition `graph_query` cost eleven
    /// `devmap` invocations, and the spawn — not the query — was the wall
    /// clock. More than `MAX_NEIGHBOR_TARGETS` targets is refused rather than
    /// trimmed, so a short answer is never mistaken for a complete one.
    Neighbors(commands::neighbors::Args),
    Trace(commands::trace::Args),
    /// Dead-symbol candidates with per-row confidence — not one flat delete list.
    ///
    /// High confidence (above the degraded ceiling) means no inbound evidence
    /// after the call walk; `only_ambiguous_callers` and unresolved-namesake
    /// rows sit at 0.4 (unconfirmed). Sites classified `NoNamesake` are
    /// explained gaps in the unresolved ledger, not dead findings — they do
    /// not appear here. Read `walk_incomplete` before treating an empty or
    /// short list as complete coverage.
    Dead(commands::dead::Args),
    /// Definitions in one file as signature plus span — never the body.
    ///
    /// When a signature was not extracted the span is still returned and the
    /// row says so. An empty file and a path the index does not contain are
    /// different envelopes (`presence`), so "nothing here" is never confused
    /// with "not examined".
    Skeleton(commands::skeleton::Args),
    /// Which commits since `--since` could have caused a symptom.
    ///
    /// Runs the graph first and git second: the symptom names a symbol, the
    /// code graph names everything that symbol transitively *depends on*, and
    /// only the byte spans of those symbols are blamed. A file's other lines
    /// cannot have caused this failure, so blaming them is noise — which is
    /// what a blame-first tool spends most of its output on.
    ///
    /// The direction is deliberate: a symptom is caused by its own body or by
    /// something it calls. Its callers sit downstream of the failure.
    ///
    /// The answer distinguishes "nothing in the window touched the cone" from
    /// "something could not be examined": an empty list with `complete: true`
    /// is a finding, an empty list with `complete: false` is not.
    Suspects(commands::suspects::Args),
    /// What a change affects: the symbols, files, modules and tests downstream
    /// of the lines it touched.
    ///
    /// The mirror of `suspects`. That command runs from a symptom back to the
    /// commits that could have caused it, walking outbound call edges. This
    /// runs from a change forward to what depends on it, walking **inbound**
    /// ones — a change breaks what calls it, never what it calls.
    ///
    /// The post-image is always the revision the index was built at, because
    /// that is the only content the graph's byte offsets describe. A diff
    /// against any other revision would pair spans from one revision with
    /// lines from another, which the join refuses rather than clamping past.
    ///
    /// Changed lines that land in no symbol — imports, top-level constants,
    /// attributes, macro invocations — are reported as unattributed rather
    /// than dropped, and make the report incomplete. A change to a module's
    /// central constant must never read as a change that affects nothing.
    ///
    /// For an *uncommitted* buffer, use `preview`: it diffs candidate content
    /// against the index and reports what a removal or re-declaration would
    /// break. This command needs a committed post-image so the basis is a real
    /// blob rather than a worktree read that nothing can be compared against.
    Blast(commands::blast::Args),
    /// Definitions matching a query, with source, callers, callees and a
    /// layered blast radius — the whole neighbourhood in one invocation.
    ///
    /// Replaces the Python `CodeIntelQueryEngine.explore`, which loaded the
    /// entire graph into process memory to answer. The budget is divided across
    /// the four parts and the division is reported in `budget`, so a thin edge
    /// list is attributable to the allowance rather than mistaken for a symbol
    /// nothing calls.
    Explore(commands::explore::Args),
    /// Where a string constant is written.
    ///
    /// Matches the literal text, not a symbol name. A prefix is the default
    /// (`session.` matches `session.spawn`); `--exact` requires equality.
    /// Each site names the file, line and enclosing symbol. A Rust
    /// module-level const's value is also reported at each use of that const.
    Literals(commands::literals::Args),
    /// Test files reachable through the inbound blast radius of some targets.
    ///
    /// Ranked nearest-first: a budget-trimmed list keeps the tests closest to
    /// the change. Targets that match nothing are named rather than dropped.
    Affected(commands::affected::Args),
    /// Ask what an unsaved edit would do to the graph, without writing it.
    ///
    /// Reads the candidate content from `--content` (a file, or `-` for stdin)
    /// and diffs it against the indexed version of `--file`, reporting symbols
    /// added, removed and re-declared, plus the calls from other files that a
    /// removal or re-declaration would break.
    Preview(commands::preview::Args),
    /// Manage the workspace registry and query across every repository in it.
    ///
    /// `devmap serve` indexes one root, which is the right unit for a build and
    /// the wrong one for a question: "who calls this" does not stop at a
    /// repository boundary when the caller is a sibling service.
    Workspace(commands::workspace::Args),
    /// Report what the map cost against what reading files would have.
    ///
    /// Every figure is bytes divided by 4, which is an estimate and is labelled
    /// one. With `--query`, also reports one search's actual token spend beside
    /// the size of the files that search pointed into.
    Savings(commands::savings::Args),
    /// Report duplicated symbol bodies in the latest generation.
    ///
    /// `exact` groups are the same code modulo formatting and comments;
    /// `structural` groups are the same shape under renaming, and are limited
    /// to callables.
    Clones(commands::clones::Args),
    /// Write the consumer artifacts: `repo_map.json` and its symbol-level
    /// companion `code_graph.json`.
    ///
    /// Both come from one invocation because that is how consumers get them
    /// from `dev map`: eleven modules under `src/devcouncil/` read the graph,
    /// and a second subcommand would let a repository sit with a fresh map
    /// beside a stale graph built from a different generation.
    Manifest(commands::manifest::Args),
    /// Render the repo map as one self-contained, offline HTML page.
    ///
    /// Reads the `repo_map.json` `manifest` writes — no store, so it answers
    /// for any checkout that has a map, and never blocks on an indexing run.
    ///
    /// Supersedes the Python renderer at `src/devcouncil/indexing/map_viz.py`,
    /// which coloured nodes by hashing the area name and dropped the file
    /// inventory before it could say anything about language or coverage.
    MapHtml(commands::map_html::Args),

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
    Freshness(commands::freshness::Args),
    /// Index health. With `--auto-rebuild`, SessionStart hooks rebuild when
    /// `rebuild_required` is set (payload-obsolete or schema-behind) instead of
    /// only reporting stale — bounded by the hook's own timeout.
    Status(commands::status::Args),
    /// Binary and store health for hosts that install or verify `devmap`.
    ///
    /// Emits the store schema on disk (if any), the schema this binary speaks,
    /// the code-graph artifact schema, how many tree-sitter grammars are linked
    /// into this build, the resolved store path, and every `devmap` found on
    /// `PATH` plus common host configs (with version), so binary skew is visible
    /// before a hook or MCP entry points at the wrong one.
    Doctor,
    /// Write a session insights report from the MCP query log.
    ///
    /// SessionEnd hooks share a short budget, so this only reads the live log
    /// and never builds an index. `--last` prints the previous report (for
    /// SessionStart context) and writes nothing.
    SessionReport(commands::session_report::Args),
    /// Record a DevMap gap — a question the index could not answer — in the
    /// ledger `session-report` reads.
    ///
    /// The agent guide has told every agent to write
    /// `.devcouncil/codeintel/sessions/gaps.jsonl` since it was written, and
    /// nothing could: the kernel named and read that file but never appended to
    /// it, and a harness that protects the state directory refuses the shell
    /// redirect an agent reaches for instead — correctly, since the store
    /// beside it is what every later answer comes from. This is the writer that
    /// was missing, so the instruction is one a tool carries out rather than one
    /// an agent works around.
    GapRecord(commands::gap_record::Args),
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
    Paths(commands::paths::Args),
    /// Longitudinal view: how the map has moved across recent builds.
    History(commands::history::Args),
    Repair(commands::repair::Args),
    Snapshots(commands::snapshots::Args),
    /// Serve queries over IPC, watching the tree for changes.
    ///
    /// The daemon retires itself after 30 minutes with no IPC request, no
    /// pending work and no watcher event; the next client respawns it against
    /// the current kernel. Set DEVMAP_MAX_IDLE_SECS (seconds) to change the
    /// bound, or to 0 to keep serving forever.
    Serve(commands::serve::Args),

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
    Mcp(commands::mcp::Args),

    /// Control and data dependency graphs for the functions in a file.
    ///
    /// Intra-procedural: a closure's body is its own PDG, not part of its
    /// enclosing function's control flow.
    ///
    /// Python only. That is the language the analysis this ports covered, and
    /// claiming a language whose statement tree nothing produces would return an
    /// empty result that reads as "this file has no control flow".
    Pdg(commands::pdg::Args),

    /// Run a small openCypher subset over the graph.
    ///
    /// Supported: `MATCH (a)-[r:calls|imports|…]->(b) WHERE … RETURN a, b
    /// LIMIT n`, with `contains(a.name, '…')` and `starts with(b.path, '…')`
    /// joined by `AND`.
    ///
    /// Anything outside that is **refused**, never silently widened: a `WHERE`
    /// term this cannot evaluate would otherwise return every row in the graph
    /// under a successful status.
    Cypher(commands::cypher::Args),

    /// Find symbols by kind, language and name, from the parsed index.
    ///
    /// The structural counterpart to `search`: `search` ranks by name relevance
    /// and budgets its answer; this enumerates everything matching a filter and
    /// reports the exact total.
    Ast(commands::ast::Args),

    /// Write the graph as GraphML, for Gephi, yEd, Cytoscape or networkx.
    ///
    /// Attributed: every node carries its kind, path, area, community and its
    /// dead/unwired/unreachable flags; every edge its kind and confidence.
    Export(commands::export::Args),

    /// HTTP routes, their handlers, and the clients that call them.
    ///
    /// Routes come from `routes_to` edges, whose source the resolver writes as
    /// `"VERB /path"`. Client call sites are found by pattern, over a bounded
    /// walk of the files the graph names — so every answer carries what the
    /// scan read and whether it finished.
    Routes(commands::routes::Args),

    /// Compare what a handler returns against what its callers read.
    #[command(name = "shape-check")]
    ShapeCheck(commands::shape_check::Args),

    /// What changing one route reaches: callers, shape, and a risk band.
    #[command(name = "api-impact")]
    ApiImpact(commands::api_impact::Args),

    /// Render the graph as one self-contained HTML file.
    ///
    /// No network and no build step: the renderer is embedded, so the page
    /// opens from a `file://` URL on a machine that has never seen a package
    /// manager. Capped by node count and honest about it — see `--max-nodes`.
    Html(commands::html::Args),

    /// Host-neutral hook entry point for Claude Code, Cursor, and Codex.
    ///
    /// Reads one JSON object from stdin (bounded to 1 MiB). Exit 0 on success
    /// or no-op, 1 on failure — never 2 (hosts treat exit 2 as "block").
    Hook(commands::hook::Args),

    /// Emit and check Dev Map's own Claude Code integration.
    ///
    /// Hook specs and a plugin manifest, built and validated here rather than
    /// by a generator living somewhere else: Claude Code drops configuration it
    /// cannot make sense of *quietly* — an unknown event name is ignored at
    /// runtime, a matcher on an event without matcher support is ignored, an
    /// `if` outside a tool event means the handler never runs — so a writer
    /// that only serializes reports the same success for a dead install as for
    /// a working one.
    Claude(commands::claude::Args),

    /// Install the five embedded DevMap skills into a host skill directory.
    ///
    /// Receipt-guarded: identical files are adopted, unmanaged edits are
    /// refused, concurrent installers serialize through a directory lock with
    /// a five-second timeout. Bounds match the Python scaffolder (256 KiB per
    /// skill, 8 MiB batch, 4,096 receipt entries).
    Skills(commands::skills::Args),

    /// Register DevMap with a host: guides, Cursor rule, skills, global MCP.
    ///
    /// Global Cursor/Claude MCP entries are `devmap mcp` without `--db` so one
    /// registration serves every repository. Stale per-project `--db` entries
    /// this installer owns are rewritten. Unrelated MCP servers are preserved.
    Integrate(commands::integrate::Args),

    /// Report build and schema versions.
    Version(commands::version::Args),
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
pub(crate) fn validate_limits(command: &Commands) -> Result<(), String> {
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
        Commands::Search(commands::search::Args { budget, .. })
        | Commands::Literals(commands::literals::Args { budget, .. })
        | Commands::Snapshots(commands::snapshots::Args { budget, .. })
        | Commands::Savings(commands::savings::Args { budget, .. }) => check_budget(*budget),
        Commands::Ask(commands::ask::Args {
            budget,
            min_confidence,
            ..
        }) => {
            check_budget(*budget)?;
            check_confidence(*min_confidence)
        }
        Commands::Dead(commands::dead::Args { budget }) => check_budget(*budget),
        Commands::Skeleton(commands::skeleton::Args { budget, .. }) => check_budget(*budget),
        Commands::Suspects(commands::suspects::Args { since, depth, .. }) => {
            // A blank revision would make the window `..HEAD`, which git reads
            // as every commit ever — the opposite of the bounded question this
            // command exists to ask.
            if since.trim().is_empty() {
                return Err(
                    "--since must name a revision: it is the last known-good point, and \
                     an empty one asks about the whole history rather than a window"
                        .to_string(),
                );
            }
            check_depth(*depth as usize)
        }
        Commands::Blast(commands::blast::Args {
            since,
            at,
            depth,
            format: _,
        }) => {
            match (since.as_deref(), at.as_deref()) {
                (None, None) => {
                    return Err(
                        "blast needs a starting point: --since <rev> for what a range of \
                         commits affects, or --at <path>:<start>-<end> for what a specific \
                         range of lines affects"
                            .to_string(),
                    )
                }
                (Some(since), None) if since.trim().is_empty() => {
                    return Err(
                        "--since must name a revision: an empty one asks about the whole \
                         history rather than a change"
                            .to_string(),
                    )
                }
                // `conflicts_with` already refuses both; this arm exists so the
                // match is exhaustive over what clap can hand back rather than
                // relying on a guarantee stated elsewhere.
                (Some(_), Some(_)) => {
                    return Err("--since and --at are alternatives; pass one".to_string())
                }
                (None, Some(at)) => {
                    // Parsed twice — here for the message, and again at
                    // dispatch. The duplication buys a refusal the user sees
                    // before any store is opened, and the parser is one
                    // function in `dc-regress` so the two calls, and the
                    // daemon's, cannot disagree about what is valid.
                    dc_regress::change::parse_location(at)?;
                }
                (Some(_), None) => {}
            }
            check_depth(*depth as usize)
        }
        Commands::Deps(commands::deps::Args {
            budget,
            min_confidence,
            min_rung,
            ..
        }) => {
            check_budget(*budget)?;
            check_rung(min_rung)?;
            check_confidence(*min_confidence)
        }
        Commands::Impact(commands::impact::Args {
            budget,
            depth,
            min_rung,
            layers,
            ..
        }) => {
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
        Commands::Trace(commands::trace::Args {
            budget,
            depth,
            min_rung,
            ..
        }) => {
            check_rung(min_rung)?;
            check_budget(*budget)?;
            check_depth(*depth)
        }
        Commands::Neighbors(commands::neighbors::Args {
            targets,
            budget,
            depth,
            min_confidence,
            min_rung,
        }) => {
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
        Commands::Explore(commands::explore::Args {
            limit,
            budget,
            depth,
            min_confidence,
            ..
        }) => {
            if *limit == 0 {
                return Err("--limit must be at least 1".to_string());
            }
            check_budget(*budget)?;
            check_depth(*depth)?;
            check_confidence(*min_confidence)
        }
        Commands::Affected(commands::affected::Args {
            budget,
            depth,
            min_confidence,
            ..
        }) => {
            check_budget(*budget)?;
            check_depth(*depth)?;
            check_confidence(*min_confidence)
        }
        Commands::Preview(commands::preview::Args {
            budget,
            min_confidence,
            ..
        }) => {
            check_budget(*budget)?;
            check_confidence(*min_confidence)
        }
        Commands::Clones(commands::clones::Args { budget, .. }) => check_budget(*budget),
        Commands::History(commands::history::Args { last }) => {
            if *last == 0 {
                return Err("--last must be at least 1".to_string());
            }
            Ok(())
        }
        // No numeric query arguments reach the engine from these.
        Commands::Build(_)
        | Commands::Status(_)
        | Commands::Doctor
        | Commands::SessionReport(_)
        // Its arguments are validated by `session::record_gap`, which refuses
        // before it writes. Checking them here as well would put the same rule
        // in two places, and the one that has to hold is the one guarding the
        // write — the MCP surface can reach it without passing through here.
        | Commands::GapRecord(_)
        | Commands::Paths(_)
        | Commands::Manifest(_)
        | Commands::MapHtml(_)
        | Commands::Freshness(_)
        | Commands::Repair(_)
        | Commands::Workspace(_)
        | Commands::Serve(_)
        | Commands::Mcp(_)
        | Commands::Html(_)
        | Commands::Cypher(_)
        | Commands::Pdg(_)
        | Commands::Ast(_)
        | Commands::Export(_)
        | Commands::Routes(_)
        | Commands::ShapeCheck(_)
        | Commands::ApiImpact(_)
        | Commands::Claude(_)
        | Commands::Skills(_)
        | Commands::Integrate(_)
        | Commands::Version(_)
        | Commands::Hook(_) => Ok(()),
    }
}

impl Commands {
    /// Whether this command stays up to answer other processes.
    ///
    /// The daemon and the MCP servers write to sockets and pipes whose peers
    /// come and go; for them a closed peer is an `EPIPE` to handle, not a
    /// reason to exit, and the ignored-`SIGPIPE` disposition Rust's runtime
    /// installs is the right one. Everything else is a one-shot command.
    pub(crate) fn serves(&self) -> bool {
        matches!(self, Commands::Serve(_) | Commands::Mcp(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrate;
    use clap::CommandFactory;

    /// `devmap integrate --help` offers exactly the hosts the integrator takes.
    ///
    /// The help used to be a doc comment reading "`cursor`, `claude`, or
    /// `codex`" over a bare `String`, while `Host::parse` had accepted six for
    /// some time: the text was a claim about another file that nothing checked,
    /// and it told users that half the supported hosts were unsupported. Typing
    /// the positional moved the list into clap's hands; this asserts it stayed
    /// there, and fails if anyone reintroduces a hand-written set.
    #[test]
    fn integrate_advertises_every_host_the_integrator_accepts() {
        let command = Cli::command();
        let integrate = command
            .get_subcommands()
            .find(|candidate| candidate.get_name() == "integrate")
            .expect("`integrate` is a subcommand");
        let host = integrate
            .get_arguments()
            .find(|argument| argument.get_id() == "host")
            .expect("`integrate` takes a host");

        let advertised: Vec<String> = host
            .get_possible_values()
            .iter()
            .map(|value| value.get_name().to_string())
            .collect();
        let accepted: Vec<String> = <integrate::Host as ValueEnum>::value_variants()
            .iter()
            .map(|value| value.as_str().to_string())
            .collect();

        // Not `is_empty`-tolerant on purpose: an untyped positional advertises
        // nothing, which is the state this replaced.
        assert_eq!(
            advertised, accepted,
            "the help offers a different set of hosts than `Host` accepts"
        );

        // The name each is offered under is the name that parses, so a user can
        // copy one out of `--help` and have it work.
        for name in &advertised {
            assert!(
                integrate::Host::parse(name).is_ok(),
                "`--help` offers {name:?}, which the integrator refuses"
            );
        }
    }
}
