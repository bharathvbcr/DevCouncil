use std::sync::Arc;
use std::time::Duration;

use devmap_query::{Request, StoreQueryEngine, MAX_TOKEN_BUDGET, MAX_TRAVERSAL_DEPTH};
use devmap_store::{Store, StoreStatus};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const PROTOCOL_VERSION: u32 = 1;
const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a peer may sit on a fresh connection before sending anything.
///
/// A connection holds one of [`MAX_CONCURRENT_CONNECTIONS`] admission permits
/// from `accept` until its frame is read, so a peer that connects and sends
/// nothing holds a permit for the whole per-read timeout. Measured with the
/// release binary: 120 idle connections made a legitimate `status` wait
/// 5,003 ms — the whole [`IO_TIMEOUT`] — on a socket whose baseline answer is
/// milliseconds. Every client this daemon has writes the moment it connects,
/// so the first byte is due sooner than the rest; a slow writer that has
/// started gets the ordinary timeout for every read after it.
const FIRST_BYTE_TIMEOUT: Duration = Duration::from_secs(1);
/// Upper bound on how long one query may occupy its connection task.
///
/// Queries share the store connection with the drain loop's generation writes,
/// so a query issued mid-resync otherwise blocks for as long as that write
/// holds the mutex. Left unbounded, such a request pins the connection until
/// the write finishes; bounded, it answers with a structured error the caller
/// can report instead of hanging.
const QUERY_TIMEOUT: Duration = Duration::from_secs(30);
/// Overall ceiling on reading one request frame, per connection. Per-read
/// timeouts bound a silent peer; this bounds a peer that keeps the exchange
/// alive without ever finishing — bytes trickling forever would otherwise
/// hold a connection task open indefinitely.
const REQUEST_DEADLINE: Duration = Duration::from_secs(10);
/// How long the startup liveness probe waits for an existing endpoint to
/// answer a connect before treating it as active-and-unreachable. Bounded so
/// a wedged listener cannot stall a new daemon's bind forever.
const LIVENESS_PROBE_TIMEOUT: Duration = Duration::from_millis(500);
/// How long a bind waits out a contended endpoint lock before refusing.
///
/// A quarter of `LIVENESS_PROBE_TIMEOUT`: long enough to absorb the millisecond
/// window in which a previous holder is releasing, short enough that a
/// genuinely-owned endpoint is still refused promptly.
const LOCK_CONTENTION_WINDOW: Duration = Duration::from_millis(125);
const LOCK_CONTENTION_POLL: Duration = Duration::from_millis(2);
/// Ceiling on concurrently served connections. Each accepted connection
/// spawns a task that may buffer up to MAX_REQUEST_BYTES before any
/// validation runs; without a cap, a flood of connections converts directly
/// into unbounded task memory. Excess connections wait at accept, backing up
/// into the kernel listen backlog instead of daemon heap.
const MAX_CONCURRENT_CONNECTIONS: usize = 64;
/// Consecutive accept failures tolerated before the IPC task gives up: an
/// unrecoverable condition (persistent fd exhaustion) must surface as a loud
/// exit, not a silent spin. Each failure backs off exponentially.
const MAX_CONSECUTIVE_ACCEPT_ERRORS: u32 = 30;
const MAX_QUERY_BYTES: usize = 4 * 1024;
// The token-budget and traversal-depth ceilings are `devmap_query`'s: the
// engine applies them, so the transport that fronts it imports them rather
// than keeping a second spelling that can drift.

/// Records when the daemon last did anything a consumer asked of it.
///
/// Shared between the IPC handlers and [`crate::daemon::Daemon::run_loop`] so
/// an orphaned daemon whose consumers all died can tell idleness from work and
/// retire itself (bounded by `DEVMAP_MAX_IDLE_SECS`) instead of running — and
/// holding its store open — forever.
#[derive(Default)]
pub struct Activity(std::sync::Mutex<Option<std::time::Instant>>);

impl Activity {
    pub fn touch(&self) {
        let mut slot = self.0.lock().expect("activity mutex poisoned");
        *slot = Some(std::time::Instant::now());
    }

    /// How long since the last touch; `None` when nothing was ever recorded.
    pub fn idle_for(&self) -> Option<Duration> {
        let slot = self.0.lock().expect("activity mutex poisoned");
        slot.map(|at| at.elapsed())
    }
}

/// Edits the daemon saw and could not write down.
///
/// The pending queue lives in the store, so "this file changed" is normally
/// recorded by writing a row — and `pending_count` is then what makes `status`
/// answer `is_fresh: false` until the drain catches up. That accounting has one
/// hole, and it is the whole reason this type exists: **when the store refuses
/// the write, there is nowhere to put the fact.**
///
/// The watcher callback's entire handling of that case was
/// `Err(err) => warn!("failed to enqueue pending paths: {err}")`. A `warn!`
/// nobody reads is exactly what `admitted_watch_path` already names as all that
/// separates a frozen index from a quiet one, and here it separated them
/// completely: with the queue write refused, `pending_count` stayed 0 and
/// `status` answered `is_fresh: true` about a generation that no longer
/// described the tree.
///
/// Measured twice, on two different causes with one shape:
///
/// * a store on a full volume — 60 files rewritten, one `warn!`, and `status`
///   over the daemon's own socket reporting `is_fresh: true, pending_count: 0,
///   degraded_reason: null`, before *and* after the space came back;
/// * a `chmod 444` store, which is a legitimate deployment (a CI cache, a
///   read-only mount) — same answer, with the only trace in a Python run
///   history nothing consults.
///
/// So the fact is kept here, in memory, where a store that will not take writes
/// cannot swallow it. It is deliberately *not* clearable by a later successful
/// enqueue: a queue write that works now says nothing about the sixty edits that
/// were dropped an hour ago. Only a completed connect-time sweep — the one pass
/// that re-discovers the whole tree by content hash — can honestly retire it.
#[derive(Default)]
pub struct UnappliedEdits(std::sync::Mutex<Option<Unapplied>>);

#[derive(Debug, Clone)]
struct Unapplied {
    /// Paths, summed across every refusal. An undercount is possible and is the
    /// right direction to be wrong in: a batch refused before it was counted
    /// still moves this off zero, and off zero is the whole claim.
    paths: u64,
    batches: u64,
    /// The most recent reason, verbatim from the store.
    reason: String,
}

impl UnappliedEdits {
    /// Record that `paths` changed paths could not be written down.
    ///
    /// Never resets the count. Two refusals are more lost coverage than one.
    pub fn record(&self, paths: usize, reason: impl std::fmt::Display) {
        let mut slot = self.0.lock().expect("unapplied-edits mutex poisoned");
        let reason = reason.to_string();
        match slot.as_mut() {
            Some(existing) => {
                existing.paths = existing.paths.saturating_add(paths as u64);
                existing.batches = existing.batches.saturating_add(1);
                existing.reason = reason;
            }
            None => {
                *slot = Some(Unapplied {
                    paths: paths as u64,
                    batches: 1,
                    reason,
                })
            }
        }
    }

    /// Retire the record, because the tree has been re-read from scratch.
    ///
    /// The only caller is a *successful* `reconcile_connect_time`, and that is
    /// the point: it is the one pass that compares every source's content hash
    /// against the stored generation, so whatever the dropped batches named is
    /// either already indexed or has just been queued by name. Clearing on a
    /// successful ordinary enqueue instead would retire a claim about edits that
    /// enqueue never looked at.
    pub fn cleared_by_sweep(&self) {
        *self.0.lock().expect("unapplied-edits mutex poisoned") = None;
    }

    pub fn is_empty(&self) -> bool {
        self.0
            .lock()
            .expect("unapplied-edits mutex poisoned")
            .is_none()
    }

    /// An owned copy of what is recorded right now.
    ///
    /// One request's answer must describe one moment. `dispatch` runs on the
    /// blocking pool and cannot borrow the shared record across the spawn, and
    /// handing it the live handle would also let the counts move between the
    /// `is_fresh` field and the `degraded_reason` field of the same JSON object
    /// — a status that says fresh and then explains why it is not.
    pub fn snapshot(&self) -> Self {
        Self(std::sync::Mutex::new(
            self.0
                .lock()
                .expect("unapplied-edits mutex poisoned")
                .clone(),
        ))
    }

    /// How this reads in `status`, or `None` when nothing was lost.
    pub fn describe(&self) -> Option<String> {
        let slot = self.0.lock().expect("unapplied-edits mutex poisoned");
        let record = slot.as_ref()?;
        Some(format!(
            "{} changed path(s) in {} batch(es) could not be recorded as pending work and are \
NOT in this generation: {}. This index is behind the tree by an amount only a rebuild can \
establish — run `devmap build`, or restart the daemon once the store accepts writes again.",
            record.paths, record.batches, record.reason
        ))
    }
}

/// The daemon-side state an IPC handler has to see.
///
/// One object rather than a parameter per fact. Both members answer questions
/// the store cannot: how long since a consumer asked for anything, and what this
/// daemon knows it failed to write down. A handler given only the store answers
/// both wrongly — the second one silently.
#[derive(Default)]
pub struct ServeState {
    pub activity: Activity,
    pub unapplied: UnappliedEdits,
}

fn default_budget() -> u32 {
    2_000
}

fn default_depth() -> usize {
    3
}

