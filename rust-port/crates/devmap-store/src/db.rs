use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::edge_index::GenerationEdges;
use devmap_analyze::clones::CloneCandidate;
use devmap_analyze::model::*;
use devmap_extract::model::*;
#[cfg(feature = "parse")]
use devmap_resolve::model::*;
use rusqlite::{params, Connection, OptionalExtension, Result, TransactionBehavior};
use std::collections::{BTreeMap, BTreeSet};

use crate::coverage::{CoverageGapRow, CoverageGapSample, CoverageGaps, DiscoveryRefusal};
use crate::edge_index::ResolutionSource;
use crate::schema::{
    BUILD_HISTORY_RETENTION, BUILD_HISTORY_TABLE, COVERAGE_GAPS_TABLE, CREATE_SCHEMA_V3,
    CURRENT_SCHEMA_VERSION, MIGRATION_V10_TO_V11, MIGRATION_V11_TO_V12, MIGRATION_V12_TO_V13,
    MIGRATION_V14_TO_V15, MIGRATION_V15_TO_V16, MIGRATION_V16_TO_V17, MIGRATION_V3_TO_V4,
    MIGRATION_V4_TO_V5, MIGRATION_V5_TO_V6, MIGRATION_V6_TO_V7, MIGRATION_V7_TO_V8,
    MIGRATION_V8_TO_V9, MIGRATION_V9_TO_V10, UNRESOLVED_TABLE,
};

/// Failed drain attempts after which a pending path stops being retried.
///
/// Public because the queue's hygiene is now a cross-crate contract: the daemon
/// bumps it, `devmap build` and `devmap repair --pending` drop rows that reach
/// it, and `status` names them. A test that asserts quarantine behaviour has to
/// be able to say what quarantined means without copying the number.
pub const MAX_PENDING_ATTEMPTS: u32 = 5;

/// Hard ceiling for the git subprocess. `git` can stall on pathological
/// repositories, network mounts or hook misconfigurations; unbounded, it hung
/// every drain batch and CLI status behind it. On expiry the child is killed
/// and the caller gets an error — `current_git_head`'s callers already treat
/// an unavailable head as "unavailable", so a stalled git degrades honestly
/// instead of wedging the daemon.
const GIT_HEAD_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);

fn run_git_head_with_deadline(program: &str, root: &Path) -> anyhow::Result<String> {
    use std::io::Read;
    use std::process::Stdio;

    let mut child = std::process::Command::new(program)
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "HEAD"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| anyhow::anyhow!("cannot spawn {program}: {error}"))?;

    // Drain both pipes on helper threads: reading them only after exit would
    // deadlock once a pipe buffer filled. Kill on deadline; the readers then
    // see EOF when the child dies.
    let mut stdout_pipe = child.stdout.take().unwrap();
    let mut stderr_pipe = child.stderr.take().unwrap();
    let stdout_reader = std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = stdout_pipe.read_to_string(&mut buf);
        buf
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = stderr_pipe.read_to_string(&mut buf);
        buf
    });

    let started = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if started.elapsed() >= GIT_HEAD_DEADLINE {
                    let _ = child.kill();
                    let _ = child.wait();
                    anyhow::bail!(
                        "{program} rev-parse HEAD exceeded \
                         {GIT_HEAD_DEADLINE:?} and was killed"
                    );
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(error) => anyhow::bail!("{program} rev-parse HEAD failed: {error}"),
        }
    };
    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();
    if !status.success() {
        anyhow::bail!(
            "git rev-parse HEAD failed for {:?}: {}",
            root,
            stderr.trim()
        );
    }
    let head = stdout.trim().to_string();
    if !(7..=64).contains(&head.len()) || !head.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        anyhow::bail!("git returned an invalid HEAD identity for {:?}", root);
    }
    Ok(head)
}

pub fn current_git_head(root: &Path) -> anyhow::Result<String> {
    run_git_head_with_deadline("git", root)
}

/// Whether a pending-queue entry is a control token rather than a path.
///
/// The daemon's git-HEAD sentinel is `"\0devmap:git-head-changed"`. A leading
/// NUL cannot begin a real filesystem path, which makes the namespace safe, and
/// it lets the store recognise the token without depending on `devmap-serve` —
/// which depends on *this* crate, so the reverse edge cannot exist. Control
/// tokens are never path-normalised, never structurally reconciled, and are
/// retired by a full build like any other superseded work.
fn is_control_token(entry: &str) -> bool {
    entry.starts_with('\0')
}

/// What [`canonical_pending_entry`] could establish about a raw queue entry.
///
/// Three states, not two. "Outside the repository" is a positive claim that
/// costs the row its place in the queue, and it must not be the answer given
/// when the containment test itself could not run.
enum PendingEntry {
    /// The canonical repo-relative spelling.
    Canonical(String),
    /// Structurally outside the repository: no retry can change this.
    Outside,
    /// Containment is unknown because `root.canonicalize()` failed — a symlink
    /// loop, a parent that lost `+x`, a stale handle on a network mount. The
    /// caller must not treat this as either answer.
    Undecidable(std::io::Error),
}

/// The canonical pending-queue spelling of `raw` relative to `root`.
///
/// Canonical means: repo-relative, forward slashes, no `.` or `..` components,
/// and `"."` for the root itself. Both queue producers now go through this, so
/// the watcher's absolute paths and the reconcile sweep's relative ones become
/// the same row instead of two rows for one file — see
/// [`Store::enqueue_pending_paths_under_root`].
fn canonical_pending_entry(root: &Path, raw: &str) -> PendingEntry {
    if is_control_token(raw) {
        return PendingEntry::Canonical(raw.to_string());
    }
    // `\` is a path separator on Windows and an ordinary, legal filename
    // character everywhere else. Rewriting it unconditionally renamed the Unix
    // file `a\b.py` to `a/b.py`, which then matched nothing on disk, failed
    // classification, and was deleted from the queue as garbage — the file was
    // never indexed and `status` still reported fresh.
    #[cfg(windows)]
    let normalized = raw.replace('\\', "/");
    #[cfg(windows)]
    let candidate = Path::new(&normalized);
    #[cfg(not(windows))]
    let candidate = Path::new(raw);

    let relative = if candidate.is_absolute() {
        // Compare against the canonical root as well: a symlinked temp
        // directory, or a `.`-rooted daemon, makes the lexical prefix test
        // fail on paths that are genuinely inside the tree.
        match candidate.strip_prefix(root) {
            Ok(stripped) => stripped.to_path_buf(),
            Err(_) => match root.canonicalize() {
                Ok(canonical) => match candidate.strip_prefix(&canonical) {
                    Ok(stripped) => stripped.to_path_buf(),
                    Err(_) => return PendingEntry::Outside,
                },
                // The rescue itself failed, so nothing here has established
                // where the entry lives. `.ok()` used to collapse this into
                // `None`, which the reconcile sweep deletes as a row that
                // escapes the root: a definite verdict from a check that never
                // ran, and the row is gone.
                Err(error) => return PendingEntry::Undecidable(error),
            },
        }
    } else {
        candidate.to_path_buf()
    };

    let mut parts: Vec<String> = Vec::new();
    for component in relative.components() {
        match component {
            std::path::Component::Normal(part) => match part.to_str() {
                Some(text) => parts.push(text.to_string()),
                None => return PendingEntry::Outside,
            },
            std::path::Component::CurDir => {}
            // `..` can only ever climb out of the root from a relative entry,
            // and an absolute entry that needed it was already refused above.
            std::path::Component::ParentDir => return PendingEntry::Outside,
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {
                return PendingEntry::Outside
            }
        }
    }
    PendingEntry::Canonical(if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    })
}

/// Whether a canonical pending entry can ever be processed, and why not.
///
/// `Err(reason)` means no number of retries will help — see
/// [`Store::reconcile_pending_paths`] for what that cost in practice.
fn classify_pending_entry(
    root: &Path,
    canonical: &str,
    indexed: &BTreeSet<String>,
    caches: &mut devmap_extract::CacheDirectoryCache,
) -> std::result::Result<(), String> {
    if canonical == "." {
        // The root itself: a whole-tree rescan the drain expands.
        return Ok(());
    }
    // K7: a path inside a tagged build cache is not source and never will be.
    // Discovery no longer walks these directories, so a queued row naming one
    // can only ever fail — and 47,000 of them were queued from two cargo output
    // trees on this repository before discovery learned to skip them.
    match caches.tagged_ancestor(root, canonical) {
        devmap_extract::CacheVerdict::Inside(cache) => {
            return Err(format!(
                "inside {cache}, a build cache marked with CACHEDIR.TAG"
            ));
        }
        // `canonical_pending_entry` is supposed to have made this repo-relative
        // already, so reaching here means the row was written by something that
        // bypassed it. Unprocessable either way — and now it says which rule the
        // path broke instead of being waved through as "not a build cache".
        devmap_extract::CacheVerdict::NotRepoRelative(why) => {
            return Err(format!("{why}, so it names nothing inside the repository"));
        }
        devmap_extract::CacheVerdict::Outside => {}
    }
    let absolute = root.join(canonical);
    // What this path is, asked at the one owner the *cold walk* asks
    // (`devmap_extract::candidate_kind`). This used to be a second, stricter
    // rule spelled out locally: `symlink_metadata` plus "anything that is
    // neither a plain file nor a plain directory is garbage", which refused
    // every symlink — including the in-repository ones the cold walk indexes.
    // A cold build then indexed `src/util.py -> shared/util.py` and the next
    // drain of that path deleted the queued edit as unprocessable, leaving the
    // stored extraction stale while `status` reported fresh.
    match devmap_extract::candidate_kind(root, &absolute) {
        // The whole-subtree rescan the drain expands.
        devmap_extract::CandidateKind::Directory => Ok(()),
        devmap_extract::CandidateKind::File { bytes } => {
            if bytes > devmap_extract::MAX_SOURCE_BYTES {
                return Err(devmap_extract::DiscoverySkipReason::Oversized {
                    bytes,
                    limit: devmap_extract::MAX_SOURCE_BYTES,
                }
                .to_string());
            }
            if !devmap_extract::is_indexable_source(canonical) {
                return Err(devmap_extract::DiscoverySkipReason::NonSource.to_string());
            }
            Ok(())
        }
        // The walk never descends a link, so the files under the target are
        // indexed under their real names and only under those. Expanding this
        // row would put the same bytes in the graph twice, under a second path
        // no cold build ever produces.
        devmap_extract::CandidateKind::LinkedDirectory => Err(
            "a symlink to a directory, which discovery never descends; the files under it \
             are queued under their own names"
                .to_string(),
        ),
        // A socket, fifo or device. Never a source this build reads.
        devmap_extract::CandidateKind::Other => Err("not a regular file or directory".to_string()),
        // Refused by discovery — a link out of the repository, or one whose
        // target will not resolve to say either way. A `devmap build` writes no
        // rows for it, so there is work here only while the graph still claims
        // one: the drain has to remove it, exactly as a full build's output
        // would. With nothing claimed, the row is garbage and is dropped with
        // discovery's own wording rather than a file-type complaint that sends
        // the reader looking for a socket.
        devmap_extract::CandidateKind::Refused(reason) => {
            if still_claimed(canonical, indexed) {
                Ok(())
            } else {
                Err(reason.to_string())
            }
        }
        devmap_extract::CandidateKind::Absent => {
            // Absent. This is a deletion the drain must process only if the
            // graph still claims the path — or claims something beneath it,
            // which is how a removed directory reaches its indexed children.
            if still_claimed(canonical, indexed) {
                Ok(())
            } else {
                Err("no longer exists under the root and is not in the stored graph".to_string())
            }
        }
        // The stat could not run: ELOOP from a symlink loop in a parent,
        // EACCES from a parent that lost `+x`, EIO or ESTALE from a network
        // mount. None of those is evidence that the file is gone, and this
        // branch's verdict *deletes the row*. Every error used to land here
        // and be read as absence, so a stat that could not run silently
        // discarded queued work while `status` went on reporting fresh.
        //
        // Keep it. A transient failure is retried, and a path that keeps
        // failing is quarantined after `MAX_PENDING_ATTEMPTS`, which is a
        // visible state an operator can act on.
        devmap_extract::CandidateKind::Undecidable(_) => Ok(()),
    }
}

/// Does the stored graph still assert this path, or anything beneath it?
///
/// A removed directory reaches its indexed children through the prefix scan;
/// without it the drain would drop the row and leave the graph describing files
/// that are gone.
fn still_claimed(canonical: &str, indexed: &BTreeSet<String>) -> bool {
    if indexed.contains(canonical) {
        return true;
    }
    let prefix = format!("{canonical}/");
    indexed
        .range(prefix.clone()..)
        .next()
        .is_some_and(|entry| entry.starts_with(&prefix))
}

/// What [`Store::enqueue_pending_paths_under_root`] accepted and refused.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PendingEnqueueReport {
    /// Canonical entries actually queued, deduplicated and sorted.
    pub enqueued: Vec<String>,
    /// `(raw entry, why)` for entries refused as outside the repository. A
    /// refusal is reported rather than dropped: a watcher emitting paths from
    /// outside the tree is a bug in the watcher, and silently swallowing them
    /// is how it stays one.
    pub refused: Vec<(String, String)>,
}

/// What [`Store::convert_page_size`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageSizeConversion {
    pub before: i64,
    pub after: i64,
    /// False when the store was already at the target — reported rather than
    /// inferred from `before == after`, so "already correct" and "rewritten to
    /// the same value" stay distinguishable.
    pub converted: bool,
}

/// What [`Store::reconcile_pending_paths`] found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PendingReconcile {
    /// `(path, why)` for rows deleted as structurally unprocessable.
    pub dropped: Vec<(String, String)>,
    /// `(old spelling, canonical spelling)` for rows normalised in place.
    pub rewritten: Vec<(String, String)>,
    /// Rows left queued because they still name real work.
    pub retained: usize,
}

/// What a committed build proved about the pending queue.
///
/// The distinction is the fix for K1(e2): "which paths did this build write"
/// and "what did this build read" are different questions, and only the second
/// can retire a row that names a directory.
#[derive(Debug, Clone, Copy)]
pub enum PendingSupersede<'a> {
    /// A build with no `--affected` narrowing: it walked the whole tree, so it
    /// answered every request queued at or before the instant it started.
    /// Carries that instant on [`Store::queue_clock_now`]'s clock.
    WholeTreeBuiltAt(f64),
    /// A narrowed build: it read only the paths it was handed, so only those
    /// rows are answered.
    IndexedPaths(&'a [String]),
}

/// One pending row claimed for a drain attempt.
///
/// Carries `queued_at` because that is what makes the acknowledgement safe: a
/// watcher event arriving mid-drain re-enqueues the path with a *new*
/// `queued_at`, so clearing the claim leaves the newer request queued. The
/// previous mechanism used `attempts > 0`, which forced the drain to bump the
/// attempt counter of every path it was about to succeed at — the accounting
/// that made one store-level failure quarantine an entire batch (K1(d)).
#[derive(Debug, Clone, PartialEq)]
pub struct PendingClaim {
    pub path: String,
    pub queued_at: f64,
}

/// Fail closed: poisoned mutex is an error, never a panic.
fn lock_conn(
    mutex: &Mutex<Connection>,
) -> std::result::Result<MutexGuard<'_, Connection>, rusqlite::Error> {
    mutex
        .lock()
        .map_err(|_: PoisonError<MutexGuard<'_, Connection>>| {
            rusqlite::Error::InvalidParameterName(
                "store mutex poisoned — refusing to continue (fail-closed)".into(),
            )
        })
}

pub struct Store {
    conn: Mutex<Connection>,
    /// The file this store was opened from, when it has one.
    ///
    /// Remembered solely so [`Store::lock_writer`] can find the sibling
    /// `.writer.lock`. `None` for an in-memory store, which no other process
    /// can reach and therefore has nothing to serialise against.
    db_path: Option<std::path::PathBuf>,
    /// Whether this process can write the store at all.
    ///
    /// A store on a read-only mount, in a CI cache restored without write
    /// bits, or `chmod 444`'d by an operator is an ordinary store that can be
    /// *read*. SQLite opens such a file read-only without complaint and only
    /// fails at the first write — which, before this flag existed, was a
    /// header rewrite in [`Store::configure_connection`], so every query
    /// refused with "attempt to write a readonly database" and a readable map
    /// looked like no map at all. Recorded once at open so a write can be
    /// refused by name ([`Store::refuse_if_read_only`]) instead of by SQLite
    /// error code.
    read_only: bool,
    /// The latest generation's full edge set, kept for the life of that
    /// generation.
    ///
    /// `latest_edges` re-ran a two-JOIN, fully-ordered scan of every edge on
    /// **every** request. Measured on this repository (71,598 edges, warm
    /// daemon): `impact` and `trace` cost 99.6-160 ms *regardless of `--depth`*
    /// — depth 1, 3 and 8 all landed within noise of each other — because the
    /// cost is the load, not the traversal.
    ///
    /// Caching it also *reduces* memory rather than adding to it, which is the
    /// opposite of what it looks like. The uncached daemon allocated a fresh
    /// 71,598-edge vector per query and did not give the memory back: RSS went
    /// 531.6 MB after startup -> 625.2 MB after 6 queries -> 801.4 MB after 26,
    /// about 10 MB per query of allocator churn. One retained copy replaces an
    /// unbounded series of transient ones.
    ///
    /// Keyed by generation id, so a build that commits a new generation
    /// invalidates it by construction — there is no separate invalidation path
    /// to forget to call. Only the newest generation is held, so the memory is
    /// bounded by one edge set and not by the number of generations retained.
    /// The cached set is unfiltered; `min_confidence` is applied per request
    /// against the same rounding rule the SQL used, so the answer is unchanged.
    edge_cache: Mutex<Option<CachedEdges>>,
    /// Adjacency over the same rows [`Store::edge_cache`] holds, keyed by the
    /// same generation id.
    ///
    /// It shares that `Arc` rather than copying the edge text, so what this
    /// adds is four maps of `u32` ids — see
    /// [`GenerationEdges::adjacency_bytes`]. Without it every graph question
    /// paid for the whole generation before the walk began: the clone out of
    /// the cache, the conversion of every row, and an adjacency map over every
    /// row, for a question whose answer touches a few dozen edges.
    edge_index: Mutex<Option<(u32, std::sync::Arc<GenerationEdges>)>>,
    /// `(generation, node_count, edge_count)` for the generation last asked
    /// about.
    ///
    /// `status` is the cheapest question the kernel answers and it scaled with
    /// the corpus — two `COUNT(*)`s over the generation's whole node and edge
    /// tables, per call, for numbers that cannot change while the generation
    /// stands. Rows are only inserted under a *new* generation id and only
    /// deleted a whole generation at a time, so the id is a complete key: a
    /// memo under it cannot go stale, it can only be replaced by a newer
    /// generation's. Only the newest asked-about generation is held, so this is
    /// three words of memory rather than a map that grows with history.
    generation_counts: Mutex<Option<(u32, usize, usize)>>,
    /// The analysis status of the generation last asked about.
    ///
    /// Immutable for the same reason the counts are — a generation's
    /// `analysis_json` is written once, under a new id — so the id is a
    /// complete key. Reading it at all means going to the summary blob, which
    /// on the ScholarLM corpus is milliseconds; `devmap status` asks for it on
    /// every call and nothing else about it can change.
    generation_analysis_status: Mutex<Option<(u32, AnalysisStatus)>>,
}

/// A held cross-process writer lock on one store (K13).
///
/// Released when dropped — and, because it is an `flock`, also when the holding
/// process dies. That is the whole reason for using one rather than a marker
/// file: a build killed with SIGKILL leaves nothing behind to clean up, whereas
/// a stale marker would wedge every later build until someone deleted it by
/// hand.
///
/// An in-memory store holds `file: None`. That is not a check being skipped: an
/// in-memory database is private to one process and one `Store`, whose own
/// mutex already serialises writers, so there is no second writer for a
/// cross-process lock to exclude.
#[derive(Debug)]
pub struct WriterLock {
    file: Option<std::fs::File>,
    path: Option<std::path::PathBuf>,
}

impl WriterLock {
    /// The lock file backing this guard, or `None` for an in-memory store.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Whether a real cross-process lock is held, as opposed to the in-memory
    /// no-op. Callers that need to *assert* exclusivity ask this rather than
    /// inferring it from the guard's existence.
    pub fn is_held(&self) -> bool {
        self.file.is_some()
    }
}

impl Drop for WriterLock {
    fn drop(&mut self) {
        if let Some(file) = self.file.take() {
            // Explicit rather than relying on close-releases-flock, so the
            // release is a statement in the code and not a side effect of drop
            // order. A failure here is not actionable — the descriptor closes
            // on the next line either way, which releases the lock.
            let _ = file.unlock();
        }
    }
}

/// One generation's edge rows and the evidence tier behind each of them.
///
/// One entry rather than two caches: an evidence tier read from a different
/// generation than the edge it labels is precisely the drift the index already
/// refuses to allow for its coverage disclosure.
type CachedEdges = (
    u32,
    std::sync::Arc<Vec<StoredEdge>>,
    std::sync::Arc<Vec<crate::edge_index::EdgeResolution>>,
);