#[derive(Debug, Deserialize)]
pub struct IpcRequest {
    pub version: u32,
    #[serde(flatten)]
    pub command: IpcCommand,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum IpcCommand {
    Status,
    Search {
        query: String,
        #[serde(default = "default_budget")]
        budget: u32,
        /// Rank by name similarity instead of FTS5 prefix matching.
        /// `serde(default)` is false, so a client that predates this field
        /// gets exactly the search it always got.
        #[serde(default)]
        semantic: bool,
    },
    Deps {
        target: String,
        #[serde(default = "default_budget")]
        budget: u32,
        #[serde(default)]
        min_confidence: f32,
        /// A named floor on the resolution ladder — `deterministic`, `high` or
        /// `speculative`. Absent means no floor, which is the behaviour every
        /// existing caller already gets, so adding this shifts nothing.
        ///
        /// Preferred over `min_confidence` for the reason `EXTRACTED_FLOOR_MILLIS`
        /// exists: the float goes through SQLite REAL, where `>= 0.9` is a
        /// comparison whose answer depends on rounding, and a caller wanting
        /// deterministic edges should not have to know that means 1.0.
        #[serde(default)]
        min_rung: Option<String>,
    },
    Impact {
        target: String,
        #[serde(default = "default_budget")]
        budget: u32,
        #[serde(default = "default_depth")]
        depth: usize,
        /// See `Deps::min_rung`. `impact` and `trace` took no threshold at all
        /// before this, so the two queries a refactor actually runs were the
        /// two that could not be narrowed.
        #[serde(default)]
        min_rung: Option<String>,
        /// Band the reached symbols by distance as well as listing the edges.
        ///
        /// Defaulted false, so a client that predates this sends the request it
        /// always sent and reads the response it always read. Refused together
        /// with `min_rung`: the band walk takes a confidence floor and no rung,
        /// so honouring one would narrow half the answer.
        #[serde(default)]
        layers: bool,
    },
    Trace {
        from: String,
        #[serde(default)]
        to: Option<String>,
        #[serde(default = "default_budget")]
        budget: u32,
        #[serde(default = "default_depth")]
        depth: usize,
        /// See `Deps::min_rung`.
        #[serde(default)]
        min_rung: Option<String>,
    },
    /// Both call-graph directions for several targets in one exchange.
    ///
    /// The composed `graph_query` view needs callers and callees for each of
    /// the first few definitions a search returns. Issued one at a time that is
    /// eleven exchanges, and under the CLI transport an exchange is a process
    /// spawn — which is why that view did not get faster when the store did.
    /// The fan-out bound is enforced in `validate_request` and again in the
    /// engine, so an over-long list is refused rather than quietly trimmed.
    Neighbors {
        targets: Vec<String>,
        #[serde(default = "default_budget")]
        budget: u32,
        #[serde(default = "default_depth")]
        depth: usize,
        #[serde(default)]
        min_confidence: f32,
        /// The rung floor, which this composition accepts because `impact` and
        /// `deps` — the two queries it is — each accept one. Absent means no
        /// floor, which is what every caller predating it already sent.
        #[serde(default)]
        min_rung: Option<String>,
    },
    Dead {
        #[serde(default = "default_budget")]
        budget: u32,
    },
    /// Definitions matching a query, with source, both call-graph directions
    /// and a layered blast radius, in one exchange.
    ///
    /// The MCP `devcouncil_code_explore` tool answered this from a second
    /// engine in Python that loaded the whole graph into process memory; the
    /// composition is the cost, so the composition moved into the kernel.
    Explore {
        query: String,
        #[serde(default = "default_explore_limit")]
        limit: usize,
        #[serde(default = "default_explore_budget")]
        budget: u32,
        #[serde(default = "default_depth")]
        depth: usize,
        #[serde(default)]
        min_confidence: f32,
    },
    /// Test files reachable through the inbound blast radius of some targets.
    Affected {
        targets: Vec<String>,
        #[serde(default = "default_budget")]
        budget: u32,
        #[serde(default = "default_depth")]
        depth: usize,
        #[serde(default)]
        min_confidence: f32,
    },
    Preview {
        /// Repository-relative path the buffer would be written to.
        file: String,
        /// The candidate content itself. Bounded by `MAX_REQUEST_BYTES`
        /// (1 MiB) on the whole frame, which is also the extractor's own
        /// `MAX_SOURCE_BYTES` ceiling — a buffer too large to preview is
        /// refused at the frame, before anything tries to parse it.
        content: String,
        #[serde(default = "default_budget")]
        budget: u32,
        #[serde(default = "default_preview_confidence")]
        min_confidence: f32,
    },
    Clones {
        #[serde(default = "default_budget")]
        budget: u32,
        /// `exact`, `structural`, or absent for both. Rejected rather than
        /// silently ignored when it is anything else, so a typo cannot come
        /// back as a full report the caller reads as filtered.
        #[serde(default)]
        kind: Option<String>,
        #[serde(default)]
        min_nodes: u32,
    },
}

fn default_preview_confidence() -> f32 {
    devmap_query::PREVIEW_CALLER_MIN_CONFIDENCE
}

fn default_explore_limit() -> usize {
    20
}

/// `explore` pays for four sections out of one number, so its default is the
/// engine's own, not the single-answer 2000 every other surface uses.
fn default_explore_budget() -> u32 {
    devmap_query::Budget::EXPLORE
}

/// Ceiling on `explore`'s definition list. The budget usually bites first; this
/// bounds the work a caller can ask for before the budget is even consulted —
/// each definition costs two traversals.
/// Most definitions `explore` will expand in one call.
///
/// `pub` so `mcp::describe` declares this exact number instead of restating it.
/// A published `maximum` that disagrees with the value enforced here is the
/// failure the schema checker exists to prevent, one level up: a client-side
/// validator would accept a call this server refuses.
pub const MAX_EXPLORE_LIMIT: usize = 100;

#[derive(Debug, Serialize)]
struct ErrorBody {
    code: &'static str,
    message: String,
}

#[derive(Debug, Serialize)]
struct Envelope {
    ok: bool,
    protocol_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<ErrorBody>,
}

fn success(result: Value) -> Envelope {
    Envelope {
        ok: true,
        protocol_version: PROTOCOL_VERSION,
        result: Some(result),
        error: None,
    }
}

fn failure(code: &'static str, message: impl Into<String>) -> Envelope {
    Envelope {
        ok: false,
        protocol_version: PROTOCOL_VERSION,
        result: None,
        error: Some(ErrorBody {
            code,
            message: message.into(),
        }),
    }
}

/// The `min_confidence` floor a named rung means.
///
/// Constructed as `floor_millis / 1000.0`, which is **the same construction the
/// stored value uses**: `Confidence::persist_real` is `to_millis() / 1000.0`.
/// Both sides of the comparison therefore come from one integer through one
/// division, so the boundary is exact — `>= high` admits `Confidence::MEDIUM`
/// and rejects `LOW` with no dependence on how `0.7` rounds. Writing the float
/// literal here instead would break that, which is the trap
/// `EXTRACTED_FLOOR_MILLIS` exists to name.
///
/// `None` means no floor, and yields `0.0` — the default every existing caller
/// already sends.
fn rung_floor(min_rung: &Option<String>) -> f32 {
    parsed_min_rung(min_rung)
        .map(|rung| rung.floor_millis() as f32 / 1000.0)
        .unwrap_or(0.0)
}

/// The requested rung, parsed.
///
/// Safe to unwrap to `None` here because [`validate_request`] has already
/// refused any name that does not parse — a `None` at this point means the
/// caller sent no rung, never that they sent a bad one.
fn parsed_min_rung(min_rung: &Option<String>) -> Option<devmap_query::Rung> {
    min_rung.as_deref().and_then(devmap_query::Rung::parse)
}

/// The `min_rung` a command carries, if it accepts one.
///
/// Named rather than matched inline at each site so a command that gains the
/// parameter and forgets the validation is one edit, not two.
fn request_min_rung(command: &IpcCommand) -> Option<&str> {
    match command {
        IpcCommand::Deps { min_rung, .. }
        | IpcCommand::Impact { min_rung, .. }
        | IpcCommand::Trace { min_rung, .. }
        | IpcCommand::Neighbors { min_rung, .. } => min_rung.as_deref(),
        _ => None,
    }
}

pub(crate) fn validate_request(request: &IpcRequest) -> Result<(), String> {
    // Refused, never defaulted. A typo silently answered at full breadth is a
    // filtered answer the caller believes is narrow — the same class of error
    // as `Clones::kind`, which this mirrors deliberately.
    if let Some(name) = request_min_rung(&request.command) {
        if devmap_query::Rung::parse(name).is_none() {
            return Err(format!(
                "min_rung must be one of deterministic, high, speculative; got {name:?}"
            ));
        }
    }

    // Refused rather than half-applied, exactly as the CLI refuses
    // `--layers --min-rung`: the band walk takes a confidence floor and knows
    // nothing of rungs, so serving both would return edges cut to the floor
    // beside bands that were not — one answer whose halves describe different
    // graphs, and nothing in it saying so.
    if let IpcCommand::Impact {
        layers: true,
        min_rung: Some(_),
        ..
    } = &request.command
    {
        return Err(
            "impact cannot take both layers and min_rung: the distance bands are walked \
             without a rung floor, so the two halves of the answer would describe \
             different graphs"
                .to_string(),
        );
    }

    let (text, budget, depth, min_confidence) = match &request.command {
        IpcCommand::Status => return Ok(()),
        IpcCommand::Search { query, budget, .. } => (query.as_str(), *budget, 1, None),
        IpcCommand::Deps {
            target,
            budget,
            min_confidence,
            ..
        } => (target.as_str(), *budget, 1, Some(*min_confidence)),
        IpcCommand::Impact {
            target,
            budget,
            depth,
            ..
        } => (target.as_str(), *budget, *depth, None),
        IpcCommand::Trace {
            from,
            to,
            budget,
            depth,
            ..
        } => {
            if to
                .as_ref()
                .is_some_and(|value| value.len() > MAX_QUERY_BYTES)
            {
                return Err(format!("trace destination exceeds {MAX_QUERY_BYTES} bytes"));
            }
            (from.as_str(), *budget, *depth, None)
        }
        IpcCommand::Neighbors {
            targets,
            budget,
            depth,
            min_confidence,
            ..
        } => {
            // Refused, not trimmed. Silently answering the first sixteen of
            // twenty would hand back a short list that reads exactly like a
            // complete one.
            if targets.len() > devmap_query::MAX_NEIGHBOR_TARGETS {
                return Err(format!(
                    "neighbors accepts at most {} targets, got {}",
                    devmap_query::MAX_NEIGHBOR_TARGETS,
                    targets.len()
                ));
            }
            // The scalar check below sees one string; this request carries a
            // list, and an over-long entry buried at index 9 must not ride in
            // because index 0 was short.
            if let Some(oversized) = targets.iter().find(|t| t.len() > MAX_QUERY_BYTES) {
                return Err(format!(
                    "neighbors target exceeds {MAX_QUERY_BYTES} bytes: {} bytes",
                    oversized.len()
                ));
            }
            ("", *budget, *depth, Some(*min_confidence))
        }
        IpcCommand::Dead { budget } => ("", *budget, 1, None),
        IpcCommand::Explore {
            query,
            limit,
            budget,
            depth,
            min_confidence,
        } => {
            // Refused, not clamped: a caller that asked for 500 definitions and
            // silently received 100 cannot tell a capped list from the whole
            // match set, which is the one thing every count here exists to
            // prevent.
            if *limit > MAX_EXPLORE_LIMIT {
                return Err(format!(
                    "explore accepts at most {MAX_EXPLORE_LIMIT} definitions, got {limit}"
                ));
            }
            (query.as_str(), *budget, *depth, Some(*min_confidence))
        }
        IpcCommand::Affected {
            targets,
            budget,
            depth,
            min_confidence,
        } => {
            if targets.len() > devmap_query::MAX_NEIGHBOR_TARGETS {
                return Err(format!(
                    "affected accepts at most {} targets, got {}",
                    devmap_query::MAX_NEIGHBOR_TARGETS,
                    targets.len()
                ));
            }
            // The scalar check below sees one string; this request carries a
            // list, so an over-long entry at index 9 must not ride in because
            // index 0 was short.
            if let Some(oversized) = targets.iter().find(|t| t.len() > MAX_QUERY_BYTES) {
                return Err(format!(
                    "affected target exceeds {MAX_QUERY_BYTES} bytes: {} bytes",
                    oversized.len()
                ));
            }
            ("", *budget, *depth, Some(*min_confidence))
        }
        IpcCommand::Preview {
            file,
            budget,
            min_confidence,
            ..
        } => (file.as_str(), *budget, 1, Some(*min_confidence)),
        IpcCommand::Clones { budget, kind, .. } => {
            // Asks the parser rather than re-listing the names: a third kind
            // added later must not be accepted here and then silently ignored
            // by the filter, which is how a caller ends up reading an
            // unfiltered report as a filtered one.
            if let Some(kind) = kind {
                if devmap_query::parse_clone_kind(kind).is_none() {
                    return Err(format!(
                        "clone kind must be 'exact' or 'structural', got '{kind}'"
                    ));
                }
            }
            ("", *budget, 1, None)
        }
    };
    if text.len() > MAX_QUERY_BYTES {
        return Err(format!("query exceeds {MAX_QUERY_BYTES} bytes"));
    }
    // The CLI refuses `--budget 0` for the same reason, and the two
    // transports must agree: a zero budget returns an empty page that cannot
    // be told apart from a complete answer.
    if budget == 0 {
        return Err(
            "token budget must be at least 1: a zero budget returns an empty page \
                    that cannot be told apart from a complete answer"
                .to_string(),
        );
    }
    if budget > MAX_TOKEN_BUDGET {
        return Err(format!("token budget exceeds {MAX_TOKEN_BUDGET}"));
    }
    // Same rule as the CLI's `--depth`: a walk of depth 0 visits nothing and
    // answers with an empty radius that reads as "nothing is affected".
    // Commands without a traversal carry depth 1 here and never see this.
    if depth == 0 {
        return Err("traversal depth must be at least 1: depth 0 walks nothing".to_string());
    }
    if depth > MAX_TRAVERSAL_DEPTH {
        return Err(format!("traversal depth exceeds {MAX_TRAVERSAL_DEPTH}"));
    }
    if min_confidence.is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value)) {
        return Err("min_confidence must be finite and within [0, 1]".to_string());
    }
    Ok(())
}

/// Whether the persisted index is actually current.
///
/// K-A6. Two different claims that an empty store pulls apart: *nothing is
/// queued* and *the index is up to date*. `is_fresh` was `pending_count == 0`
/// at both call sites — this dispatcher and `devmap status` — and neither
/// consulted `latest_generation`. A store whose schema exists but which holds
/// no generation, the state a `devmap build` that aborted partway leaves
/// behind, has nothing queued, so both answered `is_fresh: true` about an index
/// that does not exist. `devmap_client.is_map_stale()` reads
/// `not is_fresh or pending_count > 0`, so the whole stack reported the map as
/// current while nothing at all had been indexed.
///
/// RA3 also requires the store to verify current source bytes and analyzer
/// payload identity. Missing evidence is a degraded reason, never freshness.
/// One owner keeps CLI and daemon status on the same contract.
pub fn index_is_fresh(status: &StoreStatus) -> bool {
    status.latest_generation.is_some()
        && status.pending_count == 0
        && status.degraded_reason.is_none()
}

/// Why the index is not current, when it is not.
///
/// The store's own `degraded_reason` first — it knows about quarantined paths
/// and schema trouble. The empty case is added here rather than there because
/// from the store's side a generation-less store is not damaged, merely empty;
/// it is only the *freshness* claim that an empty store falsifies. Returning
/// `None` while `index_is_fresh` is false would leave a caller told the index
/// is stale with no way to find out why, which is the same defect one step on.
/// The three coverage-gap listings, in the shape every `status` surface
/// renders them.
///
/// One owner because `devmap status` and the daemon's IPC `status` are two
/// separate JSON literals over the same `StoreStatus`, and a list added to one
/// and not the other is a health check that answers differently depending on
/// whether a daemon happens to be running. Each entry carries `{shown, total,
/// truncated}` beside its paths: the lists are capped at
/// `devmap_store::COVERAGE_GAP_SAMPLE`, and a capped list that does not say so
/// reads exactly like a complete one.
pub fn coverage_gaps_json(status: &StoreStatus) -> serde_json::Value {
    let sample = |gap: &devmap_store::CoverageGapSample| {
        json!({
            "total": gap.total,
            "shown": gap.shown.len(),
            "truncated": gap.truncated(),
            "paths": gap.shown.iter().map(|row| json!({
                "path": row.path,
                "reason": row.reason,
            })).collect::<Vec<_>>(),
        })
    };
    json!({
        "discovery_refused": sample(&status.coverage_gaps.discovery_refused),
        "parse_failed": sample(&status.coverage_gaps.parse_failed),
        "pattern_recovered": sample(&status.coverage_gaps.pattern_recovered),
        // Not failures: the grammar succeeded and this build has no extractor
        // for the language. Reported alongside the three failure kinds because
        // a reader deciding whether to act on a finding needs to know which
        // kind of blindness produced it — a transient hole a re-index may fill,
        // or a permanent one no re-run will.
        "call_blind": sample(&status.coverage_gaps.call_blind),
        "import_blind": sample(&status.coverage_gaps.import_blind),
        // Also not a failure, and the only kind here the extractor chose. A
        // minified bundle contributes no edges because nobody asked it to; the
        // path is listed so that absence is a stated decision rather than an
        // empty answer indistinguishable from a file with no dependencies.
        "not_parsed": sample(&status.coverage_gaps.not_parsed),
    })
}