#[derive(Debug, Clone)]
pub struct StoreStatus {
    pub db_path: String,
    pub latest_generation: Option<u32>,
    pub pending_count: usize,
    pub node_count: usize,
    pub edge_count: usize,
    pub degraded_reason: Option<String>,
    pub quarantined_count: usize,
    /// Up to [`Store::DEGRADED_SAMPLE`] of the quarantined paths, oldest first.
    ///
    /// K1(g): the degraded reason used to be a bare count — "64 path(s)
    /// exceeded the retry threshold" — which tells an operator that something
    /// is stuck and nothing about what. On the store this was measured against,
    /// the 64 were paths under a *previous* location of the repository, a 30 MB
    /// vendored `parser.c` that can never fit under `MAX_SOURCE_BYTES`, and
    /// directories: every one of them diagnosable on sight, and none of them
    /// visible.
    ///
    /// A sample, and labelled as one. `quarantined_count` carries the true
    /// total, because a capped list that reads as the whole set is the failure
    /// this codebase treats as worse than a visible gap.
    pub quarantined_paths: Vec<String>,
    /// What the latest generation could not read, by path.
    ///
    /// `degraded_reason` has always carried the three *numbers* — "2 file(s)
    /// failed to parse, 1 recovered by pattern, 1 refused by discovery" — and
    /// nothing anywhere carried the paths, so an operator could not tell a
    /// correct refusal (a 30.6 MB vendored `parser.c` against a 1 MiB ceiling)
    /// from a broken one without opening the database by hand. Each list is
    /// capped at [`crate::COVERAGE_GAP_SAMPLE`] and carries its own total.
    pub coverage_gaps: CoverageGaps,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalCheckpointMode {
    Truncate,
    Passive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WalCheckpointResult {
    pub mode: WalCheckpointMode,
    pub busy: i64,
    pub log_frames: i64,
    pub checkpointed_frames: i64,
}

/// A caller listing and the unfiltered total it is measured against, from one
/// generation.
///
/// `preview` reports how many callers the confidence floor excluded, as
/// `count_callers_of(.., 0.0) - callers.len()`. Taken from two separate reads
/// those numbers can describe two generations, and the subtraction is
/// saturating — so when the newer generation has fewer callers the difference
/// clamps to zero and `preview` reports that the floor hid nothing. "No
/// ambiguous callers" and "I counted a different corpus" then look identical,
/// which is the reading that lets an edit through as safe.
#[derive(Debug, Clone)]
pub struct CallersPage {
    pub generation: u32,
    /// Callers at or above the requested floor.
    pub callers: Vec<StoredEdge>,
    /// Callers at floor 0.0 — the denominator the floor is measured against.
    pub total_unfiltered: usize,
}

/// The dead-symbol rows and the analysis that qualifies them, from one
/// generation.
///
/// `dead_symbols` attaches a coverage disclosure derived from
/// `AnalysisSummary` to rows read separately. Two reads, two generations: the
/// disclosure could say the corpus was fully covered while the rows came from a
/// generation that was not, which is the exact combination that promotes a
/// finding from "look at this" to "safe to delete".
#[derive(Debug, Clone)]
pub struct DeadPage {
    pub generation: u32,
    pub analysis: Option<AnalysisDisclosure>,
    /// Non-exempt rows, ranked, at most the requested limit.
    pub rows: Vec<DeadSymbolReport>,
    /// Every non-exempt row in this generation, independent of the limit.
    ///
    /// The denominator, and the reason the limit is safe. `Response` carries
    /// `shown + hidden == total` and clients enforce it, so a bounded read that
    /// also shrank the count would not merely under-report — it would turn
    /// "66 of 80,000" into "66 of 66", which is the flattering reading of a
    /// list that was cut off.
    pub total_non_exempt: usize,
}

/// One consistent snapshot of a search: the matching rows, the count they were
/// drawn from, the repo root they resolve against, and the disclosure that says
/// how much of the repository the corpus behind them covers — all from the same
/// generation. See [`Store::search_page`] for why they must travel together.
#[derive(Debug, Clone)]
pub struct SearchPage {
    pub generation: u32,
    pub total: u32,
    pub rows: Vec<StoredSymbol>,
    pub repo_root: Option<String>,
    /// How much of the repository this generation actually read.
    ///
    /// Travels with the rows for the reason [`DeadPage`] states: a disclosure
    /// resolved by a second "latest" read could describe a generation the
    /// answer did not come from, and a coverage claim about the wrong corpus is
    /// worse than none. `None` means the analysis blob could not be read, which
    /// is itself a check that did not run — not a clean corpus.
    pub analysis: Option<AnalysisDisclosure>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoredSymbol {
    pub name: String,
    pub qualified_name: String,
    pub kind: String,
    pub path: String,
    pub span_start: usize,
    pub span_end: usize,
    pub is_exported: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoredEdge {
    pub source_file: String,
    pub target_file: String,
    pub source_symbol: String,
    pub target_symbol: String,
    pub edge_kind: String,
    pub confidence: f32,
    /// `generation_edges.resolution` as stored: the resolver's evidence kind
    /// (`StoredResolutionKind::label`), or `None` on a row written before the
    /// column existed. Decoded by `edge_resolution`, which labels the `None`
    /// case as a reconstruction rather than a reading.
    pub resolution: Option<String>,
}

/// One generation's `paths` rows, ordered once so a path comparison is a `u32`
/// comparison.
///
/// The edge read orders ~100k rows on two path strings. Interning them here
/// costs one scan of a 1,567-row table and turns both keys into ranks whose
/// integer order *is* the byte order of the paths they stand for — see
/// [`edge_read_order`].
struct PathRanks {
    /// Paths in ascending byte order. A rank indexes this.
    ordered: Vec<String>,
    /// `paths.id` to its rank in [`Self::ordered`].
    rank_by_id: std::collections::HashMap<i64, u32>,
}

impl PathRanks {
    fn read(conn: &Connection) -> Result<Self> {
        let mut stmt = conn.prepare("SELECT id, path FROM paths")?;
        let mut rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>>>()?;
        // Byte order, which is what SQLite's default BINARY collation compares
        // and therefore what the `ORDER BY sp.path, tp.path` this replaces was.
        rows.sort_unstable_by(|left, right| left.1.cmp(&right.1));
        let rank_by_id = rows
            .iter()
            .enumerate()
            .map(|(rank, (id, _))| (*id, rank as u32))
            .collect();
        Ok(Self {
            ordered: rows.into_iter().map(|(_, path)| path).collect(),
            rank_by_id,
        })
    }

    /// The rank of a `paths.id`, or an error.
    ///
    /// An edge naming a path row that is not there is a **refusal**, not a
    /// dropped edge. The `INNER JOIN` this replaces answered the same question
    /// by omitting the row, so a store whose `paths` table had lost an entry
    /// answered "nothing depends on this" from a graph it had only partly
    /// read — the same failure `edge_kind_from_stored` refuses for an unknown
    /// kind. `generation_edges.source_file_id` is `REFERENCES paths(id)`, so a
    /// well-formed store cannot reach this.
    fn rank_of(&self, id: i64) -> Result<u32> {
        self.rank_by_id.get(&id).copied().ok_or_else(|| {
            rusqlite::Error::InvalidParameterName(format!(
                "generation edge names path id {id}, which is not in `paths`; \
                 the store is inconsistent and answering over the edges that \
                 remain would be a wrong answer rather than a partial one"
            ))
        })
    }

    fn path_of(&self, rank: u32) -> &str {
        &self.ordered[rank as usize]
    }
}

/// A generation edge before it has been put in read order.
///
/// Holds the ranks rather than the paths, and the `f64` confidence SQLite
/// stored rather than the `f32` [`StoredEdge`] narrows it to, because both are
/// sort keys and both must compare exactly as SQL compared them.
struct UnorderedEdge {
    source_rank: u32,
    target_rank: u32,
    source_symbol: String,
    target_symbol: String,
    edge_kind: String,
    confidence: f64,
    resolution: Option<String>,
    ordinal: u32,
}

/// The order every reader of a generation's edges sees, as one comparator.
///
/// `confidence DESC, source path, target path, source symbol, target symbol,
/// edge kind` — the key `latest_edges_uncached`'s SQL used to hand to SQLite —
/// and then `ordinal`, which SQL had no equivalent of and which makes the tail
/// of the order defined instead of arbitrary. This order is the final
/// tie-break of every answer derived from a walk (R4), so it has exactly one
/// owner.
fn edge_read_order(left: &UnorderedEdge, right: &UnorderedEdge) -> std::cmp::Ordering {
    right
        .confidence
        .total_cmp(&left.confidence)
        .then_with(|| left.source_rank.cmp(&right.source_rank))
        .then_with(|| left.target_rank.cmp(&right.target_rank))
        .then_with(|| left.source_symbol.cmp(&right.source_symbol))
        .then_with(|| left.target_symbol.cmp(&right.target_symbol))
        .then_with(|| left.edge_kind.cmp(&right.edge_kind))
        .then_with(|| left.ordinal.cmp(&right.ordinal))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredFile {
    pub path: String,
    pub language: String,
    pub content_hash: u64,
    pub parse_outcome: ParseOutcome,
    pub engine: ExtractionEngine,
}

#[derive(Debug, Clone, Default)]
pub struct GenerationWriteOpts {
    /// Files whose rows must be rewritten (differential path). Empty = full rewrite of inputs.
    pub affected_paths: Vec<String>,
    /// Files known deleted from the working tree — must not remain as live nodes (N2).
    pub deleted_paths: Vec<String>,
    /// Start of measured build work. The store samples it while persisting the
    /// history row, so the duration includes generation writes. `None` remains
    /// SQL NULL rather than becoming an ambiguous numeric zero.
    pub build_started: Option<std::time::Instant>,
    /// Absolute root the sources were read from. Node paths are stored
    /// repo-relative, so without this a query process resolves them against its
    /// own working directory and every span read from elsewhere comes back
    /// empty. `None` stays NULL — "root unknown", never a wrong root.
    pub repo_root: Option<String>,
    /// Every path discovery refused for this generation, with its verdict.
    ///
    /// The whole inventory, not a delta: the generation stores what it could
    /// not read, and `discovery_refused_files` is `COUNT(*)` over these rows.
    /// The daemon's incremental drain, which never re-walks discovery, builds
    /// it by carrying the previous generation's rows minus every path in this
    /// batch's affected set and adding what this batch was turned away from.
    ///
    /// `None` means *this writer did not measure discovery* — a caller that
    /// supplied its own corpus, which is every test and the single-file preview
    /// path — and is not the same as `Some(vec![])`, a walk that ran and
    /// refused nothing. It is the same distinction
    /// [`devmap_analyze::DiscoveryCoverage::none`] draws, and
    /// `save_generation_with_metadata` refuses a generation whose two halves
    /// disagree about which of them it is.
    pub discovery_refusals: Option<Vec<DiscoveryRefusal>>,
}

/// One committed build, as recorded by [`Store::build_history`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildHistoryRow {
    pub generation_id: u32,
    pub built_at: i64,
    pub head_sha: String,
    pub files: u64,
    pub symbols: u64,
    pub edges: u64,
    pub dead_confident: u64,
    pub dead_ambiguous: u64,
    pub parse_failed: u64,
    pub languages_covered: u64,
    pub build_ms: Option<u64>,
    pub db_bytes: u64,
}

/// What [`Store::vacuum_if_needed`] did, and what it saw when it decided.
///
/// Returned rather than discarded because "declined to reclaim" and "reclaimed
/// nothing" leave an identical database behind, and telling them apart is the
/// difference between a healthy store and one growing forever. That is not
/// hypothetical: a stale freelist read made this function decline eight builds
/// in a row while a third of the file was free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VacuumOutcome {
    pub freelist_before: i64,
    pub page_count_before: i64,
    pub action: VacuumAction,
    /// The WAL checkpoint that decides whether the reclaim reached the file.
    ///
    /// K2: `PRAGMA incremental_vacuum` truncates the database, and in WAL mode
    /// that truncation is a WAL frame like any other — it does not touch the
    /// main file until a checkpoint folds it back. `vacuum_if_needed`
    /// checkpointed only *before* reading the page accounting, so the reclaim
    /// ran, reported pages, and left the file exactly as large as it found it:
    /// measured at 42% freelist, unchanged file size across eight builds, and a
    /// 109 MB WAL.
    ///
    /// `None` means the checkpoint could not be run at all. A `busy` other than
    /// zero means an active reader held the WAL and the truncation is still
    /// pending — the build reports that rather than discarding it, because
    /// "reclaimed and the file shrank" and "reclaimed and nothing moved" are
    /// otherwise indistinguishable from the outside.
    pub checkpoint: Option<WalCheckpointResult>,
    /// Free pages this call actually returned to the end of the file.
    ///
    /// Counted, not assumed. `PRAGMA incremental_vacuum(N)` frees **one page
    /// per row stepped**, and rusqlite 0.31's `execute_batch` steps exactly
    /// once (`lib.rs::execute_batch`: `stmt.step()?`, then straight to the
    /// tail) — so the pragma freed a single page per build while the action
    /// beside it reported the 65,536 it had been asked for. Measured on the
    /// live 701 MB store: four consecutive builds moved the freelist 116,116 ->
    /// 116,045 and the file never left 701 MB. Fully stepping the same pragma
    /// on a copy took the freelist to 49,545 and the file to 429 MB in 4.5 s.
    ///
    /// Carrying the count is what makes the difference visible: a request and a
    /// result that print identically cannot be told apart from a log.
    pub pages_freed: i64,
}

impl VacuumOutcome {
    /// Free pages as a percentage of the file when the decision was made.
    pub fn freelist_ratio(&self) -> f64 {
        if self.page_count_before <= 0 {
            return 0.0;
        }
        self.freelist_before as f64 / self.page_count_before as f64
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VacuumAction {
    /// Below the threshold; nothing worth reclaiming.
    Declined,
    /// Bounded reclaim, asked to move up to `requested` free pages.
    ///
    /// `requested` is the ceiling, not the result: read
    /// [`VacuumOutcome::pages_freed`] for what actually moved. The two were
    /// conflated, and printing the request as though it were the outcome is how
    /// a one-page reclaim reported `incremental(65536 pages)` for four builds
    /// running while the file never shrank.
    Incremental { requested: i64 },
    /// Whole-file rewrite that also converts a legacy store to incremental
    /// mode, so this is the last time that store pays for one.
    FullConverting,
}

impl std::fmt::Display for VacuumAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Declined => write!(f, "declined"),
            Self::Incremental { requested } => write!(f, "incremental(<={requested} pages)"),
            Self::FullConverting => write!(f, "full+convert"),
        }
    }
}

/// Refuse a confidence threshold that no comparison can evaluate.
///
/// `min_confidence` is compared two ways in this file — in SQL for the
/// file-scoped query, and in Rust for the cached whole-generation one — and on
/// NaN they disagree completely rather than at a boundary: Rust admits every
/// edge, SQLite admits none. Whichever answered, the caller could not tell that
/// the filter had not run, because an empty edge list is also what a real
/// filter returns.
///
/// Refused rather than clamped or defaulted. NaN means the caller does not know
/// what it is asking for, and picking a threshold on its behalf publishes a
/// number nobody chose. Infinities are left alone: `>= inf` and `>= -inf` are
/// degenerate but both implementations agree on them, and so is any finite
/// value outside 0.0..=1.0 — an empty answer there is a filter that ran and
/// matched nothing, which is a real result.
/// A `usize` row cap as SQLite's `LIMIT` reads it.
///
/// SQLite takes `LIMIT` as a signed 64-bit value and treats a **negative** one
/// as *unbounded*. `limit as i64` therefore inverts the request for every
/// `usize` at or above `2^63`: `usize::MAX as i64` is `-1`, so a caller asking
/// for the largest cap it can name got no cap at all. Clamping keeps it a cap
/// — the largest one SQLite can express — and a caller that wanted everything
/// still gets everything.
///
/// One owner for the rule. Four bounded readers each carried their own copy of
/// this clamp and a fifth, `latest_unresolved`, was written without it; that is
/// the shape a shared helper exists to prevent.
fn sqlite_limit(limit: usize) -> i64 {
    limit.min(i64::MAX as usize) as i64
}

/// The stored byte span of a symbol row, or a refusal naming the row.
///
/// S-11: three readers decoded the same two columns and two of them disagreed
/// with the third. `search_symbols` errored on a corrupt span while
/// `all_symbols` and `latest_clone_candidates` clamped it with `.max(0)` and
/// published `0..0` — a span that looks real, points at the top of the file,
/// and is indistinguishable from a zero-length symbol at offset 0. One corrupt
/// row therefore made `search` fail closed and the other two lie, which is the
/// exact shape "a check that could not run must not answer like one that ran"
/// exists to forbid. The loud policy wins: a span is a byte range into a file,
/// a negative start or an end before the start is not one, and a fabricated
/// range is worse than a refusal that names the symbol.
fn checked_span(path: &str, name: &str, start: i64, end: i64) -> Result<(usize, usize)> {
    let corrupt = || {
        rusqlite::Error::InvalidParameterName(format!(
            "stored span for symbol {name:?} in {path} is not a byte range: \
             span_start={start}, span_end={end}"
        ))
    };
    if end < start {
        return Err(corrupt());
    }
    let start = usize::try_from(start).map_err(|_| corrupt())?;
    let end = usize::try_from(end).map_err(|_| corrupt())?;
    Ok((start, end))
}

/// Decode the two `generation_files` columns that record how a file was read.
///
/// One owner: [`Store::latest_file`] and the `build_history` parse-failure
/// count both need them, and a quiet decode in either would report a store
/// fault as a fact about the code.
fn decode_stored_outcome(
    path: &str,
    parse_json: &str,
    engine_json: &str,
) -> Result<(ParseOutcome, ExtractionEngine)> {
    let parse_outcome = serde_json::from_str(parse_json).map_err(|error| {
        rusqlite::Error::InvalidParameterName(format!(
            "stored parse outcome for {path} is invalid: {error}"
        ))
    })?;
    let engine = serde_json::from_str(engine_json).map_err(|error| {
        rusqlite::Error::InvalidParameterName(format!(
            "stored extraction engine for {path} is invalid: {error}"
        ))
    })?;
    Ok((parse_outcome, engine))
}

/// Is a stored row a parse failure?
///
/// The same rule `devmap_extract::model::Extraction::is_parse_failure` applies
/// in memory, asked of the two columns that carry it. Rehydrating the whole
/// payload to call the canonical method would mean deserializing every
/// extraction in the generation — measured at 198 MiB on one corpus — to answer
/// a yes/no question, so the *rule* is restated over the stored fields and
/// `the_stored_parse_failure_rule_matches_the_canonical_classifier` fails if
/// the two ever disagree on a real corpus.
#[cfg(feature = "parse")]
fn stored_is_parse_failure(outcome: &ParseOutcome, engine: &ExtractionEngine) -> bool {
    matches!(outcome, ParseOutcome::Failed { .. })
        && !matches!(engine, ExtractionEngine::NotApplicable { .. })
}

/// The FTS5 `MATCH` expression for a user's search string, or an error naming
/// why the store cannot express it.
///
/// The rule is that the *whole* input is one quoted prefix phrase, so FTS5
/// operators, column filters, parentheses, wildcards and hyphens stay data
/// rather than becoming syntax. Doubling interior quotes is the FTS5 escape.
///
/// **An interior NUL breaks that rule, and the escape cannot fix it.** SQLite
/// hands the MATCH argument to FTS5's parser as a C string, so `"alpha\0beta"*`
/// is parsed as `"alpha` — the closing quote is beyond the terminator. The
/// observable result was `unterminated string` raised from inside SQLite: a
/// query surface leaking a parser error for an input the caller was entitled to
/// pass. Silently truncating at the NUL is worse — the search would then run on
/// a prefix of what was asked and report the answer as if it had run on all of
/// it. So the store refuses and says so.
///
/// One function rather than three copies: `search_fts`, `search_symbols` and
/// `count_search_symbols` each carried their own `replace('"', "\"\"")` and
/// their own `format!`, which is why the NUL hole existed in all three and
/// would have been closed in one.
fn fts_match_query(query: &str) -> Result<String> {
    if let Some(offset) = query.find('\0') {
        return Err(rusqlite::Error::InvalidParameterName(format!(
            "search query contains a NUL byte at offset {offset}; SQLite's \
             full-text parser reads the query as a C string, so no escaping \
             can carry one through"
        )));
    }
    Ok(format!("\"{}\"*", query.replace('"', "\"\"")))
}

pub fn checked_min_confidence(value: f32) -> Result<f32> {
    if value.is_nan() {
        return Err(rusqlite::Error::InvalidParameterName(
            "min_confidence must be a number; got NaN, which no confidence \
             comparison can evaluate"
                .to_string(),
        ));
    }
    Ok(value)
}

/// Every table and column this binary's readers and writers address by name.
///
/// S-8: this list is the schema gate, and it was narrower than the schema it
/// claimed to assert — `generation_files.grammar_version`/`analyzer_version`
/// (v8) and `generation_unresolved.classification`/`receiver` (v10/v11) were
/// missing, so a store stamped at the current version without them opened
/// clean and failed at the first *write* instead of at the gate. A gate that
/// passes a store it cannot write to is worse than no gate: it moves the
/// failure from "this store is not usable" to a mid-build error naming a
/// column.
///
/// Completeness is enforced, not asserted:
/// `the_schema_gate_names_every_column_the_current_schema_creates` builds a
/// fresh store and fails if any column of these tables is missing here, so a
/// future migration cannot add a column and silently leave the gate behind.
/// FTS5's *shadow* tables (`nodes_fts_data`, `_idx`, `_content`, `_docsize`,
/// `_config`) are deliberately absent — SQLite owns their layout and it is not
/// this crate's to assert. `nodes_fts` itself is this crate's DDL and is
/// searched by column name, so it is asserted.
/// One stored extraction payload, as `file_payloads` holds it.
///
/// A struct rather than eight positional parameters: five of the eight are
/// `&str`, so a transposed pair would compile and store an engine description
/// in the parse-outcome column. Named fields make that a compile error.
struct StoredPayload<'a> {
    file_id: u32,
    content_hash: i64,
    language: &'a str,
    grammar_version: &'a str,
    analyzer_version: &'a str,
    parse_outcome_json: &'a str,
    engine_json: &'a str,
    extraction_json: &'a str,
}

const REQUIRED_SCHEMA: &[(&str, &[&str])] = &[
    ("paths", &["id", "path"]),
    (
        "generations",
        &["id", "created_at", "head_sha", "analysis_json", "repo_root"],
    ),
    (
        "generation_nodes",
        &[
            "generation_id",
            "ordinal",
            "file_id",
            "name",
            "qualified_name",
            "kind",
            "span_start",
            "span_end",
            "is_exported",
            "body_exact",
            "body_structural",
            "body_nodes",
        ],
    ),
    (
        "generation_files",
        &[
            "generation_id",
            "file_id",
            "language",
            "content_hash",
            "parse_outcome_json",
            "engine_json",
            "extraction_json",
            "grammar_version",
            "analyzer_version",
        ],
    ),
    (
        "generation_edges",
        &[
            "generation_id",
            "ordinal",
            "source_file_id",
            "target_file_id",
            "source_symbol",
            "target_symbol",
            "edge_kind",
            "confidence",
            "resolution",
            "candidate_total",
        ],
    ),
    (
        "generation_coverage_gaps",
        &["generation_id", "gap", "path", "reason"],
    ),
    (
        "generation_unresolved",
        &[
            "generation_id",
            "ordinal",
            "source_file",
            "source_symbol",
            "callee_name",
            "reason",
            "classification",
            "receiver",
        ],
    ),
    (
        "generation_dead_symbols",
        &[
            "generation_id",
            "ordinal",
            "file_path",
            "symbol_name",
            "confidence",
            "is_exempt",
            "exemption_reason",
        ],
    ),
    (
        "extraction_cache",
        &[
            "content_hash",
            "language",
            "grammar_version",
            "analyzer_version",
            "payload_json",
            "accessed_at",
        ],
    ),
    (
        "extraction_retry",
        &[
            "content_hash",
            "language",
            "attempts",
            "last_reason",
            "updated_at",
        ],
    ),
    ("pending_paths", &["path", "queued_at", "attempts"]),
    ("nodes_fts", &["name", "qualified_name", "path"]),
    ("nodes_fts_map", &["rowid_ref", "generation_id"]),
    (
        "build_history",
        &[
            "generation_id",
            "built_at",
            "head_sha",
            "files",
            "symbols",
            "edges",
            "dead_confident",
            "dead_ambiguous",
            "parse_failed",
            "languages_covered",
            "build_ms",
            "db_bytes",
        ],
    ),
];

/// The identity a payload written by *this* build would carry, or `None` when
/// this build cannot know.
///
/// The answer is the compiled grammar versions, so without the `parse` feature
/// there is no answer — not "current" and not "stale", but *unknown*. That
/// distinction is the whole reason this is one function: both callers previously
/// reached straight into `devmap_extract::cache`, which is `#[cfg(feature =
/// "parse")]`, so `--no-default-features` did not compile at all and the
/// feature's own documentation ("Off, this crate builds without tree-sitter and
/// answers questions about a persisted map rather than building one") was false.
/// That configuration is not hypothetical: `devmap-extract/Cargo.toml` records
/// GitPulse linking `devmap-query` to answer impact queries in-process, never
/// indexing, and paying 49 crates and 32 C-compiled grammars for it.
///
/// Neither caller may turn `None` into a match. A payload whose currency was
/// never checked must not be reported as current.
fn current_payload_identity(language: &str) -> Option<(String, String)> {
    #[cfg(feature = "parse")]
    {
        Some(devmap_extract::cache::current_payload_identity(language))
    }
    #[cfg(not(feature = "parse"))]
    {
        let _ = language;
        None
    }
}

impl Store {
    /// Page cache for a write connection, in KiB (negative = KiB, per SQLite).
    ///
    /// 64 MiB against SQLite's 2 MiB default. A generation write is a bulk
    /// insert that revisits index pages across the whole file — at 2 MiB the
    /// working set does not fit and the same pages are read, evicted and read
    /// again for the length of the transaction.
    /// Page size a store created by this code uses. See the pragma in
    /// `configure_connection` for the measurements behind it.
    ///
    /// Public because `devmap repair --page-size` converts an existing store to
    /// it, and a second copy of the number in the CLI is exactly the mirror that
    /// let `VACUUM_MAX_PAGES` drift.
    pub const PAGE_SIZE: i64 = 16384;

    const CACHE_SIZE_KIB: i32 = -65_536;

    /// How long any connection waits for a lock before giving up.
    ///
    /// S-7: `stored_schema_version` opens its own read-only connection and
    /// never passes through [`Self::configure_connection`], so the crate's
    /// contention policy was stated in one place and *inherited* in the other
    /// — rusqlite happens to default to the same five seconds, which is why
    /// the two agree today. An inherited default is not a policy: a
    /// dependency bump that changed it would silently give one reader a
    /// different wait from every other, and nothing would fail. Stated once
    /// and applied at both openers instead.
    const BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

    /// How many quarantined paths [`Store::status`] names in its degraded
    /// reason. Bounded because the reason is a one-line diagnostic, not a
    /// dump — the honest total stays in `quarantined_count`.
    pub const DEGRADED_SAMPLE: usize = 5;

    /// Names bound into one [`Store::callers_of`] statement.
    ///
    /// Each name is a bind parameter, and SQLite's `SQLITE_MAX_VARIABLE_NUMBER`
    /// is 32,766 in the bundled build — so a single generated file with more
    /// changed symbols than that made the statement unpreparable. 512 is far
    /// below that ceiling rather than adjacent to it, because the ceiling is a
    /// compile-time option of whatever SQLite the binary links, and a bound
    /// derived from it would be a bound this crate does not control.
    ///
    /// This bounds the *statement*, not the answer: `callers_of` walks every
    /// chunk and returns the union. A cap on the result would manufacture false
    /// "nothing depends on this" verdicts, which is the one thing this query
    /// must never do.
    pub const MAX_CALLER_BATCH: usize = 512;

    fn configure_connection(conn: &Connection) -> Result<()> {
        conn.busy_timeout(Self::BUSY_TIMEOUT)?;
        conn.pragma_update(None, "foreign_keys", "ON")?;

        // `synchronous = NORMAL`, not the `FULL` default.
        //
        // This is a durability trade and worth stating plainly. Under WAL,
        // NORMAL stops fsync-ing on every commit and syncs at checkpoints
        // instead. The documented consequence is that a power loss or OS crash
        // (**not** a process crash — WAL still recovers from that) can lose the
        // most recent transactions. It cannot corrupt the database; that is the
        // difference between NORMAL and OFF, and why OFF is not used here.
        //
        // Losing the most recent transaction here costs a rebuild, not data.
        // Every row in this store is derived from files in the working tree: a
        // generation that vanishes is recomputed by the next `devmap build`,
        // which is exactly what happens today whenever the extraction schema
        // changes. Paying an fsync per commit to durably persist a cache of
        // something already durable on disk buys nothing.
        // 16 KiB pages, against SQLite's 4 KiB default.
        //
        // `extraction_json` averages 54 KB per file in this repository, which
        // is an overflow chain however it is stored — but the chain is ~14
        // pages at 4 KiB and ~4 at 16 KiB, and every page is a WAL frame that
        // has to be written and then checkpointed back into the database.
        //
        // That is where the write actually goes. A `sample` of the persist
        // phase puts it in `pwrite` (433 samples), WAL checkpoint (392) and
        // `fsync` (207), against `sqlite3BtreeInsert` (131): the cost is pages
        // reaching the disk, not rows being inserted. Fewer, larger pages move
        // the same bytes in fewer frames.
        //
        // Measured on this repository (1,533 files), interleaved, n=7, minimum
        // reported — page size is the only variable:
        //
        //            4 KiB     8 KiB    16 KiB    32 KiB
        //   cold     3.23 s    2.78 s    2.70 s    2.60 s
        //   incr     1.84 s    1.56 s    1.49 s    1.48 s
        //   write    1.02 s    0.81 s    0.74 s    0.79 s
        //   store     285 MB    290 MB    296 MB    312 MB
        //
        // 16 KiB is the knee: 32 KiB buys no more time and costs 9% more
        // store, and the 4% this one costs over the default is paid back in a
        // quarter of the write time.
        //
        // Like `auto_vacuum` below, this only takes on a database with no
        // tables yet — which is why it sits here, before `enable_wal` and
        // `migrate`. An existing 4 KiB store accepts the statement, ignores it,
        // and keeps 4 KiB;
        // `an_existing_small_page_store_opens_and_reads` pins that this is not
        // an error.
        //
        // It does **not** share auto_vacuum's conversion path, and an earlier
        // version of this comment claimed it did. `VACUUM` adopts a pending
        // `auto_vacuum`, but it cannot change `page_size` on a WAL database —
        // SQLite silently leaves the page size alone, which is exactly what
        // makes the wrong claim survive a test that only checks the store still
        // works. Measured: `PRAGMA page_size=16384; VACUUM;` on a 299 MB WAL
        // store returned page_size 4096.
        //
        // Converting an existing store means leaving WAL for the rewrite:
        //
        //     PRAGMA journal_mode=DELETE;
        //     PRAGMA page_size=16384;
        //     VACUUM;
        //     PRAGMA journal_mode=WAL;
        //
        // (2 s on that same store, 299 MB -> 296 MB.) That is deliberately not
        // done automatically: it takes an exclusive lock and drops the database
        // out of WAL for the duration, which is not something to do to somebody
        // else's store as a side effect of opening it. Existing stores keep
        // 4 KiB and keep working; new ones get 16 KiB.
        // `a_plain_vacuum_does_not_convert_an_existing_page_size` pins the
        // half that is easy to get wrong.
        conn.pragma_update(None, "page_size", Self::PAGE_SIZE)?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "cache_size", Self::CACHE_SIZE_KIB)?;
        // Pruning and vacuuming sort large intermediate result sets. On disk
        // those spill to temp files in the filesystem's temp directory, which
        // on this platform is neither the database's filesystem nor necessarily
        // fast.
        conn.pragma_update(None, "temp_store", "MEMORY")?;

        // Incremental auto-vacuum, so reclaim costs what the waste costs rather
        // than what the database costs. See [`Self::vacuum_if_needed`].
        //
        // This only takes effect on a database with no tables yet, which is why
        // it sits in `configure_connection` — called before `migrate` creates
        // the schema. On an existing mode-NONE store the statement is accepted
        // and ignored; that store is converted on its next full vacuum instead.
        // Read before set. Setting `auto_vacuum` rewrites the database header
        // even when the mode is already the one being set — measured with the
        // sqlite3 shell on a `chmod 444` store: every other pragma here is
        // silent, this one fails with "attempt to write a readonly database
        // (8)". A store this process can only read must not be refused by its
        // own open, so the write happens only when the mode actually differs;
        // and a read-only store whose mode differs keeps its mode, because
        // reclaim is the only thing that mode serves and reclaim is a write.
        const INCREMENTAL: i64 = 2;
        let auto_vacuum: i64 = conn.query_row("PRAGMA auto_vacuum", [], |row| row.get(0))?;
        if auto_vacuum != INCREMENTAL && !conn.is_readonly(rusqlite::DatabaseName::Main)? {
            conn.pragma_update(None, "auto_vacuum", "INCREMENTAL")?;
        }
        Ok(())
    }