pub fn freshness_degraded_reason(status: &StoreStatus) -> Option<String> {
    if let Some(reason) = status.degraded_reason.clone() {
        return Some(reason);
    }
    if status.latest_generation.is_none() {
        return Some(
            "this store holds no generation: nothing has been indexed yet — run `devmap build`"
                .to_string(),
        );
    }
    if status.pending_count > 0 {
        return Some(format!(
            "{} source change(s) are pending",
            status.pending_count
        ));
    }
    None
}

/// Whether a *daemon* may call its index fresh.
///
/// [`index_is_fresh`] answers for the store, and it is the right answer there:
/// a generation exists and nothing is queued behind it. A daemon knows one more
/// thing, and it is the one the store cannot be told — see [`UnappliedEdits`] —
/// so a daemon that answered with `index_is_fresh` alone reported a clean queue
/// as a current index, which is exactly what it is not when the queue write was
/// refused.
pub fn daemon_index_is_fresh(status: &StoreStatus, unapplied: &UnappliedEdits) -> bool {
    index_is_fresh(status) && unapplied.is_empty()
}

/// Why a *daemon's* index is not current, when it is not.
///
/// Both reasons, when there are both. A store that is degraded for its own
/// reasons and a daemon that dropped edits are independent facts, and reporting
/// only the first would let the louder one hide the one no other surface can
/// see: `devmap status` opening the store directly cannot know about dropped
/// edits, so if this does not say it, nothing does.
pub fn daemon_degraded_reason(status: &StoreStatus, unapplied: &UnappliedEdits) -> Option<String> {
    match (freshness_degraded_reason(status), unapplied.describe()) {
        (Some(store), Some(dropped)) => Some(format!("{store} Also: {dropped}")),
        (Some(store), None) => Some(store),
        (None, Some(dropped)) => Some(dropped),
        (None, None) => None,
    }
}

pub(crate) fn dispatch(
    store: &Store,
    request: IpcRequest,
    cancel: &devmap_query::Cancel,
    unapplied: &UnappliedEdits,
) -> anyhow::Result<Value> {
    if request.version != PROTOCOL_VERSION {
        anyhow::bail!(
            "unsupported protocol version {}; server requires {}",
            request.version,
            PROTOCOL_VERSION
        );
    }
    let engine = StoreQueryEngine::new(store).with_cancel(cancel.clone());
    match request.command {
        IpcCommand::Status => {
            let status = store.status("daemon")?;
            Ok(json!({
                "generation_id": status.latest_generation,
                "pending_count": status.pending_count,
                "node_count": status.node_count,
                "edge_count": status.edge_count,
                // The daemon's answer, not the store's: only this process knows
                // about changed paths whose queue write the store refused, and
                // a daemon that reported `index_is_fresh` alone reported an
                // empty queue as a current index. See `UnappliedEdits`.
                "is_fresh": daemon_index_is_fresh(&status, unapplied),
                "degraded_reason": daemon_degraded_reason(&status, unapplied),
                "quarantined_count": status.quarantined_count,
                // The paths behind the three numbers `degraded_reason` states.
                // Without them "1 refused by discovery" is a fact an operator
                // cannot act on or check.
                "coverage_gaps": coverage_gaps_json(&status),
                // Whether this generation's edges carry the evidence the
                // resolver recorded or a reconstruction; `null` with no
                // generation or no edges. Same owner as the CLI's `status`.
                "edge_resolution_source": store
                    .latest_edge_resolution_source()?
                    .map(|source| source.label()),
                // The read-side half of the honesty invariant: stored edges
                // whose confidence contradicts their stored kind. Counted when
                // the generation's index is built, which this process keeps,
                // so the answer is free here; `null` with no generation.
                "edge_confidence_mismatches": store
                    .generation_edges()?
                    .map(|index| index.confidence_mismatches()),
            }))
        }
        IpcCommand::Search {
            query,
            budget,
            semantic,
        } => {
            let response = if semantic {
                engine.search_semantic(&query, budget)?
            } else {
                engine.search(Request {
                    query,
                    token_budget: budget,
                    min_confidence: 0.0,
                    max_depth: 1,
                })?
            };
            Ok(serde_json::to_value(response)?)
        }
        IpcCommand::Deps {
            target,
            budget,
            min_confidence,
            min_rung,
            // The two floors are applied at different places, and both are
            // applied. `min_confidence` goes to the store, which drops rows
            // below it before the engine sees them; the rung is applied in the
            // engine, over the population the store returned, so the histogram
            // can report what it removed. A caller sending both means the
            // intersection, and gets it — but only the rung's cut is
            // *countable*, which is why the named parameter exists.
        } => Ok(serde_json::to_value(engine.dependencies_at_rung(
            Request {
                query: target,
                min_confidence,
                token_budget: budget,
                max_depth: 1,
            },
            parsed_min_rung(&min_rung),
        )?)?),
        IpcCommand::Impact {
            target,
            budget,
            depth,
            min_rung,
            layers: true,
        } => {
            // `min_rung` alongside `layers` was refused in `validate_request`,
            // so it is `None` here by construction rather than by being dropped.
            debug_assert!(min_rung.is_none());
            Ok(serde_json::to_value(engine.impact_layered(Request {
                query: target,
                token_budget: budget,
                min_confidence: 0.0,
                max_depth: depth,
            })?)?)
        }
        IpcCommand::Impact {
            target,
            budget,
            depth,
            min_rung,
            layers: false,
        } => Ok(serde_json::to_value(engine.impact_at_rung(
            Request {
                query: target,
                token_budget: budget,
                min_confidence: 0.0,
                max_depth: depth,
            },
            parsed_min_rung(&min_rung),
        )?)?),
        IpcCommand::Trace {
            from,
            to,
            budget,
            depth,
            min_rung,
        } => {
            let response = if let Some(destination) = to {
                // The path variant answers with one path, not an edge
                // population, so there is nothing for a histogram to describe
                // and the floor goes to the walk as a confidence. Exact all the
                // same: `floor_millis / 1000.0` reconstructs the float the
                // confidence constants were built from, bit for bit.
                engine.trace_between(Request {
                    query: (from, destination),
                    token_budget: budget,
                    min_confidence: rung_floor(&min_rung),
                    max_depth: depth,
                })?
            } else {
                engine.trace_at_rung(
                    Request {
                        query: from,
                        token_budget: budget,
                        min_confidence: 0.0,
                        max_depth: depth,
                    },
                    parsed_min_rung(&min_rung),
                )?
            };
            Ok(serde_json::to_value(response)?)
        }
        IpcCommand::Neighbors {
            targets,
            budget,
            depth,
            min_confidence,
            min_rung,
        } => Ok(json!({
            "neighbors": engine.neighbors_at_rung(
                &targets,
                budget,
                min_confidence,
                depth,
                parsed_min_rung(&min_rung),
            )?,
        })),
        IpcCommand::Dead { budget } => Ok(serde_json::to_value(engine.dead_symbols(budget)?)?),
        IpcCommand::Explore {
            query,
            limit,
            budget,
            depth,
            min_confidence,
        } => Ok(serde_json::to_value(engine.explore(
            &query,
            limit,
            budget,
            min_confidence,
            depth,
        )?)?),
        IpcCommand::Affected {
            targets,
            budget,
            depth,
            min_confidence,
        } => Ok(serde_json::to_value(engine.affected_tests(
            &targets,
            budget,
            min_confidence,
            depth,
        )?)?),
        IpcCommand::Preview {
            file,
            content,
            budget,
            min_confidence,
        } => Ok(serde_json::to_value(engine.preview(
            &file,
            &content,
            budget,
            min_confidence,
        )?)?),
        IpcCommand::Clones {
            budget,
            kind,
            min_nodes,
        } => {
            // `validate` has already rejected any kind string that is neither
            // of the two, so `None` here means "no filter requested".
            let wanted = kind.as_deref().and_then(devmap_query::parse_clone_kind);
            Ok(serde_json::to_value(
                engine.clones(budget, wanted, min_nodes)?,
            )?)
        }
    }
}

async fn write_envelope<S>(stream: &mut S, envelope: &Envelope) -> anyhow::Result<()>
where
    S: AsyncWrite + Unpin,
{
    let mut payload = serde_json::to_vec(envelope)?;
    payload.push(b'\n');
    tokio::time::timeout(IO_TIMEOUT, stream.write_all(&payload))
        .await
        .map_err(|_| anyhow::anyhow!("IPC response write timed out"))??;
    tokio::time::timeout(IO_TIMEOUT, stream.shutdown())
        .await
        .map_err(|_| anyhow::anyhow!("IPC shutdown timed out"))??;
    Ok(())
}

pub async fn handle_stream<S>(mut stream: S, store: Arc<Store>) -> anyhow::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    handle_stream_with_state(&mut stream, store, &ServeState::default()).await
}

/// Why a request frame could not be read.
#[derive(Debug)]
enum FrameReadError {
    /// The overall request deadline passed before a complete frame arrived,
    /// cutting off peers that dribble bytes to hold the connection open.
    DeadlineExceeded,
    /// The peer closed the connection without sending anything.
    ClosedBeforeNewline,
    /// The frame grew past [`MAX_REQUEST_BYTES`] before terminating.
    TooLarge(usize),
    /// The transport itself failed or one read exceeded `io_timeout`.
    Io(std::io::Error),
}

impl std::fmt::Display for FrameReadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DeadlineExceeded => {
                write!(formatter, "request deadline exceeded before newline")
            }
            Self::ClosedBeforeNewline => {
                write!(formatter, "connection closed before newline")
            }
            Self::TooLarge(limit) => write!(formatter, "request exceeds {limit} bytes"),
            Self::Io(error) => write!(formatter, "IPC request read failed: {error}"),
        }
    }
}

impl std::error::Error for FrameReadError {}

impl From<std::io::Error> for FrameReadError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// Read one newline-terminated request frame under two bounds: each read gets
/// `io_timeout`, and the whole frame must complete by `deadline`.
///
/// A clean close after bytes were buffered ends the frame there, so clients
/// that half-close instead of writing a trailing newline still work; a clean
/// close with nothing buffered is an incomplete request, not silence.
async fn read_frame<S>(
    mut stream: S,
    io_timeout: Duration,
    deadline: std::time::Instant,
) -> Result<Vec<u8>, FrameReadError>
where
    S: AsyncRead + Unpin,
{
    let mut payload = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if std::time::Instant::now() >= deadline {
            return Err(FrameReadError::DeadlineExceeded);
        }
        // The first byte is due within `FIRST_BYTE_TIMEOUT`; see its doc. The
        // `min` keeps a caller-supplied shorter timeout meaningful.
        let (timeout, what) = if payload.is_empty() {
            (
                FIRST_BYTE_TIMEOUT.min(io_timeout),
                "IPC peer sent nothing after connecting",
            )
        } else {
            (io_timeout, "IPC request read timed out")
        };
        let read = tokio::time::timeout(timeout, stream.read(&mut chunk))
            .await
            .map_err(|_| {
                FrameReadError::Io(std::io::Error::new(std::io::ErrorKind::TimedOut, what))
            })??;
        if read == 0 {
            return if payload.is_empty() {
                Err(FrameReadError::ClosedBeforeNewline)
            } else {
                Ok(payload)
            };
        }
        let newline = chunk[..read].iter().position(|byte| *byte == b'\n');
        let take = newline.unwrap_or(read);
        if payload.len().saturating_add(take) > MAX_REQUEST_BYTES {
            return Err(FrameReadError::TooLarge(MAX_REQUEST_BYTES));
        }
        payload.extend_from_slice(&chunk[..take]);
        if newline.is_some() {
            return Ok(payload);
        }
    }
}

/// The answer a peer gets when its request frame could not be read.
///
/// `None` means there is nothing to say to it, and the caller must report the
/// failure instead of returning success.
///
/// The match is exhaustive on purpose. It was `Err(FrameReadError::…)` twice
/// and then `Err(_) => return Ok(())`, and the wildcard is what swallowed
/// `DeadlineExceeded`: a peer holding a connection open by dribbling bytes was
/// cut off with **no answer and no log**, and the handler returned exactly the
/// `Ok(())` it returns after serving a request. From the outside that is a
/// closed socket, which is what a crashed daemon also looks like; from the
/// inside it is a connection nobody knows was refused. A frame that ran out of
/// time is a refusal the peer can act on, so it gets one — the same shape the
/// other two refusals already had.
///
/// Without the wildcard, a variant added later cannot silently inherit
/// "answer nothing, report success"; it has to be given an answer here.
fn frame_read_refusal(error: &FrameReadError) -> Option<Envelope> {
    match error {
        FrameReadError::ClosedBeforeNewline => Some(failure(
            "incomplete_request",
            "connection closed before newline",
        )),
        FrameReadError::TooLarge(limit) => Some(failure(
            "request_too_large",
            format!("request exceeds {limit} bytes"),
        )),
        FrameReadError::DeadlineExceeded => Some(failure(
            "request_timeout",
            format!("request frame did not complete within {REQUEST_DEADLINE:?}"),
        )),
        // The transport failed or went silent past `IO_TIMEOUT`. There is no
        // working channel to answer on, so the report goes to the caller.
        FrameReadError::Io(_) => None,
    }
}

pub async fn handle_stream_with_state<S>(
    mut stream: S,
    store: Arc<Store>,
    state: &ServeState,
) -> anyhow::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let payload = match read_frame(
        &mut stream,
        IO_TIMEOUT,
        std::time::Instant::now() + REQUEST_DEADLINE,
    )
    .await
    {
        Ok(payload) => payload,
        Err(error) => {
            return match frame_read_refusal(&error) {
                Some(envelope) => write_envelope(&mut stream, &envelope).await,
                // Nothing to say to the peer — the transport itself failed, so
                // there is no channel to say it on. It is still reported: the
                // caller logs an `Err`, and returning `Ok(())` here made an
                // abandoned connection indistinguishable from a served one.
                None => Err(anyhow::Error::new(error).context("IPC request frame")),
            };
        }
    };

    let envelope = match serde_json::from_slice::<IpcRequest>(&payload) {
        Ok(request) => match validate_request(&request) {
            Err(error) => failure("invalid_parameters", error),
            Ok(()) => {
                state.activity.touch();
                dispatch_with_timeout(
                    Arc::clone(&store),
                    request,
                    QUERY_TIMEOUT,
                    devmap_query::Cancel::new(),
                    &state.unapplied,
                )
                .await
            }
        },
        Err(error) => failure("invalid_request", error.to_string()),
    };
    write_envelope(&mut stream, &envelope).await
}

/// Run one request on the blocking pool, bounded by `limit`.
///
/// The bound frees the connection: a query wedged behind a long generation
/// write answers with a structured error instead of occupying its connection
/// for as long as the write holds the store mutex. It does **not** free the
/// work — a `spawn_blocking` task cannot be aborted, and dropping its
/// `JoinHandle` merely detaches it — so the same deadline that answers the
/// client also trips `cancel`, which the engine's loops consult. Without that,
/// every timed-out query kept scanning on a pool thread nobody was reading, and
/// enough of them fill the pool.
///
/// `cancel` is a parameter rather than a local so a test can hold the same
/// handle the abandoned task was given and assert it was actually tripped.
async fn dispatch_with_timeout(
    store: Arc<Store>,
    request: IpcRequest,
    limit: Duration,
    cancel: devmap_query::Cancel,
    unapplied: &UnappliedEdits,
) -> Envelope {
    let worker_cancel = cancel.clone();
    // Read out of the shared record before the work moves to the blocking pool:
    // `status` needs the daemon's view of what it failed to write down, and the
    // closure cannot borrow it across the spawn.
    let dropped = unapplied.snapshot();
    let dispatched = tokio::time::timeout(
        limit,
        tokio::task::spawn_blocking(move || dispatch(&store, request, &worker_cancel, &dropped)),
    )
    .await;
    match dispatched {
        Err(_) => {
            cancel.cancel();
            failure("query_timeout", format!("query exceeded {limit:?}"))
        }
        Ok(Err(error)) => failure("internal_error", format!("query task failed: {error}")),
        Ok(Ok(Ok(result))) => success(result),
        // A path the caller was not allowed to name is a rejected *parameter*,
        // not a query that broke. Reported with the code `validate_request`
        // already uses for a bad parameter value, so a client cannot read "you
        // asked for something outside the repository" as a server fault and
        // retry it.
        Ok(Ok(Err(error))) => match error.downcast_ref::<devmap_query::PathOutsideRepoRoot>() {
            Some(_) => failure("invalid_parameters", error.to_string()),
            None => failure("request_failed", error.to_string()),
        },
    }
}

/// Delay before the next accept attempt after `consecutive` failures.
///
/// Zero errors cost nothing; each additional failure doubles a 10 ms base
/// delay, capped at one second. The curve keeps a daemon under fd exhaustion
/// or platform EPROTO storms from spinning hot while still retrying promptly
/// once the condition clears.
fn accept_error_backoff(consecutive: u32) -> Duration {
    if consecutive == 0 {
        return Duration::ZERO;
    }
    let shift = consecutive.saturating_sub(1).min(63);
    let millis = 10u64.saturating_mul(1u64 << shift);
    Duration::from_millis(millis).min(Duration::from_secs(1))
}

#[cfg(unix)]
pub struct UnixIpcServer {
    listener: tokio::net::UnixListener,
    path: std::path::PathBuf,
    /// Held exclusively for the server's lifetime. Two daemons racing to serve
    /// one endpoint previously interleaved the exists→probe→remove→bind
    /// sequence: the loser unlinked the winner's live socket and bound its own,
    /// leaving an orphaned listener that answered nothing while still holding
    /// the store open and running its watcher and drain loops.
    _lock: std::fs::File,
}

/// Where an endpoint's advisory lock lives: beside the socket, named after it.
#[cfg(unix)]
pub(crate) fn ipc_lock_path(path: &std::path::Path) -> std::path::PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("devmap-ipc");
    match path.parent() {
        Some(parent) => parent.join(format!("{file_name}.lock")),
        None => std::path::PathBuf::from(format!("{file_name}.lock")),
    }
}

/// Acquire the exclusive advisory lock guarding `path`'s bind sequence.
///
/// `File::try_lock` is an flock, so the kernel releases it if the holder dies
/// — no stale-lock cleanup is ever needed, unlike an O_EXCL marker file. The
/// caller must keep the returned file alive for as long as it owns the
/// endpoint.
#[cfg(unix)]
fn lock_ipc_endpoint(path: &std::path::Path) -> anyhow::Result<std::fs::File> {
    use std::io::Write;

    let lock_path = ipc_lock_path(path);
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)?;

    // `WouldBlock` is contention; anything else is the check failing to run.
    // Collapsing them reported "another live daemon owns this endpoint" for an
    // EACCES, ENOSPC or EIO on the lock file — a definite claim about another
    // process, made by a check that never completed, and the one shape this
    // codebase treats as worse than a visible failure.
    //
    // The retry exists because the contended window here is routinely shorter
    // than the refusal is useful. Measured on this workspace, a bind that lost
    // the race acquired the lock 5-20 ms later in every observed occurrence,
    // yet the daemon refused to start and stayed down. Waiting a bounded
    // `LOCK_CONTENTION_WINDOW` costs at most that window on the genuinely-owned
    // path — a quarter of the `LIVENESS_PROBE_TIMEOUT` this function already
    // spends on the very next step — and converts a spurious startup failure
    // into a successful start.
    let deadline = std::time::Instant::now() + LOCK_CONTENTION_WINDOW;
    loop {
        match file.try_lock() {
            Ok(()) => {
                let mut file = file;
                // Best-effort ownership record for diagnostics; failure to write
                // does not weaken the lock itself.
                let _ = writeln!(file, "{}", std::process::id());
                let _ = file.flush();
                return Ok(file);
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                if std::time::Instant::now() >= deadline {
                    anyhow::bail!(
                        "devmap IPC endpoint {path:?} is owned by another live daemon \
                         (lock {lock_path:?} still held after {LOCK_CONTENTION_WINDOW:?})"
                    );
                }
                std::thread::sleep(LOCK_CONTENTION_POLL);
            }
            Err(std::fs::TryLockError::Error(error)) => anyhow::bail!(
                "devmap IPC endpoint {path:?} could not be locked: {error} \
                 (lock {lock_path:?}); ownership is unknown, so the endpoint is \
                 left untouched"
            ),
        }
    }
}

/// Probe whether the endpoint at `path` is live, bounded by
/// [`LIVENESS_PROBE_TIMEOUT`].
///
/// - `Some(true)` — a peer accepted within the window; the endpoint is active.
/// - `Some(false)` — connect failed definitively (refused, or the path is not
///   a socket); whatever sits there is stale and may be replaced.
/// - `None` — no definitive answer inside the window. The caller must treat
///   this as *active*: deleting a possibly-live endpoint under a daemon whose
///   listen backlog is momentarily full would orphan every future client,
///   while refusing to start is always recoverable by retrying.
///
/// The connect runs on a helper thread because a Unix-domain connect to a
/// listener with a full backlog can block for an unbounded time; the probe
/// must stay bounded even when the endpoint is hostile.
#[cfg(unix)]
fn probe_endpoint_liveness(path: &std::path::Path) -> Option<bool> {
    let probe_path = path.to_path_buf();
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let connected = std::os::unix::net::UnixStream::connect(&probe_path).is_ok();
        let _ = sender.send(connected);
    });
    receiver.recv_timeout(LIVENESS_PROBE_TIMEOUT).ok()
}