    /// Put the database into WAL mode, tolerating a concurrent opener (SC28).
    ///
    /// Changing the journal mode needs an exclusive lock, and SQLite returns
    /// `SQLITE_BUSY` for it **without consulting the busy handler** — so the
    /// 5-second `busy_timeout` configured above does not cover this one
    /// statement. Several processes opening a brand-new store at once is
    /// exactly when that happens, and it surfaced as a bare "database is
    /// locked" from four racing builds.
    ///
    /// Losing the race is not an error: the winner sets WAL for everyone. So a
    /// busy result re-reads the mode, and succeeds if the database is already
    /// where it needs to be. Retries are bounded and the final failure is
    /// propagated — falling back to journal mode silently would leave readers
    /// blocking on every write, which is a performance cliff nobody would
    /// attribute to this.
    fn enable_wal(conn: &Connection) -> Result<()> {
        const ATTEMPTS: usize = 10;
        // Switching the journal mode is a write. A read-only store is read in
        // whatever mode it was left in — WAL if the writer finished cleanly,
        // rollback-journal otherwise — and both serve reads; retrying the
        // switch would spend the whole back-off below to report a mode this
        // process could never change.
        if conn.is_readonly(rusqlite::DatabaseName::Main)? {
            return Ok(());
        }
        let mut last: Option<rusqlite::Error> = None;
        for attempt in 0..ATTEMPTS {
            match conn.query_row("PRAGMA journal_mode=WAL", [], |row| row.get::<_, String>(0)) {
                Ok(mode) if mode.eq_ignore_ascii_case("wal") => return Ok(()),
                Ok(mode) => {
                    last = Some(rusqlite::Error::InvalidParameterName(format!(
                        "journal_mode is {mode}, not wal"
                    )));
                }
                Err(error) => last = Some(error),
            }
            // Another connection may have set it already while this one lost
            // the lock race.
            if let Ok(mode) =
                conn.query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))
            {
                if mode.eq_ignore_ascii_case("wal") {
                    return Ok(());
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(20 * (attempt as u64 + 1)));
        }
        Err(last.unwrap_or_else(|| {
            rusqlite::Error::InvalidParameterName("could not enable WAL mode".to_string())
        }))
    }

    fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool> {
        let mut stmt = conn.prepare(&format!("PRAGMA table_info(\"{table}\")"))?;
        let mut names = stmt.query_map([], |row| row.get::<_, String>(1))?;
        names.try_fold(false, |found, name| Ok(found || name? == column))
    }

    /// The id of the stored payload with this identity, inserting it if new.
    ///
    /// Keyed by the **file** plus the four fields the extraction cache keys on.
    ///
    /// `file_id` is in the key and must be: a payload is a serialized
    /// `Extraction`, which carries its own `file_path`, so content-addressing
    /// alone collapses two byte-identical files into one payload and makes both
    /// report the same path. A symlink and its target are byte-identical by
    /// construction, and the end-to-end symlink test caught exactly that on the
    /// first run.
    ///
    /// What B3 deduplicates is the same file, unchanged, across generations —
    /// 1,530 of the 1,530 duplicate rows measured on this repository — so
    /// nothing real is lost by narrowing the key.
    ///
    /// SELECT-then-INSERT rather than an upsert because the unique index is on
    /// COALESCE expressions — `grammar_version` and `analyzer_version` are
    /// nullable and SQLite treats NULLs as distinct inside a UNIQUE index, so a
    /// plain constraint would let identical NULL-version payloads both insert.
    /// The probe uses the same expressions the index does. Safe without a
    /// retry loop: every caller holds the generation write transaction, and the
    /// store has one writer.
    fn ensure_payload_id(tx: &Connection, payload: StoredPayload<'_>) -> Result<i64> {
        let StoredPayload {
            file_id,
            content_hash,
            language,
            grammar_version,
            analyzer_version,
            parse_outcome_json,
            engine_json,
            extraction_json,
        } = payload;
        if let Some(id) = tx
            .prepare_cached(
                "SELECT payload_id FROM file_payloads
                  WHERE file_id = ?1 AND content_hash = ?2 AND language = ?3
                    AND COALESCE(grammar_version, '') = ?4
                    AND COALESCE(analyzer_version, '') = ?5",
            )?
            .query_row(
                params![
                    file_id,
                    content_hash,
                    language,
                    grammar_version,
                    analyzer_version
                ],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
        {
            return Ok(id);
        }
        tx.prepare_cached(
            "INSERT INTO file_payloads
             (file_id, content_hash, language, grammar_version, analyzer_version,
              parse_outcome_json, engine_json, extraction_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?
        .execute(params![
            file_id,
            content_hash,
            language,
            grammar_version,
            analyzer_version,
            parse_outcome_json,
            engine_json,
            extraction_json
        ])?;
        Ok(tx.last_insert_rowid())
    }

    /// Whether `name` exists and is a base table rather than a view.
    ///
    /// The migration chain runs over both a genuine old store and a
    /// freshly-created one carrying the current shape, so a step that is legal
    /// only against a table has to ask. Absent counts as "not a table": a step
    /// guarded by this must be skipped when its target does not exist either.
    fn relation_is_table(conn: &Connection, name: &str) -> Result<bool> {
        let kind: Option<String> = conn
            .query_row(
                "SELECT type FROM sqlite_master WHERE name = ?1",
                params![name],
                |row| row.get(0),
            )
            .optional()?;
        Ok(kind.as_deref() == Some("table"))
    }

    fn validate_schema(conn: &Connection) -> Result<()> {
        for (table, required_columns) in REQUIRED_SCHEMA {
            let object_type: Option<String> = conn
                .query_row(
                    "SELECT type FROM sqlite_master WHERE name = ?1",
                    params![table],
                    |row| row.get(0),
                )
                .optional()?;
            // A view satisfies this contract as fully as a table does, and
            // `generation_files` became one in v17 so that twenty-five read
            // sites could keep asking the same question after its payload moved
            // to a content-addressed table. What this validates is that the
            // *relation* exists and carries the columns readers name — which
            // `PRAGMA table_info` answers for a view exactly as for a table.
            if !matches!(object_type.as_deref(), Some("table") | Some("view")) {
                return Err(rusqlite::Error::InvalidParameterName(format!(
                    "required schema object {table:?} is neither a table nor a view"
                )));
            }

            let mut stmt = conn.prepare(&format!("PRAGMA table_info(\"{table}\")"))?;
            let columns: std::collections::BTreeSet<String> = stmt
                .query_map([], |row| row.get(1))?
                .collect::<Result<_>>()?;
            for column in *required_columns {
                if !columns.contains(*column) {
                    return Err(rusqlite::Error::InvalidParameterName(format!(
                        "required column {table}.{column} is missing"
                    )));
                }
            }
        }
        Ok(())
    }

    /// Refusal text for a store whose schema this binary cannot handle.
    ///
    /// K3: the old messages were `unsupported future schema version 99` and
    /// `unsupported schema version 2` — no store path, no statement of what
    /// this binary supports, and no remedy. An operator with several stores on
    /// disk could not tell which one was refused, and nothing said whether the
    /// fix was to rebuild the kernel or to rebuild the database. Those are
    /// opposite actions and getting them the wrong way round destroys an index.
    fn unsupported_schema(store: &str, found: i32) -> rusqlite::Error {
        let remedy = if found > CURRENT_SCHEMA_VERSION {
            "this devmap binary is older than the store; rebuild it with \
             `cargo build --release -p devmap-cli` or set DEVMAP_BINARY to a newer build"
        } else {
            "run `devmap build` to migrate the store — and check `--db` actually names a \
             devmap store: `.devcouncil/codeintel/index.sqlite` is the Python engine's \
             database (schema 2), not this kernel's"
        };
        rusqlite::Error::InvalidParameterName(format!(
            "devmap store {store}: schema version {found} is not supported by this binary \
             (schema {CURRENT_SCHEMA_VERSION}); {remedy}"
        ))
    }

    /// The schema version stamped on an existing store, without migrating it.
    ///
    /// K3: `Store::open` runs the migration chain under an exclusive
    /// transaction from *every* open, so a read-only command like
    /// `devmap status` silently upgraded the store it was asked to describe.
    /// Opening read-only makes that impossible rather than merely unlikely: the
    /// connection cannot write, so no migration, WAL switch or file creation
    /// can happen behind the question.
    ///
    /// `None` when no store exists at `db_path`. A file that exists but is not
    /// a database is an error, not a `None` — "there is nothing here" and "what
    /// is here is not readable" are different answers.
    pub fn stored_schema_version<P: AsRef<Path>>(db_path: P) -> Result<Option<i32>> {
        let path = db_path.as_ref();
        if !path.is_file() {
            return Ok(None);
        }
        let conn = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        // The same wait every other connection gets. A store locked for a
        // moment — a vacuum, a competing opener — must make `status` and
        // `doctor` wait, not report a failure.
        conn.busy_timeout(Self::BUSY_TIMEOUT)?;
        let version: i32 = match conn.query_row("PRAGMA user_version", [], |row| row.get(0)) {
            Ok(version) => version,
            // A WAL store in a directory this process cannot write: the same
            // shape `Store::open` handles, reached here first because `status`
            // and `doctor` probe the schema before opening.
            Err(error) if Self::directory_refused_the_wal(&error) => {
                let conn = Self::open_immutable(path)?;
                conn.busy_timeout(Self::BUSY_TIMEOUT)?;
                conn.query_row("PRAGMA user_version", [], |row| row.get(0))?
            }
            Err(error) => return Err(error),
        };
        Ok(Some(version))
    }

    /// Whether the migration chain has a path from `version` to
    /// [`CURRENT_SCHEMA_VERSION`].
    ///
    /// The single owner of that question. It was previously implicit in the
    /// shape of [`Self::migrate`] — a version with no `if` arm fell through to
    /// the final equality check — which meant the only way to *ask* was to run
    /// the migration, and running the migration meant having already written to
    /// the file. `Store::open` needs the answer before it writes anything, so
    /// the predicate is stated once and consulted from both places.
    ///
    /// 0 is a store with no schema yet; 1 and 2 are the Python engine's
    /// databases, which this kernel never wrote and cannot read.
    pub fn schema_is_migratable(version: i32) -> bool {
        version == 0 || (3..=CURRENT_SCHEMA_VERSION).contains(&version)
    }

    fn migrate(conn: &mut Connection, store: &str) -> Result<()> {
        let version: i32 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if !Self::schema_is_migratable(version) {
            return Err(Self::unsupported_schema(store, version));
        }
        let mut version = version;
        if version == 0 {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            // SC28: re-read inside the write lock. `version` was sampled before
            // the transaction, so two processes racing to create the same store
            // both observed 0 — the second then reached the unconditional
            // `ADD COLUMN repo_root` below and died with "duplicate column
            // name". `Immediate` serialises the writers but does not make a
            // stale read current, and a fresh store is exactly when a daemon,
            // an editor hook and a manual build are most likely to collide.
            let observed: i32 = tx.query_row("PRAGMA user_version", [], |row| row.get(0))?;
            if observed == 0 {
                tx.execute_batch(CREATE_SCHEMA_V3)?;
                tx.execute_batch(BUILD_HISTORY_TABLE)?;
                // Probed rather than unconditional, for the same reason the
                // v6→v7 step probes: `ADD COLUMN` is not idempotent, so a
                // partially-created store must not make this fatal.
                if !Self::has_column(&tx, "generations", "repo_root")? {
                    tx.execute_batch(MIGRATION_V6_TO_V7)?;
                }
                // A fresh database stamps CURRENT_SCHEMA_VERSION directly and
                // never runs the migration chain, so every table added by a
                // later migration must also be created here.
                tx.execute_batch(UNRESOLVED_TABLE)?;
                tx.execute_batch(COVERAGE_GAPS_TABLE)?;
                Self::validate_schema(&tx)?;
                tx.execute(
                    &format!("PRAGMA user_version = {}", CURRENT_SCHEMA_VERSION),
                    [],
                )?;
                tx.commit()?;
                return Ok(());
            }
            // Another process created the schema while this one waited for the
            // write lock. Continue down the chain from what it actually left,
            // rather than from the stale zero.
            tx.rollback()?;
            if observed > CURRENT_SCHEMA_VERSION {
                return Err(Self::unsupported_schema(store, observed));
            }
            version = observed;
        }
        if version == 3 {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute_batch(CREATE_SCHEMA_V3)?;
            tx.execute_batch(MIGRATION_V3_TO_V4)?;
            tx.execute("PRAGMA user_version = 4", [])?;
            tx.commit()?;
            version = 4;
        }
        if version == 4 {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute_batch(CREATE_SCHEMA_V3)?;
            tx.execute_batch(MIGRATION_V4_TO_V5)?;
            let has_analysis_json = {
                let mut stmt = tx.prepare("PRAGMA table_info(generations)")?;
                let columns = stmt.query_map([], |row| row.get::<_, String>(1))?;
                let mut found = false;
                for column in columns {
                    if column? == "analysis_json" {
                        found = true;
                        break;
                    }
                }
                found
            };
            if !has_analysis_json {
                tx.execute(
                    "ALTER TABLE generations ADD COLUMN analysis_json TEXT NOT NULL
                     DEFAULT '{\"total_files\":0,\"total_symbols\":0,\"total_edges\":0,\"dead_symbols\":[],\"communities\":[],\"status\":\"Ok\"}'",
                    [],
                )?;
            }
            // Stamp exactly 5, never `CURRENT_SCHEMA_VERSION`. Stamping the
            // moving target would mark this database as carrying every later
            // migration's tables while creating none of them.
            tx.execute("PRAGMA user_version = 5", [])?;
            tx.commit()?;
            version = 5;
        }
        if version == 5 {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute_batch(MIGRATION_V5_TO_V6)?;
            tx.execute("PRAGMA user_version = 6", [])?;
            // No validation mid-chain: `validate_schema` asserts the *current*
            // schema, which a v6 database legitimately does not satisfy yet.
            // The end-of-migration check below is the authoritative gate.
            tx.commit()?;
            version = 6;
        }
        if version == 6 {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            // `ADD COLUMN` is not idempotent, and a database can reach this step
            // already carrying the column (a re-stamped user_version, or a fresh
            // create that applied the current schema before migrating). Probe
            // first so re-running the step is safe rather than fatal.
            if !Self::has_column(&tx, "generations", "repo_root")? {
                tx.execute_batch(MIGRATION_V6_TO_V7)?;
            }
            tx.execute("PRAGMA user_version = 7", [])?;
            // No mid-chain validation: `validate_schema` asserts the *current*
            // schema, which a v7 database legitimately does not satisfy yet.
            tx.commit()?;
            version = 7;
        }
        if version == 7 {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            // Same idempotency probe as v7: `ADD COLUMN` is not repeatable, and
            // a database can arrive here already carrying the columns from a
            // fresh create that applied the current schema before migrating.
            if !Self::has_column(&tx, "generation_files", "grammar_version")? {
                tx.execute_batch(MIGRATION_V7_TO_V8)?;
            }
            tx.execute("PRAGMA user_version = 8", [])?;
            // No mid-chain validation, for the same reason as v7 above:
            // `validate_schema` asserts the *current* schema, and a v8 database
            // legitimately does not satisfy it until v9 adds
            // `generation_unresolved`. The final validation below covers it.
            tx.commit()?;
            version = 8;
        }
        if version == 8 {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            // `CREATE TABLE IF NOT EXISTS` is idempotent, so this needs no
            // probe — unlike the ADD COLUMN migrations above.
            tx.execute_batch(MIGRATION_V8_TO_V9)?;
            tx.execute("PRAGMA user_version = 9", [])?;
            // No mid-chain validation: `validate_schema` asserts the *current*
            // schema, and a v9 database legitimately lacks the v10
            // `classification` column until the next step adds it.
            tx.commit()?;
            version = 9;
        }
        if version == 9 {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            // Same idempotency probe as v7/v8: `ADD COLUMN` is not repeatable,
            // and a fresh create applies the current `UNRESOLVED_TABLE`, which
            // already carries the column, before this chain runs.
            if !Self::has_column(&tx, "generation_unresolved", "classification")? {
                tx.execute_batch(MIGRATION_V9_TO_V10)?;
            }
            tx.execute("PRAGMA user_version = 10", [])?;
            // No mid-chain validation: a v10 database legitimately lacks the
            // v11 `receiver` column until the next step adds it.
            tx.commit()?;
            version = 10;
        }
        if version == 10 {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if !Self::has_column(&tx, "generation_unresolved", "receiver")? {
                tx.execute_batch(MIGRATION_V10_TO_V11)?;
            }
            tx.execute("PRAGMA user_version = 11", [])?;
            // No mid-chain validation: a v11 database legitimately lacks the
            // v12 body-signature columns until the next step adds them.
            tx.commit()?;
            version = 11;
        }
        if version == 11 {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if !Self::has_column(&tx, "generation_nodes", "body_exact")? {
                tx.execute_batch(MIGRATION_V11_TO_V12)?;
            }
            tx.execute("PRAGMA user_version = 12", [])?;
            // No mid-chain validation: v13 adds the extraction-cache index
            // below, and the end-of-chain check is the authoritative one.
            tx.commit()?;
            version = 12;
        }
        if version == 12 {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            // `CREATE INDEX IF NOT EXISTS` is idempotent, but it is not legal
            // on a view, and `generation_files` became one in v17. A fresh
            // store applies `CREATE_SCHEMA_V3` — which carries the current
            // shape, as every later migration's probe assumes — and then walks
            // this chain, so this step *does* meet a view and must ask first.
            // The index it creates has a successor there:
            // `idx_file_payloads_identity`, on the same four columns, over one
            // row per distinct payload instead of one per generation and file.
            if Self::relation_is_table(&tx, "generation_files")? {
                tx.execute_batch(MIGRATION_V12_TO_V13)?;
            }
            tx.execute("PRAGMA user_version = 13", [])?;
            // No mid-chain validation: v14 adds the coverage-gap inventory and
            // the edge resolution column below, and the end-of-chain check is
            // the authoritative one.
            tx.commit()?;
            version = 13;
        }
        if version == 13 {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            // `CREATE TABLE IF NOT EXISTS` is idempotent, so this needs no
            // probe — unlike the ADD COLUMN migrations above.
            tx.execute_batch(COVERAGE_GAPS_TABLE)?;
            tx.execute("PRAGMA user_version = 14", [])?;
            // No mid-chain validation: v15 adds the edge resolution column
            // below, and the end-of-chain check is the authoritative one.
            tx.commit()?;
            version = 14;
        }
        if version == 14 {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            // Same idempotency probe as v7/v8/v10/v11: `ADD COLUMN` is not
            // repeatable, and a fresh create applies `CREATE_SCHEMA_V3`, which
            // already carries the column, before this chain runs.
            if !Self::has_column(&tx, "generation_edges", "resolution")? {
                tx.execute_batch(MIGRATION_V14_TO_V15)?;
            }
            tx.execute("PRAGMA user_version = 15", [])?;
            Self::validate_schema(&tx)?;
            tx.commit()?;
            version = 15;
        }
        if version == 15 {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            // Same idempotency probe as v7/v8/v10/v11/v14.
            if !Self::has_column(&tx, "generation_edges", "candidate_total")? {
                tx.execute_batch(MIGRATION_V15_TO_V16)?;
            }
            tx.execute("PRAGMA user_version = 16", [])?;
            Self::validate_schema(&tx)?;
            tx.commit()?;
            version = 16;
        }
        if version == 16 {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            // Not an `ADD COLUMN`, so the idempotency probe is different: the
            // step is complete exactly when `generation_files` has become a
            // view. A fresh create applies `CREATE_SCHEMA_V3`, which already
            // carries the split, before this chain runs.
            let already_split: bool = tx
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master
                      WHERE name = 'generation_files' AND type = 'view'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .map(|count| count > 0)?;
            if !already_split {
                tx.execute_batch(MIGRATION_V16_TO_V17)?;
            }
            tx.execute("PRAGMA user_version = 17", [])?;
            Self::validate_schema(&tx)?;
            tx.commit()?;
            version = 17;
        }
        if version != CURRENT_SCHEMA_VERSION {
            return Err(Self::unsupported_schema(store, version));
        }
        Self::validate_schema(conn)?;
        Ok(())
    }

    pub fn open<P: AsRef<Path>>(db_path: P) -> Result<Self> {
        let path = db_path.as_ref();
        // Before the connection exists: SQLite maps the `-shm` sidecar as it
        // opens a WAL database, with whatever mode the sidecar has, so a
        // repair after `Connection::open` is a repair the connection never
        // sees. See `repair_sidecar_modes`.
        Self::repair_sidecar_modes(path);
        let mut conn = Connection::open(path)?;
        let store = path.display().to_string();

        // Decide whether this binary may touch the file *before* touching it.
        //
        // `enable_wal` used to run first, so pointing any devmap command at a
        // store this kernel cannot read — the Python engine's `index.sqlite` at
        // `user_version = 2` is the live instance PLAN.md §3.1 Class D names —
        // rewrote its header into WAL mode and left `-wal`/`-shm` beside it,
        // and only then printed the refusal. "Refuses rather than degrades on
        // mismatch" is not satisfied by a refusal that has already written.
        //
        // This can only refuse, never admit: `migrate` re-reads the version
        // itself, under the write lock, so a store migrated by another process
        // between these two reads is still handled there.
        let stamped: i32 = match conn.query_row("PRAGMA user_version", [], |row| row.get(0)) {
            Ok(stamped) => stamped,
            // A WAL-mode store in a directory this process cannot write has no
            // `-shm` and no way to create one, so even the first read fails
            // with `SQLITE_READONLY_DIRECTORY`. SQLite's documented answer for
            // that shape is an *immutable* read-only open: nothing can be
            // writing a file in a directory nobody can write to, so the shared
            // memory the WAL index needs can live in this process alone. Only
            // taken for a file that exists — a missing store in a read-only
            // directory is a missing store, and creating one is impossible
            // rather than immutable.
            Err(error) if path.is_file() && Self::directory_refused_the_wal(&error) => {
                conn = Self::open_immutable(path)?;
                conn.query_row("PRAGMA user_version", [], |row| row.get(0))?
            }
            Err(error) => return Err(error),
        };
        if !Self::schema_is_migratable(stamped) {
            return Err(Self::unsupported_schema(&store, stamped));
        }
        let read_only = conn.is_readonly(rusqlite::DatabaseName::Main)?;
        if read_only && stamped != CURRENT_SCHEMA_VERSION {
            // Migration is a write. A read-only store at an older schema can
            // neither be migrated nor, with the columns this kernel reads
            // missing, be answered from; say which, rather than letting the
            // first `ALTER TABLE` report a bare SQLite code.
            return Err(rusqlite::Error::InvalidParameterName(format!(
                "devmap store {store} is read-only and at schema {stamped}, which this kernel \
                 (schema {CURRENT_SCHEMA_VERSION}) would have to migrate before reading; make \
                 it writable and run `devmap build`, or rebuild it elsewhere"
            )));
        }

        Self::configure_connection(&conn)?;
        Self::enable_wal(&conn)?;
        if !read_only {
            Self::migrate(&mut conn, &store)?;
        }
        Ok(Self {
            conn: Mutex::new(conn),
            edge_cache: Mutex::new(None),
            edge_index: Mutex::new(None),
            generation_counts: Mutex::new(None),
            generation_analysis_status: Mutex::new(None),
            db_path: Some(path.to_path_buf()),
            read_only,
        })
    }

    /// `SQLITE_READONLY_DIRECTORY`: the database is read-only because the
    /// directory holding it is, so the `-shm` a WAL read needs cannot be made.
    /// Spelled out because `libsqlite3-sys` exposes the extended codes as bare
    /// integers, and this is the one [`Store::open`] must tell apart from every
    /// other read-only failure.
    const SQLITE_READONLY_DIRECTORY: i32 = 1544;

    fn directory_refused_the_wal(error: &rusqlite::Error) -> bool {
        matches!(
            error,
            rusqlite::Error::SqliteFailure(failure, _)
                if failure.extended_code == Self::SQLITE_READONLY_DIRECTORY
        )
    }

    /// Open `path` read-only and immutable, for a store in a directory this
    /// process cannot write. See the fallback in [`Store::open`].
    fn open_immutable(path: &Path) -> Result<Connection> {
        // A URI filename: `%`, `?` and `#` in the path would be read as URI
        // syntax, so they are percent-encoded — the only three characters the
        // SQLite URI grammar reserves inside the path component.
        let mut encoded = String::with_capacity(path.as_os_str().len() + 8);
        for byte in path.to_string_lossy().bytes() {
            match byte {
                b'%' => encoded.push_str("%25"),
                b'?' => encoded.push_str("%3F"),
                b'#' => encoded.push_str("%23"),
                other => encoded.push(other as char),
            }
        }
        Connection::open_with_flags(
            format!("file:{encoded}?immutable=1"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                | rusqlite::OpenFlags::SQLITE_OPEN_URI
                | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
    }

    /// Whether this store can only be read. See the `read_only` field.
    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// Give a writable store's WAL sidecars the write bit the store has.
    ///
    /// SQLite creates `-wal` and `-shm` with the *database file's* mode. A
    /// read of a `chmod 444` store therefore leaves 444 sidecars behind, and
    /// when the operator later restores the store's write bit the sidecars
    /// keep theirs off — so the next build fails with "attempt to write a
    /// readonly database" against a file that is, by every check the
    /// operator would make, writable. Measured on 2026-09-06: a 444 store
    /// read once, `chmod 644`, then `devmap build` — code 8, `user_version`
    /// unchanged. The sidecars are this kernel's, so their mode is this
    /// kernel's to keep consistent. Best-effort and owner-only: a sidecar
    /// another user owns is left for that user, and the write that follows
    /// reports it.
    #[cfg(unix)]
    fn repair_sidecar_modes(db_path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        const OWNER_WRITE: u32 = 0o200;
        let Ok(own) = std::fs::metadata(db_path) else {
            return;
        };
        // Runs before the connection exists, so "writable" is the store's own
        // owner-write bit: a store without it is read-only and its sidecars
        // are left exactly as SQLite made them.
        if own.permissions().mode() & OWNER_WRITE == 0 {
            return;
        }
        let target = own.permissions().mode() | OWNER_WRITE;
        let name = db_path.as_os_str().to_os_string();
        for suffix in ["-wal", "-shm"] {
            let mut sidecar = name.clone();
            sidecar.push(suffix);
            let sidecar = std::path::PathBuf::from(sidecar);
            let Ok(meta) = std::fs::metadata(&sidecar) else {
                continue;
            };
            let mode = meta.permissions().mode();
            if mode & OWNER_WRITE == 0 {
                let mut permissions = meta.permissions();
                permissions.set_mode(target & 0o7777 | (mode & 0o7777));
                let _ = std::fs::set_permissions(&sidecar, permissions);
            }
        }
    }

    #[cfg(not(unix))]
    fn repair_sidecar_modes(_db_path: &Path) {}

    /// The one place a write against a read-only store is refused, so the
    /// refusal is the same sentence from every writer and names the store
    /// rather than an SQLite error code.
    fn refuse_if_read_only(&self) -> Result<()> {
        if !self.read_only {
            return Ok(());
        }
        let store = self
            .db_path
            .as_deref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| ":memory:".to_string());
        Err(rusqlite::Error::InvalidParameterName(format!(
            "devmap store {store} is read-only: the file or its directory is not writable by \
             this process, so it can be queried but not rebuilt"
        )))
    }

    /// The file this store was opened from, or `None` for an in-memory store.
    ///
    /// `None` is a fact, not a failure: an in-memory database has no path that
    /// could be deleted, moved or locked, so a caller asking "is my store still
    /// there" has its answer.
    pub fn path(&self) -> Option<&Path> {
        self.db_path.as_deref()
    }

    /// Longest a writer waits for another process's writer lock before giving
    /// up and naming the holder.
    ///
    /// A full build of a large repository takes seconds, not minutes, so a
    /// minute is generous headroom rather than a guess. Bounded because an
    /// unbounded wait turns a crashed-but-not-dead holder into a hang with no
    /// diagnostic, which is strictly worse than a refusal that names a pid.
    pub const WRITER_LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(60);

    /// Poll interval while waiting for the writer lock. Short enough that a
    /// released lock is picked up promptly, long enough not to spin a core.
    const WRITER_LOCK_POLL: std::time::Duration = std::time::Duration::from_millis(25);

    /// Transaction behaviour for a generation write.
    ///
    /// K13: `Immediate`, matching the prunes, which already use it and document
    /// why — the write lock is taken at `BEGIN` rather than at whichever
    /// statement first needs it, so two writers queue on the busy handler
    /// instead of discovering the conflict partway through and failing an
    /// upgrade that SQLite does not retry.
    ///
    /// Exposed as a named constant because the effect is not observable: SQLite
    /// offers no way to read a transaction's behaviour back, so the policy is
    /// asserted directly rather than inferred from a race that reproduces only
    /// sometimes. The same reason `should_vacuum` and
    /// `should_retire_for_new_binary` are pure functions.
    pub const GENERATION_TX_BEHAVIOR: TransactionBehavior = TransactionBehavior::Immediate;

    /// Path of the advisory writer lock guarding `db_path`.
    pub fn writer_lock_path(db_path: &Path) -> std::path::PathBuf {
        let mut name = db_path.file_name().map_or_else(
            || std::ffi::OsString::from("devmap-store"),
            |name| name.to_os_string(),
        );
        name.push(".writer.lock");
        match db_path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.join(name),
            _ => std::path::PathBuf::from(name),
        }
    }

    /// Take the cross-process writer lock for the store at `db_path` (K13).
    ///
    /// There was no such lock. Two `devmap build` processes — or a build and
    /// the daemon's drain — raced on SQLite's five-second `busy_timeout` alone,
    /// and the loser surfaced `database is locked` after having already paid
    /// for a full extraction and resolution. That is the worst possible place
    /// to fail: all of the cost, none of the result, and an error message that
    /// names neither the other writer nor anything the caller can do.
    ///
    /// An `flock`, mirroring `protocol::lock_ipc_endpoint`: the kernel releases
    /// it when the holder dies, so no stale-lock cleanup exists to go wrong.
    /// `try_lock` in a bounded poll rather than the blocking `lock`, because a
    /// blocking wait cannot be given a deadline and a writer that hangs forever
    /// behind a wedged peer is not an improvement on one that fails.
    ///
    /// The holder writes its pid into the file, so the timeout can say who.
    pub fn lock_writer_at(db_path: &Path, wait: std::time::Duration) -> anyhow::Result<WriterLock> {
        use std::io::{Seek, Write};

        let lock_path = Self::writer_lock_path(db_path);
        if let Some(parent) = lock_path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|error| {
                anyhow::anyhow!(
                    "cannot create the writer lock {}: {error}; a build needs the store's \
                     directory to be writable, though the store can still be queried",
                    lock_path.display()
                )
            })?;

        Self::poll_writer_lock(|| file.try_lock(), wait, Self::WRITER_LOCK_POLL, &lock_path)?;

        // Record ownership for the *next* waiter's diagnostic. Best-effort: a
        // failure to write the pid does not weaken the lock, it only makes a
        // future timeout less specific.
        let _ = file.set_len(0);
        let _ = file.rewind();
        let _ = write!(file, "{}", std::process::id());
        let _ = file.flush();
        Ok(WriterLock {
            file: Some(file),
            path: Some(lock_path),
        })
    }

    /// The bounded `try_lock` poll behind [`Store::lock_writer_at`].
    ///
    /// Contention and a failed check are different events and must not share
    /// an answer. `Err(_busy)` matched both `TryLockError::WouldBlock` — some
    /// other process holds it, so wait — and `TryLockError::Error` — the lock
    /// call itself failed, so nothing at all is known about ownership. On a
    /// filesystem that does not implement `flock` (ENOLCK, EOPNOTSUPP) the
    /// second is what *every* attempt returns, so a build polled the full
    /// `wait` and then failed with "another devmap writer holds … (pid
    /// unknown)": a definite claim about a process that does not exist, made
    /// by a check that never ran, after a minute spent waiting for it.
    /// `protocol::lock_ipc_endpoint` refuses that collapse for the IPC
    /// endpoint; this is the same policy for the store's writer lock.
    ///
    /// The attempt arrives as a closure so this decision has exactly one
    /// owner and can be driven by a test — no filesystem refuses `flock` on
    /// demand, and an untestable policy is how the collapse survived here
    /// while being explicitly rejected one crate away.
    fn poll_writer_lock<F>(
        mut attempt: F,
        wait: std::time::Duration,
        poll: std::time::Duration,
        lock_path: &Path,
    ) -> anyhow::Result<()>
    where
        F: FnMut() -> std::result::Result<(), std::fs::TryLockError>,
    {
        let deadline = std::time::Instant::now() + wait;
        loop {
            match attempt() {
                Ok(()) => return Ok(()),
                Err(std::fs::TryLockError::WouldBlock) => {
                    if std::time::Instant::now() >= deadline {
                        let owner = Self::writer_lock_holder(lock_path);
                        anyhow::bail!(
                            "another devmap writer holds {lock_path:?} (pid {owner}); \
                             waited {wait:?}. Wait for it to finish, or stop that process."
                        );
                    }
                    std::thread::sleep(poll);
                }
                Err(std::fs::TryLockError::Error(error)) => anyhow::bail!(
                    "the devmap writer lock {lock_path:?} could not be taken: {error}; \
                     ownership is unknown, so no claim is made about another writer"
                ),
            }
        }
    }

    /// The pid a lock holder recorded in its lock file, or `"unknown"`.
    ///
    /// Diagnostic only: the lock is the `flock`, not the file's contents, so
    /// every failure here degrades the message rather than the exclusion.
    fn writer_lock_holder(lock_path: &Path) -> String {
        use std::io::Read;

        let mut holder = String::new();
        std::fs::File::open(lock_path)
            .and_then(|mut handle| handle.read_to_string(&mut holder))
            .ok()
            .map(|_| holder.trim().to_string())
            .filter(|pid| !pid.is_empty())
            .unwrap_or_else(|| "unknown".to_string())
    }

    /// [`Store::lock_writer_at`] for the file this store was opened from.
    ///
    /// An in-memory store returns an unheld guard — see [`WriterLock`]: it is
    /// private to this process and this `Store`, whose mutex already serialises
    /// its writers, so there is no second writer to exclude.
    pub fn lock_writer(&self, wait: std::time::Duration) -> anyhow::Result<WriterLock> {
        match &self.db_path {
            Some(path) => Self::lock_writer_at(path, wait),
            None => Ok(WriterLock {
                file: None,
                path: None,
            }),
        }
    }

    /// Open a store **without creating one**, for read commands.
    ///
    /// `Store::open` uses `Connection::open`, which creates the file — so every
    /// read was also a write. `devmap status` against a repository with no
    /// store left an empty database behind, and that file is what let
    /// `DevMapClient._start_daemon` spawn `devmap serve` on the *next* call,
    /// which built a generation in the background. An identical command then
    /// failed on the first invocation and succeeded on the second: "unavailable"
    /// was a race, not a state.
    ///
    /// A read answers from what exists, or reports that nothing is there. It
    /// does not create the thing it is reading.
    pub fn open_existing<P: AsRef<Path>>(db_path: P) -> Result<Option<Self>> {
        let path = db_path.as_ref();
        if !path.is_file() {
            return Ok(None);
        }
        Ok(Some(Self::open(path)?))
    }

    pub fn open_in_memory() -> Result<Self> {
        let mut conn = Connection::open_in_memory()?;
        Self::configure_connection(&conn)?;
        Self::migrate(&mut conn, ":memory:")?;
        Ok(Self {
            conn: Mutex::new(conn),
            edge_cache: Mutex::new(None),
            edge_index: Mutex::new(None),
            generation_counts: Mutex::new(None),
            generation_analysis_status: Mutex::new(None),
            db_path: None,
            read_only: false,
        })
    }

    pub fn get_or_create_path_id(&self, path: &str) -> Result<u32> {
        let conn = lock_conn(&self.conn)?;
        if let Some(id) = conn
            .query_row(
                "SELECT id FROM paths WHERE path = ?1",
                params![path],
                |row| row.get(0),
            )
            .optional()?
        {
            return Ok(id);
        }
        conn.execute(
            "INSERT OR IGNORE INTO paths (path) VALUES (?1)",
            params![path],
        )?;
        conn.query_row(
            "SELECT id FROM paths WHERE path = ?1",
            params![path],
            |row| row.get(0),
        )
    }

    #[cfg(feature = "parse")]
    /// `ensure_path_id`, memoised for the life of one generation write.
    ///
    /// Path ids are stable within a transaction — `paths` is insert-only here —
    /// so the second lookup of a path can only return what the first did. The
    /// repetition is severe rather than incidental: every edge names a source
    /// and a target file, and a 73,000-edge generation over 1,280 files asks
    /// for ~146,000 ids drawn from 1,280 distinct values. The cache turns that
    /// into 1,280 queries.
    ///
    /// Deliberately scoped to a single call rather than held on `Store`: a
    /// cache outliving its transaction would hand out ids from a write that
    /// rolled back.
    fn ensure_path_id_cached(
        tx: &rusqlite::Transaction<'_>,
        cache: &mut std::collections::HashMap<String, u32>,
        path: &str,
    ) -> Result<u32> {
        if let Some(id) = cache.get(path) {
            return Ok(*id);
        }
        let id = Self::ensure_path_id(tx, path)?;
        cache.insert(path.to_string(), id);
        Ok(id)
    }

    #[cfg(feature = "parse")]
    fn ensure_path_id(tx: &rusqlite::Transaction<'_>, path: &str) -> Result<u32> {
        // `prepare_cached`, not `query_row`/`execute`: those compile the SQL
        // afresh on every call, and this is the most-called statement in the
        // writer — twice per edge, so ~146,000 compilations of two 40-character
        // queries in a single DevCouncil generation.
        let mut select = tx.prepare_cached("SELECT id FROM paths WHERE path = ?1")?;
        if let Some(id) = select
            .query_row(params![path], |row| row.get(0))
            .optional()?
        {
            return Ok(id);
        }
        drop(select);
        tx.prepare_cached("INSERT OR IGNORE INTO paths (path) VALUES (?1)")?
            .execute(params![path])?;
        tx.prepare_cached("SELECT id FROM paths WHERE path = ?1")?
            .query_row(params![path], |row| row.get(0))
    }

    /// Enqueue verbatim. Callers that know the repository root must use
    /// [`Store::enqueue_pending_paths_under_root`] instead.
    ///
    /// Kept as the raw primitive because the queue is also written by tests and
    /// by callers replaying rows that are already canonical. It performs no
    /// normalisation and no containment check, which is exactly what made the
    /// queue rot: see K1 on `enqueue_pending_paths_under_root`.
    pub fn enqueue_pending_paths(&self, paths: &[String]) -> Result<()> {
        self.refuse_if_read_only()?;
        let now = Self::now_secs();
        let conn = lock_conn(&self.conn)?;
        let tx = conn.unchecked_transaction()?;
        for path in paths {
            Self::upsert_pending(&tx, path, now)?;
        }
        tx.commit()
    }

    fn now_secs() -> f64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64()
    }

    fn upsert_pending(tx: &rusqlite::Transaction<'_>, path: &str, now: f64) -> Result<()> {
        tx.prepare_cached(
            "INSERT INTO pending_paths (path, queued_at, attempts) VALUES (?1, ?2, 0)
             ON CONFLICT(path) DO UPDATE SET
               queued_at=excluded.queued_at,
               attempts=0",
        )?
        .execute(params![path, now])?;
        Ok(())
    }

    /// Enqueue changed paths in the queue's canonical form: repo-relative,
    /// forward-slash, deduplicated, and inside `root`.
    ///
    /// K1(a): the queue had two producers writing two different things. The
    /// watcher enqueued **absolute** paths; the connect-time reconcile enqueued
    /// **repo-relative** ones; `enqueue_pending_paths` inserted whichever it was
    /// given, verbatim, with no containment check. Nothing ever reconciled the
    /// two, so a repository that moved on disk left rows naming a directory
    /// that no longer existed — measured on this store as 64 permanently
    /// quarantined rows under `/Users/…/Code/DevCouncil` after the checkout
    /// moved to `/Users/…/Code/devtools/DevCouncil`, which pinned
    /// `devmap status` at `is_fresh=false` forever.
    ///
    /// One canonical form, enforced where rows enter. A path outside `root` is
    /// refused *here*, where the caller can be told, rather than accepted and
    /// then failed forever by a drain that has no way to delete it.
    pub fn enqueue_pending_paths_under_root(
        &self,
        root: &Path,
        paths: &[String],
    ) -> Result<PendingEnqueueReport> {
        let mut report = PendingEnqueueReport::default();
        let mut canonical: BTreeSet<String> = BTreeSet::new();
        let mut caches = devmap_extract::CacheDirectoryCache::default();
        for raw in paths {
            match canonical_pending_entry(root, raw) {
                PendingEntry::Canonical(entry) => {
                    // K7: refuse build caches at the door. The watcher fires on
                    // every write cargo makes into its output directory, and
                    // those events reached this queue as work — 47,000 rows
                    // from `target-serve` and `target-store` on this
                    // repository. Discovery skips the directory, so every one
                    // of those rows was guaranteed to be dropped later or to
                    // index something that is not source.
                    match caches.tagged_ancestor(root, &entry) {
                        devmap_extract::CacheVerdict::Inside(cache) => {
                            report.refused.push((
                                raw.clone(),
                                format!("inside {cache}, a build cache marked with CACHEDIR.TAG"),
                            ));
                            continue;
                        }
                        devmap_extract::CacheVerdict::NotRepoRelative(why) => {
                            report.refused.push((
                                raw.clone(),
                                format!("{why}, so it names nothing inside the repository"),
                            ));
                            continue;
                        }
                        devmap_extract::CacheVerdict::Outside => {}
                    }
                    canonical.insert(entry);
                }
                PendingEntry::Outside => report.refused.push((
                    raw.clone(),
                    format!("outside the repository root {}", root.display()),
                )),
                // Refused either way — an entry with no canonical spelling
                // cannot be queued — but the reason is the one the reader can
                // act on. "Outside the repository root" sends them after the
                // watcher; the truth is that the root could not be read.
                PendingEntry::Undecidable(error) => report.refused.push((
                    raw.clone(),
                    format!(
                        "could not be checked against the repository root {}: {error}",
                        root.display()
                    ),
                )),
            }
        }
        let now = Self::now_secs();
        {
            let conn = lock_conn(&self.conn)?;
            let tx = conn.unchecked_transaction()?;
            for entry in &canonical {
                Self::upsert_pending(&tx, entry, now)?;
            }
            tx.commit()?;
        }
        report.enqueued = canonical.into_iter().collect();
        Ok(report)
    }

    /// Failed drain attempts recorded against `path`, or `None` when it is not
    /// queued. Diagnostic and test-facing: "the queue is stuck" and "the queue
    /// is retrying" look identical from a row count.
    pub fn pending_attempts(&self, path: &str) -> Result<Option<u32>> {
        let conn = lock_conn(&self.conn)?;
        conn.query_row(
            "SELECT attempts FROM pending_paths WHERE path = ?1",
            params![path],
            |row| row.get(0),
        )
        .optional()
    }

    /// Drop pending rows that no amount of retrying can ever process (K1(b)).
    ///
    /// The queue's only deleters were an acknowledgement of *successful* work
    /// and a test-only clear, so a row that could not succeed was retried five
    /// times, quarantined, and then kept forever. The 64 rows measured on this
    /// store were: paths under a previous location of the repository,
    /// directories, `.md`/`.json` files, and a 30 MB vendored `parser.c` that
    /// is over `MAX_SOURCE_BYTES` and therefore could never be extracted by
    /// any number of attempts. None of them was a transient failure; all of
    /// them were structural, and structural failures are deleted, not retried.
    ///
    /// Deletion is **not** applied to a path that is merely absent. A file that
    /// vanished but is still a node in the latest generation is a deletion the
    /// drain has to process, and dropping it would leave the graph asserting a
    /// file that is gone. Only an absent path with nothing indexed under it is
    /// dropped.
    ///
    /// Non-canonical rows are rewritten rather than deleted where they still
    /// name something inside the root, so a queue written by the old absolute
    /// path producer converges instead of being thrown away.
    pub fn reconcile_pending_paths(&self, root: &Path) -> Result<PendingReconcile> {
        let indexed: BTreeSet<String> = self.latest_file_hashes()?.into_keys().collect();
        let rows: Vec<(String, f64, u32)> = {
            let conn = lock_conn(&self.conn)?;
            let mut stmt =
                conn.prepare("SELECT path, queued_at, attempts FROM pending_paths ORDER BY path")?;
            let rows = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect::<Result<Vec<_>>>()?;
            rows
        };

        let mut outcome = PendingReconcile::default();
        // One memo for the whole sweep: 51,136 rows were measured on the live
        // store, and without it each would re-`open` every ancestor's tag.
        let mut caches = devmap_extract::CacheDirectoryCache::default();
        let mut deletes: Vec<String> = Vec::new();
        let mut rewrites: Vec<(String, String, f64, u32)> = Vec::new();
        for (stored, queued_at, attempts) in rows {
            if is_control_token(&stored) {
                outcome.retained += 1;
                continue;
            }
            let canonical = match canonical_pending_entry(root, &stored) {
                PendingEntry::Canonical(entry) => entry,
                PendingEntry::Outside => {
                    deletes.push(stored.clone());
                    outcome.dropped.push((
                        stored,
                        format!("escapes the repository root {}", root.display()),
                    ));
                    continue;
                }
                // The containment test could not run, so this row has not been
                // shown to escape anything. Keeping it costs one non-canonical
                // row until the root is readable again; deleting it on this
                // evidence costs the file.
                PendingEntry::Undecidable(_) => {
                    outcome.retained += 1;
                    continue;
                }
            };
            match classify_pending_entry(root, &canonical, &indexed, &mut caches) {
                Err(reason) => {
                    deletes.push(stored.clone());
                    outcome.dropped.push((stored, reason));
                }
                Ok(()) => {
                    if canonical != stored {
                        rewrites.push((stored, canonical, queued_at, attempts));
                    }
                    outcome.retained += 1;
                }
            }
        }

        if !deletes.is_empty() || !rewrites.is_empty() {
            let mut conn = lock_conn(&self.conn)?;
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            for path in &deletes {
                tx.prepare_cached("DELETE FROM pending_paths WHERE path = ?1")?
                    .execute(params![path])?;
            }
            for (stored, canonical, queued_at, attempts) in &rewrites {
                tx.prepare_cached("DELETE FROM pending_paths WHERE path = ?1")?
                    .execute(params![stored])?;
                // Preserve the work: the row still names real pending work,
                // only under the wrong spelling. `MIN` on attempts so a
                // rewritten row cannot inherit a *worse* history than the
                // canonical row it merges into.
                tx.prepare_cached(
                    "INSERT INTO pending_paths (path, queued_at, attempts) VALUES (?1, ?2, ?3)
                     ON CONFLICT(path) DO UPDATE SET
                       queued_at=MIN(pending_paths.queued_at, excluded.queued_at),
                       attempts=MIN(pending_paths.attempts, excluded.attempts)",
                )?
                .execute(params![canonical, queued_at, attempts])?;
                outcome.rewritten.push((stored.clone(), canonical.clone()));
            }
            tx.commit()?;
        }
        Ok(outcome)
    }

    /// Drop every quarantined row, returning what was dropped (K1(f)).
    pub fn drop_quarantined_pending_paths(&self) -> Result<Vec<String>> {
        let mut conn = lock_conn(&self.conn)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let dropped: Vec<String> = {
            let mut stmt = tx.prepare(
                "SELECT path FROM pending_paths WHERE attempts >= ?1 ORDER BY queued_at, path",
            )?;
            let rows = stmt
                .query_map(params![MAX_PENDING_ATTEMPTS], |row| row.get(0))?
                .collect::<Result<Vec<_>>>()?;
            rows
        };
        tx.execute(
            "DELETE FROM pending_paths WHERE attempts >= ?1",
            params![MAX_PENDING_ATTEMPTS],
        )?;
        tx.commit()?;
        Ok(dropped)
    }

    /// Retire the pending work a committed build has superseded (K1(e)).
    ///
    /// A build that persisted a generation has answered some set of queued
    /// requests. *Which* set is the question `PendingSupersede` answers, and
    /// getting it wrong is how 918 rows survived a full `dev map` on the live
    /// store: the rule used to be "delete rows whose path is in the extraction
    /// set", and a directory is never an extraction. Every one of those 918
    /// rows named a directory, all of them still existed, so the structural
    /// reconcile correctly kept them and `repair --pending` could not touch
    /// them either — `status` simply reported NOT FRESH forever.
    ///
    /// Quarantined rows and control tokens go in both cases: the first are work
    /// five drains could not do and a build has now either done or proved
    /// unnecessary, the second is the daemon's git-HEAD sentinel, which a
    /// generation written at the current HEAD answers by construction.
    pub fn clear_pending_superseded(&self, rule: PendingSupersede<'_>) -> Result<Vec<String>> {
        let indexed: BTreeSet<&str> = match rule {
            PendingSupersede::IndexedPaths(paths) => paths.iter().map(String::as_str).collect(),
            PendingSupersede::WholeTreeBuiltAt(_) => BTreeSet::new(),
        };
        let mut conn = lock_conn(&self.conn)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let rows: Vec<(String, f64, u32)> = {
            let mut stmt = tx.prepare("SELECT path, queued_at, attempts FROM pending_paths")?;
            let rows = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect::<Result<Vec<_>>>()?;
            rows
        };
        let mut cleared = Vec::new();
        for (path, queued_at, attempts) in rows {
            let answered = match rule {
                // The build read the whole tree, so it answered every request
                // that existed when it started — whatever that request named.
                // Strictly `<=` against the *start*, never the finish: a
                // watcher event that arrived while the build was extracting
                // describes an edit the build may not have seen, and deleting
                // it would drop a real change on the floor.
                PendingSupersede::WholeTreeBuiltAt(started) => queued_at <= started,
                // A narrowed build only read what it was told to read.
                PendingSupersede::IndexedPaths(_) => indexed.contains(path.as_str()),
            };
            if answered || attempts >= MAX_PENDING_ATTEMPTS || is_control_token(&path) {
                tx.prepare_cached("DELETE FROM pending_paths WHERE path = ?1")?
                    .execute(params![path])?;
                cleared.push(path);
            }
        }
        tx.commit()?;
        Ok(cleared)
    }

    /// Wall clock in the units `pending_paths.queued_at` is written in.
    ///
    /// Public so a build can stamp "I started here" on the same clock the queue
    /// uses, which is what makes [`PendingSupersede::WholeTreeBuiltAt`]
    /// comparable at all. `Instant` cannot be used: it is monotonic and process
    /// local, while the queue is durable and written by other processes.
    pub fn queue_clock_now() -> f64 {
        Self::now_secs()
    }

    /// Every queued path a drain may still retry, oldest first.
    pub fn get_pending_paths(&self) -> Result<Vec<String>> {
        self.get_pending_paths_limited(usize::MAX)
    }

    /// Return the oldest pending paths, bounded in SQL so a large queue cannot
    /// defeat the daemon's batch limit before application-level truncation.
    pub fn get_pending_paths_limited(&self, limit: usize) -> Result<Vec<String>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let conn = lock_conn(&self.conn)?;
        let mut stmt = conn.prepare(
            "SELECT path FROM pending_paths
             WHERE attempts < ?2
             ORDER BY queued_at ASC, path ASC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![sqlite_limit(limit), MAX_PENDING_ATTEMPTS], |row| {
            row.get(0)
        })?;
        let mut paths = Vec::new();
        for r in rows {
            paths.push(r?);
        }
        Ok(paths)
    }

    /// Claim up to `limit` retryable pending rows for one drain attempt.
    ///
    /// Same selection as [`Store::get_pending_paths_limited`], but each row
    /// carries the `queued_at` it was claimed at so the acknowledgement can be
    /// conditional on the row not having been re-enqueued meanwhile. See
    /// [`PendingClaim`].
    pub fn claim_pending_batch(&self, limit: usize) -> Result<Vec<PendingClaim>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let conn = lock_conn(&self.conn)?;
        let mut stmt = conn.prepare(
            "SELECT path, queued_at FROM pending_paths
             WHERE attempts < ?2
             ORDER BY queued_at ASC, path ASC
             LIMIT ?1",
        )?;
        let rows = stmt
            .query_map(params![sqlite_limit(limit), MAX_PENDING_ATTEMPTS], |row| {
                Ok(PendingClaim {
                    path: row.get(0)?,
                    queued_at: row.get(1)?,
                })
            })?
            .collect::<Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Acknowledge claimed work, leaving anything re-enqueued since the claim.
    ///
    /// The `queued_at` guard replaces the old `attempts > 0` one, which only
    /// worked because the drain bumped the attempt counter of every path in the
    /// batch *before* doing any work — so a single failure in a later,
    /// batch-wide step (a persist, a prune) charged an attempt to all 64 paths
    /// in the batch and five such failures quarantined the lot. See K1(d).
    pub fn clear_claimed_pending_paths(&self, claims: &[PendingClaim]) -> Result<usize> {
        let conn = lock_conn(&self.conn)?;
        let tx = conn.unchecked_transaction()?;
        let mut cleared = 0;
        for claim in claims {
            cleared += tx
                .prepare_cached("DELETE FROM pending_paths WHERE path = ?1 AND queued_at = ?2")?
                .execute(params![claim.path, claim.queued_at])?;
        }
        tx.commit()?;
        Ok(cleared)
    }

    pub fn bump_pending_attempts(&self, paths: &[String]) -> Result<()> {
        let conn = lock_conn(&self.conn)?;
        let tx = conn.unchecked_transaction()?;
        for path in paths {
            tx.execute(
                "UPDATE pending_paths SET attempts = attempts + 1 WHERE path = ?1",
                params![path],
            )?;
        }
        tx.commit()
    }

    pub fn clear_pending_paths(&self, paths: &[String]) -> Result<()> {
        let conn = lock_conn(&self.conn)?;
        let tx = conn.unchecked_transaction()?;
        for path in paths {
            tx.execute("DELETE FROM pending_paths WHERE path = ?1", params![path])?;
        }
        tx.commit()
    }

    /// Generation-scoped FTS rowid: high 32 bits = generation, low 32 = ordinal.
    fn fts_rowid(gen_id: u32, node_ord: u32) -> i64 {
        ((gen_id as i64) << 32) | (node_ord as i64)
    }

    /// Writes a generation. Part of the build path, so it needs the parsing
    /// frontend's grammar-identity stamps and is gated with it.
    #[cfg(feature = "parse")]
    pub fn save_generation(
        &self,
        extractions: &[Extraction],
        resolution: &ResolutionResult,
        analysis: &AnalysisSummary,
    ) -> Result<u32> {
        self.save_generation_with_opts(
            extractions,
            resolution,
            analysis,
            GenerationWriteOpts::default(),
        )
    }

    /// Differential membership write with deletion reconciliation (B3 + N2).
    ///
    /// Steps:
    /// 1. Carry forward prior-generation rows whose source file is not in affected∪deleted
    /// 2. Insert freshly resolved rows for affected (from `extractions`)
    /// 3. Deleted paths contribute zero rows (explicit absence — N2)
    #[cfg(feature = "parse")]
    pub fn save_generation_with_opts(
        &self,
        extractions: &[Extraction],
        resolution: &ResolutionResult,
        analysis: &AnalysisSummary,
        opts: GenerationWriteOpts,
    ) -> Result<u32> {
        self.save_generation_with_metadata(extractions, resolution, analysis, opts, "unknown")
    }

    #[cfg(feature = "parse")]
    pub fn save_generation_with_metadata(
        &self,
        extractions: &[Extraction],
        resolution: &ResolutionResult,
        analysis: &AnalysisSummary,
        opts: GenerationWriteOpts,
        head_sha: &str,
    ) -> Result<u32> {
        self.refuse_if_read_only()?;
        if head_sha.is_empty() || head_sha.len() > 128 || head_sha.chars().any(char::is_whitespace)
        {
            return Err(rusqlite::Error::InvalidParameterName(
                "head_sha must be non-empty, whitespace-free, and at most 128 characters".into(),
            ));
        }
        let mut unique_paths = std::collections::BTreeSet::new();
        for extraction in extractions {
            if !unique_paths.insert(extraction.file_path.as_str()) {
                return Err(rusqlite::Error::InvalidParameterName(format!(
                    "duplicate extraction path in generation input: {}",
                    extraction.file_path
                )));
            }
        }
        let mut conn = lock_conn(&self.conn)?;
        let tx = conn.transaction_with_behavior(Self::GENERATION_TX_BEHAVIOR)?;
        // One path-id memo for the whole generation write. See
        // `ensure_path_id_cached`: the edge loop alone asks for two ids per
        // edge drawn from a file set two orders of magnitude smaller.
        let mut path_ids: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        let durable_analysis = analysis.clone();
        // Keep the summary semantically complete even though dead rows also
        // have a normalized table. An authoritative-looking empty list makes
        // latest_analysis() disagree with latest_dead_symbols().
        let analysis_json = serde_json::to_string(&durable_analysis).map_err(|error| {
            rusqlite::Error::InvalidParameterName(format!("analysis serialization failed: {error}"))
        })?;

        tx.execute(
            "INSERT INTO generations (created_at, head_sha, analysis_json, repo_root)
             VALUES (?1, ?2, ?3, ?4)",
            params![now, head_sha, analysis_json, opts.repo_root],
        )?;
        let gen_id: u32 = tx.last_insert_rowid() as u32;

        let prev_gen: Option<u32> = tx
            .query_row(
                "SELECT id FROM generations WHERE id < ?1 ORDER BY id DESC LIMIT 1",
                params![gen_id],
                |row| row.get(0),
            )
            .optional()?;

        let affected: std::collections::HashSet<String> =
            opts.affected_paths.iter().cloned().collect();
        let deleted: std::collections::HashSet<String> =
            opts.deleted_paths.iter().cloned().collect();
        let full_rewrite = affected.is_empty() && deleted.is_empty();

        // Which prior rows may be reused at all.
        //
        // "Unaffected" used to be the whole test, and unaffected meant only
        // "content hash unchanged". That is not enough to make a stored payload
        // reusable: it must also have been produced by the extractor and
        // grammar this build is running. The extraction *cache* has always
        // known that — its key carries both versions — but the generation
        // carry-forward did not, so after two schema bumps DevCouncil's store
        // still held 1,152 `extract-v23` rows under a `v25` binary, and the
        // first changed build was refused by the edge/analysis equality below
        // (65,615 stored against 65,798 analysed) with no way forward but
        // deleting the database.
        //
        // Same three fields the cache keys on, asked of the same owner, so the
        // two cannot drift: content hash, grammar version, analyzer version.
        // A NULL version is a row from before those columns existed — unknown
        // identity is not a matching identity, so it is not reused.
        let current_hashes: std::collections::HashMap<&str, i64> = extractions
            .iter()
            .map(|ext| (ext.file_path.as_str(), ext.content_hash as i64))
            .collect();
        let mut carry: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut stale_identity: Vec<String> = Vec::new();
        if let Some(prev) = prev_gen {
            if !full_rewrite {
                let mut stmt = tx.prepare(
                    "SELECT p.path, f.language, f.content_hash, f.grammar_version, f.analyzer_version
                     FROM generation_files f
                     JOIN paths p ON p.id = f.file_id
                     WHERE f.generation_id = ?1",
                )?;
                let rows = stmt.query_map(params![prev], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                })?;
                for row in rows {
                    let (path, language, content_hash, grammar, analyzer) = row?;
                    if deleted.contains(&path) || affected.contains(&path) {
                        continue;
                    }
                    // `None` (no parsing frontend) is deliberately not a
                    // match: carrying a row forward on an identity this build
                    // could not compute would claim a currency nothing checked.
                    // Not carrying is merely conservative.
                    let identity_matches = current_payload_identity(&language).is_some_and(
                        |(current_grammar, current_analyzer)| {
                            grammar.as_deref() == Some(current_grammar.as_str())
                                && analyzer.as_deref() == Some(current_analyzer.as_str())
                        },
                    );
                    // A content hash that moved without the path being declared
                    // affected means the caller's affected set is wrong; the
                    // stored payload describes different bytes either way.
                    let content_matches = current_hashes
                        .get(path.as_str())
                        .is_none_or(|hash| *hash == content_hash);
                    if identity_matches && content_matches {
                        carry.insert(path);
                    } else {
                        stale_identity.push(path);
                    }
                }
            }
        }
        // A stale path this write cannot replace would simply vanish from the
        // generation — the file silently absent from the map rather than out of
        // date. Refused loudly instead, naming the remedy, because the callers
        // that can rebuild it (the CLI's cold-build closure, the daemon's
        // full resync) both check the identity first and never reach here.
        let unreplaceable: Vec<&String> = stale_identity
            .iter()
            .filter(|path| !current_hashes.contains_key(path.as_str()))
            .collect();
        if !unreplaceable.is_empty() {
            return Err(rusqlite::Error::InvalidParameterName(format!(
                "cannot carry forward {} file(s) whose stored payload was produced by a different \
                 extractor or grammar (for example {}); rebuild this generation from a full \
                 extraction rather than a differential write",
                unreplaceable.len(),
                unreplaceable[0]
            )));
        }

        // Carrying a payload forward is now a `payload_id`, not a payload.
        //
        // This block used to SELECT each unaffected file's row — language,
        // hashes, and a `parse_outcome_json`, `engine_json` and
        // `extraction_json` averaging 53.7 KB together — into Rust and INSERT
        // it back under the new generation id. Measured on this repository, a
        // one-line edit to one file moved **~82 MB of JSON** that way, and left
        // 1,530 byte-identical duplicate rows behind. Since v17 the bytes live
        // once in `file_payloads` keyed by the identity the extraction cache
        // already uses, and a carried file is a 16-byte membership row.
        //
        // One statement, executed inside SQLite, rather than a loop: there is
        // nothing for Rust to decide here — `carry` has already decided it —
        // and a round trip per file was the whole cost.
        if let Some(prev) = prev_gen {
            if !full_rewrite && !carry.is_empty() {
                let mut stmt = tx.prepare(
                    "INSERT INTO generation_file_rows (generation_id, file_id, payload_id)
                     SELECT ?1, m.file_id, m.payload_id
                       FROM generation_file_rows m
                       JOIN paths p ON p.id = m.file_id
                      WHERE m.generation_id = ?2 AND p.path = ?3",
                )?;
                for path in &carry {
                    stmt.execute(params![gen_id, prev, path])?;
                }
            }
        }

        for extraction in extractions {
            // Not "is it affected" but "was it carried". They differ exactly
            // when a prior payload failed the identity gate: the file is
            // unaffected, nothing was carried for it, and its fresh rows are
            // the only ones this generation will have.
            if !full_rewrite && carry.contains(&extraction.file_path) {
                continue;
            }
            if deleted.contains(&extraction.file_path) {
                continue;
            }
            // SQLite has no unsigned integer type. Preserve all 64 bits using
            // the same two's-complement representation as the extraction cache.
            let content_hash = extraction.content_hash as i64;
            let parse_json = serde_json::to_string(&extraction.parse_outcome).map_err(|error| {
                rusqlite::Error::InvalidParameterName(format!(
                    "parse outcome serialization failed for {}: {error}",
                    extraction.file_path
                ))
            })?;
            let engine_json = serde_json::to_string(&extraction.engine).map_err(|error| {
                rusqlite::Error::InvalidParameterName(format!(
                    "extraction engine serialization failed for {}: {error}",
                    extraction.file_path
                ))
            })?;
            let mut durable_extraction = extraction.for_durable_store();
            durable_extraction.source_code = None;
            let extraction_json = serde_json::to_string(&durable_extraction).map_err(|error| {
                rusqlite::Error::InvalidParameterName(format!(
                    "extraction serialization failed for {}: {error}",
                    extraction.file_path
                ))
            })?;
            let file_id = Self::ensure_path_id_cached(&tx, &mut path_ids, &extraction.file_path)?;
            // The identity this payload was produced with, so a stored row is
            // usable as a cache fallback without discarding the staleness
            // guarantee the cache key exists to enforce (SC8). Since v17 it is
            // also the payload's own key.
            let identity = devmap_extract::cache::CacheKey::for_extraction(extraction);
            let payload_id = Self::ensure_payload_id(
                &tx,
                StoredPayload {
                    file_id,
                    content_hash,
                    language: &extraction.language,
                    grammar_version: &identity.grammar_version,
                    analyzer_version: &identity.analyzer_version,
                    parse_outcome_json: &parse_json,
                    engine_json: &engine_json,
                    extraction_json: &extraction_json,
                },
            )?;
            tx.execute(
                "INSERT INTO generation_file_rows (generation_id, file_id, payload_id)
                 VALUES (?1, ?2, ?3)",
                params![gen_id, file_id, payload_id],
            )?;
        }

        let mut node_ord: u32 = 0;

        // Carry forward unchanged files from previous generation (differential).
        if let Some(prev) = prev_gen {
            if !full_rewrite {
                // The signature columns are carried with the row. Dropping
                // them here would make every unchanged file look unsigned after
                // one incremental build, and a clone report reads unsigned as
                // "not examined" — so the whole tree would quietly go dark
                // except the handful of files that happened to be edited.
                let mut stmt = tx.prepare(
                    "SELECT p.path, n.name, n.qualified_name, n.kind, n.span_start, n.span_end, n.is_exported,
                            n.body_exact, n.body_structural, n.body_nodes
                     FROM generation_nodes n
                     JOIN paths p ON p.id = n.file_id
                     WHERE n.generation_id = ?1",
                )?;
                let rows = stmt.query_map(params![prev], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, Option<i64>>(7)?,
                        row.get::<_, Option<i64>>(8)?,
                        row.get::<_, Option<i64>>(9)?,
                    ))
                })?;
                for row in rows {
                    let (path, name, qn, kind, start, end, exported, b_exact, b_struct, b_nodes) =
                        row?;
                    if !carry.contains(&path) {
                        continue;
                    }
                    let file_id = Self::ensure_path_id_cached(&tx, &mut path_ids, &path)?;
                    tx.prepare_cached(
                        "INSERT INTO generation_nodes (generation_id, ordinal, file_id, name, qualified_name, kind, span_start, span_end, is_exported, body_exact, body_structural, body_nodes)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                    )?
                    .execute(params![
                        gen_id, node_ord, file_id, name, qn, kind, start, end, exported, b_exact,
                        b_struct, b_nodes
                    ])?;
                    let fts_rowid = Self::fts_rowid(gen_id, node_ord);
                    tx.prepare_cached(
                        "INSERT INTO nodes_fts (rowid, name, qualified_name, path) VALUES (?1, ?2, ?3, ?4)",
                    )?
                    .execute(params![fts_rowid, name, qn, path])?;
                    tx.prepare_cached(
                        "INSERT INTO nodes_fts_map (rowid_ref, generation_id) VALUES (?1, ?2)",
                    )?
                    .execute(params![fts_rowid, gen_id])?;
                    node_ord += 1;
                }
            }
        }

        // Insert fresh rows for every extraction whose file was not carried.
        for ext in extractions {
            if !full_rewrite && carry.contains(&ext.file_path) {
                continue;
            }
            if deleted.contains(&ext.file_path) {
                continue;
            }
            let file_id = Self::ensure_path_id_cached(&tx, &mut path_ids, &ext.file_path)?;
            for sym in &ext.symbols {
                tx.execute(
                    "INSERT INTO generation_nodes (generation_id, ordinal, file_id, name, qualified_name, kind, span_start, span_end, is_exported, body_exact, body_structural, body_nodes)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                    params![
                        gen_id,
                        node_ord,
                        file_id,
                        sym.name,
                        sym.qualified_name,
                        sym.kind.as_str(),
                        sym.span.start_byte,
                        sym.span.end_byte,
                        sym.is_exported as i32,
                        // SQLite integers are signed. The cast is bit-preserving
                        // and reversed on read, so the stored value round-trips
                        // even though half the hash space reads back negative.
                        sym.body_signature.map(|s| s.exact as i64),
                        sym.body_signature.map(|s| s.structural as i64),
                        sym.body_signature.map(|s| i64::from(s.nodes))
                    ],
                )?;
                let fts_rowid = Self::fts_rowid(gen_id, node_ord);
                tx.prepare_cached(
                    "INSERT INTO nodes_fts (rowid, name, qualified_name, path) VALUES (?1, ?2, ?3, ?4)",
                )?
                .execute(params![fts_rowid, sym.name, sym.qualified_name, ext.file_path])?;
                tx.prepare_cached(
                    "INSERT INTO nodes_fts_map (rowid_ref, generation_id) VALUES (?1, ?2)",
                )?
                .execute(params![fts_rowid, gen_id])?;
                node_ord += 1;
            }
        }

        // Edges come from this build's resolution, always — never from the
        // previous generation.
        //
        // Carrying them forward was sound only while a changed build resolved
        // just the changed files. It no longer does: the build resolves the
        // whole tree so that the analysis means the same thing on both paths,
        // which means `resolution.edges` already holds the current, correct
        // edge for every file, unaffected ones included. Copying the prior
        // generation's rows over the top of that was not a saving — it read
        // rows and re-inserted the same number — it was only a way to keep an
        // older answer.
        //
        // And the answer did drift, in two ways the affected-set closure
        // cannot see. A payload produced by an older extractor stayed until its
        // file's bytes changed. An edge from an unchanged file into a target
        // whose *identity* moved without its name changing — a Go package
        // renamed, an import alias repointed — resolves differently today while
        // the source file itself never entered the affected set. Both showed up
        // as the same symptom: the equality below refusing the write.
        //
        // Writing every resolved edge makes that equality true by construction
        // rather than by argument. It stays below as a regression check.
        let mut edge_ord: u32 = 0;

        for edge in &resolution.edges {
            // Deleted paths are not extracted, so a resolution over the current
            // tree has no edge touching one. Kept as an explicit guard for
            // callers that pass a resolution computed before the deletion.
            if deleted.contains(&edge.source_file) || deleted.contains(&edge.target_file) {
                continue;
            }
            let src_f_id = Self::ensure_path_id_cached(&tx, &mut path_ids, &edge.source_file)?;
            let tgt_f_id = Self::ensure_path_id_cached(&tx, &mut path_ids, &edge.target_file)?;
            // `prepare_cached` so this 8-parameter INSERT is compiled once per
            // transaction rather than once per edge. It is the single
            // highest-frequency statement in the writer: one execution for
            // every resolved edge, 73,000 of them in a DevCouncil generation.
            tx.prepare_cached(
                "INSERT INTO generation_edges (generation_id, ordinal, source_file_id, target_file_id, source_symbol, target_symbol, edge_kind, confidence, resolution, candidate_total)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )?
            .execute(
                params![
                    gen_id,
                    edge_ord,
                    src_f_id,
                    tgt_f_id,
                    edge.source_symbol,
                    edge.target_symbol,
                    format!("{:?}", edge.edge_kind),
                    edge.confidence.persist_real(),
                    // The evidence tier, so the read path does not have to
                    // guess it back out of the row's file layout. NULL only for
                    // an edge built without a resolution at all, which the
                    // resolver never produces — `ResolvedEdge::new` takes one —
                    // and which the read path therefore reports as
                    // `ResolutionSource::Reconstructed`.
                    edge.resolution
                        .as_ref()
                        .map(|resolution| crate::edge_index::resolution_kind_label(resolution)),
                    // How many candidates the ambiguous rung actually weighed,
                    // which since `AMBIGUOUS_FANOUT_CAP` is no longer the number
                    // of rows this site produces. NULL for every other rung: a
                    // resolution that names one target has no candidate list,
                    // and writing 1 there would make a certain edge look like a
                    // one-candidate ambiguity.
                    crate::edge_index::ambiguous_candidate_total(edge.resolution.as_deref()),
                ],
            )?;
            edge_ord += 1;
        }

        // The analysis must have been computed over the edge set being stored.
        //
        // These two numbers come from different places: `edge_ord` counts the
        // rows this generation will hold, while `total_edges` is what the
        // analyser actually saw. Every consumer of `dead_symbols` and
        // `communities` assumes they are the same set. They once were not. A
        // build that resolved only the changed files handed the analyser 63 of
        // 15,017 edges and committed a generation with 433 dead-code candidates
        // instead of 14; the graph was intact and only the analysis of it was
        // wrong, so nothing failed and `devmap dead` reported plainly-called
        // symbols as callerless.
        //
        // Now that every resolved edge is stored, agreement is structural: both
        // sides count the same `resolution.edges`. The check stays because it
        // costs one comparison and it is the thing that caught the carry-forward
        // drift — a generation whose stored edges came from an older extractor
        // than its analysis. It should now be unfailable; if it ever fires
        // again, a *new* asymmetry has been introduced between what this
        // function stores and what the caller analysed.
        //
        // Deletions are covered too, rather than exempted. The worry was that
        // `--deleted` drops rows the analyser had counted, but it cannot: a
        // deleted file is not extracted, so a resolution over the current tree
        // has no edge touching it, and the carried-forward rows that did are
        // dropped on both sides of this equality. Checked as well as argued —
        // 30 randomised deletion builds (6–25 files, up to a third removed)
        // held it exactly. Exempting the case would have left the watcher, the
        // most frequent writer of all, unguarded precisely when it deletes.
        if edge_ord as usize != analysis.total_edges {
            return Err(rusqlite::Error::InvalidParameterName(format!(
                "generation would store {edge_ord} edges but its analysis was computed over {}; \
                 dead-code and community results would describe a different graph than the one stored",
                analysis.total_edges
            )));
        }

        // The inventory of what this generation could not read.
        //
        // Two halves with different provenance and one rule. The extraction
        // gaps are derived here, from the same `extractions` slice the caller
        // analysed, through `devmap_analyze::extraction_gaps` — the owner
        // `extraction_coverage` folds — so a stored path list and the counts in
        // `AnalysisStatus` cannot describe different files. The discovery
        // refusals cannot be derived from anything: a refused file has no
        // `Extraction` at all, so they arrive on `opts` from whoever walked the
        // tree.
        //
        // Deleted paths are excluded on both halves. A file the caller is
        // removing from the generation must not leave a coverage row behind
        // claiming the graph is missing something it no longer contains.
        let mut gap_rows: Vec<(String, String, String)> = Vec::new();
        // The extraction gaps carry forward exactly as the file rows above do,
        // and for the same reason: a differential write is handed only the
        // extractions it re-read, so deriving the whole inventory from them
        // would drop every gap in a file this batch did not touch. Skipping
        // `affected` and `deleted` is what lets a file that used to fail to
        // parse leave the list on the build that parses it.
        if let Some(prev) = prev_gen {
            if !full_rewrite {
                let mut stmt = tx.prepare(
                    "SELECT gap, path, reason FROM generation_coverage_gaps
                     WHERE generation_id = ?1 AND gap != ?2",
                )?;
                let rows = stmt.query_map(
                    params![prev, crate::coverage::GAP_DISCOVERY_REFUSED],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )?;
                for row in rows {
                    let (gap, path, reason) = row?;
                    if deleted.contains(&path) || affected.contains(&path) {
                        continue;
                    }
                    gap_rows.push((gap, path, reason));
                }
            }
        }
        for entry in devmap_analyze::extraction_gaps(extractions) {
            if deleted.contains(&entry.path) {
                continue;
            }
            gap_rows.push((entry.gap.label().to_string(), entry.path, entry.reason));
        }
        // The refusal half is never carried forward here. It cannot be: a
        // refused path has no `Extraction`, so this function has no way to tell
        // a path the caller re-decided from one it never looked at. The caller
        // that walked the tree owns that decision — the cold walk replaces the
        // inventory outright, the drain carries it minus its affected set — and
        // hands the whole answer down.
        //
        // Deleted paths are *not* excluded. A containment refusal deliberately
        // deletes the path's rows while charging the refusal to coverage; that
        // is the drain agreeing with `devmap build` about where the repository
        // ends, and dropping the row here would make the refusal invisible on
        // the one path that produces it most.
        let measured_refusals = match &opts.discovery_refusals {
            Some(refusals) => {
                // Deduplicated by path, because the count below is checked
                // against `COUNT(*)` of the rows and the table is keyed by
                // path: a caller that names one file twice would otherwise
                // claim a refusal the inventory cannot hold.
                let unique: std::collections::BTreeMap<&str, &str> = refusals
                    .iter()
                    .map(|refusal| (refusal.path.as_str(), refusal.reason.as_str()))
                    .collect();
                for (path, reason) in &unique {
                    gap_rows.push((
                        crate::coverage::GAP_DISCOVERY_REFUSED.to_string(),
                        (*path).to_string(),
                        (*reason).to_string(),
                    ));
                }
                Some(unique.len())
            }
            None => None,
        };
        // The same guard the edge count above gets, for the same reason: the
        // number a consumer reads and the rows it is supposed to count come
        // from two places, and nothing but this obliges them to agree. Getting
        // it wrong is not a cosmetic mismatch — `discovery_refused_files` caps
        // the dead-code confidence, so a summary claiming a refusal the
        // inventory cannot name is a graph degraded for a file nobody can look
        // at, and a summary claiming none while rows exist is the over-claim
        // this whole inventory exists to end.
        if measured_refusals != analysis.discovery_refused_files {
            return Err(rusqlite::Error::InvalidParameterName(format!(
                "generation would store {measured_refusals:?} discovery refusal(s) but its \
                 analysis was computed over {:?}; `discovery_refused_files` is derived from \
                 the inventory and the two must be one measurement",
                analysis.discovery_refused_files
            )));
        }
        {
            let mut insert = tx.prepare(
                "INSERT OR REPLACE INTO generation_coverage_gaps
                 (generation_id, gap, path, reason)
                 VALUES (?1, ?2, ?3, ?4)",
            )?;
            for (gap, path, reason) in &gap_rows {
                insert.execute(params![gen_id, gap, path, reason])?;
            }
        }

        for (ordinal, dead) in analysis.dead_symbols.iter().enumerate() {
            let ordinal = u32::try_from(ordinal).map_err(|_| {
                rusqlite::Error::InvalidParameterName(
                    "dead-symbol row count exceeds SQLite generation ordinal capacity".into(),
                )
            })?;
            tx.execute(
                "INSERT INTO generation_dead_symbols
                 (generation_id, ordinal, file_path, symbol_name, confidence, is_exempt, exemption_reason)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    gen_id,
                    ordinal,
                    dead.file_path,
                    dead.symbol_name,
                    confidence_millis(dead.confidence) as f64 / 1000.0,
                    dead.is_exempt as i32,
                    dead.exemption_reason
                ],
            )?;
        }

        // D17: the unresolved-call ledger. Written inside the same transaction
        // as everything else, so a generation can never be observable while
        // claiming a completeness it did not record.
        // One prepared statement for the whole ledger. A repository of this size
        // produces tens of thousands of unresolved calls per generation, and
        // re-preparing the INSERT for each one cost seconds of the build — the
        // self-build gate caught it as a regression the moment this table
        // landed.
        {
            let mut insert = tx.prepare(
                "INSERT INTO generation_unresolved
                 (generation_id, ordinal, source_file, source_symbol, callee_name, reason,
                  classification, receiver)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?;
            for (ordinal, unresolved) in resolution.unresolved.iter().enumerate() {
                let ordinal = u32::try_from(ordinal).map_err(|_| {
                    rusqlite::Error::InvalidParameterName(
                        "unresolved row count exceeds SQLite generation ordinal capacity".into(),
                    )
                })?;
                insert.execute(params![
                    gen_id,
                    ordinal,
                    unresolved.source_file,
                    unresolved.source_symbol,
                    unresolved.callee_name,
                    format!("{:?}", unresolved.resolution),
                    unresolved.class.label(),
                    unresolved.receiver.as_deref(),
                ])?;
            }
        }

        // The history row is written inside the generation's own transaction.
        // A build is therefore never observable without its history entry, and
        // a rolled-back generation leaves no phantom row behind.
        let symbols: i64 = tx.query_row(
            "SELECT COUNT(*) FROM generation_nodes WHERE generation_id = ?1",
            params![gen_id],
            |row| row.get(0),
        )?;
        let edges: i64 = tx.query_row(
            "SELECT COUNT(*) FROM generation_edges WHERE generation_id = ?1",
            params![gen_id],
            |row| row.get(0),
        )?;
        let files: i64 = tx.query_row(
            "SELECT COUNT(*) FROM generation_files WHERE generation_id = ?1",
            params![gen_id],
            |row| row.get(0),
        )?;
        // "Confident" and "ambiguous" are the two tiers a reader acts on:
        // an exempt symbol is one liveness could not rule out, so counting it
        // as confidently dead is exactly the dishonesty D6 removed.
        let is_ambiguous = |dead: &&DeadSymbolReport| {
            dead.exemption_reason.as_deref() == Some("only_ambiguous_callers")
        };
        let dead_confident = durable_analysis
            .dead_symbols
            .iter()
            .filter(|dead| !dead.is_exempt && !is_ambiguous(dead))
            .count() as i64;
        let dead_ambiguous = durable_analysis
            .dead_symbols
            .iter()
            .filter(is_ambiguous)
            .count() as i64;
        // S-2: both of these are counted over the generation's own rows, like
        // `files`/`symbols`/`edges` above, and not over `extractions`.
        //
        // `extractions` is the slice this *write* carried. On an incremental
        // build that is the handful of edited files, while `files` beside it is
        // `COUNT(*)` over the whole generation — a partial numerator against a
        // whole denominator, in the one table whose entire purpose is the
        // trend. A one-line edit in a twelve-language tree wrote
        // `languages_covered: 1, parse_failed: 0` next to the real file count,
        // so `devmap history` showed the repository shedding eleven languages
        // and repairing every parse failure on each incremental build, then
        // regaining both on the next cold one.
        let languages_covered: i64 = tx.query_row(
            "SELECT COUNT(DISTINCT language) FROM generation_files WHERE generation_id = ?1",
            params![gen_id],
            |row| row.get(0),
        )?;
        // K5: ask the canonical classifier, not the raw variant. A prose or
        // data format reports `ParseOutcome::Failed` because no grammar exists
        // for it, so the raw test counted 294 of this repository's 1,310 files
        // as parse failures — all Markdown, JSON, YAML, config and HTML — and
        // buried the 16 files a grammar actually parsed and flagged. Reading it
        // off the stored columns keeps that rule and applies it to carried-
        // forward rows too, which the in-memory slice cannot see.
        let mut parse_failed: i64 = 0;
        {
            let mut stmt = tx.prepare(
                "SELECT p.path, f.parse_outcome_json, f.engine_json
                 FROM generation_files f
                 JOIN paths p ON p.id = f.file_id
                 WHERE f.generation_id = ?1",
            )?;
            let rows = stmt.query_map(params![gen_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            for row in rows {
                let (path, parse_json, engine_json) = row?;
                let (outcome, engine) = decode_stored_outcome(&path, &parse_json, &engine_json)?;
                if stored_is_parse_failure(&outcome, &engine) {
                    parse_failed += 1;
                }
            }
        }
        let page_count: i64 = tx.query_row("PRAGMA page_count", [], |row| row.get(0))?;
        let page_size: i64 = tx.query_row("PRAGMA page_size", [], |row| row.get(0))?;
        let build_ms = opts
            .build_started
            .map(|started| i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX));

        tx.execute(
            "INSERT OR REPLACE INTO build_history
             (generation_id, built_at, head_sha, files, symbols, edges,
              dead_confident, dead_ambiguous, parse_failed, languages_covered,
              build_ms, db_bytes)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                gen_id,
                now,
                head_sha,
                files,
                symbols,
                edges,
                dead_confident,
                dead_ambiguous,
                parse_failed,
                languages_covered,
                build_ms,
                page_count.saturating_mul(page_size),
            ],
        )?;
        // Retention is by row count, not by surviving generation: history must
        // outlive the graphs it describes or it cannot show a trend.
        tx.execute(
            "DELETE FROM build_history WHERE generation_id NOT IN
             (SELECT generation_id FROM build_history ORDER BY built_at DESC, generation_id DESC LIMIT ?1)",
            params![BUILD_HISTORY_RETENTION as i64],
        )?;

        tx.commit()?;
        Ok(gen_id)
    }

    /// Most recent builds, newest first. `limit` is clamped to the retention cap.
    pub fn build_history(&self, limit: usize) -> Result<Vec<BuildHistoryRow>> {
        let conn = lock_conn(&self.conn)?;
        let mut stmt = conn.prepare(
            "SELECT generation_id, built_at, head_sha, files, symbols, edges,
                    dead_confident, dead_ambiguous, parse_failed, languages_covered,
                    build_ms, db_bytes
             FROM build_history
             ORDER BY built_at DESC, generation_id DESC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit.min(BUILD_HISTORY_RETENTION) as i64], |row| {
            Ok(BuildHistoryRow {
                generation_id: row.get(0)?,
                built_at: row.get::<_, f64>(1)? as i64,
                head_sha: row.get(2)?,
                files: row.get::<_, i64>(3)? as u64,
                symbols: row.get::<_, i64>(4)? as u64,
                edges: row.get::<_, i64>(5)? as u64,
                dead_confident: row.get::<_, i64>(6)? as u64,
                dead_ambiguous: row.get::<_, i64>(7)? as u64,
                parse_failed: row.get::<_, i64>(8)? as u64,
                languages_covered: row.get::<_, i64>(9)? as u64,
                build_ms: row
                    .get::<_, Option<i64>>(10)?
                    .map(|value| {
                        u64::try_from(value)
                            .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(10, value))
                    })
                    .transpose()?,
                db_bytes: row.get::<_, i64>(11)? as u64,
            })
        })?;
        rows.collect()
    }

    pub fn latest_generation_id(&self) -> Result<Option<u32>> {
        let conn = lock_conn(&self.conn)?;
        conn.query_row(
            "SELECT id FROM generations ORDER BY id DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()
    }

    pub fn latest_generation_head(&self) -> Result<Option<String>> {
        let conn = lock_conn(&self.conn)?;
        conn.query_row(
            "SELECT head_sha FROM generations ORDER BY id DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()
    }

    /// Absolute root the newest generation was built from, when recorded.
    /// D17: unresolved calls recorded for the latest generation.
    ///
    /// This is the honest denominator for graph completeness — a symbol with no
    /// callers is a different claim depending on whether anything failed to
    /// resolve against it.
    pub fn latest_unresolved(&self, limit: usize) -> Result<Vec<(String, String, String)>> {
        let conn = lock_conn(&self.conn)?;
        let mut stmt = conn.prepare(
            "SELECT source_symbol, callee_name, reason
             FROM generation_unresolved
             WHERE generation_id = (SELECT max(id) FROM generations)
             ORDER BY ordinal
             LIMIT ?1",
        )?;
        let rows = stmt
            .query_map(params![sqlite_limit(limit)], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?
            .collect::<Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Total unresolved rows across every retained generation. Test-facing:
    /// the point is to prove the table is pruned, not just written.
    pub fn count_unresolved_rows(&self) -> Result<usize> {
        let conn = lock_conn(&self.conn)?;
        let count: i64 =
            conn.query_row("SELECT COUNT(*) FROM generation_unresolved", [], |row| {
                row.get(0)
            })?;
        Ok(count as usize)
    }

    /// Whether every row in the latest generation was produced by the extractor
    /// and grammars this build is running.
    ///
    /// A build asks this *before* deciding to go differential. The extraction
    /// cache re-extracts a file whose analyzer or grammar version moved, but a
    /// generation used to carry its stored rows forward on content hash alone,
    /// so an upgraded kernel kept committing generations made of old payloads
    /// until a changed file finally made the stored edges disagree with the
    /// fresh analysis — at which point every incremental build failed and the
    /// only way out was deleting the database. Answering false here turns that
    /// into one full build.
    ///
    /// True when there is no generation yet: a cold build carries nothing.
    /// Whether the stored payload was produced by the current grammars.
    /// A build-path question: it compares against grammar identities only
    /// the parsing frontend can supply.
    #[cfg(feature = "parse")]
    /// The git HEAD the latest generation was built from, if any.
    ///
    /// Exists for B5: a commit, branch switch, rebase or stash changes what the
    /// index should contain while touching no watched file. Comparing this
    /// against the working tree's current HEAD is what lets the daemon notice
    /// that its generation describes a tree that no longer exists.
    ///
    /// `None` means no generation has been written. A stored `"unavailable"`
    /// (what the CLI stamps outside a git repository) is returned verbatim
    /// rather than mapped to `None`, because "built outside git" and "never
    /// built" are different facts and only one of them warrants a rebuild.
    pub fn latest_generation_head_sha(&self) -> Result<Option<String>> {
        let conn = lock_conn(&self.conn)?;
        let sha = conn
            .query_row(
                "SELECT head_sha FROM generations WHERE id = (SELECT max(id) FROM generations)",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        Ok(sha)
    }

    pub fn latest_generation_payload_is_current(&self) -> Result<bool> {
        let conn = lock_conn(&self.conn)?;
        let mut stmt = conn.prepare(
            "SELECT DISTINCT language, grammar_version, analyzer_version
             FROM generation_files
             WHERE generation_id = (SELECT max(id) FROM generations)",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?;
        for row in rows {
            let (language, grammar, analyzer) = row?;
            let Some((current_grammar, current_analyzer)) = current_payload_identity(&language)
            else {
                // Loud, not `false`. `false` means "rebuild", and a build with
                // no parsing frontend cannot rebuild — the caller would loop.
                // `true` would be worse: a currency claim from a check that did
                // not run.
                return Err(rusqlite::Error::InvalidParameterName(format!(
                    "whether the stored payload is current cannot be decided by this build: \
                     the answer is the compiled grammar version for {language:?}, and this \
                     binary was built without the parsing frontend. Build with \
                     `--features parse` to ask."
                )));
            };
            // A NULL version predates these columns: unknown identity is not a
            // matching one.
            if grammar.as_deref() != Some(current_grammar.as_str())
                || analyzer.as_deref() != Some(current_analyzer.as_str())
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// `(path, content_hash)` for every file in the latest generation.
    ///
    /// Lets a build decide, before resolving anything, whether the tree it just
    /// scanned is the one already committed.
    pub fn latest_file_hashes(&self) -> Result<BTreeMap<String, u64>> {
        let conn = lock_conn(&self.conn)?;
        let mut stmt = conn.prepare(
            "SELECT p.path, f.content_hash
             FROM generation_files f
             JOIN paths p ON p.id = f.file_id
             WHERE f.generation_id = (SELECT max(id) FROM generations)",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64))
            })?
            .collect::<Result<BTreeMap<_, _>>>()?;
        Ok(rows)
    }

    /// Symbol *names* per file in the latest generation.
    ///
    /// Names, not qualified names: the resolver's global indexes are keyed by
    /// bare name, so that is the granularity at which a definition moving can
    /// change another file's resolution.
    pub fn latest_symbol_names_by_file(&self) -> Result<BTreeMap<String, BTreeSet<String>>> {
        let conn = lock_conn(&self.conn)?;
        let mut stmt = conn.prepare(
            "SELECT p.path, n.name
             FROM generation_nodes n
             JOIN paths p ON p.id = n.file_id
             WHERE n.generation_id = (SELECT max(id) FROM generations)",
        )?;
        let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (path, name) = row?;
            out.entry(path).or_default().insert(name);
        }
        Ok(out)
    }

    /// Every edge of the latest generation, rendered for comparison.
    /// Test-facing: proving incremental output equals cold output needs the
    /// whole edge set, not a count.
    pub fn latest_edges_for_test(&self) -> Result<Vec<String>> {
        let conn = lock_conn(&self.conn)?;
        let mut stmt = conn.prepare(
            "SELECT source_symbol, target_symbol, edge_kind, printf('%.5f', confidence)
             FROM generation_edges
             WHERE generation_id = (SELECT max(id) FROM generations)",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(format!(
                    "{}>{}:{}:{}",
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?
                ))
            })?
            .collect::<Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn latest_repo_root(&self) -> Result<Option<String>> {
        let conn = lock_conn(&self.conn)?;
        let root: Option<Option<String>> = conn
            .query_row(
                "SELECT repo_root FROM generations ORDER BY id DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        Ok(root.flatten().filter(|root| !root.is_empty()))
    }

    /// The latest generation's analysis summary.
    ///
    /// `discovery_refused_files` is **derived** from the generation's refusal
    /// inventory rather than read out of the stored JSON, so the number a
    /// consumer acts on and the paths it can ask for are one measurement. The
    /// serialized field survives only as the record of whether discovery was
    /// measured at all: `None` there stays `None` here — nobody walked, and
    /// `Some(0)` would say the corpus was seen in full. `save_generation`
    /// refuses a write whose two halves disagree, so the two can only differ
    /// for a generation written before the inventory existed, whose rows are
    /// genuinely absent.
    pub fn latest_analysis(&self) -> Result<Option<AnalysisSummary>> {
        let conn = lock_conn(&self.conn)?;
        let Some((snapshot, generation)) = Self::latest_snapshot(&conn)? else {
            return Ok(None);
        };
        let raw: Option<String> = snapshot
            .query_row(
                "SELECT analysis_json FROM generations WHERE id = ?1",
                params![generation],
                |row| row.get(0),
            )
            .optional()?;
        let Some(raw) = raw else {
            return Ok(None);
        };
        let mut summary: AnalysisSummary = serde_json::from_str(&raw).map_err(|error| {
            rusqlite::Error::InvalidParameterName(format!(
                "stored generation analysis is invalid: {error}"
            ))
        })?;
        if summary.discovery_refused_files.is_some() {
            let refused: usize = snapshot.query_row(
                "SELECT COUNT(*) FROM generation_coverage_gaps
                 WHERE generation_id = ?1 AND gap = ?2",
                params![generation, crate::GAP_DISCOVERY_REFUSED],
                |row| row.get::<_, i64>(0).map(|count| count as usize),
            )?;
            summary.discovery_refused_files = Some(refused);
        }
        Ok(Some(summary))
    }

    /// Node and edge counts for `generation`, counted at most once.
    ///
    /// See [`Store::generation_counts`] for why the generation id is a
    /// sufficient key. Takes the caller's snapshot rather than the raw
    /// connection so the first (uncached) count is still read inside the
    /// transaction that resolved the generation id.
    fn generation_counts_locked(
        &self,
        snapshot: &rusqlite::Transaction<'_>,
        generation: u32,
    ) -> Result<(usize, usize)> {
        if let Ok(cache) = self.generation_counts.lock() {
            if let Some((cached, nodes, edges)) = *cache {
                if cached == generation {
                    return Ok((nodes, edges));
                }
            }
        }
        let nodes: usize = snapshot.query_row(
            "SELECT COUNT(*) FROM generation_nodes WHERE generation_id = ?1",
            params![generation],
            |row| row.get::<_, i64>(0).map(|n| n as usize),
        )?;
        let edges: usize = snapshot.query_row(
            "SELECT COUNT(*) FROM generation_edges WHERE generation_id = ?1",
            params![generation],
            |row| row.get::<_, i64>(0).map(|n| n as usize),
        )?;
        if let Ok(mut cache) = self.generation_counts.lock() {
            *cache = Some((generation, nodes, edges));
        }
        Ok((nodes, edges))
    }

    /// The latest generation's analysis **status**, without its summary.
    ///
    /// `devmap status` needs one enum to decide whether the graph is degraded,
    /// and reading it through [`Store::latest_analysis`] deserialises the whole
    /// `AnalysisSummary` to get there — every dead symbol, every community,
    /// every clone-coverage counter. On the ScholarLM corpus that blob is large
    /// enough to cost milliseconds on a surface whose whole budget is a few.
    ///
    /// SQLite's `->` operator returns a *JSON* representation rather than SQL
    /// text, so a unit variant comes back as `"Ok"` and a struct variant as its
    /// object, and both feed straight back into serde. That matters: the
    /// encoding of `AnalysisStatus` stays owned by its derive, and this method
    /// does not hand-decode variant names that a future variant would silently
    /// fall out of.
    ///
    /// Not the same reader as [`devmap_analyze::model::AnalysisDisclosure`],
    /// deliberately, but the difference is no longer where the bytes stop.
    /// When this was written the disclosure still transferred the whole blob
    /// and stepped serde over its vectors; `dead_page` now strips them in
    /// SQLite too, with `json_remove`, so neither reader carries the summary
    /// across. What remains is the shape of the question: the disclosure wants
    /// five fields inside a snapshot `dead_page` is already holding, and this
    /// wants one field on a surface a health check polls often enough to cache
    /// it per generation. Both decode `AnalysisStatus` through its own derive,
    /// so neither can drift from the writer.
    pub fn latest_analysis_status(&self) -> Result<Option<AnalysisStatus>> {
        let conn = lock_conn(&self.conn)?;
        // One snapshot for the generation id and the row it names, for the same
        // reason `status` takes one: resolving the newest generation and then
        // reading its analysis in two separate reads lets a prune between them
        // answer `None` for a store that holds a generation.
        let snapshot = conn.unchecked_transaction()?;
        let Some(generation) = Self::latest_generation_id_locked(&snapshot)? else {
            return Ok(None);
        };
        if let Ok(cache) = self.generation_analysis_status.lock() {
            if let Some((cached, status)) = cache.as_ref() {
                if *cached == generation {
                    return Ok(Some(status.clone()));
                }
            }
        }
        let raw: Option<Option<String>> = snapshot
            .query_row(
                "SELECT analysis_json -> '$.status' FROM generations WHERE id = ?1",
                params![generation],
                |row| row.get(0),
            )
            .optional()?;
        let Some(Some(json)) = raw else {
            return Ok(None);
        };
        let status: AnalysisStatus = serde_json::from_str(&json).map_err(|error| {
            rusqlite::Error::InvalidParameterName(format!(
                "stored generation analysis status is invalid: {error}"
            ))
        })?;
        if let Ok(mut cache) = self.generation_analysis_status.lock() {
            *cache = Some((generation, status.clone()));
        }
        Ok(Some(status))
    }

    pub fn status(&self, db_path: &str) -> Result<StoreStatus> {
        let conn = lock_conn(&self.conn)?;
        // Every number below describes one instant. `status` resolves the
        // latest generation and then counts that generation's nodes and
        // edges in separate statements: without a snapshot those are
        // separate reads, so a second process pruning between them reported
        // a live generation holding zero symbols. See `latest_snapshot`.
        let snapshot = conn.unchecked_transaction()?;
        let latest: Option<u32> = snapshot
            .query_row(
                "SELECT id FROM generations ORDER BY id DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        let pending_count: usize =
            snapshot.query_row("SELECT COUNT(*) FROM pending_paths", [], |row| {
                row.get::<_, i64>(0).map(|n| n as usize)
            })?;
        let (node_count, edge_count) = if let Some(g) = latest {
            // Counted at most once per generation. The two `COUNT(*)`s still
            // run inside the snapshot the first time, so the pair a caller sees
            // is still one instant's; what the memo removes is re-counting a
            // generation whose rows cannot change (measured on a 271k-edge
            // store: 2.87 ms of a 2.9 ms `status`).
            self.generation_counts_locked(&snapshot, g)?
        } else {
            // No generation, nothing to count. Not a cached zero — there are
            // genuinely no rows to describe.
            (0, 0)
        };
        let quarantined_count: usize = snapshot.query_row(
            "SELECT COUNT(*) FROM pending_paths WHERE attempts >= ?1",
            params![MAX_PENDING_ATTEMPTS],
            |row| row.get::<_, i64>(0).map(|count| count as usize),
        )?;
        let quarantined_paths: Vec<String> = {
            let mut stmt = snapshot.prepare(
                "SELECT path FROM pending_paths
                 WHERE attempts >= ?1
                 ORDER BY queued_at ASC, path ASC
                 LIMIT ?2",
            )?;
            let rows = stmt
                .query_map(
                    params![MAX_PENDING_ATTEMPTS, Self::DEGRADED_SAMPLE as i64],
                    |row| row.get(0),
                )?
                .collect::<Result<Vec<_>>>()?;
            rows
        };
        // Three primary-key range scans over a table whose rows are the
        // exception rather than the rule — on this repository, four rows. The
        // alternative, deriving the two extraction gaps from
        // `generation_files.parse_outcome_json` at read time, has to walk past
        // a ~47 KB `extraction_json` on every row of the generation to reach
        // three small columns; that is the scan the v13 index exists to avoid,
        // and `status` is a surface a health check polls.
        let coverage_gaps = match latest {
            Some(generation) => Self::coverage_gaps_locked(&snapshot, generation)?,
            // No generation, nothing to describe. Empty here means "there is no
            // generation", which `latest_generation: None` already says; it is
            // not a claim that a generation read everything.
            None => CoverageGaps::default(),
        };
        Ok(StoreStatus {
            db_path: db_path.to_string(),
            latest_generation: latest,
            pending_count,
            node_count,
            edge_count,
            degraded_reason: if quarantined_count > 0 {
                // Name the paths. See `StoreStatus::quarantined_paths`: the
                // count alone made a permanently degraded store undiagnosable
                // without opening the database by hand.
                let shown = quarantined_paths.join(", ");
                let elided = quarantined_count.saturating_sub(quarantined_paths.len());
                Some(if elided > 0 {
                    format!(
                        "{quarantined_count} path(s) exceeded the retry threshold \
                         (attempts >= {MAX_PENDING_ATTEMPTS}): {shown}, and {elided} more \
                         — `devmap repair --pending` drops them"
                    )
                } else {
                    format!(
                        "{quarantined_count} path(s) exceeded the retry threshold \
                         (attempts >= {MAX_PENDING_ATTEMPTS}): {shown} \
                         — `devmap repair --pending` drops them"
                    )
                })
            } else {
                None
            },
            quarantined_count,
            quarantined_paths,
            coverage_gaps,
        })
    }

    /// The latest generation's coverage-gap inventory, capped per kind.
    ///
    /// Takes the caller's snapshot for the same reason
    /// [`Store::generation_counts_locked`] does: the generation id and the rows
    /// it names have to come from one instant, or a prune between them reports
    /// a live generation with no gaps.
    fn coverage_gaps_locked(
        snapshot: &rusqlite::Transaction<'_>,
        generation: u32,
    ) -> Result<CoverageGaps> {
        let mut gaps = CoverageGaps::default();
        let mut count = snapshot.prepare(
            "SELECT COUNT(*) FROM generation_coverage_gaps
             WHERE generation_id = ?1 AND gap = ?2",
        )?;
        let mut page = snapshot.prepare(
            "SELECT path, reason FROM generation_coverage_gaps
             WHERE generation_id = ?1 AND gap = ?2
             ORDER BY path ASC
             LIMIT ?3",
        )?;
        for label in CoverageGaps::labels() {
            let total: usize = count.query_row(params![generation, label], |row| {
                row.get::<_, i64>(0).map(|total| total as usize)
            })?;
            let shown: Vec<CoverageGapRow> = page
                .query_map(
                    params![generation, label, crate::COVERAGE_GAP_SAMPLE as i64],
                    |row| {
                        Ok(CoverageGapRow {
                            path: row.get(0)?,
                            reason: row.get(1)?,
                        })
                    },
                )?
                .collect::<Result<Vec<_>>>()?;
            let slot = gaps.slot(label).expect("every label has a slot");
            *slot = CoverageGapSample { total, shown };
        }
        Ok(gaps)
    }

    /// Every path the latest generation's discovery refused, with its verdict.
    ///
    /// The whole inventory, uncapped: the daemon's drain carries it forward
    /// minus the paths this batch re-decided, and a capped read would silently
    /// drop verdicts on every drain until the corpus looked clean.
    pub fn latest_discovery_refusals(&self) -> Result<Vec<DiscoveryRefusal>> {
        let conn = lock_conn(&self.conn)?;
        let Some((snapshot, generation)) = Self::latest_snapshot(&conn)? else {
            return Ok(Vec::new());
        };
        let mut stmt = snapshot.prepare(
            "SELECT path, reason FROM generation_coverage_gaps
             WHERE generation_id = ?1 AND gap = ?2
             ORDER BY path ASC",
        )?;
        let refusals = stmt
            .query_map(params![generation, crate::GAP_DISCOVERY_REFUSED], |row| {
                Ok(DiscoveryRefusal {
                    path: row.get(0)?,
                    reason: row.get(1)?,
                })
            })?
            .collect::<Result<Vec<_>>>();
        drop(stmt);
        drop(snapshot);
        refusals
    }

    /// Whether the latest generation's edges carry the resolution the resolver
    /// recorded, or a reconstruction standing in for one it never stored.
    ///
    /// One row answers for the generation, and that is a property rather than a
    /// sample: `save_generation` writes every edge of a generation in a single
    /// transaction from one `resolution.edges`, and edges are never carried
    /// forward from an older generation (see the comment above the edge loop).
    /// So the column is present for all of a generation's edges or for none of
    /// them. `None` when there is no generation, or when it holds no edges —
    /// which is "nothing to say", not "reconstructed".
    pub fn latest_edge_resolution_source(&self) -> Result<Option<ResolutionSource>> {
        let conn = lock_conn(&self.conn)?;
        let Some((snapshot, generation)) = Self::latest_snapshot(&conn)? else {
            return Ok(None);
        };
        let stored: Option<Option<String>> = snapshot
            .query_row(
                "SELECT resolution FROM generation_edges
                 WHERE generation_id = ?1 ORDER BY ordinal LIMIT 1",
                params![generation],
                |row| row.get(0),
            )
            .optional()?;
        Ok(stored.map(|resolution| match resolution {
            Some(_) => ResolutionSource::Stored,
            None => ResolutionSource::Reconstructed,
        }))
    }

    /// Stored edges of the latest generation whose confidence contradicts the
    /// resolution kind recorded for them, counted in SQL.
    ///
    /// The same check `GenerationEdges` makes at index-build time, for a
    /// process that holds no index — `devmap status` is a fresh process per
    /// call and must not build a 271k-edge index to answer one number. The
    /// `CASE` table is generated from `ResolutionKind::ALL` so this cannot hold
    /// a second copy of the confidence ladder; a spelling the enum does not
    /// know falls to `-1` and counts as a mismatch, which is the honest reading
    /// of a kind this binary cannot vouch for. Rows without the column are not
    /// judged: a reconstruction cannot convict the row. `None` when there is no
    /// generation.
    pub fn edge_confidence_mismatches(&self) -> Result<Option<usize>> {
        use devmap_resolve::model::ResolutionKind;
        let conn = lock_conn(&self.conn)?;
        let Some((snapshot, generation)) = Self::latest_snapshot(&conn)? else {
            return Ok(None);
        };
        let ladder: String = ResolutionKind::ALL
            .iter()
            .map(|kind| {
                format!(
                    " WHEN '{}' THEN {}",
                    kind.label(),
                    kind.confidence().to_millis()
                )
            })
            .collect();
        let sql = format!(
            "SELECT COUNT(*) FROM generation_edges
             WHERE generation_id = ?1 AND resolution IS NOT NULL
               AND CAST(ROUND(confidence * 1000) AS INTEGER) != CASE resolution{ladder} ELSE -1 END"
        );
        let count: i64 = snapshot.query_row(&sql, params![generation], |row| row.get(0))?;
        Ok(Some(count.max(0) as usize))
    }

    pub fn search_fts(&self, query: &str, limit: usize) -> Result<Vec<(String, String, String)>> {
        if query.trim().is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let conn = lock_conn(&self.conn)?;
        let (snapshot, gen) = match Self::latest_snapshot(&conn)? {
            Some(pinned) => pinned,
            None => return Ok(vec![]),
        };
        let mut stmt = snapshot.prepare(
            "SELECT name, qualified_name, path
             FROM nodes_fts
             WHERE rowid IN (SELECT rowid_ref FROM nodes_fts_map WHERE generation_id = ?1)
               AND nodes_fts MATCH ?2
             ORDER BY rowid
             LIMIT ?3",
        )?;
        let match_q = fts_match_query(query)?;
        let rows = stmt.query_map(params![gen, match_q, sqlite_limit(limit)], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        if out.is_empty() {
            Self::require_searchable_index(&snapshot, gen)?;
        }
        Ok(out)
    }

    /// Search only the latest persisted generation. This never reads or parses
    /// the source tree, so callers cannot accidentally turn a query into a build.
    /// Every symbol row in the latest generation.
    ///
    /// Semantic ranking scores the whole corpus, not a keyword-matched page:
    /// the point of it is to find symbols whose *names do not contain the query
    /// terms*, which is exactly what `search_symbols` cannot return. A
    /// primary-key range scan over one generation is the cheapest way to get
    /// them, and there is nothing to precompute or keep in step.
    pub fn all_symbols(&self) -> Result<Vec<StoredSymbol>> {
        let conn = lock_conn(&self.conn)?;
        let Some((snapshot, gen)) = Self::latest_snapshot(&conn)? else {
            return Ok(Vec::new());
        };
        let mut stmt = snapshot.prepare(
            "SELECT n.name, n.qualified_name, n.kind, p.path,
                    n.span_start, n.span_end, n.is_exported
             FROM generation_nodes n
             JOIN paths p ON p.id = n.file_id
             WHERE n.generation_id = ?1
             ORDER BY n.ordinal",
        )?;
        let rows = stmt.query_map(params![gen], |row| {
            let name: String = row.get(0)?;
            let path: String = row.get(3)?;
            let (span_start, span_end) = checked_span(&path, &name, row.get(4)?, row.get(5)?)?;
            Ok(StoredSymbol {
                name,
                qualified_name: row.get(1)?,
                kind: row.get(2)?,
                path,
                span_start,
                span_end,
                is_exported: row.get::<_, i64>(6)? != 0,
            })
        })?;
        rows.collect()
    }

    pub fn search_symbols(&self, query: &str, limit: usize) -> Result<Vec<StoredSymbol>> {
        let conn = lock_conn(&self.conn)?;
        let (snapshot, gen) = match Self::latest_snapshot(&conn)? {
            Some(pinned) => pinned,
            None => return Ok(Vec::new()),
        };
        Self::search_symbols_locked(&snapshot, gen, query, limit)
    }

    /// The rows, and the count they were drawn from, against **one** generation.
    ///
    /// `count_search_symbols` and `search_symbols` each resolved "the latest
    /// generation" independently, taking and releasing the connection lock on
    /// their own. A writer committing between them — which is precisely what
    /// the daemon does while a client queries — split the answer across two
    /// generations: the count described the old one and the rows the new one.
    ///
    /// `Response` states the contract that breaks: clients enforce
    /// `shown + hidden == total`. When the newer generation matched more rows
    /// than the older one counted, `total` came back *smaller* than `shown`,
    /// `total.saturating_sub(shown)` clamped `hidden` to zero, and the response
    /// claimed `truncated: false` over a list that was neither complete nor
    /// consistent. Measured before this existed: `shown=40 hidden=0 total=1`.
    ///
    /// One lock and one explicitly pinned generation for every read, so the
    /// answer describes a single snapshot. Returns `None` when the store holds
    /// no generation at all, which is a different answer from an empty page.
    pub fn search_page(&self, query: &str, limit: usize) -> Result<Option<SearchPage>> {
        let conn = lock_conn(&self.conn)?;
        let Some((snapshot, generation)) = Self::latest_snapshot(&conn)? else {
            return Ok(None);
        };
        let repo_root: Option<Option<String>> = snapshot
            .query_row(
                "SELECT repo_root FROM generations WHERE id = ?1",
                params![generation],
                |row| row.get(0),
            )
            .optional()?;
        Ok(Some(SearchPage {
            generation,
            total: Self::count_search_symbols_locked(&snapshot, generation, query)?,
            rows: Self::search_symbols_locked(&snapshot, generation, query, limit)?,
            repo_root: repo_root.flatten().filter(|root| !root.is_empty()),
            // Inside the same snapshot as the rows and the count, through the
            // one reader that strips the summary's two vectors in SQLite. A
            // search that finds nothing is only a completed check if the corpus
            // it searched was complete, and that is the fact this carries.
            analysis: Self::analysis_disclosure_in(&snapshot, generation)?,
        }))
    }

    fn search_symbols_locked(
        conn: &Connection,
        gen: u32,
        query: &str,
        limit: usize,
    ) -> Result<Vec<StoredSymbol>> {
        if query.trim().is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let match_query = fts_match_query(query)?;
        let mut stmt = conn.prepare(
            "SELECT n.name, n.qualified_name, n.kind, p.path,
                    n.span_start, n.span_end, n.is_exported
             FROM nodes_fts
             CROSS JOIN nodes_fts_map m ON m.rowid_ref = nodes_fts.rowid
             JOIN generation_nodes n
               ON n.generation_id = m.generation_id
              AND n.ordinal = (nodes_fts.rowid & 4294967295)
             JOIN paths p ON p.id = n.file_id
             WHERE m.generation_id = ?1 AND nodes_fts MATCH ?2
             ORDER BY bm25(nodes_fts), p.path, n.name, n.span_start
             LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![gen, match_query, sqlite_limit(limit)], |row| {
            let name: String = row.get(0)?;
            let path: String = row.get(3)?;
            let (span_start, span_end) = checked_span(&path, &name, row.get(4)?, row.get(5)?)?;
            Ok(StoredSymbol {
                name,
                qualified_name: row.get(1)?,
                kind: row.get(2)?,
                path,
                span_start,
                span_end,
                is_exported: row.get::<_, i64>(6)? != 0,
            })
        })?;
        let page = rows.collect::<Result<Vec<_>>>()?;
        if page.is_empty() {
            Self::require_searchable_index(conn, gen)?;
        }
        Ok(page)
    }

    pub fn count_search_symbols(&self, query: &str) -> Result<u32> {
        let conn = lock_conn(&self.conn)?;
        let Some((snapshot, gen)) = Self::latest_snapshot(&conn)? else {
            return Ok(0);
        };
        Self::count_search_symbols_locked(&snapshot, gen, query)
    }

    fn count_search_symbols_locked(conn: &Connection, gen: u32, query: &str) -> Result<u32> {
        if query.trim().is_empty() {
            return Ok(0);
        }
        let match_query = fts_match_query(query)?;
        // CROSS JOIN pins the FTS table as the outer loop. As a plain JOIN,
        // SQLite 3.45 (the bundled version) leads with `nodes_fts_map` on
        // `generation_id` and re-scans full-text storage once per mapped row:
        // 12.7s at 200k rows, against 1.7ms for the match alone. A subquery
        // does not help because the planner flattens it. `search_symbols`
        // avoids this only by accident, via `ORDER BY bm25(...)`.
        let count: i64 = conn.query_row(
            "SELECT COUNT(*)
             FROM nodes_fts
             CROSS JOIN nodes_fts_map m
               ON m.rowid_ref = nodes_fts.rowid AND m.generation_id = ?1
             WHERE nodes_fts MATCH ?2",
            params![gen, match_query],
            |row| row.get(0),
        )?;
        if count == 0 {
            Self::require_searchable_index(conn, gen)?;
        }
        u32::try_from(count).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(0, count))
    }

    pub fn latest_path_is_indexed(&self, path: &str) -> Result<bool> {
        let conn = lock_conn(&self.conn)?;
        let Some((snapshot, gen)) = Self::latest_snapshot(&conn)? else {
            return Ok(false);
        };
        snapshot.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM generation_files f
                JOIN paths p ON p.id = f.file_id
                WHERE f.generation_id = ?1 AND p.path = ?2
             )",
            params![gen, path],
            |row| row.get::<_, i64>(0).map(|value| value != 0),
        )
    }

    pub fn latest_file(&self, path: &str) -> Result<Option<StoredFile>> {
        let conn = lock_conn(&self.conn)?;
        let Some((snapshot, gen)) = Self::latest_snapshot(&conn)? else {
            return Ok(None);
        };
        let raw: Option<(String, String, i64, String, String)> = snapshot
            .query_row(
                "SELECT p.path, f.language, f.content_hash,
                        f.parse_outcome_json, f.engine_json
                 FROM generation_files f
                 JOIN paths p ON p.id = f.file_id
                 WHERE f.generation_id = ?1 AND p.path = ?2",
                params![gen, path],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .optional()?;
        raw.map(|(path, language, content_hash, parse_json, engine_json)| {
            let (parse_outcome, engine) = decode_stored_outcome(&path, &parse_json, &engine_json)?;
            Ok(StoredFile {
                path,
                language,
                content_hash: content_hash as u64,
                parse_outcome,
                engine,
            })
        })
        .transpose()
    }

    /// Load the canonical extraction payloads for the latest generation. This
    /// supports differential re-resolution without touching unchanged files.
    pub fn latest_extractions(&self) -> Result<Vec<Extraction>> {
        let conn = lock_conn(&self.conn)?;
        let Some((snapshot, gen)) = Self::latest_snapshot(&conn)? else {
            return Ok(Vec::new());
        };
        let mut stmt = snapshot.prepare(
            "SELECT f.extraction_json, p.path
             FROM generation_files f
             JOIN paths p ON p.id = f.file_id
             WHERE f.generation_id = ?1
             ORDER BY p.path",
        )?;
        let rows = stmt.query_map(params![gen], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut extractions = Vec::new();
        for row in rows {
            let (json, path) = row?;
            let extraction = serde_json::from_str(&json).map_err(|error| {
                rusqlite::Error::InvalidParameterName(format!(
                    "stored extraction for {path} is invalid: {error}"
                ))
            })?;
            extractions.push(extraction);
        }
        Ok(extractions)
    }

    /// One file's stored extraction, or `None` when the latest generation does
    /// not contain it.
    ///
    /// `latest_extractions` deserialises every file in the generation — 1,300
    /// JSON payloads on this repository — which is the wrong shape for a
    /// question about one path. `None` distinguishes "this file is not
    /// indexed" from "this file is indexed and empty", and a preview has to
    /// tell those apart: against an unindexed file every symbol in the buffer
    /// is an addition, which is true but worth saying out loud rather than
    /// presenting as a diff against known content.
    pub fn latest_extraction_for_path(&self, path: &str) -> Result<Option<Extraction>> {
        let conn = lock_conn(&self.conn)?;
        let Some((snapshot, gen)) = Self::latest_snapshot(&conn)? else {
            return Ok(None);
        };
        let json: Option<String> = snapshot
            .query_row(
                "SELECT f.extraction_json
                 FROM generation_files f
                 JOIN paths p ON p.id = f.file_id
                 WHERE f.generation_id = ?1 AND p.path = ?2",
                params![gen, path],
                |row| row.get(0),
            )
            .optional()?;
        let Some(json) = json else {
            return Ok(None);
        };
        let extraction = serde_json::from_str(&json).map_err(|error| {
            rusqlite::Error::InvalidParameterName(format!(
                "stored extraction for {path} is invalid: {error}"
            ))
        })?;
        Ok(Some(extraction))
    }

    /// Call edges whose *target* is one of `names`, excluding those originating
    /// in `exclude_file`.
    ///
    /// `names` are **qualified** names (`path::Symbol`), which is what
    /// `generation_edges.target_symbol` holds. Bare names match nothing here,
    /// and match nothing quietly: the query returns zero rows and the caller
    /// reports that nothing depends on the symbol.
    ///
    /// The exclusion is what makes the answer mean "who outside this file
    /// depends on these symbols". A file's own internal calls are not callers
    /// that a rewrite of that file would break — they are being rewritten too —
    /// and counting them inflates every preview of a self-contained module.
    ///
    /// An empty `names` returns no rows without touching the database, rather
    /// than building `IN ()`, which SQLite rejects.
    ///
    /// The threshold goes through [`checked_min_confidence`] *before* that
    /// shortcut. This was the one confidence-filtered edge query that skipped
    /// it, and skipping it is not a missing error message: rusqlite binds
    /// `f32::NAN` as a REAL, SQLite stores that as NULL, and the
    /// `CAST(ROUND(...)) >= CAST(ROUND(?3 * 1000))` predicate below is then
    /// NULL for every row — so the query returned `Ok(vec![])` and `preview`
    /// reported "no calls from other files are affected" for callers at
    /// confidence 1.00, while blaming the omission on the confidence floor.
    /// An empty edge list is also what a filter that ran returns, so the
    /// caller had no way to tell that the filter had not run at all.
    /// A large `names` is **chunked**, never refused and never truncated. Each
    /// name becomes one bind parameter, so an unchunked query with more than
    /// `SQLITE_MAX_VARIABLE_NUMBER` names failed to even prepare — surfacing
    /// `too many SQL variables` from the middle of a `preview`, naming neither
    /// the caller nor the limit. Truncating the list instead would have been
    /// worse: a dropped name contributes zero callers, which reads exactly like
    /// a symbol nothing depends on. Chunking keeps the answer complete and
    /// bounds only the statement, and the documented total order is restored
    /// across chunks by the sort below.
    /// How many callers `callers_of` would return, without building them.
    ///
    /// `preview` needs two numbers from the same query: the confident callers
    /// it lists, and how many more the confidence floor excluded. The second
    /// was obtained by calling `callers_of` again at floor 0.0 and taking
    /// `.len()` — materialising every matching `StoredEdge`, six `String`
    /// allocations apiece, to produce one integer. On this repository the
    /// busiest symbol has 918 callers, so previewing a file that declares one
    /// built ~1,836 rows and discarded all of them.
    ///
    /// Deliberately shares every filter, guard and chunk boundary with
    /// `callers_of` — including `checked_min_confidence`, so a NaN floor is
    /// refused here too rather than counting zero. `count_callers_of_matches_the_listing_it_replaces`
    /// pins the two against each other; a filter added to one and not the other
    /// fails that test rather than silently making the "hidden" number wrong.
    pub fn count_callers_of(
        &self,
        names: &[String],
        exclude_file: &str,
        min_confidence: f32,
    ) -> Result<usize> {
        let min_confidence = checked_min_confidence(min_confidence)?;
        if names.is_empty() {
            return Ok(0);
        }
        let unique: Vec<&String> = {
            let mut seen = BTreeSet::new();
            names.iter().filter(|name| seen.insert(*name)).collect()
        };
        let conn = lock_conn(&self.conn)?;
        let Some((snapshot, gen)) = Self::latest_snapshot(&conn)? else {
            return Ok(0);
        };
        Self::count_callers_in(&snapshot, gen, &unique, exclude_file, min_confidence)
    }

    fn count_callers_in(
        snapshot: &Connection,
        gen: u32,
        unique: &[&String],
        exclude_file: &str,
        min_confidence: f32,
    ) -> Result<usize> {
        let mut total: usize = 0;
        for chunk in unique.chunks(Self::MAX_CALLER_BATCH) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT COUNT(*)
                 FROM generation_edges e
                 JOIN paths sp ON sp.id = e.source_file_id
                 WHERE e.generation_id = ?1
                   AND e.edge_kind = 'Calls'
                   AND sp.path <> ?2
                   AND CAST(ROUND(e.confidence * 1000) AS INTEGER) >= CAST(ROUND(?3 * 1000) AS INTEGER)
                   AND e.target_symbol IN ({placeholders})"
            );
            let mut stmt = snapshot.prepare(&sql)?;
            let mut bound: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(chunk.len() + 3);
            bound.push(&gen);
            bound.push(&exclude_file);
            bound.push(&min_confidence);
            for name in chunk {
                bound.push(*name);
            }
            // Chunks partition the *names*, and each edge names one target, so
            // the per-chunk counts sum without double-counting — the same
            // property that makes `callers_of`'s chunked concatenation exact.
            let count: i64 = stmt.query_row(bound.as_slice(), |row| row.get(0))?;
            total += usize::try_from(count).unwrap_or(0);
        }
        Ok(total)
    }

    pub fn callers_of(
        &self,
        names: &[String],
        exclude_file: &str,
        min_confidence: f32,
    ) -> Result<Vec<StoredEdge>> {
        // Same guard as `latest_edges` and `latest_edges_for_file`: NaN cannot
        // be compared, and binding it makes SQLite evaluate `>= NULL` as NULL
        // so every row is rejected. The empty result that comes back is the
        // sentence "nothing calls this", produced by a filter that never ran —
        // and `dead_symbols.py` reads exactly that emptiness as proof a symbol
        // is unused. See `checked_min_confidence`.
        let min_confidence = checked_min_confidence(min_confidence)?;
        if names.is_empty() {
            return Ok(Vec::new());
        }
        // `IN` already ignores duplicates, so deduplicating preserves the
        // result exactly while making each edge belong to a single chunk.
        let unique: Vec<&String> = {
            let mut seen = BTreeSet::new();
            names.iter().filter(|name| seen.insert(*name)).collect()
        };
        let conn = lock_conn(&self.conn)?;
        let Some((snapshot, gen)) = Self::latest_snapshot(&conn)? else {
            return Ok(Vec::new());
        };
        Self::callers_in(&snapshot, gen, &unique, exclude_file, min_confidence)
    }

    fn callers_in(
        snapshot: &Connection,
        gen: u32,
        unique: &[&String],
        exclude_file: &str,
        min_confidence: f32,
    ) -> Result<Vec<StoredEdge>> {
        let mut out: Vec<StoredEdge> = Vec::new();
        for chunk in unique.chunks(Self::MAX_CALLER_BATCH) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT sp.path, tp.path, e.source_symbol, e.target_symbol,
                        e.edge_kind, e.confidence, e.resolution
                 FROM generation_edges e
                 JOIN paths sp ON sp.id = e.source_file_id
                 JOIN paths tp ON tp.id = e.target_file_id
                 WHERE e.generation_id = ?1
                   AND e.edge_kind = 'Calls'
                   AND sp.path <> ?2
                   AND CAST(ROUND(e.confidence * 1000) AS INTEGER) >= CAST(ROUND(?3 * 1000) AS INTEGER)
                   AND e.target_symbol IN ({placeholders})"
            );
            let mut stmt = snapshot.prepare(&sql)?;
            let mut bound: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(chunk.len() + 3);
            bound.push(&gen);
            bound.push(&exclude_file);
            bound.push(&min_confidence);
            for name in chunk {
                bound.push(*name);
            }
            let rows = stmt.query_map(bound.as_slice(), |row| {
                Ok(StoredEdge {
                    source_file: row.get(0)?,
                    target_file: row.get(1)?,
                    source_symbol: row.get(2)?,
                    target_symbol: row.get(3)?,
                    edge_kind: row.get(4)?,
                    confidence: row.get(5)?,
                    resolution: row.get(6)?,
                })
            })?;
            for row in rows {
                out.push(row?);
            }
        }
        // The order the single-statement form got from SQL, restored in Rust so
        // a chunked answer and an unchunked one are byte-identical.
        out.sort_by(|left, right| {
            right
                .confidence
                .total_cmp(&left.confidence)
                .then_with(|| left.target_symbol.cmp(&right.target_symbol))
                .then_with(|| left.source_file.cmp(&right.source_file))
                .then_with(|| left.source_symbol.cmp(&right.source_symbol))
        });
        Ok(out)
    }

    pub fn latest_edges_for_file(
        &self,
        path: &str,
        min_confidence: f32,
    ) -> Result<Vec<StoredEdge>> {
        let min_confidence = checked_min_confidence(min_confidence)?;
        let conn = lock_conn(&self.conn)?;
        let Some((snapshot, gen)) = Self::latest_snapshot(&conn)? else {
            return Ok(Vec::new());
        };
        let mut stmt = snapshot.prepare(
            "SELECT sp.path, tp.path, e.source_symbol, e.target_symbol,
                    e.edge_kind, e.confidence, e.resolution
             FROM generation_edges e
             JOIN paths sp ON sp.id = e.source_file_id
             JOIN paths tp ON tp.id = e.target_file_id
             WHERE e.generation_id = ?1
               AND (sp.path = ?2 OR tp.path = ?2)
               AND CAST(ROUND(e.confidence * 1000) AS INTEGER) >= CAST(ROUND(?3 * 1000) AS INTEGER)
             ORDER BY e.confidence DESC, sp.path, tp.path,
                      e.source_symbol, e.target_symbol, e.edge_kind",
        )?;
        let rows = stmt.query_map(params![gen, path, min_confidence], |row| {
            Ok(StoredEdge {
                source_file: row.get(0)?,
                target_file: row.get(1)?,
                source_symbol: row.get(2)?,
                target_symbol: row.get(3)?,
                edge_kind: row.get(4)?,
                confidence: row.get(5)?,
                resolution: row.get(6)?,
            })
        })?;
        rows.collect()
    }

    /// Every edge in the latest generation at or above `min_confidence`.
    ///
    /// Served from [`Store::edge_cache`] when the generation has not moved. See
    /// that field for the measurements that motivate it.
    pub fn latest_edges(&self, min_confidence: f32) -> Result<Vec<StoredEdge>> {
        // The confidence comparison is the SQL's, moved into Rust unchanged, so
        // a cached answer and a freshly-queried one cannot disagree — *given a
        // finite threshold*. That qualifier was missing and the claim was false:
        // on NaN the two implementations disagreed completely. Rust saturates
        // `(NaN * 1000.0).round() as i64` to 0 and admits everything; SQLite
        // stores NaN as NULL and `>= NULL` is NULL, so the SQL admits nothing.
        // A caller got "this depends on nothing" — a positive claim — from a
        // comparison that never ran. `checked_min_confidence` refuses the input
        // instead, so neither implementation is asked an unanswerable question.
        let min_confidence = checked_min_confidence(min_confidence)?;
        let Some((_, all, _)) = self.latest_edge_rows()? else {
            return Ok(Vec::new());
        };
        Ok(all
            .iter()
            .filter(|edge| crate::edge_index::admits(edge.confidence, min_confidence))
            .cloned()
            .collect())
    }

    /// The latest generation's unfiltered edge rows, and the generation they
    /// came from.
    ///
    /// The one place [`Store::edge_cache`] is consulted and filled, so
    /// [`Store::latest_edges`] and [`Store::generation_edges`] read the same
    /// rows for the same generation and share one allocation of them. `None`
    /// means no generation has been persisted.
    ///
    /// Keyed by the generation the rows were *read from*, not by the one
    /// sampled before the load. This function asks the question twice — once
    /// to probe the cache, once inside the load's own snapshot — and a writer
    /// committing between the two made the entry `(N, edges of N+1)`: a key
    /// that can never be hit again, so the cache silently stopped being one
    /// until the next load rewrote it. Labelling the entry with the generation
    /// its rows came from makes the key mean what it says.
    fn latest_edge_rows(&self) -> Result<Option<CachedEdges>> {
        let current = {
            let conn = lock_conn(&self.conn)?;
            Self::latest_generation_id_locked(&conn)?
        };
        let Some(current) = current else {
            return Ok(None);
        };
        if let Ok(cache) = self.edge_cache.lock() {
            if let Some((generation, edges, resolutions)) = cache.as_ref() {
                if *generation == current {
                    return Ok(Some((
                        current,
                        std::sync::Arc::clone(edges),
                        std::sync::Arc::clone(resolutions),
                    )));
                }
            }
        }
        let Some((loaded, all, resolutions)) = self.latest_edges_uncached(0.0)? else {
            return Ok(None);
        };
        let all = std::sync::Arc::new(all);
        // Decoded once per generation, beside the rows they describe rather
        // than in a cache of their own: an evidence tier read from a different
        // generation than the edge it labels is the drift `GenerationEdges`
        // already refuses to allow for its coverage disclosure.
        let resolutions = std::sync::Arc::new(resolutions);
        if let Ok(mut cache) = self.edge_cache.lock() {
            *cache = Some((
                loaded,
                std::sync::Arc::clone(&all),
                std::sync::Arc::clone(&resolutions),
            ));
        }
        Ok(Some((loaded, all, resolutions)))
    }

    /// Adjacency over the latest generation's edges, built once per generation.
    ///
    /// `None` when no generation has been persisted. The returned index is a
    /// snapshot: it stays internally consistent — every edge in it comes from
    /// one generation — even if a build commits a newer one while a walk is
    /// running, and the next call after that build gets the newer generation
    /// because the memo is keyed by its id.
    ///
    /// Errors on an unknown stored edge kind, which is where the per-request
    /// conversion used to fail: a store written by a binary that knows an edge
    /// kind this one does not is refused rather than half-read.
    pub fn generation_edges(&self) -> Result<Option<std::sync::Arc<GenerationEdges>>> {
        let Some((current, rows, resolutions)) = self.latest_edge_rows()? else {
            return Ok(None);
        };
        if let Ok(cache) = self.edge_index.lock() {
            if let Some((generation, index)) = cache.as_ref() {
                if *generation == current {
                    return Ok(Some(std::sync::Arc::clone(index)));
                }
            }
        }
        if rows.len() > u32::MAX as usize {
            return Err(rusqlite::Error::InvalidParameterName(format!(
                "generation {current} holds {} edges, more than the {} an edge \
                 index can address; answering over a prefix of it would be a \
                 wrong answer rather than a bounded one",
                rows.len(),
                u32::MAX
            )));
        }
        // Read for `current` specifically — the generation the rows came from,
        // which may already be behind the store's latest.
        let analysis = self.analysis_disclosure_for(current)?;
        let index = std::sync::Arc::new(
            GenerationEdges::build_with_resolutions(
                rows,
                analysis,
                Some(resolutions.as_ref().clone()),
            )
            .map_err(|error| rusqlite::Error::InvalidParameterName(error.to_string()))?,
        );
        if let Ok(mut cache) = self.edge_index.lock() {
            *cache = Some((current, std::sync::Arc::clone(&index)));
        }
        Ok(Some(index))
    }

    /// Every edge of the latest generation, and the generation they came from.
    ///
    /// The generation travels with the rows because [`Self::latest_edges`]
    /// caches them under it; returning only the rows left the caller to label
    /// them with a generation it had resolved separately. `None` when the store
    /// holds no generation.
    ///
    /// # Why the order is not SQL's any more
    ///
    /// This read is the whole fixed cost of arriving at [`GenerationEdges`],
    /// which is what a one-shot `devmap impact` pays and never amortises. Split
    /// on this repository's 101,503 edges, minima of three runs each:
    ///
    /// | part | cost |
    /// |---|---|
    /// | `ORDER BY confidence DESC, sp.path, tp.path, …` | **~72 ms** |
    /// | the two `paths` joins | ~9 ms |
    /// | the row scan and its string materialisation | ~19 ms |
    /// | building the adjacency in [`GenerationEdges::build_with_resolutions`] | ~23 ms |
    ///
    /// The sort is the single biggest term and it is the one SQLite is worst
    /// at here: the key spans two joined `paths` strings, so no index can
    /// supply it (`generation_edges` is keyed `(generation_id, ordinal)`, and
    /// `ordinal` is the *resolver's* emission order, not this one), and the
    /// plan is `USE TEMP B-TREE FOR ORDER BY` over every row of the
    /// generation — ~15 MB of records through SQLite's sorter to order a Vec
    /// that is about to be built in memory anyway.
    ///
    /// So the ordering moves to Rust, and with it the joins: the `paths` table
    /// is 1,567 rows, read once and *ranked* once, which turns the two most
    /// discriminating string keys of the comparison into `u32` compares.
    /// Measured end to end, the same rows in the same order: **~100 ms → ~39
    /// ms**.
    ///
    /// # Why the result is the same order
    ///
    /// [`edge_read_order`] is the comparator, and it is SQL's key by key:
    /// SQLite's default collation is BINARY, which is `str`'s byte ordering,
    /// and the confidence is compared as the `f64` SQLite stored rather than
    /// the `f32` [`StoredEdge`] narrows it to, so no pair that SQL separated
    /// can collapse into a tie here. It then adds `ordinal` as a final key,
    /// which SQL had no equivalent of: SQLite's sorter is not stable, so rows
    /// equal on all six of its keys came back in an order nothing defined.
    /// The extra key can only order pairs SQL left unordered, and it makes the
    /// result reproducible instead of merely unspecified.
    #[allow(clippy::type_complexity)]
    fn latest_edges_uncached(
        &self,
        min_confidence: f32,
    ) -> Result<Option<(u32, Vec<StoredEdge>, Vec<crate::edge_index::EdgeResolution>)>> {
        let conn = lock_conn(&self.conn)?;
        let Some((snapshot, gen)) = Self::latest_snapshot(&conn)? else {
            return Ok(None);
        };
        let paths = PathRanks::read(&snapshot)?;
        let mut stmt = snapshot.prepare(
            "SELECT e.source_file_id, e.target_file_id, e.source_symbol,
                    e.target_symbol, e.edge_kind, e.confidence, e.resolution,
                    e.ordinal
             FROM generation_edges e
             WHERE e.generation_id = ?1 AND CAST(ROUND(e.confidence * 1000) AS INTEGER) >= CAST(ROUND(?2 * 1000) AS INTEGER)",
        )?;
        let rows = stmt.query_map(params![gen, min_confidence], |row| {
            Ok(UnorderedEdge {
                source_rank: paths.rank_of(row.get(0)?)?,
                target_rank: paths.rank_of(row.get(1)?)?,
                source_symbol: row.get(2)?,
                target_symbol: row.get(3)?,
                edge_kind: row.get(4)?,
                confidence: row.get(5)?,
                resolution: row.get(6)?,
                ordinal: row.get(7)?,
            })
        })?;
        let mut unordered = rows.collect::<Result<Vec<_>>>()?;
        unordered.sort_unstable_by(edge_read_order);

        // The evidence tier is decoded in the same pass that materialises the
        // rows, from the row it describes. Taking it from a second query would
        // let the two describe different generations, and taking it later would
        // need this ordering reproduced somewhere else — which is exactly the
        // alignment a shifted resolution column would break.
        let mut edges = Vec::with_capacity(unordered.len());
        let mut resolutions = Vec::with_capacity(unordered.len());
        for row in unordered {
            let edge = StoredEdge {
                source_file: paths.path_of(row.source_rank).to_string(),
                target_file: paths.path_of(row.target_rank).to_string(),
                source_symbol: row.source_symbol,
                target_symbol: row.target_symbol,
                edge_kind: row.edge_kind,
                confidence: row.confidence as f32,
                resolution: row.resolution,
            };
            resolutions.push(
                crate::edge_index::edge_resolution(&edge)
                    .map_err(|error| rusqlite::Error::InvalidParameterName(error.to_string()))?,
            );
            edges.push(edge);
        }
        Ok(Some((gen, edges, resolutions)))
    }

    /// The callers of `names` and the unfiltered total, against one generation.
    ///
    /// `callers_of` and `count_callers_of` each open their own snapshot, so a
    /// caller that needs both numbers to agree cannot get that by calling them
    /// in sequence — which is what `preview` was doing. See [`CallersPage`].
    pub fn callers_page(
        &self,
        names: &[String],
        exclude_file: &str,
        min_confidence: f32,
    ) -> Result<Option<CallersPage>> {
        let min_confidence = checked_min_confidence(min_confidence)?;
        let conn = lock_conn(&self.conn)?;
        let Some((snapshot, generation)) = Self::latest_snapshot(&conn)? else {
            return Ok(None);
        };
        if names.is_empty() {
            return Ok(Some(CallersPage {
                generation,
                callers: Vec::new(),
                total_unfiltered: 0,
            }));
        }
        let unique: Vec<&String> = {
            let mut seen = BTreeSet::new();
            names.iter().filter(|name| seen.insert(*name)).collect()
        };
        Ok(Some(CallersPage {
            generation,
            callers: Self::callers_in(
                &snapshot,
                generation,
                &unique,
                exclude_file,
                min_confidence,
            )?,
            // The denominator is deliberately unfiltered: the difference from
            // `callers` is precisely what the floor excluded.
            total_unfiltered: Self::count_callers_in(
                &snapshot,
                generation,
                &unique,
                exclude_file,
                0.0,
            )?,
        }))
    }

    /// How much of the corpus one generation's analysis actually covered.
    ///
    /// The two big arrays are dropped *inside SQLite*, so they never cross into
    /// this process. `AnalysisDisclosure` already skipped them, but skipping is
    /// per token and there are 10 MB of tokens: measured on the benchmark
    /// corpus this column is 10,122,764 bytes and what survives the strip is
    /// 200. `dead_symbols` is the list being paged beside this; `communities`
    /// is the other unbounded array and no disclosure reads it.
    ///
    /// Absence and corruption stay distinguishable, which is the whole reason
    /// this is safe: `json_remove(NULL, ...)` is NULL, so a generation with no
    /// analysis still reads as none, while a malformed blob makes SQLite raise
    /// ("malformed JSON") rather than quietly returning NULL — a corrupt
    /// analysis must not read as an absent one.
    ///
    /// One owner because two readers of the same column would eventually
    /// disagree about which arrays to strip, and the caller that strips less
    /// pulls 10 MB per query without anything saying so.
    fn analysis_disclosure_in(
        snapshot: &Connection,
        generation: u32,
    ) -> Result<Option<AnalysisDisclosure>> {
        let raw: Option<String> = snapshot
            .query_row(
                "SELECT json_remove(analysis_json, '$.dead_symbols', '$.communities')
                 FROM generations WHERE id = ?1",
                params![generation],
                |row| row.get(0),
            )
            .optional()?;
        // Into the disclosure, not the whole summary: the summary embeds a
        // second copy of the dead-symbol list, so parsing it here would undo
        // the bound above. See `AnalysisDisclosure`.
        raw.map(|json| {
            serde_json::from_str::<AnalysisDisclosure>(&json).map_err(|error| {
                rusqlite::Error::InvalidParameterName(format!(
                    "stored generation analysis is invalid: {error}"
                ))
            })
        })
        .transpose()
    }

    /// [`Self::analysis_disclosure_in`] for a generation the caller already
    /// resolved.
    ///
    /// Addressed by id rather than by "latest" on purpose: the edge rows this
    /// qualifies may have come from a cache filled before a newer generation
    /// landed, and a disclosure describing a snapshot the answer did not come
    /// from is worse than none. A generation pruned between the two reads has
    /// no row here, which reads as `None` — "could not be read" — and that is
    /// the honest answer.
    fn analysis_disclosure_for(&self, generation: u32) -> Result<Option<AnalysisDisclosure>> {
        let conn = lock_conn(&self.conn)?;
        Self::analysis_disclosure_in(&conn, generation)
    }

    /// The dead-symbol rows and the analysis that qualifies them, against one
    /// generation. See [`DeadPage`].
    pub fn dead_page(&self, limit: usize) -> Result<Option<DeadPage>> {
        let conn = lock_conn(&self.conn)?;
        let Some((snapshot, generation)) = Self::latest_snapshot(&conn)? else {
            return Ok(None);
        };
        let analysis = Self::analysis_disclosure_in(&snapshot, generation)?;
        Ok(Some(DeadPage {
            generation,
            analysis,
            rows: Self::dead_symbols_page_in(&snapshot, generation, limit)?,
            total_non_exempt: Self::count_dead_non_exempt_in(&snapshot, generation)?,
        }))
    }

    /// The ranked head of the non-exempt dead rows.
    ///
    /// The exempt filter and the limit both belong in SQL. `dead_symbols`
    /// discarded exempt rows in Rust after materialising every row of the
    /// generation, and then the budgeter kept a few dozen: measured at 80,000
    /// rows read to show 66, and on this repository 6,976 of 7,176 rows read
    /// were exempt and dropped on arrival. The work was proportional to the
    /// corpus, never to the answer.
    ///
    /// Ordering is unchanged. The old query sorted by `is_exempt` first, so
    /// filtering on it makes that key constant and leaves the surviving rows in
    /// exactly the order they already had.
    fn dead_symbols_page_in(
        snapshot: &Connection,
        gen: u32,
        limit: usize,
    ) -> Result<Vec<DeadSymbolReport>> {
        let mut stmt = snapshot.prepare(
            "SELECT symbol_name, file_path, confidence, is_exempt, exemption_reason
             FROM generation_dead_symbols
             WHERE generation_id = ?1 AND is_exempt = 0
             ORDER BY confidence DESC, file_path, symbol_name, ordinal
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(
            params![gen, i64::try_from(limit).unwrap_or(i64::MAX)],
            |row| {
                Ok(DeadSymbolReport {
                    symbol_name: row.get(0)?,
                    file_path: row.get(1)?,
                    confidence: row.get(2)?,
                    is_exempt: row.get::<_, i64>(3)? != 0,
                    exemption_reason: row.get(4)?,
                })
            },
        )?;
        rows.collect()
    }

    fn count_dead_non_exempt_in(snapshot: &Connection, gen: u32) -> Result<usize> {
        snapshot.query_row(
            "SELECT COUNT(*) FROM generation_dead_symbols
             WHERE generation_id = ?1 AND is_exempt = 0",
            params![gen],
            |row| row.get::<_, i64>(0).map(|count| count as usize),
        )
    }

    pub fn latest_dead_symbols(&self) -> Result<Vec<DeadSymbolReport>> {
        let conn = lock_conn(&self.conn)?;
        let Some((snapshot, gen)) = Self::latest_snapshot(&conn)? else {
            return Ok(Vec::new());
        };
        Self::dead_symbols_in(&snapshot, gen)
    }

    /// Every dead-symbol row of a generation, exempt ones included.
    ///
    /// Deliberately unbounded and unfiltered: its callers compare whole
    /// generations for incremental-vs-cold equivalence, where an omitted row is
    /// the failure they exist to detect. The bounded, non-exempt read the query
    /// engine wants is [`Store::dead_symbols_page_in`].
    fn dead_symbols_in(snapshot: &Connection, gen: u32) -> Result<Vec<DeadSymbolReport>> {
        let mut stmt = snapshot.prepare(
            "SELECT symbol_name, file_path, confidence, is_exempt, exemption_reason
             FROM generation_dead_symbols
             WHERE generation_id = ?1
             ORDER BY is_exempt, confidence DESC, file_path, symbol_name, ordinal",
        )?;
        let rows = stmt.query_map(params![gen], |row| {
            Ok(DeadSymbolReport {
                symbol_name: row.get(0)?,
                file_path: row.get(1)?,
                confidence: row.get(2)?,
                is_exempt: row.get::<_, i64>(3)? != 0,
                exemption_reason: row.get(4)?,
            })
        })?;
        rows.collect()
    }

    /// Rebuild clone candidates from the latest generation's symbol rows.
    ///
    /// Returns the candidates and the number of symbols with no signature. The
    /// second half is not decoration: `group_clones` needs it to report a
    /// denominator, and a caller that assumed zero would turn "most of this
    /// tree was never examined" into "this tree is clean".
    ///
    /// Reads every symbol row of one generation. `generation_nodes` is
    /// `WITHOUT ROWID` keyed on `(generation_id, ordinal)`, so this is a
    /// primary-key range scan rather than a table scan of every generation.
    pub fn latest_clone_candidates(&self) -> Result<(Vec<CloneCandidate>, usize)> {
        let conn = lock_conn(&self.conn)?;
        let Some((snapshot, gen)) = Self::latest_snapshot(&conn)? else {
            return Ok((Vec::new(), 0));
        };
        let mut stmt = snapshot.prepare(
            "SELECT p.path, n.name, n.qualified_name, n.kind, n.span_start, n.span_end,
                    n.body_exact, n.body_structural, n.body_nodes
             FROM generation_nodes n
             JOIN paths p ON p.id = n.file_id
             WHERE n.generation_id = ?1
             ORDER BY n.ordinal",
        )?;
        let rows = stmt.query_map(params![gen], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, Option<i64>>(6)?,
                row.get::<_, Option<i64>>(7)?,
                row.get::<_, Option<i64>>(8)?,
            ))
        })?;

        let mut candidates = Vec::new();
        let mut unsigned = 0usize;
        for row in rows {
            let (path, name, qn, kind, start, end, exact, structural, nodes) = row?;
            // All three or none. A row missing any part carries no usable
            // signature, and half a signature must not be grouped on.
            let (Some(exact), Some(structural), Some(nodes)) = (exact, structural, nodes) else {
                unsigned += 1;
                continue;
            };
            // A kind this binary does not know cannot be grouped: the Type-2
            // rule is stated in terms of kinds, and applying it to an
            // uninterpretable one would be a guess.
            let Some(kind) = SymbolKind::from_persisted(&kind) else {
                unsigned += 1;
                continue;
            };
            let (span_start, span_end) = checked_span(&path, &name, start, end)?;
            candidates.push(CloneCandidate {
                file_path: path,
                symbol_name: name,
                qualified_name: qn,
                span_start,
                span_end,
                kind,
                // Reverses the bit-preserving cast made on write.
                exact: exact as u64,
                structural: structural as u64,
                nodes: nodes.clamp(0, i64::from(u32::MAX)) as u32,
            });
        }
        Ok((candidates, unsigned))
    }

    /// Count dead-symbol rows whose persisted confidence is at least `min`
    /// in milliconfidence space, so `0.9` matches HIGH rows SQLite REAL
    /// cannot round-trip from `f32`.
    pub fn count_dead_at_least(&self, min: f32) -> Result<u32> {
        let conn = lock_conn(&self.conn)?;
        let Some((snapshot, gen)) = Self::latest_snapshot(&conn)? else {
            return Ok(0);
        };
        snapshot.query_row(
            "SELECT COUNT(*) FROM generation_dead_symbols
             WHERE generation_id = ?1
               AND CAST(ROUND(confidence * 1000) AS INTEGER)
                   >= CAST(ROUND(?2 * 1000) AS INTEGER)",
            params![gen, min],
            |row| row.get(0),
        )
    }

    fn latest_generation_id_locked(conn: &Connection) -> Result<Option<u32>> {
        conn.query_row(
            "SELECT id FROM generations ORDER BY id DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()
    }

    /// The latest generation, and a read snapshot its rows are still in.
    ///
    /// Every reader here resolved "the latest generation" with one statement
    /// and then read that generation's rows with another. Those are two
    /// statements in SQLite's autocommit mode, which means **two snapshots**:
    /// `lock_conn` is a Rust mutex and serialises this process's own threads,
    /// it does not hold a database read. A second *process* — the daemon,
    /// which is designed to commit while clients query — could therefore
    /// commit and prune between them, and a reader that had pinned a
    /// now-deleted generation read zero rows out of it and returned them as
    /// the answer.
    ///
    /// Measured with a real second process committing and pruning in a loop
    /// against a store that always held twelve files, 20 s per run, release
    /// build:
    ///
    /// | retention | reads | false-empty answers |
    /// |---|---|---|
    /// | `prune(1)` | 182,781 | 31 (18 `search_page`, 6 `latest_edges_for_file`, 4 `latest_extractions`, 3 `all_symbols`) |
    /// | `prune(GENERATION_RETENTION)` | 192,970 | 1 (`latest_edges_for_file`) |
    ///
    /// Every one of those is a Class A failure and not merely a stale answer:
    /// `search_page` returned `total: 0, rows: []`, which is byte-identical to
    /// a query that ran and matched nothing, and an empty `callers_of` is read
    /// by `dev verify` as proof a symbol has no callers.
    ///
    /// A `DEFERRED` transaction takes its snapshot at its first statement,
    /// which is the generation lookup below, and holds it for every later read
    /// — so the generation a reader pins is still there, with its rows, for as
    /// long as it is reading. It takes no write lock and blocks no writer; the
    /// only thing it defers is WAL truncation, for the microseconds to
    /// milliseconds a read takes. Rolled back on drop, which for a read
    /// transaction is free.
    ///
    /// `None` means the store holds no generation at all, which is a different
    /// answer from a generation that matched nothing.
    fn latest_snapshot(conn: &Connection) -> Result<Option<(rusqlite::Transaction<'_>, u32)>> {
        let snapshot = conn.unchecked_transaction()?;
        match Self::latest_generation_id_locked(&snapshot)? {
            Some(generation) => Ok(Some((snapshot, generation))),
            None => Ok(None),
        }
    }

    /// Refuse a search whose index is not there, instead of reporting that the
    /// corpus does not contain the query.
    ///
    /// The full-text index lives in `nodes_fts`/`nodes_fts_map`, structures
    /// separate from `generation_nodes`, and the store already assumes they can
    /// be lost on their own: [`Store::repair_fts`] and `devmap repair --fts`
    /// exist for that state and nothing else. Nothing *detected* it. Measured
    /// on a store whose four symbol rows were intact and whose index rows had
    /// been removed:
    ///
    /// ```text
    /// all_symbols  = 4
    /// search_page  = Some(SearchPage { generation: 1, total: 0, rows: [] })
    /// status       = node_count 4, degraded_reason: None
    /// ```
    ///
    /// `total: 0, rows: []` is byte-identical to a healthy index that matched
    /// nothing, so every `search` against that store answered "this repository
    /// does not contain that symbol" — permanently, and without ever naming the
    /// one command that fixes it.
    ///
    /// **What this detects, and what it does not.** It answers "does this
    /// generation have any searchable row at all", not "is the index complete".
    /// A *partially* lost index is not caught: with half of one generation's
    /// postings deleted, search returned 10 of 40 symbols and reported the 10
    /// as the whole answer, and this check passes that store. Catching a
    /// partial loss means counting the generation's index rows against its
    /// symbol rows on every query, which is O(symbols) on a path that is
    /// otherwise a bounded FTS lookup. `devmap repair --fts` rebuilds the index
    /// unconditionally and is the complete answer; this is the cheap one that
    /// turns the total loss from silence into a refusal.
    ///
    /// Called only when a search came back empty, so a query that matched
    /// nothing pays two `EXISTS` probes — both primary-key range lookups on
    /// `WITHOUT ROWID` tables — and a query that matched pays nothing.
    fn require_searchable_index(conn: &Connection, gen: u32) -> Result<()> {
        let has_symbols: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM generation_nodes WHERE generation_id = ?1)",
            params![gen],
            |row| row.get::<_, i64>(0).map(|found| found != 0),
        )?;
        // A generation that indexed no symbols has nothing for the index to
        // hold, so its empty answer is the truth rather than a missing check.
        if !has_symbols {
            return Ok(());
        }
        // Both halves of the desync fail here, and they fail differently: the
        // map can be lost while the postings survive (no row matches the
        // generation), and the postings can be lost while the map survives (the
        // rowid join finds nothing). One query covers both.
        let searchable: bool = conn.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM nodes_fts_map m
                JOIN nodes_fts f ON f.rowid = m.rowid_ref
                WHERE m.generation_id = ?1
             )",
            params![gen],
            |row| row.get::<_, i64>(0).map(|found| found != 0),
        )?;
        if searchable {
            return Ok(());
        }
        Err(rusqlite::Error::InvalidParameterName(format!(
            "generation {gen} has symbol rows but no full-text index rows, so \
             this search could not run and its empty result is not an answer \
             about the repository; rebuild the index with `devmap repair --fts`"
        )))
    }

    /// Every file indexed by one named generation.
    ///
    /// Refuses a generation the store does not hold, rather than answering
    /// `[]`. This is the only reader that takes its generation id from the
    /// caller, and every caller resolves that id in a *separate* call —
    /// `devmap-query`'s `savings` does `latest_generation_id()` and then this,
    /// with a prune-capable writer free to commit twice in between. Measured
    /// against the pre-refusal code, all three of these returned the same
    /// `Ok([])`: a live generation holding one file, that same generation once
    /// `prune_generations_except_latest` had removed it, and generation 9999,
    /// which was never written.
    ///
    /// So a lost generation reached `savings` as `corpus_bytes: 0,
    /// corpus_files_unreadable: 0` — a repository with nothing in it — and the
    /// report whose own documentation refuses to count an unreadable file as
    /// zero bytes did exactly that one level up. "Not in this store" and
    /// "indexed no files" are different facts and only one of them is an
    /// answer.
    pub fn list_generation_paths(&self, generation_id: u32) -> Result<Vec<String>> {
        let conn = lock_conn(&self.conn)?;
        let present: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM generations WHERE id = ?1)",
            params![generation_id],
            |row| row.get::<_, i64>(0).map(|found| found != 0),
        )?;
        if !present {
            return Err(rusqlite::Error::InvalidParameterName(format!(
                "generation {generation_id} is not in this store — it was pruned \
                 or never written — so the files it indexed are unknown, not none; \
                 re-read the latest generation id and ask again"
            )));
        }
        let mut stmt = conn.prepare(
            "SELECT DISTINCT p.path FROM generation_nodes n
             JOIN paths p ON p.id = n.file_id
             WHERE n.generation_id = ?1",
        )?;
        let rows = stmt.query_map(params![generation_id], |row| row.get(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Fraction of the database that must be free before a full `VACUUM` earns
    /// its exclusive lock and whole-file rewrite.
    pub const VACUUM_FREELIST_RATIO: f64 = 0.05;

    /// Whether the current page accounting justifies a `VACUUM`.
    ///
    /// Split out from [`Store::vacuum_if_needed`] because the decision and the
    /// effect are separately wrong-able and only the decision is cheaply
    /// observable. Mutation testing replaced this predicate's `&&` with `||`
    /// and its `/` with `*` and `%` without any test failing: every surviving
    /// mutant still vacuumed in the one scenario under test, and below the
    /// threshold "declined to vacuum" and "vacuumed but reclaimed nothing" are
    /// indistinguishable from page counts alone. Exposed as a pure function so
    /// the policy can be asserted directly instead of inferred from a side
    /// effect it does not reliably produce.
    pub fn should_vacuum(freelist_count: i64, page_count: i64) -> bool {
        page_count > 0 && (freelist_count as f64 / page_count as f64) > Self::VACUUM_FREELIST_RATIO
    }

    /// Rewrite an existing store at [`Self::PAGE_SIZE`].
    ///
    /// Page size is fixed when a database first gets content, so a store
    /// written before the default was raised keeps its old one for life — the
    /// pragma in `configure_connection` is accepted and ignored, and the daemon
    /// reopens whatever it finds, so nothing in the normal course of running
    /// ever converts one. This is the supported way, and it is deliberately an
    /// operator action: the rewrite takes an exclusive lock and leaves WAL for
    /// its duration.
    ///
    /// `VACUUM` alone will not do it. SQLite refuses to change `page_size` on a
    /// WAL database and reports no error when it refuses, so the journal mode
    /// has to come down for the rewrite and go back up after. Measured on a
    /// 299 MB store: 2 s, 299 MB -> 296 MB.
    ///
    /// WAL is restored on the failure path too. A store left in DELETE mode
    /// still works but blocks readers behind every writer, which is a
    /// performance cliff nobody would attribute to a repair that errored.
    pub fn convert_page_size(&self) -> anyhow::Result<PageSizeConversion> {
        let _writer = self.lock_writer(Self::WRITER_LOCK_WAIT)?;
        let conn = lock_conn(&self.conn)?;
        let before: i64 = conn.query_row("PRAGMA page_size", [], |row| row.get(0))?;
        if before == Self::PAGE_SIZE {
            return Ok(PageSizeConversion {
                before,
                after: before,
                converted: false,
            });
        }

        let rewrite = (|| -> rusqlite::Result<()> {
            conn.pragma_update(None, "journal_mode", "DELETE")?;
            conn.pragma_update(None, "page_size", Self::PAGE_SIZE)?;
            conn.execute_batch("VACUUM")?;
            Ok(())
        })();
        // Back to WAL whether or not the rewrite worked.
        let restored = conn.pragma_update(None, "journal_mode", "WAL");
        rewrite?;
        restored?;

        let after: i64 = conn.query_row("PRAGMA page_size", [], |row| row.get(0))?;
        if after != Self::PAGE_SIZE {
            anyhow::bail!(
                "page size is still {after} after the rewrite; expected {}. The database was \
                 not converted and is unchanged.",
                Self::PAGE_SIZE
            );
        }
        Ok(PageSizeConversion {
            before,
            after,
            converted: true,
        })
    }

    /// Free pages one `vacuum_if_needed` will reclaim at most.
    ///
    /// Incremental vacuum costs time proportional to the pages it moves, so
    /// this bounds a single build's reclaim rather than the database's size.
    /// 65,536 pages is 256 MiB at the default 4 KiB page size — far above the
    /// per-build churn measured here (a prune frees on the order of 5% of the
    /// file), so the steady state reclaims everything in one pass and the cap
    /// only bites when a long-neglected store has accumulated a backlog. That
    /// backlog then drains over consecutive builds instead of stalling one.
    /// Expressed in bytes, then converted to pages against the page size the
    /// database actually has.
    ///
    /// This bound is on *time*, and the doc above says why: incremental vacuum
    /// costs time proportional to the pages it moves. Pages are not a fixed
    /// amount of work — a page is 4 KiB in a store written before the page size
    /// was raised and 16 KiB in one written after, so a constant expressed in
    /// pages means four times the bytes, and four times the stall, depending on
    /// which store it is applied to. It was 65,536 pages, calibrated at 4 KiB;
    /// 256 MiB is that same budget stated in the unit the cost is actually
    /// proportional to.
    const INCREMENTAL_VACUUM_MAX_BYTES: i64 = 256 * 1024 * 1024;

    /// The cap above in pages, for a database with `page_size`-byte pages.
    ///
    /// Never zero: a page size larger than the whole budget would otherwise
    /// request a reclaim of nothing and report it as a bounded one, which is a
    /// check that could not run reporting as a check that passed.
    pub fn incremental_vacuum_max_pages(page_size: i64) -> i64 {
        if page_size <= 0 {
            return 1;
        }
        (Self::INCREMENTAL_VACUUM_MAX_BYTES / page_size).max(1)
    }

    /// How long a TRUNCATE checkpoint waits for a reader before falling back to
    /// PASSIVE. See [`Store::checkpoint_wal`] for why it is not zero.
    const CHECKPOINT_BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(250);

    /// Reclaim free pages, cheaply where the database allows it.
    ///
    /// **Why not a plain `VACUUM`.** `VACUUM` rebuilds the entire database into
    /// a new file: its cost is proportional to the *database*, not to the waste
    /// being reclaimed, and it takes an exclusive lock for the duration. Because
    /// every build prunes a generation, the freelist crosses
    /// [`Self::VACUUM_FREELIST_RATIO`] on essentially every build — so the
    /// whole-file rewrite ran nearly every time. Measured on DevCouncil's own
    /// store: 937 ms of a 3.40 s incremental build, 28% of the wall time, to
    /// reclaim a few percent of the file.
    ///
    /// `PRAGMA incremental_vacuum(N)` moves only free pages to the end and
    /// truncates, costing what the waste costs. It requires the database to
    /// have been created with `auto_vacuum = INCREMENTAL`; a database in mode
    /// NONE cannot be switched without a full rewrite, so those keep the old
    /// path. That is the honest fallback — an incremental vacuum on a mode-NONE
    /// database is a silent no-op, and a reclaim that quietly reclaims nothing
    /// is exactly the failure `vacuum_returns_freed_pages_to_the_filesystem`
    /// exists to catch.
    ///
    /// **The trade this makes, stated plainly.** A full `VACUUM` compacted the
    /// file to its live size every build; this does not. Measured over 15
    /// consecutive incremental builds of DevCouncil, the store settles at
    /// 295 MB against ~197 MB of live data and *stays there* — the free pages
    /// left by each prune are reused by the next generation's write instead of
    /// being returned to the filesystem and immediately re-allocated. So the
    /// cost is a bounded ~50% space overhead, not unbounded growth, and the
    /// bound is what makes it acceptable: the file did not move off 295 MB
    /// across those 15 builds, and the WAL stayed truncated. Reclaim time went
    /// from 937 ms to 2 ms over the same window.
    pub fn vacuum_if_needed(&self) -> Result<VacuumOutcome> {
        // Checkpoint before reading the page accounting.
        //
        // In WAL mode `PRAGMA freelist_count` reports the *main database file*.
        // Pages freed by the two prunes that run immediately before this live
        // in the WAL until a checkpoint folds them back, so the freelist read
        // here was reporting the state before this build's pruning — and it
        // read *below* the threshold while a third of the file was in fact
        // free. Measured on this repository: eight consecutive builds each
        // declined to reclaim in 0 ms while the freelist sat at 33.2% and the
        // store stayed pinned at 295 MB; a manual `incremental_vacuum` on the
        // same file immediately took it to 0.4% and 50,684 pages.
        //
        // A reclaim policy reading stale accounting does not merely reclaim
        // late — it reports "nothing to reclaim" with perfect confidence, which
        // is the failure mode that hides indefinitely. A checkpoint failure is
        // not fatal here: the decision is then made on the same stale numbers
        // as before, so this can only improve the accuracy of the answer, and
        // refusing to reclaim because bookkeeping was unavailable would be
        // worse than reclaiming on a conservative estimate.
        let checkpoint_before = self.checkpoint_wal().ok();

        let conn = lock_conn(&self.conn)?;
        let freelist_count: i64 = conn.query_row("PRAGMA freelist_count", [], |row| row.get(0))?;
        let page_count: i64 = conn.query_row("PRAGMA page_count", [], |row| row.get(0))?;
        if !Self::should_vacuum(freelist_count, page_count) {
            return Ok(VacuumOutcome {
                freelist_before: freelist_count,
                page_count_before: page_count,
                action: VacuumAction::Declined,
                // Nothing was reclaimed, so the checkpoint that matters is the
                // one taken above to make the accounting current.
                checkpoint: checkpoint_before,
                pages_freed: 0,
            });
        }
        // 0 = NONE, 1 = FULL, 2 = INCREMENTAL. Only 2 supports the pragma.
        let auto_vacuum: i64 = conn.query_row("PRAGMA auto_vacuum", [], |row| row.get(0))?;
        if auto_vacuum == 2 {
            let page_size: i64 = conn.query_row("PRAGMA page_size", [], |row| row.get(0))?;
            let requested = freelist_count.min(Self::incremental_vacuum_max_pages(page_size));
            // Step the pragma to exhaustion, and count what it moved.
            //
            // `PRAGMA incremental_vacuum(N)` is not a statement that does its
            // work on the first step and then reports: it frees **one page per
            // row stepped**, up to N. Neither `execute` nor `execute_batch`
            // does that. `execute` refuses a statement that returns rows
            // outright (`ExecuteReturnedResults`), and `execute_batch` — the
            // workaround that was here — steps once and moves to the next
            // statement in the batch (rusqlite 0.31 `lib.rs::execute_batch`).
            // So the reclaim freed exactly one page per build, for as long as
            // this code has existed, while printing the number it had asked
            // for. Measured on the live store: 1 ms, one page, four builds in a
            // row, 701 MB unchanged at a 67.7% freelist.
            //
            // A PRAGMA argument cannot be bound as a parameter; `requested` is
            // derived from `PRAGMA freelist_count` and a compile-time constant,
            // never from a caller.
            let pages_freed = {
                let mut stmt = conn.prepare(&format!("PRAGMA incremental_vacuum({requested})"))?;
                let mut rows = stmt.query([])?;
                let mut freed: i64 = 0;
                while rows.next()?.is_some() {
                    freed += 1;
                }
                freed
            };
            // Checkpoint *after* the reclaim, not only before it. The
            // truncation the pragma just performed is a WAL frame; without this
            // it never reaches the main file, and the store reports pages
            // reclaimed while its size does not move. See
            // `VacuumOutcome::checkpoint`.
            drop(conn);
            let checkpoint = self.checkpoint_wal().ok();
            return Ok(VacuumOutcome {
                freelist_before: freelist_count,
                page_count_before: page_count,
                action: VacuumAction::Incremental { requested },
                checkpoint,
                pages_freed,
            });
        }

        // A store already in mode NONE is converted here rather than at open.
        // Switching `auto_vacuum` on a populated database only takes effect on
        // the next full rewrite — and this branch is that rewrite. The
        // conversion is therefore free: this build was going to pay for a
        // `VACUUM` either way, and every build after it takes the bounded path
        // above. Doing it in `open` instead would put a whole-file rewrite in
        // front of read commands like `devmap status`, which must stay cheap.
        conn.pragma_update(None, "auto_vacuum", "INCREMENTAL")?;
        conn.execute("VACUUM", [])?;
        drop(conn);
        let checkpoint = self.checkpoint_wal().ok();
        Ok(VacuumOutcome {
            freelist_before: freelist_count,
            page_count_before: page_count,
            action: VacuumAction::FullConverting,
            checkpoint,
            // A full `VACUUM` rewrites the file without its free pages, so
            // every page that was free is gone.
            pages_freed: freelist_count,
        })
    }

    /// Attempt to truncate the WAL and explicitly fall back to a non-blocking
    /// passive checkpoint when an active reader prevents truncation (S18).
    pub fn checkpoint_wal(&self) -> Result<WalCheckpointResult> {
        fn run(conn: &Connection, pragma: &str) -> Result<(i64, i64, i64)> {
            conn.query_row(pragma, [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
        }

        let conn = lock_conn(&self.conn)?;
        let previous_busy_ms: i64 = conn.query_row("PRAGMA busy_timeout", [], |row| row.get(0))?;
        let previous_busy_ms = u64::try_from(previous_busy_ms).map_err(|_| {
            rusqlite::Error::InvalidParameterName(
                "SQLite returned a negative busy_timeout".to_string(),
            )
        })?;

        // TRUNCATE honors busy_timeout and could otherwise monopolize the
        // store mutex for seconds while a reader holds a snapshot. Bound the
        // wait instead of removing it, then use PASSIVE as the non-blocking
        // fallback.
        //
        // K2: the bound used to be zero, which is not a short wait — it is no
        // wait at all, and it loses to any reader that happens to hold the WAL
        // at that instant. PASSIVE then runs, and PASSIVE *cannot truncate*, so
        // the pages an incremental vacuum just freed stayed in a WAL that grew
        // to 109 MB while the main file never moved. A quarter of a second is
        // long enough to outlast a transient reader and short enough that no
        // build notices it.
        conn.busy_timeout(Self::CHECKPOINT_BUSY_TIMEOUT)?;
        let checkpoint = (|| {
            let (busy, log_frames, checkpointed_frames) =
                run(&conn, "PRAGMA wal_checkpoint(TRUNCATE)")?;
            if busy == 0 {
                return Ok(WalCheckpointResult {
                    mode: WalCheckpointMode::Truncate,
                    busy,
                    log_frames,
                    checkpointed_frames,
                });
            }

            let (busy, log_frames, checkpointed_frames) =
                run(&conn, "PRAGMA wal_checkpoint(PASSIVE)")?;
            Ok(WalCheckpointResult {
                mode: WalCheckpointMode::Passive,
                busy,
                log_frames,
                checkpointed_frames,
            })
        })();
        let restored = conn.busy_timeout(std::time::Duration::from_millis(previous_busy_ms));
        match (checkpoint, restored) {
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
            (Ok(result), Ok(())) => Ok(result),
        }
    }

    pub fn repair_fts(&self) -> Result<()> {
        let mut conn = lock_conn(&self.conn)?;
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM nodes_fts", [])?;
        tx.execute("DELETE FROM nodes_fts_map", [])?;
        let gen: Option<u32> = tx
            .query_row(
                "SELECT id FROM generations ORDER BY id DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(g) = gen {
            let mut stmt = tx.prepare(
                "SELECT n.ordinal, n.name, n.qualified_name, p.path
                 FROM generation_nodes n
                 JOIN paths p ON n.file_id = p.id
                 WHERE n.generation_id = ?1",
            )?;
            let rows = stmt.query_map(params![g], |row| {
                Ok((
                    row.get::<_, u32>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?;
            let collected: Vec<_> = rows.collect::<Result<Vec<_>>>()?;
            drop(stmt);
            for (ord, name, qn, path) in collected {
                let fts_rowid = Self::fts_rowid(g, ord);
                tx.prepare_cached(
                    "INSERT INTO nodes_fts (rowid, name, qualified_name, path) VALUES (?1, ?2, ?3, ?4)",
                )?
                .execute(params![fts_rowid, name, qn, path])?;
                tx.prepare_cached(
                    "INSERT INTO nodes_fts_map (rowid_ref, generation_id) VALUES (?1, ?2)",
                )?
                .execute(params![fts_rowid, g])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn prune_generations_except_latest(&self, keep_generations: usize) -> Result<usize> {
        let mut conn = lock_conn(&self.conn)?;

        // Always retain the latest generation; the method name promises that
        // older generations are pruned while the current one remains usable.
        let keep_generations = keep_generations.max(1);

        // The candidate list is read inside the write transaction. Choosing the
        // rows to delete and deleting them is one decision: a DEFERRED
        // transaction would let a concurrent writer commit a new generation
        // between the SELECT and the DELETEs, so the stale list could prune a
        // generation that is now within the retention window.
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        let gen_ids: Vec<u32> = {
            let mut stmt = tx.prepare("SELECT id FROM generations ORDER BY id DESC")?;
            let ids = stmt
                .query_map([], |row| row.get(0))?
                .collect::<Result<Vec<_>>>()?;
            ids
        };

        if gen_ids.len() <= keep_generations {
            return Ok(0);
        }

        let to_prune = &gen_ids[keep_generations..];
        let mut pruned_count = 0;

        for &old_gen in to_prune {
            tx.execute(
                "DELETE FROM nodes_fts WHERE rowid IN (SELECT rowid_ref FROM nodes_fts_map WHERE generation_id = ?1)",
                params![old_gen],
            )?;
            tx.execute(
                "DELETE FROM nodes_fts_map WHERE generation_id = ?1",
                params![old_gen],
            )?;
            tx.execute(
                "DELETE FROM generation_nodes WHERE generation_id = ?1",
                params![old_gen],
            )?;
            tx.execute(
                "DELETE FROM generation_file_rows WHERE generation_id = ?1",
                params![old_gen],
            )?;
            tx.execute(
                "DELETE FROM generation_edges WHERE generation_id = ?1",
                params![old_gen],
            )?;
            tx.execute(
                "DELETE FROM generation_unresolved WHERE generation_id = ?1",
                params![old_gen],
            )?;
            tx.execute(
                "DELETE FROM generation_coverage_gaps WHERE generation_id = ?1",
                params![old_gen],
            )?;
            tx.execute(
                "DELETE FROM generation_dead_symbols WHERE generation_id = ?1",
                params![old_gen],
            )?;
            tx.execute("DELETE FROM generations WHERE id = ?1", params![old_gen])?;
            pruned_count += 1;
        }

        // A payload outlives its generation only for as long as some *other*
        // generation still names it. Deleting the membership rows above frees
        // nothing on its own — the bytes are in `file_payloads`, and since v17
        // that is where 54% of this store lives — so the orphans go too.
        //
        // Deferred to after the loop rather than run per generation: a payload
        // shared by two pruned generations would otherwise be probed twice, and
        // the anti-join is one index scan either way.
        tx.execute(
            "DELETE FROM file_payloads
              WHERE payload_id NOT IN (SELECT payload_id FROM generation_file_rows)",
            [],
        )?;

        // FTS5 deletes only tombstone their postings; without a merge the freed
        // space stays inside the index and the prune reclaims nothing there.
        //
        // Unconditional by construction: the early return above leaves
        // `gen_ids.len() > keep_generations`, so `to_prune` is never empty and
        // the loop always deleted at least one generation. A `pruned_count > 0`
        // guard here was always true — mutation testing flagged it precisely
        // because no test could distinguish its branches.
        tx.execute("INSERT INTO nodes_fts(nodes_fts) VALUES('optimize')", [])?;

        tx.commit()?;
        Ok(pruned_count)
    }

    /// Drop cached extractions no retained generation can still use (SC7).
    ///
    /// `extraction_cache` is keyed by content hash, so every edit to a file
    /// adds a row for the new content and leaves the old one behind forever —
    /// nothing ever deleted from this table. Measured: five edits to one file
    /// leave five rows, and on a 4,742-file repository the table reached
    /// 198 MiB of a 525 MiB database. An always-on watcher would grow it
    /// without bound.
    ///
    /// Eviction is by reachability, not recency. Recency is actively wrong
    /// here: a file untouched for months has an old `accessed_at` but its
    /// cached entry is precisely the one the next build needs, while the rows
    /// worth dropping are the superseded versions of files being edited right
    /// now. Keying on "is this content still referenced by a generation we
    /// kept" bounds the cache to the retained working set.
    ///
    /// A row is kept only when it is the *only* thing that can answer a lookup
    /// for its content: reachable from a retained generation, and not already
    /// answerable from that generation's own payload.
    ///
    /// S-4: the rule used to be stated as two clauses — drop what no generation
    /// references, and drop what a generation holds under the *same* full
    /// identity — and between them sat the rows an extraction-schema bump
    /// creates. A file cached under `(hash, python, g1, a1)` and re-extracted
    /// after a bump into `(hash, python, g2, a2)` kept its `(hash, python)`
    /// reachability, so clause one spared it, and its identity no longer
    /// matched, so clause two could not touch it — while the *servable* copy
    /// was evicted as a duplicate. Nothing could serve it and nothing could
    /// evict it, so every bump added a full extra copy of every payload to a
    /// table the paragraph above calls bounded.
    ///
    /// Stated as reachability instead: if a retained generation records a
    /// usable identity for this content, [`Self::try_get_cached_extraction`]
    /// answers from that generation, so no cache copy of it is reachable —
    /// whether its identity matches (the generation serves it) or not (nothing
    /// can). Only content whose generation rows carry NULL identity — written
    /// before schema v8, and deliberately never eligible for the fallback —
    /// still needs its cache row, and that row survives.
    ///
    /// Must run *after* `prune_generations_except_latest`, so `generation_files`
    /// already describes only retained generations.
    pub fn prune_extraction_cache(&self) -> Result<usize> {
        let mut conn = lock_conn(&self.conn)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let removed = tx.execute(
            "DELETE FROM extraction_cache
             WHERE (content_hash, language) NOT IN
                   (SELECT content_hash, language FROM generation_files)
                OR EXISTS (SELECT 1 FROM generation_files g
                            WHERE g.content_hash = extraction_cache.content_hash
                              AND g.language     = extraction_cache.language
                              AND g.grammar_version  IS NOT NULL
                              AND g.analyzer_version IS NOT NULL)",
            [],
        )?;
        tx.commit()?;
        Ok(removed)
    }

    #[cfg(feature = "parse")]
    pub fn try_get_cached_extraction(
        &self,
        key: &devmap_extract::cache::CacheKey,
    ) -> Result<Option<devmap_extract::model::Extraction>> {
        let conn = lock_conn(&self.conn)?;
        let payload: Option<String> = conn
            .query_row(
                "SELECT payload_json FROM extraction_cache
                 WHERE content_hash = ?1 AND language = ?2
                   AND grammar_version = ?3 AND analyzer_version = ?4",
                params![
                    key.content_hash as i64,
                    key.language,
                    key.grammar_version,
                    key.analyzer_version
                ],
                |row| row.get(0),
            )
            .optional()?;

        // Fall back to a retained generation's copy (SC8).
        //
        // `generation_files` holds a byte-identical payload for the same
        // content, so keeping both was storing every extraction twice — 198 MiB
        // of a 525 MiB database on one corpus. The fallback matches on the FULL
        // cache identity, including grammar and analyzer version, so it cannot
        // serve a payload produced by older extraction semantics; rows written
        // before schema v8 carry NULL there and are therefore never eligible.
        // Absence of a recorded identity is not proof of a matching one.
        let payload = match payload {
            Some(found) => Some(("extraction_cache", found)),
            None => conn
                .query_row(
                    "SELECT extraction_json FROM generation_files
                     WHERE content_hash = ?1 AND language = ?2
                       AND grammar_version = ?3 AND analyzer_version = ?4
                     LIMIT 1",
                    params![
                        key.content_hash as i64,
                        key.language,
                        key.grammar_version,
                        key.analyzer_version
                    ],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
                .map(|json| ("generation_files", json)),
        };
        // S-5: a stored payload that will not parse is a store fault, not a
        // cache miss. `.ok()` here re-extracted the file on every build for
        // ever and threw away the only evidence that a row was corrupt — the
        // one JSON read in this file that stayed quiet while every other names
        // what it could not read.
        payload
            .map(|(table, json)| {
                serde_json::from_str(&json).map_err(|error| {
                    rusqlite::Error::InvalidParameterName(format!(
                        "stored extraction payload in {table} for content \
                         {hash:#018x} ({language}, grammar {grammar}, analyzer \
                         {analyzer}) is invalid: {error}",
                        hash = key.content_hash,
                        language = key.language,
                        grammar = key.grammar_version,
                        analyzer = key.analyzer_version,
                    ))
                })
            })
            .transpose()
    }

    #[cfg(feature = "parse")]
    pub fn admit_cached_extraction(
        &self,
        key: &devmap_extract::cache::CacheKey,
        ext: &devmap_extract::model::Extraction,
    ) -> Result<()> {
        if !devmap_extract::cache::cache_admits(&ext.parse_outcome) {
            return self.record_extraction_retry(key, "ParseOutcome::Failed");
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        let mut cached = ext.for_durable_store();
        // Source text is already identified by the content hash and remains on
        // disk; duplicating it in both cache and generation rows bloats the DB.
        cached.source_code = None;
        let payload = serde_json::to_string(&cached).map_err(|err| {
            rusqlite::Error::InvalidParameterName(format!("cache serialize failed: {err}"))
        })?;
        let conn = lock_conn(&self.conn)?;
        conn.execute(
            "INSERT INTO extraction_cache (content_hash, language, grammar_version, analyzer_version, payload_json, accessed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(content_hash, language, grammar_version, analyzer_version)
             DO UPDATE SET payload_json = excluded.payload_json, accessed_at = excluded.accessed_at",
            params![
                key.content_hash as i64,
                key.language,
                key.grammar_version,
                key.analyzer_version,
                payload,
                now
            ],
        )?;
        Ok(())
    }

    #[cfg(feature = "parse")]
    pub fn record_extraction_retry(
        &self,
        key: &devmap_extract::cache::CacheKey,
        reason: &str,
    ) -> Result<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        let conn = lock_conn(&self.conn)?;
        conn.execute(
            "INSERT INTO extraction_retry (content_hash, language, attempts, last_reason, updated_at)
             VALUES (?1, ?2, 1, ?3, ?4)
             ON CONFLICT(content_hash) DO UPDATE SET
               attempts = attempts + 1,
               last_reason = excluded.last_reason,
               updated_at = excluded.updated_at",
            params![key.content_hash as i64, key.language, reason, now],
        )?;
        Ok(())
    }

    pub fn extraction_retry_count(&self, content_hash: u64) -> Result<u32> {
        let conn = lock_conn(&self.conn)?;
        conn.query_row(
            "SELECT attempts FROM extraction_retry WHERE content_hash = ?1",
            params![content_hash as i64],
            |row| row.get(0),
        )
        .optional()
        .map(|opt| opt.unwrap_or(0))
    }
}

#[cfg(test)]
#[cfg(feature = "parse")]
mod carry_forward_tests {
    use super::*;
    use devmap_analyze::analyze;
    use devmap_extract::extract_file;
    use devmap_extract::model::Extraction;
    use devmap_resolve::Resolver;

    /// Every stored column of one `generation_files` row, in schema order.
    type FileRow = (
        i64,
        String,
        i64,
        String,
        String,
        String,
        Option<String>,
        Option<String>,
    );

    fn read_row(store: &Store, generation: u32, path: &str) -> FileRow {
        let conn = lock_conn(&store.conn).expect("connection");
        conn.query_row(
            "SELECT f.file_id, f.language, f.content_hash, f.parse_outcome_json,
                    f.engine_json, f.extraction_json, f.grammar_version, f.analyzer_version
             FROM generation_files f
             JOIN paths p ON p.id = f.file_id
             WHERE f.generation_id = ?1 AND p.path = ?2",
            params![generation, path],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ))
            },
        )
        .unwrap_or_else(|error| panic!("no row for {path} in generation {generation}: {error}"))
    }

    fn commit(store: &Store, exts: &[Extraction], opts: GenerationWriteOpts) -> Result<u32> {
        let mut resolver = Resolver::new();
        resolver.index_extractions(exts);
        let resolution = resolver.resolve_all(exts);
        let analysis = analyze(exts, &resolution);
        store.save_generation_with_opts(exts, &resolution, &analysis, opts)
    }

    fn tree(a_body: &str) -> Vec<Extraction> {
        vec![
            extract_file("a.py", a_body),
            extract_file("b.py", "def beta():\n    return 2\n"),
            extract_file("c.py", "def gamma():\n    return 3\n"),
        ]
    }

    /// A carried row is the stored row — every column, `file_id` included.
    ///
    /// The carry-forward moved from a Rust loop that read each row (payload and
    /// all) and wrote it back, to a single `INSERT … SELECT` inside SQLite. The
    /// rows must be indistinguishable, so this compares them column by column
    /// rather than trusting that the copy "looks right": a transposed column in
    /// the nine-column insert list is exactly the kind of defect that leaves a
    /// store readable and wrong.
    ///
    /// `file_id` is asserted too. The old loop re-derived it from the path
    /// string through `paths`; this carries the stored id, and the two must
    /// agree or a carried row would point at a different file than the one it
    /// was written for.
    ///
    /// The identity columns are `Option`, but a carried row can never hold
    /// `None` in them — `identity_matches` above requires `Some` on both — so
    /// this pins that they survive, not that NULL is carryable.
    #[test]
    fn the_carried_row_is_the_stored_row() {
        let store = Store::open_in_memory().expect("store");
        let first = tree("def alpha():\n    return 1\n");
        assert_eq!(
            commit(&store, &first, GenerationWriteOpts::default()).expect("cold"),
            1
        );
        let before_b = read_row(&store, 1, "b.py");
        let before_c = read_row(&store, 1, "c.py");

        // Only a.py changes, so b.py and c.py are carried.
        let second = tree("def alpha():\n    return 11\n");
        let gen = commit(
            &store,
            &second,
            GenerationWriteOpts {
                affected_paths: vec!["a.py".into()],
                ..GenerationWriteOpts::default()
            },
        )
        .expect("incremental");
        assert_eq!(gen, 2);

        assert_eq!(
            before_b,
            read_row(&store, 2, "b.py"),
            "b.py carried verbatim"
        );
        assert_eq!(
            before_c,
            read_row(&store, 2, "c.py"),
            "c.py carried verbatim"
        );
        // ... and the file that did change is not carried: its payload moved.
        assert_ne!(
            read_row(&store, 1, "a.py").5,
            read_row(&store, 2, "a.py").5,
            "a.py was affected, so its payload must be the freshly extracted one"
        );
    }

    /// A row produced by a different extractor is replaced, never carried.
    ///
    /// This is the invariant the identity gate exists for, and nothing in this
    /// crate covered it: with the gate's result ignored, all 127 devmap-store
    /// tests still passed. The comment above `current_hashes` records what that
    /// costs in production — after two schema bumps a store still held 1,152
    /// `extract-v23` rows under a `v25` binary, and the first changed build had
    /// no way forward but deleting the database.
    ///
    /// The stored identity is rewritten directly rather than by bumping a
    /// version constant, because the point is to reproduce a store written by
    /// *some* other extractor, not to pin which one.
    #[test]
    fn a_row_from_a_different_extractor_is_replaced_not_carried() {
        let store = Store::open_in_memory().expect("store");
        let first = tree("def alpha():\n    return 1\n");
        commit(&store, &first, GenerationWriteOpts::default()).expect("cold");
        let current_grammar = read_row(&store, 1, "b.py").6;

        // b.py now looks like it was written by an extractor this build is not.
        {
            let conn = lock_conn(&store.conn).expect("connection");
            // Written through `file_payloads`, not `generation_files`: since
            // v17 the latter is a view and the identity columns live on the
            // payload the generation references.
            conn.execute(
                "UPDATE file_payloads SET grammar_version = 'grammar-from-another-era'
                  WHERE payload_id = (
                        SELECT r.payload_id
                          FROM generation_file_rows r
                          JOIN paths p ON p.id = r.file_id
                         WHERE r.generation_id = 1 AND p.path = 'b.py')",
                [],
            )
            .expect("age the row");
            // Guard the guard: an UPDATE that matched nothing would leave the
            // identity current and the assertion below would pass by carrying
            // rather than by re-extracting.
            {
                let aged: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM file_payloads
                          WHERE grammar_version = 'grammar-from-another-era'",
                        [],
                        |row| row.get(0),
                    )
                    .expect("count aged payloads");
                assert_eq!(aged, 1, "the aging UPDATE must match exactly one payload");
            }
        }

        // Only a.py is declared affected, so b.py would be carried on content
        // alone — which is precisely the bug.
        let second = tree("def alpha():\n    return 11\n");
        commit(
            &store,
            &second,
            GenerationWriteOpts {
                affected_paths: vec!["a.py".into()],
                ..GenerationWriteOpts::default()
            },
        )
        .expect("incremental");

        assert_eq!(
            read_row(&store, 2, "b.py").6,
            current_grammar,
            "a row whose stored identity is not this build's must be re-extracted, \
             not carried forward under an identity nothing checked"
        );
    }
}

#[cfg(test)]
mod connection_tests {
    use super::*;

    fn scratch(label: &str) -> std::path::PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("devmap-{label}-{}-{unique}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    /// The reclaim cap is a byte budget, so it means the same at any page size.
    ///
    /// It was `65_536` pages, calibrated when every store had 4 KiB pages. The
    /// doc on the constant says the bound exists because incremental vacuum
    /// costs time proportional to the pages it moves — and pages are not a
    /// fixed amount of work once two page sizes are in play. Left in pages, the
    /// same constant licensed 256 MiB of moving on a 4 KiB store and 1 GiB on a
    /// 16 KiB one: a time bound that quietly quadrupled.
    ///
    /// 4 KiB reproducing the original 65,536 is the part that pins the budget
    /// was carried over rather than re-guessed.
    #[test]
    fn the_reclaim_cap_is_the_same_budget_at_every_page_size() {
        assert_eq!(
            Store::incremental_vacuum_max_pages(4096),
            65_536,
            "the original calibration, restated in bytes, must come back unchanged"
        );
        assert_eq!(Store::incremental_vacuum_max_pages(16384), 16_384);
        assert_eq!(Store::incremental_vacuum_max_pages(8192), 32_768);
        for page in [4096, 8192, 16384, 32768, 65536] {
            assert_eq!(
                Store::incremental_vacuum_max_pages(page) * page,
                Store::INCREMENTAL_VACUUM_MAX_BYTES,
                "every page size must reclaim the same number of bytes per pass"
            );
        }
    }

    /// A page size larger than the whole budget still reclaims something.
    ///
    /// `bytes / page_size` is zero once the page exceeds the budget, and a
    /// request of zero pages would step the pragma zero times and then report a
    /// bounded reclaim — a check that could not run reporting as one that ran.
    /// Zero and negative are included because the value is read from
    /// `PRAGMA page_size` at runtime, not from a constant.
    #[test]
    fn the_reclaim_cap_never_requests_nothing() {
        for page in [0, -1, i64::MAX, 1 << 30] {
            assert!(
                Store::incremental_vacuum_max_pages(page) >= 1,
                "page size {page} must still request at least one page"
            );
        }
    }

    /// A new store is created with 16 KiB pages.
    ///
    /// The pragma is silently ignored on a database that already has tables, so
    /// "it is in `configure_connection`" is not evidence that it took. This
    /// reads the page size back off a store the code actually created.
    #[test]
    fn a_new_store_is_created_with_the_configured_page_size() {
        let dir = scratch("pagesize");
        let store = Store::open(dir.join("devmap.sqlite")).expect("store");
        let size: i64 = lock_conn(&store.conn)
            .expect("connection")
            .query_row("PRAGMA page_size", [], |row| row.get(0))
            .expect("page_size");
        assert_eq!(
            size, 16384,
            "a store created by this code should use the configured page size"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `repair --page-size` converts a store the daemon never will.
    ///
    /// The three things that make it safe are asserted together, because any
    /// one of them alone would pass on a broken conversion: the page size
    /// actually moved, the database came back to WAL (left in DELETE it still
    /// works, but every reader blocks behind every writer — a cliff nobody
    /// would attribute to a repair), and the rows survived the rewrite.
    ///
    /// The second call pins idempotence and that "already correct" is reported
    /// as `converted: false` rather than inferred from `before == after`.
    #[test]
    fn converting_an_existing_store_moves_it_to_the_current_page_size() {
        let dir = scratch("pagesize-convert");
        let path = dir.join("devmap.sqlite");
        {
            let conn = Connection::open(&path).expect("seed connection");
            conn.pragma_update(None, "page_size", 4096).expect("4 KiB");
            conn.execute_batch("CREATE TABLE seed (x INTEGER); DROP TABLE seed;")
                .expect("fix the page size into the file header");
        }
        let store = Store::open(&path).expect("store");
        {
            let conn = lock_conn(&store.conn).expect("connection");
            conn.execute("INSERT INTO paths (path) VALUES ('survives.py')", [])
                .expect("a row to carry across the rewrite");
        }

        let outcome = store.convert_page_size().expect("conversion");
        assert_eq!(outcome.before, 4096);
        assert_eq!(outcome.after, Store::PAGE_SIZE);
        assert!(outcome.converted, "a 4 KiB store must report as converted");

        let conn = lock_conn(&store.conn).expect("connection");
        let size: i64 = conn
            .query_row("PRAGMA page_size", [], |row| row.get(0))
            .expect("page_size");
        assert_eq!(size, Store::PAGE_SIZE);
        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .expect("journal_mode");
        assert_eq!(
            mode.to_lowercase(),
            "wal",
            "the rewrite must leave the database back in WAL"
        );
        let kept: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM paths WHERE path = 'survives.py'",
                [],
                |row| row.get(0),
            )
            .expect("row");
        assert_eq!(kept, 1, "the rewrite must not lose rows");
        drop(conn);

        let again = store.convert_page_size().expect("second conversion");
        assert_eq!(again.after, Store::PAGE_SIZE);
        assert!(
            !again.converted,
            "a store already at the target reports converted=false, not a second rewrite"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A plain `VACUUM` does not convert an existing store's page size.
    ///
    /// This pins the assumption the comment on the `page_size` pragma makes,
    /// because getting it wrong is silent: `VACUUM` adopts a pending
    /// `auto_vacuum`, so "a full vacuum converts it" reads as true for both
    /// settings and is only true for one. SQLite will not change `page_size` on
    /// a WAL database, and reports no error when it declines.
    ///
    /// Both halves are asserted — that the plain vacuum leaves 4 KiB, and that
    /// leaving WAL for the rewrite is what actually converts — so the remedy in
    /// that comment is executable rather than remembered.
    #[test]
    fn a_plain_vacuum_does_not_convert_an_existing_page_size() {
        let dir = scratch("pagesize-vacuum");
        let path = dir.join("devmap.sqlite");
        {
            let conn = Connection::open(&path).expect("seed connection");
            conn.pragma_update(None, "page_size", 4096).expect("4 KiB");
            conn.execute_batch("CREATE TABLE seed (x INTEGER); DROP TABLE seed;")
                .expect("fix the page size into the file header");
        }
        // Opening puts it in WAL, which is the state a real store is in.
        drop(Store::open(&path).expect("store"));

        let conn = Connection::open(&path).expect("connection");
        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .expect("journal_mode");
        assert_eq!(
            mode.to_lowercase(),
            "wal",
            "the store under test must be WAL"
        );

        conn.execute_batch("PRAGMA page_size=16384; VACUUM;")
            .expect("a plain vacuum must succeed, not error");
        let after_plain: i64 = conn
            .query_row("PRAGMA page_size", [], |row| row.get(0))
            .expect("page_size");
        assert_eq!(
            after_plain, 4096,
            "a plain VACUUM on a WAL database leaves the page size alone — it does \
             not report failure, which is why the claim that it converts survives"
        );

        conn.execute_batch(
            "PRAGMA journal_mode=DELETE; PRAGMA page_size=16384; VACUUM; PRAGMA journal_mode=WAL;",
        )
        .expect("the documented conversion must succeed");
        let after_documented: i64 = conn
            .query_row("PRAGMA page_size", [], |row| row.get(0))
            .expect("page_size");
        assert_eq!(
            after_documented, 16384,
            "leaving WAL for the rewrite is what actually converts the page size"
        );
        drop(conn);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A store written with the old 4 KiB pages still opens, and stays 4 KiB.
    ///
    /// Page size is fixed when a database first gets content, so every store
    /// already on disk is 4 KiB and cannot be changed by a pragma. Raising the
    /// default is only safe if those stores keep working untouched — the same
    /// conversion path `auto_vacuum` already depends on. This creates a 4 KiB
    /// file, opens it with the current code, and writes through it.
    #[test]
    fn an_existing_small_page_store_opens_and_reads() {
        let dir = scratch("pagesize-legacy");
        let path = dir.join("devmap.sqlite");
        {
            let conn = Connection::open(&path).expect("seed connection");
            conn.pragma_update(None, "page_size", 4096).expect("4 KiB");
            conn.execute_batch("CREATE TABLE seed (x INTEGER); DROP TABLE seed;")
                .expect("fix the page size into the file header");
        }

        let store = Store::open(&path).expect("an existing 4 KiB store must still open");
        let size: i64 = lock_conn(&store.conn)
            .expect("connection")
            .query_row("PRAGMA page_size", [], |row| row.get(0))
            .expect("page_size");
        assert_eq!(
            size, 4096,
            "an existing store keeps its page size; the pragma is accepted and ignored"
        );
        // Not merely openable: usable. A schema that migrated onto the smaller
        // page is what the next build writes into.
        assert!(
            store.latest_generation_id().expect("query").is_none(),
            "a freshly migrated store has no generation yet"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// S-8: the gate must not drift behind the schema it asserts.
    ///
    /// `REQUIRED_SCHEMA` is hand-written and the schema is not, so the only
    /// thing keeping them in step is this test. It fails the moment a
    /// migration adds a column the gate does not name — which is exactly how
    /// `grammar_version`, `analyzer_version`, `classification` and `receiver`
    /// came to be missing, leaving a store that opened clean and failed at its
    /// first write.
    #[test]
    fn the_schema_gate_names_every_column_the_current_schema_creates() {
        let store = Store::open_in_memory().expect("store");
        let conn = lock_conn(&store.conn).expect("connection");
        for (table, required) in REQUIRED_SCHEMA {
            let mut stmt = conn
                .prepare(&format!("PRAGMA table_info(\"{table}\")"))
                .expect("table_info");
            let actual: Vec<String> = stmt
                .query_map([], |row| row.get(1))
                .expect("columns")
                .collect::<Result<_>>()
                .expect("columns");
            assert!(
                !actual.is_empty(),
                "{table} is required by the gate but a freshly created store does not have it"
            );
            for column in actual {
                assert!(
                    required.contains(&column.as_str()),
                    "{table}.{column} exists in the current schema but the gate does not \
                     require it; a store missing that column would open clean and fail at \
                     the first write instead of at the gate"
                );
            }
        }
    }

    #[test]
    fn store_connections_enable_integrity_and_contention_pragmas() {
        let store = Store::open_in_memory().expect("store");
        let conn = lock_conn(&store.conn).expect("connection");
        let foreign_keys: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .expect("foreign_keys pragma");
        let busy_timeout: i64 = conn
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .expect("busy_timeout pragma");
        assert_eq!(foreign_keys, 1);
        assert!(busy_timeout >= 5_000, "busy timeout was {busy_timeout} ms");
    }

    /// The write-path pragmas are a contract, not an incidental default.
    ///
    /// Each of these was measured: leaving `synchronous` at `FULL` and
    /// `cache_size` at SQLite's 2 MiB default made `save_generation` the single
    /// most expensive phase of a build. A later edit that drops one of them
    /// would restore that cost silently — nothing fails, builds just get slower
    /// — so the settings are asserted rather than trusted.
    ///
    /// `synchronous` is asserted as exactly 1 (NORMAL). Not `<= 1`: 0 is OFF,
    /// which trades corruption-on-crash for speed, and this store must never
    /// drift into it.
    #[test]
    fn write_connections_use_the_tuned_durability_and_cache_pragmas() {
        let store = Store::open_in_memory().expect("store");
        let conn = lock_conn(&store.conn).expect("connection");

        let synchronous: i64 = conn
            .query_row("PRAGMA synchronous", [], |row| row.get(0))
            .expect("synchronous pragma");
        assert_eq!(
            synchronous, 1,
            "expected synchronous=NORMAL (1), found {synchronous} \
             (0=OFF risks corruption, 2=FULL fsyncs every commit)"
        );

        let cache_size: i64 = conn
            .query_row("PRAGMA cache_size", [], |row| row.get(0))
            .expect("cache_size pragma");
        assert_eq!(
            cache_size,
            Store::CACHE_SIZE_KIB as i64,
            "cache_size should be the tuned {} KiB",
            -Store::CACHE_SIZE_KIB
        );

        let temp_store: i64 = conn
            .query_row("PRAGMA temp_store", [], |row| row.get(0))
            .expect("temp_store pragma");
        assert_eq!(temp_store, 2, "expected temp_store=MEMORY (2)");
    }

    /// K12: every read path fails closed on a poisoned mutex.
    ///
    /// `lock_conn` exists precisely so a poisoned store mutex becomes an error
    /// the caller can report, and five readers bypassed it with
    /// `.expect("store mutex poisoned")`. Under the release profile's
    /// `panic = "abort"` those are not recoverable panics — they end the
    /// process. A daemon serving IPC would vanish mid-request because one
    /// earlier query panicked while holding the lock; the CLI would die with no
    /// message a caller could act on.
    ///
    /// The lock is poisoned deliberately here rather than by provoking a real
    /// panic: what is under test is the failure *mode* of these five readers,
    /// not the cause of the poison.
    #[test]
    fn poisoned_store_mutex_is_an_error_on_every_reader() {
        let store = Store::open_in_memory().expect("store");

        // Poison the mutex: panic while holding it, catching the unwind so the
        // test process survives. Tests build with the default unwind profile.
        let poisoner = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = store.conn.lock().expect("first lock");
            panic!("deliberate poison");
        }));
        assert!(poisoner.is_err(), "the poisoning panic must have unwound");
        assert!(store.conn.is_poisoned(), "the mutex must now be poisoned");

        // Each of these used `.expect("store mutex poisoned")` and therefore
        // aborted rather than returning. Naming them individually so a
        // regression says which reader regressed.
        assert!(
            store.latest_unresolved(10).is_err(),
            "latest_unresolved must fail closed on a poisoned mutex"
        );
        assert!(
            store.count_unresolved_rows().is_err(),
            "count_unresolved_rows must fail closed on a poisoned mutex"
        );
        assert!(
            store.latest_file_hashes().is_err(),
            "latest_file_hashes must fail closed on a poisoned mutex"
        );
        assert!(
            store.latest_symbol_names_by_file().is_err(),
            "latest_symbol_names_by_file must fail closed on a poisoned mutex"
        );
        assert!(
            store.latest_edges_for_test().is_err(),
            "latest_edges_for_test must fail closed on a poisoned mutex"
        );
    }

    /// The deterministic guard for [`Store::latest_snapshot`].
    ///
    /// The defect it exists for reproduces only probabilistically — a second
    /// process has to commit *and* prune inside the microseconds between a
    /// reader's generation lookup and its row read, which took 182,781 reads
    /// against a continuously rebuilding writer to hit 31 times. A test that
    /// fires at that rate is not a guard: a green run from it says nothing.
    ///
    /// So the guard is stated over the mechanism instead, with the schedule
    /// forced rather than raced. A second connection — the same thing a second
    /// process is, as far as SQLite's snapshots are concerned — deletes the
    /// generation's rows strictly between the pin and the read. Both readers
    /// are run over that schedule, and they must disagree:
    ///
    /// * without a snapshot the pinned generation reads back **empty**, which
    ///   is the defect, verbatim;
    /// * with one it reads back its rows.
    ///
    /// Asserting both directions is what keeps this honest. A test that only
    /// checked the snapshot would still pass if the delete silently stopped
    /// landing, and would then be proving nothing at all.
    #[test]
    fn a_pinned_generation_keeps_its_rows_when_another_connection_prunes_it() {
        let dir = std::env::temp_dir().join(format!(
            "devmap-snapshot-guard-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let db_path = dir.join("index.sqlite");
        let store = Store::open(&db_path).expect("store");
        {
            let conn = lock_conn(&store.conn).expect("connection");
            conn.execute(
                "INSERT INTO generations (id, created_at, head_sha, analysis_json)
                 VALUES (7, 1.0, 'seed', '{}')",
                [],
            )
            .expect("generation");
            conn.execute("INSERT INTO paths (id, path) VALUES (1, 'src/a.py')", [])
                .expect("path");
            conn.execute(
                "INSERT INTO generation_nodes
                 (generation_id, ordinal, file_id, name, qualified_name, kind,
                  span_start, span_end, is_exported)
                 VALUES (7, 0, 1, 'alpha', 'src/a.py::alpha', 'Function', 0, 1, 0)",
                [],
            )
            .expect("node");
        }

        // What the pruning writer does, from a connection of its own.
        let prune = || {
            let other = Connection::open(&db_path).expect("second connection");
            other
                .busy_timeout(std::time::Duration::from_secs(5))
                .expect("busy timeout");
            other
                .execute("DELETE FROM generation_nodes WHERE generation_id = 7", [])
                .expect("prune");
            other
                .execute("DELETE FROM generations WHERE id = 7", [])
                .expect("prune");
        };
        let count_rows = |conn: &Connection, generation: u32| -> i64 {
            conn.query_row(
                "SELECT COUNT(*) FROM generation_nodes WHERE generation_id = ?1",
                params![generation],
                |row| row.get(0),
            )
            .expect("count")
        };

        // Reader A: the pre-fix shape — pin, then read, with no snapshot
        // between them. This is the control, and it must observe the deletion.
        let unpinned = {
            let conn = lock_conn(&store.conn).expect("connection");
            let generation = Store::latest_generation_id_locked(&conn)
                .expect("pin")
                .expect("a generation");
            assert_eq!(generation, 7);
            prune();
            count_rows(&conn, generation)
        };
        assert_eq!(
            unpinned, 0,
            "the control did not actually race: without a snapshot the pinned \
             generation must read back empty, or this test proves nothing"
        );

        // Put the generation back and run the same schedule through the
        // snapshot the fix installs.
        {
            let conn = lock_conn(&store.conn).expect("connection");
            conn.execute(
                "INSERT INTO generations (id, created_at, head_sha, analysis_json)
                 VALUES (7, 1.0, 'seed', '{}')",
                [],
            )
            .expect("generation");
            conn.execute(
                "INSERT INTO generation_nodes
                 (generation_id, ordinal, file_id, name, qualified_name, kind,
                  span_start, span_end, is_exported)
                 VALUES (7, 0, 1, 'alpha', 'src/a.py::alpha', 'Function', 0, 1, 0)",
                [],
            )
            .expect("node");
        }
        let pinned = {
            let conn = lock_conn(&store.conn).expect("connection");
            let (snapshot, generation) = Store::latest_snapshot(&conn)
                .expect("pin")
                .expect("a generation");
            assert_eq!(generation, 7);
            prune();
            count_rows(&snapshot, generation)
        };
        assert_eq!(
            pinned, 1,
            "a generation pinned inside a read snapshot lost its rows to a \
             concurrent prune; the reader would answer `[]` for a store that \
             holds data, which is the failure `latest_snapshot` exists to stop"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod bounded_claim_tests {
    use super::*;

    /// S-6: a lock check that could not run is not another writer.
    ///
    /// `lock_writer_at` matched `Err(_)` from `File::try_lock`, which collapses
    /// `TryLockError::WouldBlock` (contention — wait and retry) with
    /// `TryLockError::Error` (the check itself failed). On a filesystem that
    /// does not implement `flock`, the second is what *every* attempt returns:
    /// each build polled the full 60 s and then failed with "another devmap
    /// writer holds … (pid unknown)" — a definite claim about a process that
    /// does not exist, made by a check that never completed, after a minute
    /// spent waiting for it.
    #[test]
    fn s6_a_writer_lock_check_that_failed_is_not_reported_as_another_writer() {
        let lock_path = std::path::Path::new("/nonexistent/devmap.sqlite.writer.lock");
        let mut attempts = 0usize;
        let started = std::time::Instant::now();
        let error = Store::poll_writer_lock(
            || {
                attempts += 1;
                Err(std::fs::TryLockError::Error(std::io::Error::from(
                    std::io::ErrorKind::PermissionDenied,
                )))
            },
            std::time::Duration::from_secs(60),
            std::time::Duration::from_millis(10),
            lock_path,
        )
        .expect_err("a failed lock check must not be reported as a taken lock");

        assert_eq!(
            attempts, 1,
            "a check that cannot run must not be retried until the deadline"
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "must fail immediately, not after the full wait: {:?}",
            started.elapsed()
        );
        let text = error.to_string();
        assert!(
            !text.contains("another devmap writer holds"),
            "must not claim another writer exists: {text}"
        );
        assert!(
            text.contains("could not be taken") && text.contains("permission denied"),
            "must name the failure that actually happened: {text}"
        );
    }

    /// S-6 control: real contention must still wait and still name the holder.
    ///
    /// A guard that propagated every error would trade the false claim for a
    /// build that refuses to wait out a peer, which is the failure the bounded
    /// poll exists to prevent.
    #[test]
    fn s6_writer_lock_contention_still_polls_to_the_deadline_and_names_the_holder() {
        let dir = std::env::temp_dir().join(format!(
            "devmap-s6-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let lock_path = dir.join("index.sqlite.writer.lock");
        std::fs::write(&lock_path, "4242\n").unwrap();

        let mut attempts = 0usize;
        let started = std::time::Instant::now();
        let error = Store::poll_writer_lock(
            || {
                attempts += 1;
                Err(std::fs::TryLockError::WouldBlock)
            },
            std::time::Duration::from_millis(60),
            std::time::Duration::from_millis(10),
            &lock_path,
        )
        .expect_err("a permanently contended lock must time out");

        assert!(attempts >= 2, "contention must be retried, got {attempts}");
        assert!(
            started.elapsed() >= std::time::Duration::from_millis(60),
            "must wait out the deadline: {:?}",
            started.elapsed()
        );
        let text = error.to_string();
        assert!(
            text.contains("another devmap writer holds") && text.contains("pid 4242"),
            "contention must name the recorded holder: {text}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// S-6 control: a lock that is granted returns at once.
    #[test]
    fn s6_writer_lock_returns_as_soon_as_the_lock_is_granted() {
        let mut attempts = 0usize;
        Store::poll_writer_lock(
            || {
                attempts += 1;
                if attempts >= 3 {
                    Ok(())
                } else {
                    Err(std::fs::TryLockError::WouldBlock)
                }
            },
            std::time::Duration::from_secs(5),
            std::time::Duration::from_millis(1),
            std::path::Path::new("/nonexistent/lock"),
        )
        .expect("a lock granted before the deadline must succeed");
        assert_eq!(attempts, 3);
    }

    /// S-9: SQLite reads a negative `LIMIT` as *unbounded*.
    ///
    /// `latest_unresolved` bound `params![limit as i64]`. `usize::MAX as i64`
    /// is `-1`, and every `usize` at or above `2^63` casts to a negative
    /// `i64`, so a caller asking for a very large cap silently got no cap at
    /// all — the opposite of the request. Four other bounded readers already
    /// clamped inline; this makes that clamp the one owner of the rule so a
    /// fifth reader cannot be written without it.
    ///
    /// What this gate can and cannot prove: the *row-count* difference between
    /// an unbounded query and one capped at `i64::MAX` is only observable in a
    /// table of more than `2^63` rows, so no fixture can exhibit it. The gate
    /// is therefore on the binding rule itself, plus the SQLite behaviour it
    /// exists for, asserted in the test below.
    #[test]
    fn s9_a_sqlite_limit_is_never_the_negative_that_means_unbounded() {
        assert_eq!(
            sqlite_limit(usize::MAX),
            i64::MAX,
            "usize::MAX must clamp to the largest cap SQLite can express,              not wrap to -1"
        );
        for limit in [
            usize::MAX,
            usize::MAX - 1,
            i64::MAX as usize,
            i64::MAX as usize + 1,
        ] {
            assert!(
                sqlite_limit(limit) > 0,
                "{limit} must not bind a non-positive LIMIT, got {}",
                sqlite_limit(limit)
            );
        }
        // A clamp that flattened everything would be a different silent
        // wrong answer, so the ordinary range must pass through untouched.
        for limit in [0usize, 1, 2, 64, 100_000] {
            assert_eq!(sqlite_limit(limit), limit as i64);
        }
    }

    /// S-9, the behaviour the clamp protects: `LIMIT -1` really is unbounded
    /// in this SQLite build, so the raw cast was not a cosmetic defect.
    #[test]
    fn s9_negative_limits_are_unbounded_and_the_clamped_one_is_a_cap() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE t (n INTEGER);
             INSERT INTO t (n) VALUES (1), (2), (3);",
        )
        .unwrap();
        let count = |bound: i64| -> usize {
            conn.prepare("SELECT n FROM t LIMIT ?1")
                .unwrap()
                .query_map(params![bound], |row| row.get::<_, i64>(0))
                .unwrap()
                .count()
        };
        assert_eq!(
            count(usize::MAX as i64),
            3,
            "the unclamped cast asks SQLite for every row"
        );
        assert_eq!(count(sqlite_limit(2)), 2, "a real cap still truncates");
    }
}

#[cfg(test)]
mod git_head_tests {
    use super::*;

    /// A stalled git must be killed at the deadline, not waited on forever.
    ///
    /// `current_git_head` used `.output()`, which waits however long the child
    /// feels like taking; a hung git (network mount, wedged hook) stalled every
    /// drain batch behind it. The bounded runner kills at
    /// [`GIT_HEAD_DEADLINE`]; this test proves the error arrives near the
    /// deadline rather than after the sleeper's own 30s exit.
    #[test]
    fn a_stalled_git_is_killed_at_the_deadline() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("devmap-gitdeadline-{stamp}"));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("stalledgit");
        std::fs::write(&script, "#!/bin/sh\nsleep 30\n").unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let started = std::time::Instant::now();
        let result =
            run_git_head_with_deadline(&script.to_string_lossy(), std::path::Path::new("/tmp"));
        let elapsed = started.elapsed();

        let error = result.expect_err("a stalled git must produce an error");
        assert!(
            error.to_string().contains("killed"),
            "the error must say the child was killed: {error}"
        );
        assert!(
            elapsed < std::time::Duration::from_secs(GIT_HEAD_DEADLINE.as_secs() + 2),
            "kill must land near the deadline, took {elapsed:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_real_git_head_still_validates_normally() {
        // Positive control: the deadline path must not have broken honest git.
        // Any directory works — /tmp is outside a repo only if git errors, so
        // use this crate's own manifest dir which IS in a repository when the
        // workspace is checked out; fall back to asserting the failure shape
        // otherwise. Either way it must return quickly and cleanly.
        let started = std::time::Instant::now();
        let result = current_git_head(std::path::Path::new(env!("CARGO_MANIFEST_DIR")));
        assert!(started.elapsed() < GIT_HEAD_DEADLINE);
        match result {
            Ok(head) => assert!(
                (7..=64).contains(&head.len()) && head.bytes().all(|b| b.is_ascii_hexdigit()),
                "a real HEAD must pass validation: {head:?}"
            ),
            Err(error) => assert!(
                !error.to_string().contains("killed"),
                "an honest fast failure must not be a kill: {error}"
            ),
        }
    }
}