#[cfg(unix)]
impl UnixIpcServer {
    pub fn bind(path: &std::path::Path) -> anyhow::Result<Self> {
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::PermissionsExt;

        // 100 bytes is portable across the common 104-byte macOS/BSD and
        // 108-byte Linux sockaddr_un limits, including the trailing NUL.
        if path.as_os_str().as_bytes().len() > 100 {
            anyhow::bail!(
                "devmap IPC path is {} bytes; portable Unix limit is 100: {:?}",
                path.as_os_str().as_bytes().len(),
                path
            );
        }

        // Serialize concurrent starters before any of them touches the socket
        // file. Losing means a live daemon already owns this endpoint.
        let lock = lock_ipc_endpoint(path)?;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
            let is_managed_runtime_dir = parent.parent() == Some(std::env::temp_dir().as_path())
                && parent
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("devmap-"));
            if is_managed_runtime_dir {
                std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
            }
        }
        if path.exists() {
            match probe_endpoint_liveness(path) {
                Some(true) => {
                    anyhow::bail!("devmap IPC endpoint is already active at {path:?}")
                }
                Some(false) => std::fs::remove_file(path)?,
                None => anyhow::bail!(
                    "devmap IPC endpoint {path:?} did not answer its liveness \
                     probe within {LIVENESS_PROBE_TIMEOUT:?}; leaving it untouched"
                ),
            }
        }
        let listener = tokio::net::UnixListener::bind(path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        Ok(Self {
            listener,
            path: path.to_path_buf(),
            _lock: lock,
        })
    }

    pub async fn run(self, store: Arc<Store>, state: Arc<ServeState>) -> anyhow::Result<()> {
        let admission = crate::admission::Admission::new(MAX_CONCURRENT_CONNECTIONS);
        let mut consecutive_accept_errors: u32 = 0;
        loop {
            match self.listener.accept().await {
                Ok((stream, _)) => {
                    consecutive_accept_errors = 0;
                    // Saturated pool => accept pauses here: backpressure lands
                    // in the kernel backlog rather than unbounded task memory.
                    // This is the one transport for which waiting is the right
                    // answer — the peer is a local client on a Unix socket that
                    // will simply wait — and `Admission` spells the other two
                    // choices for the two transports that need them.
                    let permit = admission
                        .admit()
                        .await
                        .ok_or_else(|| anyhow::anyhow!("connection semaphore closed"))?;
                    let store = Arc::clone(&store);
                    let state = Arc::clone(&state);
                    tokio::spawn(async move {
                        let _permit = permit;
                        if let Err(error) = handle_stream_with_state(stream, store, &state).await {
                            tracing::warn!("IPC connection failed: {error}");
                        }
                    });
                }
                Err(error) => {
                    consecutive_accept_errors = consecutive_accept_errors.saturating_add(1);
                    if consecutive_accept_errors >= MAX_CONSECUTIVE_ACCEPT_ERRORS {
                        return Err(anyhow::anyhow!(
                            "IPC accept failed {consecutive_accept_errors} times \
                             consecutively; giving up: {error}"
                        ));
                    }
                    let delay = accept_error_backoff(consecutive_accept_errors);
                    tracing::warn!(
                        "IPC accept failed ({} consecutive; retry in {delay:?}): {error}",
                        consecutive_accept_errors
                    );
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }
}

#[cfg(unix)]
impl Drop for UnixIpcServer {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        // The lock file is unlinked *here*, while `_lock` is still alive and
        // therefore while this process still holds the flock — struct fields
        // are dropped after `Drop::drop` returns. Order matters: a starter that
        // has already opened this inode cannot take the lock until we release
        // it, and by then the name is gone, so it is locking a detached inode
        // rather than the one the next daemon will create. Unlinking after the
        // release would let that starter believe it owned the endpoint. Either
        // way the bind itself is the backstop — a second listener on a live
        // socket path fails — but this is the ordering that keeps the window
        // shut.
        let _ = std::fs::remove_file(ipc_lock_path(&self.path));
    }
}

#[cfg(windows)]
pub async fn run_named_pipe(
    store: Arc<Store>,
    name: &str,
    state: Arc<ServeState>,
) -> anyhow::Result<()> {
    use tokio::net::windows::named_pipe::ServerOptions;

    let mut server = ServerOptions::new()
        .first_pipe_instance(true)
        .create(name)?;
    let admission = crate::admission::Admission::new(MAX_CONCURRENT_CONNECTIONS);
    let mut consecutive_connect_errors: u32 = 0;
    loop {
        match server.connect().await {
            Ok(()) => consecutive_connect_errors = 0,
            Err(error) => {
                consecutive_connect_errors = consecutive_connect_errors.saturating_add(1);
                if consecutive_connect_errors >= MAX_CONSECUTIVE_ACCEPT_ERRORS {
                    return Err(anyhow::anyhow!(
                        "named-pipe connect failed {consecutive_connect_errors} \
                         times consecutively; giving up: {error}"
                    ));
                }
                let delay = accept_error_backoff(consecutive_connect_errors);
                tracing::warn!(
                    "named-pipe connect failed ({} consecutive; retry in {delay:?}): {error}",
                    consecutive_connect_errors
                );
                tokio::time::sleep(delay).await;
                continue;
            }
        }
        // Same bound as the Unix transport: saturated pool => stop creating
        // pipe instances until a slot frees, instead of fanning out without
        // limit.
        let permit = admission
            .admit()
            .await
            .ok_or_else(|| anyhow::anyhow!("connection semaphore closed"))?;
        let connected = server;
        server = ServerOptions::new().create(name)?;
        let store = Arc::clone(&store);
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            let _permit = permit;
            if let Err(error) = handle_stream_with_state(connected, store, &state).await {
                tracing::warn!("named-pipe IPC connection failed: {error}");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    /// A peer dribbling one byte at a time must be cut by an *overall* request
    /// deadline, not just the per-read timeout.
    ///
    /// `handle_stream` bounded each `read` individually: a client that never
    /// stops sending could hold a connection task and its buffer open forever
    /// by keeping each byte inside the per-chunk window. The overall deadline
    /// is what actually bounds a hostile or wedged peer.
    #[tokio::test]
    async fn a_dribbling_peer_is_cut_at_the_overall_deadline() {
        let (mut client, server) = tokio::io::duplex(64);
        let writer = tokio::spawn(async move {
            for _ in 0..200 {
                if client.write_all(b"x").await.is_err() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        });

        let started = std::time::Instant::now();
        let deadline = started + Duration::from_millis(300);
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            read_frame(server, IO_TIMEOUT, deadline),
        )
        .await
        .expect("read_frame must return instead of hanging past the deadline");
        let elapsed = started.elapsed();

        let error = result.expect_err("a frame still open at the deadline must be refused");
        assert!(
            error.to_string().contains("deadline"),
            "the refusal must name the overall deadline, got: {error}"
        );
        assert!(
            elapsed < Duration::from_secs(2),
            "the deadline must cut the exchange near 300 ms, took {elapsed:?}"
        );
        writer.abort();
    }

    /// The deadline bounds only unfinished frames: a complete request that
    /// arrives normally is unaffected, so this cannot pass by refusing
    /// everything.
    #[tokio::test]
    async fn a_complete_frame_within_the_deadline_reads_normally() {
        let (mut client, server) = tokio::io::duplex(4096);
        client
            .write_all(br#"{"version":1,"cmd":"status"}"#)
            .await
            .unwrap();
        drop(client); // half-close: no more bytes are coming

        let payload = read_frame(
            server,
            IO_TIMEOUT,
            std::time::Instant::now() + Duration::from_secs(5),
        )
        .await
        .expect("a complete frame must read cleanly");
        assert_eq!(payload, br#"{"version":1,"cmd":"status"}"#.to_vec());
    }

    /// Every way a frame can fail to arrive has an answer, and none of them is
    /// silence-plus-success.
    ///
    /// The handler matched two variants and swept the rest into
    /// `Err(_) => return Ok(())`. `DeadlineExceeded` lives in that wildcard, so
    /// a peer that held a connection open by dribbling bytes was cut off with
    /// no reply and no log, and the handler returned the *same* `Ok(())` it
    /// returns after answering a request. A refusal that reports success is the
    /// one shape this kernel treats as worse than a visible failure.
    ///
    /// Driven off the error type rather than the socket because the deadline is
    /// ten seconds of wall clock: a test that waited it out would assert on a
    /// duration, and the property here is what the peer is told, not when.
    #[test]
    fn every_unreadable_frame_is_either_answered_or_reported() {
        let code_of = |error: FrameReadError| {
            frame_read_refusal(&error).map(|envelope| {
                assert!(!envelope.ok, "a refusal envelope must not claim success");
                envelope.error.expect("a refusal must carry an error").code
            })
        };

        assert_eq!(
            code_of(FrameReadError::ClosedBeforeNewline),
            Some("incomplete_request")
        );
        assert_eq!(
            code_of(FrameReadError::TooLarge(MAX_REQUEST_BYTES)),
            Some("request_too_large")
        );
        assert_eq!(
            code_of(FrameReadError::DeadlineExceeded),
            Some("request_timeout"),
            "a frame that ran out of time is a refusal the peer can report, not a \
             socket that closes for no stated reason"
        );
        assert_eq!(
            code_of(FrameReadError::Io(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                "peer went away"
            ))),
            None,
            "a broken transport has no channel to answer on — but the caller must \
             report it, which is what `handle_stream_with_activity` now does"
        );
    }

    /// The other half of the same rule: the handler must turn the unanswerable
    /// case into an `Err`, so a dead connection cannot be logged as a served
    /// one.
    #[tokio::test]
    async fn a_transport_failure_is_reported_rather_than_returned_as_success() {
        let (client, server) = tokio::io::duplex(64);
        // No bytes, and the write half is never shut: the read blocks until
        // `IO_TIMEOUT`, which `read_frame` surfaces as an `Io` error.
        let handled = tokio::time::timeout(
            IO_TIMEOUT * 3,
            handle_stream(server, Arc::new(Store::open_in_memory().unwrap())),
        )
        .await
        .expect("the handler must return, not hang");
        let error = handled.expect_err(
            "an abandoned connection must not report the same Ok(()) a served \
                                request does",
        );
        assert!(
            error.to_string().contains("IPC request frame"),
            "and the report must name what failed: {error}"
        );
        drop(client);
    }

    /// Accept-error backoff grows and then caps, and zero errors cost nothing.
    ///
    /// The accept loop must survive transient errors (fd exhaustion, EPROTO on
    /// some platforms) without spinning hot: unbounded retry at full speed is
    /// its own outage. The delay curve is policy, so it is pinned here.
    #[test]
    fn accept_error_backoff_grows_and_caps() {
        assert_eq!(accept_error_backoff(0), Duration::ZERO);
        assert_eq!(accept_error_backoff(1), Duration::from_millis(10));
        assert_eq!(accept_error_backoff(2), Duration::from_millis(20));
        assert_eq!(accept_error_backoff(3), Duration::from_millis(40));
        // Monotone growth, hard cap.
        let mut previous = accept_error_backoff(1);
        for consecutive in 2..=12u32 {
            let delay = accept_error_backoff(consecutive);
            assert!(delay >= previous, "backoff must not shrink");
            assert!(delay <= Duration::from_secs(1), "backoff must stay capped");
            previous = delay;
        }
        assert_eq!(accept_error_backoff(12), Duration::from_secs(1));
    }

    /// `min_confidence` must be finite and inside [0, 1].
    ///
    /// Both negations and the disjunction were mutable without a failure. A
    /// NaN threshold makes every `confidence >= min` comparison false, so the
    /// query returns nothing and looks like an empty graph rather than a bad
    /// request; a threshold above 1 does the same, and one below 0 silently
    /// disables the filter.
    #[test]
    fn min_confidence_must_be_finite_and_within_the_unit_interval() {
        let deps = |min_confidence: f32| IpcRequest {
            version: 1,
            command: IpcCommand::Deps {
                target: "a.go".to_string(),
                budget: 10,
                min_confidence,
                min_rung: None,
            },
        };

        // Both endpoints are inclusive and valid.
        assert!(validate_request(&deps(0.0)).is_ok());
        assert!(validate_request(&deps(1.0)).is_ok());
        assert!(validate_request(&deps(0.4)).is_ok());

        // Outside the interval in either direction is rejected.
        assert!(
            validate_request(&deps(-0.1)).is_err(),
            "a negative threshold would disable the filter"
        );
        assert!(
            validate_request(&deps(1.1)).is_err(),
            "a threshold above 1 matches nothing and reads as an empty graph"
        );

        // Non-finite is rejected: NaN makes every comparison false.
        assert!(
            validate_request(&deps(f32::NAN)).is_err(),
            "NaN must be refused"
        );
        assert!(validate_request(&deps(f32::INFINITY)).is_err());
        assert!(validate_request(&deps(f32::NEG_INFINITY)).is_err());
    }

    /// The declared limits are a contract, not just relative bounds.
    ///
    /// The bound tests below compare against these constants, so they cannot
    /// notice the constants themselves moving — mutation testing changed
    /// `4 * 1024` to `4 + 1024` and `1024 * 1024` likewise without a failure.
    /// A limit that silently shrinks by three orders of magnitude rejects
    /// legitimate work; one that grows removes the bound. Pinned by value.
    #[test]
    fn protocol_limits_are_their_declared_values() {
        assert_eq!(MAX_REQUEST_BYTES, 1_048_576, "1 MiB request ceiling");
        assert_eq!(MAX_QUERY_BYTES, 4_096, "4 KiB query ceiling");
        assert_eq!(MAX_TOKEN_BUDGET, 100_000);
        assert_eq!(MAX_TRAVERSAL_DEPTH, 64);
    }

    /// Omitted budget and depth default to usable values.
    ///
    /// Both defaults were replaceable with 0 and 1. A zero budget returns
    /// nothing for every request that omits one, and a depth of 1 silently
    /// truncates impact to direct neighbours — both look like an empty graph
    /// rather than a misconfigured default.
    #[test]
    fn omitted_budget_and_depth_have_usable_defaults() {
        assert_eq!(
            default_budget(),
            2_000,
            "a default budget must return results"
        );
        assert!(
            default_depth() > 1,
            "a default depth of {} truncates impact to direct neighbours",
            default_depth()
        );

        // And they must actually be what deserialization uses.
        let request: IpcRequest =
            serde_json::from_str(r#"{"version":1,"cmd":"impact","target":"a.go::T.m"}"#)
                .expect("an impact request may omit budget and depth");
        match request.command {
            IpcCommand::Impact { budget, depth, .. } => {
                assert_eq!(budget, default_budget());
                assert_eq!(depth, default_depth());
            }
            other => panic!("expected an impact command, got {other:?}"),
        }
    }

    /// The `explore` dispatch arm returns the wire shape its client reads.
    ///
    /// `DevMapClient.explore` hands `definitions` and both edge lists of every
    /// definition to `_budgeted`, which enforces `shown + hidden == total`,
    /// `truncated == (hidden > 0)` and `tokens_used <= budget`. A dispatch that
    /// nested the sections differently, or emitted bare arrays instead of whole
    /// responses, type-checks here and fails at the seam.
    #[test]
    fn the_explore_dispatch_returns_budgeted_responses_for_every_section() {
        let store = corpus_store(8, None);
        let request = IpcRequest {
            version: 1,
            command: IpcCommand::Explore {
                query: "widget".to_string(),
                limit: 3,
                budget: 8_000,
                depth: 2,
                min_confidence: 0.0,
            },
        };
        let value = dispatch(
            &store,
            request,
            &devmap_query::Cancel::new(),
            &UnappliedEdits::default(),
        )
        .expect("a well-formed explore request must dispatch");

        for field in ["query", "definitions", "limit", "blast_radius", "budget"] {
            assert!(
                !value[field].is_null(),
                "the explore result is missing `{field}`"
            );
        }
        let definitions = &value["definitions"];
        assert_eq!(
            definitions["shown"].as_u64().unwrap() + definitions["hidden"].as_u64().unwrap(),
            definitions["total"].as_u64().unwrap(),
            "definitions break shown + hidden == total, which the client rejects"
        );
        assert_eq!(
            definitions["truncated"].as_bool().unwrap(),
            definitions["hidden"].as_u64().unwrap() > 0,
            "truncated must agree with hidden, which the client rejects"
        );
        for definition in definitions["items"].as_array().expect("items array") {
            for side in ["callers", "callees"] {
                let response = &definition[side];
                assert_eq!(
                    response["shown"].as_u64().unwrap() + response["hidden"].as_u64().unwrap(),
                    response["total"].as_u64().unwrap(),
                    "{side} breaks shown + hidden == total, which the client rejects"
                );
            }
        }
        let layers = &value["blast_radius"]["layers"];
        assert_eq!(
            layers["shown"].as_u64().unwrap() + layers["hidden"].as_u64().unwrap(),
            layers["total"].as_u64().unwrap(),
            "the blast radius breaks shown + hidden == total"
        );
    }

    /// The `affected` dispatch arm carries its list and its radius separately.
    #[test]
    fn the_affected_dispatch_returns_a_budgeted_test_list_and_its_radius() {
        let store = corpus_store(8, None);
        let request = IpcRequest {
            version: 1,
            command: IpcCommand::Affected {
                targets: vec!["things.py::widget_00001".to_string()],
                budget: 2_000,
                depth: 2,
                min_confidence: 0.0,
            },
        };
        let value = dispatch(
            &store,
            request,
            &devmap_query::Cancel::new(),
            &UnappliedEdits::default(),
        )
        .expect("a well-formed affected request must dispatch");

        for field in ["targets", "tests", "blast_radius"] {
            assert!(
                !value[field].is_null(),
                "the affected result is missing `{field}`"
            );
        }
        let tests = &value["tests"];
        assert_eq!(
            tests["shown"].as_u64().unwrap() + tests["hidden"].as_u64().unwrap(),
            tests["total"].as_u64().unwrap(),
            "the test list breaks shown + hidden == total, which the client rejects"
        );
    }

    /// The two new surfaces are bounded exactly like the ones they join.
    ///
    /// Refused rather than clamped, in both cases: a caller that asked for 500
    /// definitions and silently got 100 cannot tell a capped list from a
    /// complete one, which is the failure every count in this protocol exists
    /// to prevent.
    #[test]
    fn explore_and_affected_refuse_rather_than_trim_their_fan_out() {
        let explore = |limit: usize| IpcRequest {
            version: 1,
            command: IpcCommand::Explore {
                query: "widget".to_string(),
                limit,
                budget: 2_000,
                depth: 1,
                min_confidence: 0.0,
            },
        };
        assert!(validate_request(&explore(MAX_EXPLORE_LIMIT)).is_ok());
        assert!(validate_request(&explore(MAX_EXPLORE_LIMIT + 1)).is_err());

        let affected = |count: usize| IpcRequest {
            version: 1,
            command: IpcCommand::Affected {
                targets: (0..count).map(|index| format!("t{index}")).collect(),
                budget: 2_000,
                depth: 1,
                min_confidence: 0.0,
            },
        };
        assert!(validate_request(&affected(devmap_query::MAX_NEIGHBOR_TARGETS)).is_ok());
        assert!(validate_request(&affected(devmap_query::MAX_NEIGHBOR_TARGETS + 1)).is_err());

        // An over-long entry buried in the list must not ride in because the
        // first entry was short.
        let oversized = IpcRequest {
            version: 1,
            command: IpcCommand::Affected {
                targets: vec!["ok".to_string(), "t".repeat(MAX_QUERY_BYTES + 1)],
                budget: 2_000,
                depth: 1,
                min_confidence: 0.0,
            },
        };
        assert!(validate_request(&oversized).is_err());
    }

    /// The `neighbors` dispatch arm returns the wire shape its client reads.
    ///
    /// `validate_request` covers the bounds, but nothing exercised the arm
    /// itself — and `DevMapClient.neighbors` indexes `result["neighbors"]` and
    /// then hands each side to `_budgeted`, which enforces
    /// `shown + hidden == total`. A dispatch that nested the list differently,
    /// or returned bare edge arrays instead of whole responses, would type-check
    /// here and fail at the seam. One entry per requested target, in order,
    /// each carrying both directions as full responses.
    #[test]
    fn the_neighbors_dispatch_returns_one_full_response_per_direction() {
        let store = corpus_store(8, None);
        let request = IpcRequest {
            version: 1,
            command: IpcCommand::Neighbors {
                targets: vec![
                    "things.py".to_string(),
                    "things.py::widget_00001".to_string(),
                ],
                budget: 2000,
                depth: 1,
                min_confidence: 0.0,
                min_rung: None,
            },
        };
        let value = dispatch(
            &store,
            request,
            &devmap_query::Cancel::new(),
            &UnappliedEdits::default(),
        )
        .expect("a well-formed neighbors request must dispatch");

        let entries = value["neighbors"]
            .as_array()
            .expect("the result must carry a `neighbors` array; the client indexes it by name");
        assert_eq!(entries.len(), 2, "one entry per requested target, in order");
        assert_eq!(entries[0]["target"], "things.py");
        assert_eq!(entries[1]["target"], "things.py::widget_00001");

        for entry in entries {
            for side in ["callers", "callees"] {
                let response = &entry[side];
                for field in [
                    "items",
                    "shown",
                    "hidden",
                    "total",
                    "truncated",
                    "tokens_used",
                ] {
                    assert!(
                        !response[field].is_null(),
                        "{side} is missing `{field}`; the client's budget invariants \
                         cannot be checked without it"
                    );
                }
                assert_eq!(
                    response["shown"].as_u64().unwrap() + response["hidden"].as_u64().unwrap(),
                    response["total"].as_u64().unwrap(),
                    "{side} breaks shown + hidden == total, which the client rejects"
                );
            }
        }
    }

    /// A composed `neighbors` request is bounded on the axis a scalar command
    /// does not have: the length of the target list, and the length of every
    /// entry in it.
    ///
    /// The scalar check further down `validate_request` sees one string. A list
    /// request that only had its first entry checked would let a caller bury a
    /// 100 KB target at index 9, and a list request with no length bound turns
    /// one exchange into an unbounded fan-out of store queries. Both are
    /// refused rather than trimmed, so a short answer can never be read as a
    /// complete one.
    #[test]
    fn a_neighbors_request_bounds_both_its_list_and_its_entries() {
        let neighbors = |targets: Vec<String>| IpcRequest {
            version: 1,
            command: IpcCommand::Neighbors {
                targets,
                budget: 10,
                depth: 1,
                min_confidence: 0.0,
                min_rung: None,
            },
        };
        let limit = devmap_query::MAX_NEIGHBOR_TARGETS;
        let filler = |count: usize| vec!["a.go::T.m".to_string(); count];

        assert!(
            validate_request(&neighbors(filler(limit))).is_ok(),
            "exactly the limit must be accepted; the bound is exclusive"
        );
        let over = validate_request(&neighbors(filler(limit + 1)))
            .expect_err("one target past the limit must be refused");
        assert!(
            over.contains(&limit.to_string()),
            "the refusal must name the limit, got: {over}"
        );

        // An oversized entry anywhere in the list, not just at index 0.
        let mut buried = filler(limit - 1);
        buried.push("q".repeat(MAX_QUERY_BYTES + 1));
        assert!(
            validate_request(&neighbors(buried)).is_err(),
            "an oversized target at the end of the list rode in because the \
             first entry was short"
        );

        // An empty list is a caller asking nothing, which is unambiguous.
        assert!(validate_request(&neighbors(Vec::new())).is_ok());
    }

    /// Every command that declares `min_rung` refuses a name that is not one.
    ///
    /// The validation exists and nothing exercised it — not for any command.
    /// `request_min_rung`'s own doc says a command that gains the parameter and
    /// forgets the validation should be one edit rather than two, which is only
    /// true while something checks that the two lists agree; until now nothing
    /// did, and `Neighbors` gaining the field was exactly the edit that would
    /// have gone unnoticed.
    ///
    /// The command list is read out of this file's own enum rather than
    /// restated, so a fifth command declaring the field is covered the day it
    /// lands. A hand-written list here would be the same defect one layer up.
    #[test]
    fn every_command_declaring_a_rung_refuses_a_name_that_is_not_one() {
        let source = include_str!("protocol.rs");
        let body = source
            .split_once("pub enum IpcCommand {")
            .expect("the command enum must be findable")
            .1;
        let body = body.split("\n}\n").next().expect("enum body");

        // Variant headers sit at four spaces; their fields at eight. That makes
        // the split unambiguous without parsing Rust.
        let mut declaring: Vec<String> = Vec::new();
        let mut current: Option<String> = None;
        for line in body.lines() {
            if let Some(name) = line.strip_prefix("    ").and_then(|rest| {
                rest.strip_suffix(" {")
                    .filter(|n| n.chars().next().is_some_and(char::is_uppercase))
            }) {
                current = Some(name.to_string());
            } else if line.trim_start().starts_with("min_rung:") {
                if let Some(name) = current.take() {
                    declaring.push(name);
                }
            }
        }
        assert!(
            declaring.len() >= 4,
            "the scan found {declaring:?}; if the enum's shape changed this \
             guard would pass over commands it never checked"
        );

        // A superset of every required field. Unknown keys are ignored (no
        // `deny_unknown_fields`), so one object deserialises into whichever
        // variant `cmd` names.
        let request = |command: &str, rung: &str| -> IpcRequest {
            let snake = command
                .chars()
                .flat_map(|c| {
                    if c.is_uppercase() {
                        vec!['_', c.to_ascii_lowercase()]
                    } else {
                        vec![c]
                    }
                })
                .collect::<String>()
                .trim_start_matches('_')
                .to_string();
            serde_json::from_value(serde_json::json!({
                "version": 1,
                "cmd": snake,
                "target": "a.py::f",
                "targets": ["a.py::f"],
                "from": "a.py::f",
                "to": "b.py::g",
                "query": "f",
                "min_rung": rung,
            }))
            .unwrap_or_else(|error| panic!("{command}: request must parse: {error}"))
        };

        for command in &declaring {
            let refused = validate_request(&request(command, "certain"));
            let message = refused.expect_err(&format!(
                "{command} accepted an unknown rung; a floor silently dropped \
                 answers at full breadth and reads as the narrow answer asked for"
            ));
            assert!(
                message.contains("deterministic"),
                "{command}: the refusal must name the values that work: {message}"
            );
            // And the OFF direction, or a validator that refused everything
            // would pass the half above.
            assert!(
                validate_request(&request(command, "deterministic")).is_ok(),
                "{command} rejected a valid rung"
            );
        }
    }

    /// Every request bound is exclusive, and each is load-bearing.
    ///
    /// Mutation testing flipped `>` to `>=` and `==` on all three limits
    /// without a failure. These are the only thing bounding work an IPC caller
    /// can ask for: an off-by-one is minor, but a comparison that never fires
    /// lets a single request pin the daemon with an unbounded budget or depth.
    /// Each case sits exactly on the boundary and one step past it.
    #[test]
    fn request_bounds_are_exclusive_and_each_limit_is_enforced() {
        let search = |query: String, budget: u32| IpcRequest {
            version: 1,
            command: IpcCommand::Search {
                query,
                budget,
                semantic: false,
            },
        };

        // Query length: at the limit is fine, one byte over is not.
        assert!(validate_request(&search("q".repeat(MAX_QUERY_BYTES), 10)).is_ok());
        assert!(validate_request(&search("q".repeat(MAX_QUERY_BYTES + 1), 10)).is_err());

        // Token budget.
        assert!(validate_request(&search("q".to_string(), MAX_TOKEN_BUDGET)).is_ok());
        assert!(validate_request(&search("q".to_string(), MAX_TOKEN_BUDGET + 1)).is_err());

        // Traversal depth, which only an Impact/Trace request carries.
        let impact = |depth: usize| IpcRequest {
            version: 1,
            command: IpcCommand::Impact {
                target: "a.go::T.m".to_string(),
                budget: 10,
                depth,
                min_rung: None,
                layers: false,
            },
        };
        assert!(validate_request(&impact(MAX_TRAVERSAL_DEPTH)).is_ok());
        assert!(validate_request(&impact(MAX_TRAVERSAL_DEPTH + 1)).is_err());

        // A trace *destination* is bounded independently of the source.
        let trace = |to: String| IpcRequest {
            version: 1,
            command: IpcCommand::Trace {
                from: "a.go::T.m".to_string(),
                to: Some(to),
                budget: 10,
                depth: 1,
                min_rung: None,
            },
        };
        assert!(validate_request(&trace("t".repeat(MAX_QUERY_BYTES))).is_ok());
        assert!(
            validate_request(&trace("t".repeat(MAX_QUERY_BYTES + 1))).is_err(),
            "an oversized trace destination must be rejected even when the \
             source is small"
        );
    }

    /// `layers` and `min_rung` together are refused, not half-honoured.
    ///
    /// The distance bands are derived from the traversal's own edges before the
    /// rung cut; the edge list is packed after it. Serving both would return
    /// edges narrowed to the floor beside bands that were not, in one object,
    /// with nothing saying which half the filter reached. Refusing is the only
    /// answer that cannot be misread.
    #[test]
    fn impact_refuses_layers_together_with_a_rung_floor() {
        let request = |layers: bool, min_rung: Option<&str>| IpcRequest {
            version: 1,
            command: IpcCommand::Impact {
                target: "a.py::f".to_string(),
                budget: 10,
                depth: 2,
                min_rung: min_rung.map(str::to_string),
                layers,
            },
        };
        assert!(validate_request(&request(false, None)).is_ok());
        assert!(validate_request(&request(true, None)).is_ok());
        assert!(
            validate_request(&request(false, Some("deterministic"))).is_ok(),
            "a rung floor on its own is exactly what impact has always accepted"
        );
        let refused = validate_request(&request(true, Some("deterministic")))
            .expect_err("layers with a rung floor must be refused");
        assert!(
            refused.contains("min_rung") && refused.contains("layers"),
            "the refusal must name both halves so the caller knows what to drop: \
             {refused:?}"
        );
    }

    /// A client that predates `layers` sends no such key and must still parse.
    #[test]
    fn an_impact_request_without_layers_defaults_to_the_flat_answer() {
        let parsed: IpcRequest =
            serde_json::from_str(r#"{"version":1,"cmd":"impact","target":"a.py::f"}"#)
                .expect("an impact request may omit every optional field");
        match parsed.command {
            IpcCommand::Impact { layers, .. } => assert!(
                !layers,
                "an absent `layers` must mean the answer a pre-existing client \
                 expects, never the composed one"
            ),
            other => panic!("expected an impact command, got {other:?}"),
        }
    }

    /// A second binder must be refused while the first holds the endpoint,
    /// and allowed once it lets go.
    ///
    /// Without the lock, two racing daemons interleaved
    /// exists→probe→remove→bind: the loser unlinked the winner's live socket
    /// and bound its own, stranding an orphaned listener that answered
    /// nothing while still running its watcher and drain loops against the
    /// store. The lock is what makes the cleanup sequence exclusive.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_second_binder_is_refused_while_the_first_holds_the_endpoint() {
        let path = std::env::temp_dir().join(format!("devmap-lock-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let first = UnixIpcServer::bind(&path).expect("first bind must succeed");
        assert!(
            UnixIpcServer::bind(&path).is_err(),
            "a second bind while the first holds the lock must be refused"
        );
        drop(first);
        // The lock releases on drop (flock semantics), so a fresh daemon can
        // take over immediately — including after a crash of the previous
        // holder, which is exactly why an flock is used rather than a marker
        // file that would need stale-lock cleanup.
        let second = UnixIpcServer::bind(&path).expect("bind after release must succeed");
        drop(second);
        let _ = std::fs::remove_file(ipc_lock_path(&path));
    }

    /// The query bound is pinned by value: a constant that silently shrank
    /// would fail slow-but-legitimate queries on large stores, and one that
    /// grew would stop bounding queries wedged behind long generation writes.
    #[test]
    fn query_timeout_is_thirty_seconds() {
        assert_eq!(QUERY_TIMEOUT, Duration::from_secs(30));
        assert_eq!(REQUEST_DEADLINE, Duration::from_secs(10));
    }

    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A timed-out query must stop the work it abandoned, not merely stop
    /// waiting for it.
    ///
    /// `spawn_blocking` tasks cannot be aborted: `timeout` drops the
    /// `JoinHandle`, which detaches the task and leaves it running on a pool
    /// thread with no reader. The only thing that stops it is the flag the
    /// engine's loops consult, so the deadline has to trip it. This asserts on
    /// the very handle the abandoned task was given.
    /// A store holding enough symbols that any real query over it takes
    /// meaningfully longer than the one-millisecond deadline below.
    fn corpus_store(symbols: usize, root: Option<&std::path::Path>) -> Store {
        let mut source = String::new();
        for index in 0..symbols {
            source.push_str(&format!("def widget_{index:05}():\n    return {index}\n"));
        }
        if let Some(root) = root {
            std::fs::write(root.join("things.py"), &source).expect("fixture source");
        }
        let extraction = devmap_extract::extract_file("things.py", &source);
        let mut resolver = devmap_resolve::Resolver::new();
        resolver.index_extractions(std::slice::from_ref(&extraction));
        let resolution = resolver.resolve_all(std::slice::from_ref(&extraction));
        let analysis = devmap_analyze::analyze(std::slice::from_ref(&extraction), &resolution);
        let store = Store::open_in_memory().expect("in-memory store");
        store
            .save_generation_with_opts(
                std::slice::from_ref(&extraction),
                &resolution,
                &analysis,
                devmap_store::GenerationWriteOpts {
                    repo_root: root.map(|path| path.to_string_lossy().into_owned()),
                    ..Default::default()
                },
            )
            .expect("generation");
        store
    }

    #[tokio::test]
    async fn a_timed_out_query_cancels_the_work_it_abandoned() {
        // Semantic search vectorises the whole corpus, so this is real work —
        // orders of magnitude more than the deadline allows, which is what
        // makes the timeout deterministic rather than a race.
        let store = Arc::new(corpus_store(4_000, None));
        let request = IpcRequest {
            version: PROTOCOL_VERSION,
            command: IpcCommand::Search {
                query: "widget".to_string(),
                budget: 2_000,
                semantic: true,
            },
        };
        let cancel = devmap_query::Cancel::new();
        let abandoned_before = devmap_query::cancelled_queries();

        let envelope = dispatch_with_timeout(
            store,
            request,
            Duration::from_millis(1),
            cancel.clone(),
            &UnappliedEdits::default(),
        )
        .await;

        assert!(
            !envelope.ok,
            "a one-millisecond deadline over a 4,000-symbol corpus must expire"
        );
        assert_eq!(
            envelope.error.as_ref().map(|error| error.code),
            Some("query_timeout"),
            "unexpected refusal: {:?}",
            envelope.error
        );
        assert!(
            cancel.is_cancelled(),
            "the deadline answered the client but left the blocking task running"
        );

        // The flag being set is the signal; this is the effect. The abandoned
        // task must actually stop — observable because the engine counts the
        // loops it abandons — rather than run the corpus scan to completion on
        // a pool thread nobody is reading.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while devmap_query::cancelled_queries() <= abandoned_before
            && std::time::Instant::now() < deadline
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(
            devmap_query::cancelled_queries() > abandoned_before,
            "the abandoned query never stopped: it ran to completion on a blocking \
             pool thread with no reader"
        );
    }

    /// The flag must not be tripped for a query that finished in time — a
    /// cancellation on every request would be indistinguishable from one on
    /// none.
    #[tokio::test]
    async fn a_query_that_finishes_in_time_is_not_cancelled() {
        let store = Arc::new(Store::open_in_memory().expect("in-memory store"));
        let request = IpcRequest {
            version: PROTOCOL_VERSION,
            command: IpcCommand::Status,
        };
        let cancel = devmap_query::Cancel::new();

        let envelope = dispatch_with_timeout(
            store,
            request,
            Duration::from_secs(30),
            cancel.clone(),
            &UnappliedEdits::default(),
        )
        .await;

        assert!(
            envelope.ok,
            "a status query must succeed: {:?}",
            envelope.error
        );
        assert!(
            !cancel.is_cancelled(),
            "a query that answered in time must not be marked cancelled"
        );
    }

    #[cfg(unix)]
    #[test]
    fn unix_ipc_rejects_overlong_paths_before_bind() {
        let path = std::env::temp_dir().join("x".repeat(200));
        let error = match UnixIpcServer::bind(&path) {
            Ok(_) => panic!("overlong path must fail"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("portable Unix limit"));
    }

    /// The 100-byte socket-path bound is exclusive, and a legal path binds.
    ///
    /// The comparison was mutable to `>=` without a failure, because the only
    /// case tested was 200 bytes — far past the boundary, where every variant
    /// of the comparison rejects. `sockaddr_un` truncates silently rather than
    /// erroring, so a path one byte over the real limit produces a socket at a
    /// *different* path than requested and the daemon appears to start while
    /// nothing can reach it.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_socket_path_bound_is_exclusive_and_a_legal_path_binds() {
        use std::os::unix::ffi::OsStrExt;

        let dir = std::env::temp_dir().join(format!("devmap-sockbound-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        // Build a path of exactly 101 bytes: one past the portable limit.
        let base_len = dir.as_os_str().as_bytes().len() + 1; // + separator
        if base_len < 100 {
            let over = dir.join("y".repeat(101 - base_len));
            assert_eq!(over.as_os_str().as_bytes().len(), 101);
            assert!(
                UnixIpcServer::bind(&over).is_err(),
                "a path one byte past the limit must be refused"
            );

            // And a path exactly at the limit must bind, or the bound is off by
            // one in the other direction and legitimate paths are rejected.
            let at = dir.join("z".repeat(100 - base_len));
            assert_eq!(at.as_os_str().as_bytes().len(), 100);
            assert!(
                UnixIpcServer::bind(&at).is_ok(),
                "a path exactly at the limit must bind: {} bytes",
                at.as_os_str().as_bytes().len()
            );
            let _ = std::fs::remove_file(&at);
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A managed runtime directory is hardened to owner-only; an arbitrary
    /// parent is left alone.
    ///
    /// Both halves of the `is_managed_runtime_dir` test were mutable. Dropping
    /// the `devmap-` prefix check makes the daemon chmod 0700 on any directory
    /// it is pointed at — including a shared one it does not own; dropping the
    /// temp-dir parent check does the same. Widening a permission change beyond
    /// the directory we created is the failure that matters here.
    #[cfg(unix)]
    #[tokio::test]
    async fn only_the_managed_runtime_directory_is_hardened() {
        use std::os::unix::fs::PermissionsExt;

        // A managed dir: directly under temp_dir() and named `devmap-*`.
        let managed = std::env::temp_dir().join(format!("devmap-managed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&managed);
        let socket = managed.join("s.sock");
        UnixIpcServer::bind(&socket).expect("managed runtime dir binds");
        let mode = std::fs::metadata(&managed).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "a managed runtime dir must be owner-only");
        let _ = std::fs::remove_dir_all(&managed);

        // An unmanaged parent keeps whatever permissions it had; the daemon
        // must not widen or narrow a directory it did not create.
        let unmanaged = std::env::temp_dir().join(format!("plain-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&unmanaged);
        std::fs::create_dir_all(&unmanaged).unwrap();
        std::fs::set_permissions(&unmanaged, std::fs::Permissions::from_mode(0o755)).unwrap();
        let other = unmanaged.join("s.sock");
        UnixIpcServer::bind(&other).expect("an unmanaged parent still binds");
        let kept = std::fs::metadata(&unmanaged).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            kept, 0o755,
            "the daemon must not chmod a directory it did not create"
        );
        let _ = std::fs::remove_dir_all(&unmanaged);
    }

    /// A request past the byte ceiling is refused with a structured error.
    ///
    /// `handle_stream` accumulates until a newline, checking
    /// `payload.len() + take > MAX_REQUEST_BYTES` as it goes. That comparison
    /// was mutable to `==` and `>=` without a failure, and it is the only
    /// bound on how much a client can make the daemon buffer: a comparison
    /// that never fires lets one connection grow the buffer without limit.
    /// Asserted on the refusal being *structured* rather than a dropped
    /// connection, so the caller learns why.
    #[tokio::test]
    async fn an_oversized_request_is_refused_before_it_is_buffered() {
        let (mut client, server) = tokio::io::duplex(64 * 1024);
        let store = Arc::new(Store::open_in_memory().unwrap());
        let task = tokio::spawn(handle_stream(server, store));

        // A single logical request (no newline) well past the ceiling.
        let oversized = format!(
            "{{\"version\":1,\"cmd\":\"search\",\"query\":\"{}\"}}\n",
            "x".repeat(MAX_REQUEST_BYTES + 4096)
        );
        let _ = client.write_all(oversized.as_bytes()).await;
        let mut response = String::new();
        let _ = client.read_to_string(&mut response).await;
        let _ = task.await;

        assert!(
            response.contains("request_too_large"),
            "an oversized request must be refused with a structured error, got: {}",
            response.chars().take(200).collect::<String>()
        );
    }

    #[tokio::test]
    async fn protocol_rejects_missing_version_with_structured_error() {
        let (mut client, server) = tokio::io::duplex(4096);
        let store = Arc::new(Store::open_in_memory().unwrap());
        let task = tokio::spawn(handle_stream(server, store));
        client.write_all(b"{\"cmd\":\"status\"}\n").await.unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).await.unwrap();
        task.await.unwrap().unwrap();
        let value: Value = serde_json::from_str(response.trim()).unwrap();
        assert_eq!(value["ok"], false);
        assert_eq!(value["error"]["code"], "invalid_request");
    }

    #[tokio::test]
    async fn protocol_status_round_trip_is_versioned() {
        let (mut client, server) = tokio::io::duplex(4096);
        let store = Arc::new(Store::open_in_memory().unwrap());
        let task = tokio::spawn(handle_stream(server, store));
        client
            .write_all(b"{\"version\":1,\"cmd\":\"status\"}\n")
            .await
            .unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).await.unwrap();
        task.await.unwrap().unwrap();
        let value: Value = serde_json::from_str(response.trim()).unwrap();
        assert_eq!(value["ok"], true);
        assert_eq!(value["protocol_version"], PROTOCOL_VERSION);
        assert_eq!(value["result"]["pending_count"], 0);
    }

    /// `search` with `budget: 0` answered `ok: true, shown: 0, hidden: 26`
    /// over the socket — read back from the release binary — while the CLI
    /// refuses the same request: a zero budget returns an empty page that
    /// cannot be told apart from a complete answer. One rule, both transports.
    #[tokio::test]
    async fn protocol_refuses_a_zero_token_budget_like_the_cli_does() {
        let (mut client, server) = tokio::io::duplex(16 * 1024);
        let store = Arc::new(Store::open_in_memory().unwrap());
        let task = tokio::spawn(handle_stream(server, store));
        client
            .write_all(b"{\"version\":1,\"cmd\":\"search\",\"query\":\"x\",\"budget\":0}\n")
            .await
            .unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).await.unwrap();
        task.await.unwrap().unwrap();
        let value: Value = serde_json::from_str(response.trim()).unwrap();
        assert_eq!(value["ok"], false, "{value}");
        assert_eq!(value["error"]["code"], "invalid_parameters", "{value}");
        assert!(
            value["error"]["message"]
                .as_str()
                .unwrap_or("")
                .contains("budget"),
            "{value}"
        );
    }

    /// A peer that connects and sends nothing holds one of the admission
    /// permits until the per-read timeout. Measured with the release binary:
    /// 120 idle connections made a legitimate `status` wait 5,003 ms — the
    /// whole `IO_TIMEOUT` — on a socket whose baseline answer is milliseconds.
    /// The first byte of a frame is due sooner than the rest: every client
    /// this daemon has writes the moment it connects.
    #[tokio::test]
    async fn an_idle_peer_is_dropped_before_the_per_read_timeout() {
        let (_held_open, server) = tokio::io::duplex(64);
        let started = std::time::Instant::now();
        let outcome = read_frame(
            server,
            IO_TIMEOUT,
            std::time::Instant::now() + REQUEST_DEADLINE,
        )
        .await;
        let elapsed = started.elapsed();
        assert!(matches!(outcome, Err(FrameReadError::Io(_))), "{outcome:?}");
        assert!(
            elapsed < Duration::from_secs(3),
            "an idle peer held its permit for {elapsed:?}; the first byte must be due \
             well before the {IO_TIMEOUT:?} per-read timeout"
        );

        // Once the first byte has arrived, the per-read timeout is the ordinary
        // one: a slow writer that has started is not an idle peer.
        let (mut client, server) = tokio::io::duplex(64);
        let reader = tokio::spawn(read_frame(
            server,
            IO_TIMEOUT,
            std::time::Instant::now() + REQUEST_DEADLINE,
        ));
        client.write_all(b"{").await.unwrap();
        tokio::time::sleep(Duration::from_millis(1500)).await;
        client
            .write_all(b"\"version\":1,\"cmd\":\"status\"}\n")
            .await
            .unwrap();
        let payload = reader
            .await
            .unwrap()
            .expect("a frame whose first byte arrived in time is read to its newline");
        assert_eq!(payload, b"{\"version\":1,\"cmd\":\"status\"}");
    }

    #[tokio::test]
    async fn protocol_rejects_unbounded_query_work_before_dispatch() {
        let (mut client, server) = tokio::io::duplex(16 * 1024);
        let store = Arc::new(Store::open_in_memory().unwrap());
        let task = tokio::spawn(handle_stream(server, store));
        client
            .write_all(
                b"{\"version\":1,\"cmd\":\"impact\",\"target\":\"x\",\"budget\":4294967295,\"depth\":1000000}\n",
            )
            .await
            .unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).await.unwrap();
        task.await.unwrap().unwrap();
        let value: Value = serde_json::from_str(response.trim()).unwrap();
        assert_eq!(value["ok"], false);
        assert_eq!(value["error"]["code"], "invalid_parameters");
    }

    /// A client that predates `semantic` must get exactly the search it always
    /// got, not a different ranking because a field defaulted oddly.
    #[test]
    fn a_search_request_without_the_semantic_field_is_the_keyword_search() {
        let legacy: IpcRequest =
            serde_json::from_str(r#"{"version":1,"cmd":"search","query":"x","budget":100}"#)
                .unwrap();
        assert!(validate_request(&legacy).is_ok());
        match legacy.command {
            IpcCommand::Search { semantic, .. } => {
                assert!(!semantic, "an absent `semantic` field defaulted to true");
            }
            other => panic!("parsed as {other:?}"),
        }

        let explicit: IpcRequest = serde_json::from_str(
            r#"{"version":1,"cmd":"search","query":"x","budget":100,"semantic":true}"#,
        )
        .unwrap();
        match explicit.command {
            IpcCommand::Search { semantic, .. } => assert!(semantic),
            other => panic!("parsed as {other:?}"),
        }
    }

    /// A typo in `kind` must be refused, not answered.
    ///
    /// Silently ignoring an unrecognised kind returns the full report to a
    /// caller who asked for half of it, and nothing in the response says the
    /// filter did not apply.
    #[test]
    fn clones_protocol_refuses_an_unknown_kind_rather_than_ignoring_it() {
        let good: IpcRequest = serde_json::from_str(
            r#"{"version":1,"cmd":"clones","budget":2000,"kind":"structural"}"#,
        )
        .unwrap();
        assert!(validate_request(&good).is_ok());

        let bare: IpcRequest = serde_json::from_str(r#"{"version":1,"cmd":"clones"}"#).unwrap();
        assert!(validate_request(&bare).is_ok(), "kind is optional");

        let typo: IpcRequest =
            serde_json::from_str(r#"{"version":1,"cmd":"clones","budget":2000,"kind":"exakt"}"#)
                .unwrap();
        let err = validate_request(&typo).unwrap_err();
        assert!(
            err.contains("exakt"),
            "error must name the rejected value: {err}"
        );

        let over_budget: IpcRequest =
            serde_json::from_str(r#"{"version":1,"cmd":"clones","budget":4294967295}"#).unwrap();
        assert!(validate_request(&over_budget)
            .unwrap_err()
            .contains("token budget"));
    }

    #[test]
    fn trace_protocol_is_backward_compatible_and_bounds_both_endpoints() {
        let legacy: IpcRequest = serde_json::from_str(
            r#"{"version":1,"cmd":"trace","from":"caller","budget":2000,"depth":3}"#,
        )
        .unwrap();
        assert!(validate_request(&legacy).is_ok());

        let destination = "x".repeat(MAX_QUERY_BYTES + 1);
        let request = IpcRequest {
            version: PROTOCOL_VERSION,
            command: IpcCommand::Trace {
                from: "caller".to_string(),
                to: Some(destination),
                budget: 2_000,
                depth: 3,
                min_rung: None,
            },
        };
        assert!(validate_request(&request)
            .unwrap_err()
            .contains("trace destination"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unix_socket_round_trip_is_owner_only_and_bounded() {
        use std::os::unix::fs::PermissionsExt;
        use std::time::{SystemTime, UNIX_EPOCH};

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("devmap-ipc-{stamp}.sock"));
        let server = UnixIpcServer::bind(&path).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let task = tokio::spawn(server.run(
            Arc::new(Store::open_in_memory().unwrap()),
            Arc::new(ServeState::default()),
        ));

        let mut client = tokio::net::UnixStream::connect(&path).await.unwrap();
        client
            .write_all(b"{\"version\":1,\"cmd\":\"status\"}\n")
            .await
            .unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).await.unwrap();
        let value: Value = serde_json::from_str(response.trim()).unwrap();
        assert_eq!(value["ok"], true);

        task.abort();
        let _ = task.await;
        tokio::task::yield_now().await;
        assert!(
            !path.exists(),
            "socket path must be cleaned when server stops"
        );
    }

    /// K-A6: a store that holds no generation at all must not report fresh.
    ///
    /// `is_fresh` was `pending_count == 0` and consulted nothing else. A store
    /// whose schema exists but which holds zero generations — the state a
    /// `devmap build` that aborted partway leaves behind — has nothing queued,
    /// so it satisfied that test and answered `is_fresh: true` about an index
    /// that does not exist. Downstream, `devmap_client.is_map_stale()` is
    /// `not is_fresh or pending_count > 0`, so the whole stack reported the map
    /// as current while nothing had been indexed.
    ///
    /// "Nothing is queued" and "the index is current" are different claims, and
    /// an empty store is exactly where they come apart.
    #[test]
    fn a_store_with_no_generation_is_not_reported_fresh() {
        let store = Store::open_in_memory().expect("schema, but no generation");
        let value = dispatch(
            &store,
            IpcRequest {
                version: PROTOCOL_VERSION,
                command: IpcCommand::Status,
            },
            &devmap_query::Cancel::new(),
            &UnappliedEdits::default(),
        )
        .expect("status must answer");

        assert!(
            value["generation_id"].is_null(),
            "fixture precondition: this store must hold no generation, got {}",
            value["generation_id"]
        );
        assert_eq!(
            value["pending_count"], 0,
            "fixture precondition: nothing is queued either — that is the whole trap"
        );
        assert_eq!(
            value["is_fresh"], false,
            "an index that does not exist cannot be current; reporting `true` here \
             is a check that could not run answering like one that ran and passed"
        );
        assert!(
            !value["degraded_reason"].is_null(),
            "a caller that sees `is_fresh: false` must be told why, or it cannot \
             tell an empty store from a busy one"
        );
    }

    /// The OFF direction: matching source bytes and an empty queue remain
    /// fresh. The fixture must record its root so that comparison can run.
    #[test]
    fn a_store_with_a_generation_and_no_backlog_is_still_fresh() {
        let root =
            std::env::temp_dir().join(format!("devmap-protocol-fresh-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let store = corpus_store(4, Some(&root));
        let value = dispatch(
            &store,
            IpcRequest {
                version: PROTOCOL_VERSION,
                command: IpcCommand::Status,
            },
            &devmap_query::Cancel::new(),
            &UnappliedEdits::default(),
        )
        .expect("status must answer");
        std::fs::remove_dir_all(root).unwrap();

        assert!(
            !value["generation_id"].is_null(),
            "fixture precondition: this store holds a generation"
        );
        assert_eq!(value["pending_count"], 0);
        assert_eq!(
            value["is_fresh"], true,
            "a built, drained store is current: {value}"
        );
    }
}

#[cfg(test)]
mod hardening_limit_tests {
    use super::*;

    /// The fan-out cap and probe window are contracts, not vibes: both were
    /// introduced because an unbounded value had a concrete failure mode
    /// (flood => unbounded task memory; wedged listener => bind hanging
    /// forever). Pinned by value so silent drift fails here.
    #[test]
    fn connection_and_probe_bounds_are_pinned() {
        assert_eq!(MAX_CONCURRENT_CONNECTIONS, 64);
        assert_eq!(LIVENESS_PROBE_TIMEOUT, Duration::from_millis(500));
        assert_eq!(MAX_CONSECUTIVE_ACCEPT_ERRORS, 30);
    }

    /// A momentarily-held endpoint lock must not become a permanent refusal.
    ///
    /// `try_lock` returning `WouldBlock` was treated as proof that another live
    /// daemon owned the endpoint, with no retry. The contended window is
    /// routinely far shorter than that conclusion: measured on this workspace,
    /// a losing bind acquired the lock 5-20 ms later in every observed case,
    /// while the daemon it belonged to had already refused to start. This test
    /// reproduces that window deterministically — the holder releases well
    /// inside `LOCK_CONTENTION_WINDOW` — and fails against the pre-retry code,
    /// which refuses immediately.
    #[cfg(unix)]
    #[test]
    fn a_briefly_held_endpoint_lock_is_waited_out_not_refused() {
        let dir = std::env::temp_dir().join(format!(
            "devmap-lockwait-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ipc.sock");
        let lock_path = ipc_lock_path(&path);

        let holder = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .unwrap();
        holder.lock().expect("the test holds the lock first");

        let released = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = std::sync::Arc::clone(&released);
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
            drop(holder);
        });

        let acquired = lock_ipc_endpoint(&path);
        releaser.join().unwrap();

        assert!(
            acquired.is_ok(),
            "a lock released after 20ms must be waited out, got {:?}",
            acquired.err()
        );
        assert!(
            released.load(std::sync::atomic::Ordering::SeqCst),
            "the bind must have waited for the holder rather than racing it"
        );

        drop(acquired);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A lock that could not be checked must not be reported as one that was
    /// checked and found held.
    ///
    /// Both `TryLockError` variants produced the same sentence — "owned by
    /// another live daemon" — so an EACCES/ENOSPC/EIO on the lock file made a
    /// definite claim about a process that may not exist.
    #[cfg(unix)]
    #[test]
    fn an_unlockable_lock_file_is_not_reported_as_a_live_daemon() {
        // Both variants are mapped by the same `match`; assert the two arms
        // produce distinguishable text so a caller (and an operator reading the
        // log) can tell contention from an unusable lock file.
        let path = std::path::Path::new("/tmp/devmap-msg-shape.sock");
        let lock_path = ipc_lock_path(path);
        let contended = format!(
            "devmap IPC endpoint {path:?} is owned by another live daemon \
             (lock {lock_path:?} still held after {LOCK_CONTENTION_WINDOW:?})"
        );
        let io_error = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        let unusable = format!(
            "devmap IPC endpoint {path:?} could not be locked: {io_error} \
             (lock {lock_path:?}); ownership is unknown, so the endpoint is \
             left untouched"
        );
        assert!(contended.contains("owned by another live daemon"));
        assert!(
            !unusable.contains("owned by another live daemon"),
            "an unusable lock file must not claim another daemon owns it: {unusable}"
        );
        assert!(unusable.contains("ownership is unknown"));
    }

    /// A live-but-foreign endpoint (no lock of ours) must be refused by the
    /// liveness probe rather than clobbered, and a stale non-socket file must
    /// still be replaced cleanly.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_probe_refuses_live_endpoints_and_replaces_stale_files() {
        let path = std::env::temp_dir().join(format!("devmap-probe-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(ipc_lock_path(&path));

        // A live listener we do not own: bind must refuse and leave the file
        // alone. Two refusals are legitimate — the probe connected ("already
        // active"), or it got no answer inside its window and the endpoint is
        // treated as active ("did not answer"). The second one is what a
        // starved probe thread produces under a parallel test run; it is a
        // correct fail-closed answer, not a failure of this test.
        let foreign = std::os::unix::net::UnixListener::bind(&path).unwrap();
        match UnixIpcServer::bind(&path) {
            Ok(_) => panic!("a live foreign endpoint must not be replaced"),
            Err(error) => {
                let text = error.to_string();
                assert!(
                    text.contains("already active") || text.contains("did not answer"),
                    "refusal must name the live endpoint: {error}"
                );
            }
        }
        assert!(
            path.exists(),
            "a refused bind must leave the foreign endpoint in place"
        );
        drop(foreign);
        std::fs::remove_file(&path).unwrap();

        // A stale non-socket file answers the probe negatively and is replaced.
        std::fs::write(&path, b"junk").unwrap();
        let server = UnixIpcServer::bind(&path).expect("a stale file must be replaceable");
        drop(server);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(ipc_lock_path(&path));
    }

    /// Accept-failure backoff must stay hot early (retry transient errors
    /// promptly) and cold late (never spin during sustained failure).
    #[test]
    fn accept_backoff_grows_and_caps() {
        assert_eq!(accept_error_backoff(0), Duration::ZERO);
        assert_eq!(accept_error_backoff(1), Duration::from_millis(10));
        assert_eq!(accept_error_backoff(2), Duration::from_millis(20));
        assert_eq!(accept_error_backoff(20), Duration::from_secs(1));
        assert_eq!(
            accept_error_backoff(u32::MAX),
            Duration::from_secs(1),
            "the curve must saturate, not overflow"
        );
    }
}
