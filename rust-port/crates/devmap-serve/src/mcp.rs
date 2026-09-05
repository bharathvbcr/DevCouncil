//! A native Model Context Protocol server over the kernel's query engine.
//!
//! # Why this exists
//!
//! Before this module the only way an agent reached the Rust kernel was through
//! the Python seam, and `DevMapClient._request` spawns a `devmap` subprocess
//! whenever no daemon socket is live. That is a process launch, a store open and
//! a store close on every tool call — paid per question, not per session.
//!
//! This server holds the store open for the life of the connection and answers
//! from it directly, so the marginal cost of a tool call is the query.
//!
//! # The one rule this module is built around
//!
//! **There is exactly one dispatcher.** MCP tool calls are translated into the
//! same [`IpcCommand`] the socket protocol already speaks and handed to the same
//! [`crate::protocol::dispatch`], behind the same [`crate::protocol::validate_request`]
//! bounds. A second implementation of "what does `impact` mean" would drift from
//! the first, and the drift would be invisible: both would answer, and only one
//! would be right.
//!
//! The translation is deliberately not a `match` over tool names building
//! `IpcCommand` variants by hand. `IpcCommand` is `#[serde(tag = "cmd")]`, so a
//! tool's argument object with `"cmd": "<name>"` inserted *is* its wire form.
//! Deserializing through serde reuses the field defaults (`default_budget`,
//! `default_depth`), the `Option` handling and the rename rules that the socket
//! path already uses. A hand-written translation would restate those defaults,
//! and a restated default is a default that can disagree.
//!
//! # Honesty
//!
//! A tool call that could not run says so. It never returns an empty success.
//! This is the repository's Class A rule at the protocol edge: a check that
//! could not run must not report what a check that ran and passed reports,
//! because an agent reading `[]` deletes the function that list was supposed to
//! protect.
//!
//! *How* it says so is not a free choice. The specification defines two
//! mechanisms and they reach different readers: `isError: true` inside the
//! result reaches the model, which is asked to self-correct; a JSON-RPC error
//! reaches the client runtime, which is not. So a bad argument *value* is a tool
//! error, and a fault in the request itself — an unknown tool, a `tools/call`
//! that does not satisfy the `CallToolRequest` schema — is a protocol error. See
//! [`RpcError::is_tool_input_fault`], which carries that judgement from the
//! point that makes it to the point that acts on it.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use devmap_store::Store;
use serde_json::{json, Map, Value};
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};

use crate::admission::Admission;
use crate::protocol::{dispatch, validate_request, IpcCommand, IpcRequest, PROTOCOL_VERSION};

/// The store, opened on first successful use rather than at startup.
///
/// An agent host launches this server when the session begins, which on a fresh
/// clone is *before* anything has built an index. Refusing to start there would
/// remove the whole server from the agent's tool list — the agent would not see
/// a broken index, it would see no code intelligence at all, with nothing
/// naming the reason.
///
/// Opening lazily also survives the normal case: the daemon builds the store
/// while this process is already running, so a slot that resolved "missing" once
/// at startup and cached that answer would stay wrong for the rest of the
/// session. Each call retries until one succeeds; after that the handle is held
/// for the life of the process, which is the point of this server.
pub struct StoreSlot {
    db_path: PathBuf,
    opened: Mutex<Option<Arc<Store>>>,
}

impl StoreSlot {
    pub fn new(db_path: impl Into<PathBuf>) -> Self {
        Self {
            db_path: db_path.into(),
            opened: Mutex::new(None),
        }
    }

    /// An already-open store, for tests and for callers that own one.
    pub fn ready(db_path: impl Into<PathBuf>, store: Arc<Store>) -> Self {
        Self {
            db_path: db_path.into(),
            opened: Mutex::new(Some(store)),
        }
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// The store, or the reason there isn't one.
    ///
    /// The error is the message an agent sees, so it names the path and the
    /// command that fixes it. "No index" and "the symbol does not exist" are
    /// different facts and must not arrive looking alike.
    fn get(&self) -> Result<Arc<Store>, String> {
        let mut slot = self
            .opened
            .lock()
            .map_err(|_| "store slot mutex was poisoned by an earlier panic".to_string())?;
        if let Some(store) = slot.as_ref() {
            return Ok(Arc::clone(store));
        }
        match Store::open_existing(&self.db_path) {
            Ok(Some(store)) => {
                let store = Arc::new(store);
                *slot = Some(Arc::clone(&store));
                Ok(store)
            }
            // `open_existing` answers `Ok(None)` for anything that is not a
            // readable file, and `Path::is_file` returns false for a directory
            // *and* for any IO error it swallows. Reporting all of those as
            // "the index has not been built" makes a definite claim on behalf
            // of a check that never ran, and sends the caller to run
            // `devmap build`, which will fail again for the unstated reason.
            Ok(None) => Err(self.explain_absence()),
            Err(err) => Err(format!(
                "devmap index at {} could not be opened: {err}",
                self.db_path.display()
            )),
        }
    }

    /// Why there is no store at this path — as distinct facts, not one guess.
    ///
    /// Each arm is a different problem with a different fix, and the caller is
    /// an agent that will act on whichever one it is told. `symlink_metadata`
    /// rather than `metadata` so a dangling symlink reports as itself instead of
    /// as a missing file.
    fn explain_absence(&self) -> String {
        let path = self.db_path.display();
        match std::fs::symlink_metadata(&self.db_path) {
            Ok(meta) if meta.is_dir() => format!(
                "the devmap index path {path} is a directory, not a database file. Nothing can \
be read from it and `devmap build` will not fix it — the path is wrong, or something else \
created a directory there."
            ),
            Ok(meta) if meta.file_type().is_symlink() => format!(
                "the devmap index path {path} is a symlink that does not resolve to a readable \
file."
            ),
            Ok(_) => format!(
                "the devmap index at {path} exists but could not be read as a file — most \
likely a permissions problem on it or on a parent directory."
            ),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => format!(
                "no devmap index at {path} — run `devmap build` (or `dev map`) in this \
repository first. This is 'the index has not been built', not 'the repository is empty'."
            ),
            Err(err) => format!(
                "the devmap index at {path} could not be examined: {err}. This is not a \
statement about whether an index exists — the check itself failed."
            ),
        }
    }
}

/// Protocol revisions this server will negotiate over the stdio handshake.
///
/// Ordered oldest to newest; the newest is what we counter-offer when a client
/// asks for something we do not recognise. This mirrors the reference SDK's
/// `HANDSHAKE_PROTOCOL_VERSIONS` (`mcp_types.version`), which deliberately stops
/// short of the `2026-07-28` revision: that revision uses a stateless
/// per-request envelope and is reached over the modern HTTP transport, not over
/// an `initialize` handshake. Listing it here would advertise a negotiation this
/// transport cannot perform.
pub const HANDSHAKE_PROTOCOL_VERSIONS: &[&str] =
    &["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"];

/// The revision offered when a client's request is absent or unrecognised.
pub const LATEST_HANDSHAKE_VERSION: &str = "2025-11-25";

/// Largest single JSON-RPC frame accepted, in bytes.
///
/// Matches the socket protocol's `MAX_REQUEST_BYTES`. The `preview` tool
/// carries file content, so this cannot be small; it exists so that a peer
/// which never sends a newline cannot grow this process's heap without bound.
const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// Upper bound on one tool call's occupancy of the connection.
///
/// The same 30s the socket path allows a query. A tool call runs on a blocking
/// thread and shares the store mutex with any writer, so an unbounded call can
/// pin the connection for as long as a write holds the lock. Bounded, the agent
/// gets a structured error it can report and retry instead of a hang it can
/// only kill.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// Requests one connection may have outstanding at once.
///
/// Requests are served concurrently — they have to be, or a cancellation cannot
/// be read while the call it cancels is still running — and concurrency without
/// a ceiling is a peer choosing this process's memory and thread-pool
/// occupancy. A pipelining client that never waits for an answer got one spawned
/// task, one in-flight map entry and (for `tools/call`) one queued blocking
/// closure holding an `Arc<Store>` per frame, for as many frames as it cared to
/// write, with nothing reaping them until they finished on their own.
///
/// **Backpressure, not shedding.** At the ceiling the read loop waits for the
/// oldest task instead of refusing the new frame. Refusing was the first
/// attempt and it was wrong: `concurrent_requests_never_interleave_or_lose_an_id`
/// pipelines 200 requests and requires 200 results, which is a realistic depth
/// for an agent host draining a plan — so a shed at 64 turned a bound into
/// visible data loss for a client doing nothing wrong. Waiting costs latency and
/// loses nothing, which is the correct trade for a bound whose whole purpose is
/// to stop memory growth.
///
/// 256 sits above that 200-deep pipeline, so the ordinary case never waits, and
/// far below "unbounded". A connection parked at the ceiling delays reading —
/// including a cancellation — until one call retires, which is bounded by
/// [`CALL_TIMEOUT`]; that is the unavoidable cost of any ceiling, and 256 makes
/// it a case a real client has to work to reach.
///
/// The ceiling is *held* by [`crate::admission::Admission`], which is the one
/// implementation for all three transports and the only thing that can say what
/// a bound actually did: `peak()` is the high-water mark and `shed()` the count
/// refused, and a ceiling nobody can measure is indistinguishable from a server
/// that is merely busy. stdio takes the waiting policy — [`Admission::admit`],
/// the same one the socket transport uses — for the reason above; only the HTTP
/// transport sheds, because a 503 with `Retry-After` is something an HTTP client
/// already knows how to act on and a paused accept loop is not.
pub const MAX_IN_FLIGHT_REQUESTS: usize = 256;

/// Largest tool result this server will send, in bytes of serialized JSON.
///
/// The read side has always been bounded — [`MAX_FRAME_BYTES`] on stdio,
/// `MAX_BODY_BYTES` over HTTP, both 1 MB — and the write side was not. That
/// asymmetry is not academic: a client built the way this server is built
/// refuses to read a frame this server was willing to write, and the agent sees
/// a transport failure rather than "your budget was too large".
///
/// The number is derived, not chosen. The declared maximum token budget is
/// 100,000 and the engine charges 4 bytes per token, so the largest answer any
/// single-target tool can produce is about 400 KB; the specification asks for
/// the payload to ride twice (once as `structuredContent`, once serialized into
/// a text block for clients that read only `content`), and JSON escaping can
/// grow the text copy again, so the largest *result* those tools can produce is
/// comfortably under 2 MB. Eight leaves room for that to be wrong.
///
/// What it does not leave room for is `devmap_neighbors` at a high budget:
/// 16 targets x 2 directions x 100,000 tokens is 12.8 MB of payload and roughly
/// 26 MB of result. That call is exactly the one a caller must be *told* about
/// rather than handed, which is why the refusal below names the multiplication
/// and the parameter instead of truncating. Truncating is the one thing this
/// must not do: a cut answer is indistinguishable from a complete one.
const MAX_RESULT_BYTES: usize = 8 * 1024 * 1024;

/// JSON-RPC 2.0's reserved codes, and the ones MCP defines on top of them.
///
/// One module rather than a constant per transport, because the specification
/// partitions the implementation-defined range and the partition is a rule about
/// the *server*, not about a transport: "`-32020` to `-32099` — reserved for the
/// MCP specification. Implementations **MUST NOT** emit any code from this
/// sub-range that is not defined by this specification and **MUST** use defined
/// codes only with their specified meanings." A second copy of `-32022` living
/// in the HTTP module is one edit away from being a second copy with a different
/// number, and the number is the entire interface.
pub mod codes {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL_ERROR: i64 = -32603;

    /// `HeaderMismatch`: the HTTP headers and the body describe different
    /// requests, or a required mirrored header is missing or malformed.
    pub const HEADER_MISMATCH: i64 = -32020;

    /// `UnsupportedProtocolVersion`: the revision the request declared is not
    /// one this server serves through the mechanism the request used.
    ///
    /// Carries `data: {supported, requested}` — required, not decorative. The
    /// client's documented recovery is to "select a mutually supported version
    /// from the `supported` list and retry", so a refusal without it is a dead
    /// end that reads to the client as "the server is down".
    pub const UNSUPPORTED_PROTOCOL_VERSION: i64 = -32022;

    /// Codes this server must never emit, with the reason.
    ///
    /// `-32000`..=`-32019` is the sub-range the specification retired: "New
    /// codes **MUST NOT** be allocated in this sub-range, and new
    /// implementations **SHOULD NOT** use codes from this sub-range at all."
    /// `-32020`..=`-32099` is reserved for codes the specification defines, and
    /// the three it has defined are the two above plus `-32021`
    /// (`MissingRequiredClientCapability`), which this server has no use for: it
    /// requires no client capability, so emitting it would name a requirement
    /// that does not exist.
    pub fn is_reserved_and_undefined(code: i64) -> bool {
        (-32099..=-32000).contains(&code)
            && !matches!(code, HEADER_MISMATCH | UNSUPPORTED_PROTOCOL_VERSION)
    }
}

/// A failure with the JSON-RPC code it should be reported under.
#[derive(Debug)]
pub struct RpcError {
    code: i64,
    message: String,
    /// The machine-readable half of the refusal, when the specification defines
    /// one for this code.
    ///
    /// Separate from `message` because they reach different readers and only one
    /// of them is actionable: a human reads the message, and the client runtime
    /// reads `data` to decide what to retry with. `UnsupportedProtocolVersionError`
    /// is the case that forced this field to exist — its `data.supported` *is*
    /// the recovery path, and before this the stdio transport had no way to
    /// carry it at all, so the same refusal was actionable over HTTP and a dead
    /// end over stdio.
    data: Option<Value>,
    /// Whether the model that chose these arguments could fix this itself.
    ///
    /// The specification splits tool failures in two and the split is not
    /// stylistic. A *protocol* error — unknown tool, a `tools/call` that does not
    /// satisfy the `CallToolRequest` schema — is returned as a JSON-RPC error and
    /// reaches the client runtime, which owns the tool list and can re-read it. A
    /// *tool execution* error is returned inside the result with `isError: true`
    /// and reaches the model, "to enable self-correction".
    ///
    /// Sending one as the other has a direction that matters. A model told
    /// "unknown tool 'devmap_serch'" inside a tool *result* sees a tool that ran
    /// and failed, so it retries the same non-existent name; the runtime, which
    /// is the only layer that could correct the name, never hears about it.
    tool_input: bool,
}

impl RpcError {
    /// The JSON-RPC code, which the HTTP transport maps to a status.
    pub fn code(&self) -> i64 {
        self.code
    }

    /// The human-readable reason, which reaches the agent.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// True when this is a value the model supplied and can supply differently.
    pub fn is_tool_input_fault(&self) -> bool {
        self.tool_input
    }

    /// The structured recovery information, for the codes that define one.
    pub fn data(&self) -> Option<&Value> {
        self.data.as_ref()
    }

    /// A fault in the request itself: reported as a JSON-RPC error.
    fn new(code: i64, message: impl Into<String>) -> Self {
        debug_assert!(
            !codes::is_reserved_and_undefined(code),
            "code {code} is in the range the MCP specification reserves for itself and is not \
one of the codes it defines"
        );
        Self {
            code,
            message: message.into(),
            data: None,
            tool_input: false,
        }
    }

    /// A refusal that also carries what the client should do about it.
    fn with_data(code: i64, message: impl Into<String>, data: Value) -> Self {
        Self {
            data: Some(data),
            ..Self::new(code, message)
        }
    }

    /// A fault in the argument values: reported as `isError: true`.
    fn tool_input(code: i64, message: impl Into<String>) -> Self {
        Self {
            tool_input: true,
            ..Self::new(code, message)
        }
    }
}

/// The declared tool surface, as `(mcp tool name, ipc cmd tag)`.
///
/// A single array so the two lists cannot fall out of step: `tool_specs` builds
/// its entries from it and `to_ipc_command` resolves through it, so a tool that
/// is declared is by construction a tool that can be called. Ordered, not a
/// hash map — `tools/list` output is a wire artifact and R4 forbids hash
/// iteration reaching one.
const TOOLS: &[(&str, &str)] = &[
    ("devmap_status", "status"),
    ("devmap_search", "search"),
    ("devmap_dependencies", "deps"),
    ("devmap_impact", "impact"),
    ("devmap_trace", "trace"),
    ("devmap_neighbors", "neighbors"),
    ("devmap_dead_symbols", "dead"),
    ("devmap_clones", "clones"),
    ("devmap_preview", "preview"),
];

/// The declared tool names, in published order.
///
/// Exposed so a caller — a test, or anything else counting coverage — measures
/// against the list this server actually publishes rather than against a number
/// someone typed. "Nine tools were checked" is only a coverage claim if nine is
/// read from the same array `tools/list` is built from.
pub const TOOL_NAMES: &[&str] = &{
    let mut names = [""; TOOLS.len()];
    let mut index = 0;
    while index < TOOLS.len() {
        names[index] = TOOLS[index].0;
        index += 1;
    }
    names
};

fn budget_prop(default: u32) -> Value {
    json!({
        "type": "integer",
        "minimum": 1,
        "maximum": 100_000,
        "default": default,
        "description": "Token budget for the answer. The response reports shown/hidden/total \
    and whether it was truncated, so a small budget yields a short answer that still says how much \
    it withheld."
    })
}

/// The budget property for the one tool that spends it more than once.
///
/// `neighbors` hands the number it is given to the traversal once per target and
/// once per direction, so with the declared maximum of
/// [`devmap_query::MAX_NEIGHBOR_TARGETS`] targets a caller receives up to 32
/// times what it asked for. That is the right *engine* behaviour — a per-target
/// budget is what makes each entry in a fan-out individually useful, and
/// trimming a shared budget across targets would silently starve the last ones —
/// but it made the shared description false for this tool alone.
///
/// A budget is the only control an agent has over how much of its context a call
/// will consume, so being wrong about it by 32x in the overrunning direction is
/// not a documentation nicety. This is the same defect the `depth` default on
/// this same tool already had: a published number that the code does not use.
fn fan_out_budget_prop(default: u32) -> Value {
    let fan_out = devmap_query::MAX_NEIGHBOR_TARGETS * 2;
    json!({
        "type": "integer",
        "minimum": 1,
        "maximum": 100_000,
        "default": default,
        "description": format!(
            "Token budget **per target, per direction** — not for the answer as a whole. This \
    tool walks callers and callees separately for each target, and spends this budget on each walk, \
    so a call over the maximum {} targets can return up to {fan_out}x the number given ({fan_out} x \
    {default} tokens at the default). Size it by what one target's callers are worth to you, then \
    multiply by 2x the number of targets to predict the total. Every other tool here spends its \
    budget once.",
            devmap_query::MAX_NEIGHBOR_TARGETS
        )
    })
}

fn depth_prop(default: u32) -> Value {
    json!({
        "type": "integer",
        "minimum": 1,
        "maximum": 64,
        "default": default,
        "description": "Maximum traversal depth. A walk stopped by this cap sets \
    walk_incomplete in the response — an empty result from a capped walk means 'we stopped looking', \
    not 'nothing is there'."
    })
}

fn confidence_prop() -> Value {
    confidence_prop_defaulting(0.0)
}

/// The confidence property, stating a default the caller supplies.
///
/// Exists because `preview` is the one command whose serde default is not zero.
/// The default is passed in rather than restated so the declaration is read from
/// the same constant the parser applies.
fn confidence_prop_defaulting(default: f32) -> Value {
    json!({
        "type": "number",
        "minimum": 0.0,
        "maximum": 1.0,
        "default": default,
        "description": "Minimum edge confidence, in [0,1]. Non-finite values are refused rather \
    than coerced."
    })
}

/// The `tools/list` payload. Deterministic: built from literals and [`TOOLS`],
/// with no map iteration and no dependence on process or filesystem state.
/// The description and input schema for one command tag.
///
/// The single owner of both. `tool_specs` publishes what this returns and
/// `schema_properties` checks against what this returns, so a tool cannot
/// advertise an argument it will not accept.
fn describe(cmd: &str) -> (&'static str, Value) {
    match cmd {
        "status" => (
            "Index health for this repository: generation id, node and edge counts, how \
many files are pending, and any degraded reason. Call this first when another tool returns an \
empty or surprising answer — a zero node_count means the index is not built, which is a \
different fact from 'the symbol does not exist'.",
            json!({"type": "object", "properties": {}, "additionalProperties": false}),
        ),
        "search" => (
            "Find symbols by name across the indexed repository. Returns ranked hits with \
file and line, budgeted to a token cap.",
            json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "maxLength": 4096,
                        "description": "Symbol name or prefix to search for."},
                    "budget": budget_prop(2000),
                    "semantic": {"type": "boolean", "default": false,
                        "description": "Rank by name similarity instead of FTS prefix matching."}
                },
                "required": ["query"],
                "additionalProperties": false
            }),
        ),
        "deps" => (
            "What the target depends on — its outbound edges. Accepts a file path or a \
symbol name.",
            json!({
                "type": "object",
                "properties": {
                    "target": {"type": "string", "maxLength": 4096,
                        "description": "File path or symbol name."},
                    "budget": budget_prop(2000),
                    "min_confidence": confidence_prop()
                },
                "required": ["target"],
                "additionalProperties": false
            }),
        ),
        "impact" => (
            "Blast radius: what reaches the target, walked in reverse to a depth cap. Use \
before deleting or changing a symbol. A result carrying walk_incomplete is a partial blast \
radius, not a complete one.",
            json!({
                "type": "object",
                "properties": {
                    "target": {"type": "string", "maxLength": 4096,
                        "description": "File path or symbol name."},
                    "budget": budget_prop(2000),
                    "depth": depth_prop(3)
                },
                "required": ["target"],
                "additionalProperties": false
            }),
        ),
        "trace" => (
            "Forward call paths from a symbol, or the paths between two symbols when 'to' \
is supplied.",
            json!({
                "type": "object",
                "properties": {
                    "from": {"type": "string", "maxLength": 4096,
                        "description": "Origin symbol or file."},
                    "to": {"type": "string", "maxLength": 4096,
                        "description": "Optional destination; when set, returns paths between the two."},
                    "budget": budget_prop(2000),
                    "depth": depth_prop(3)
                },
                "required": ["from"],
                "additionalProperties": false
            }),
        ),
        "neighbors" => (
            "Callers and callees for up to 16 targets in one exchange. Prefer this over \
separate dependencies+impact calls: it is one round trip and it detects when the index moved \
underneath the answer.",
            json!({
                "type": "object",
                "properties": {
                    "targets": {
                        "type": "array",
                        "items": {"type": "string", "maxLength": 4096},
                        "minItems": 1,
                        "maxItems": devmap_query::MAX_NEIGHBOR_TARGETS,
                        "description": "Symbols or file paths. More than the maximum is refused, never silently trimmed."
                    },
                    // Not `budget_prop`: this is the one tool that spends the
                    // budget more than once, and the shared description says it
                    // is spent once.
                    "budget": fan_out_budget_prop(2000),
                    // 3, not 1. `IpcCommand::Neighbors::depth` carries
                    // `#[serde(default = "default_depth")]`, and that function
                    // returns 3. Declaring 1 told an agent it had asked for
                    // immediate neighbours while handing it a three-hop
                    // transitive closure — and an agent that read the schema,
                    // saw the default it wanted, and therefore omitted the
                    // field could not get the answer the schema described.
                    "depth": depth_prop(3),
                    "min_confidence": confidence_prop()
                },
                "required": ["targets"],
                "additionalProperties": false
            }),
        ),
        "dead" => (
            "Symbols with no inbound edges and no entry-point exemption. Every report \
carries its confidence and reason; treat it as a candidate list to verify, not a delete list.",
            json!({
                "type": "object",
                "properties": {"budget": budget_prop(2000)},
                "additionalProperties": false
            }),
        ),
        "clones" => (
            "Duplicate and structurally similar code groups.",
            json!({
                "type": "object",
                "properties": {
                    "budget": budget_prop(2000),
                    "kind": {"type": "string", "enum": ["exact", "structural"],
                        "description": "Restrict to one clone kind. Omit for both."},
                    "min_nodes": {"type": "integer", "minimum": 1,
                        "description": "Ignore groups whose members are smaller than this AST node count."}
                },
                "additionalProperties": false
            }),
        ),
        "preview" => (
            "Speculative edit preview: given replacement content for a file, report which \
callers the edit would break before it is written. Reads the index and the supplied content; \
writes nothing.",
            json!({
                "type": "object",
                "properties": {
                    "file": {"type": "string", "maxLength": 4096,
                        "description": "Repository-relative path of the file being edited."},
                    "content": {"type": "string",
                        "description": "Proposed full replacement content for that file."},
                    "budget": budget_prop(2000),
                    // Read from the constant the parser uses, not restated.
                    // `IpcCommand::Preview::min_confidence` defaults to
                    // `PREVIEW_CALLER_MIN_CONFIDENCE`, and this is the "which
                    // callers would my edit break" tool: declaring the filter
                    // off while it silently drops every caller below the
                    // threshold turns a shortened breakage list into "this edit
                    // breaks nothing".
                    "min_confidence": confidence_prop_defaulting(
                        devmap_query::PREVIEW_CALLER_MIN_CONFIDENCE
                    )
                },
                "required": ["file", "content"],
                "additionalProperties": false
            }),
        ),
        other => unreachable!("command tag {other} has no schema"),
    }
}

/// The budget envelope every ranked answer is wrapped in.
///
/// `devmap_query::Response<T>` in schema form. The fields listed as required are
/// exactly the ones that struct declares without `Option` and without
/// `skip_serializing_if`, so they are present by construction rather than by
/// habit; `walk_incomplete` is the one field that carries both, and it is
/// therefore the one field that is optional here.
///
/// `items` is left as a bare array on purpose. What varies between these tools
/// is the element type, and restating each element's fields here would be a
/// second copy of a Rust struct that nothing keeps in step — the exact drift
/// [`describe`] exists to prevent on the input side. What this schema is for is
/// the envelope: `truncated`, `walk_incomplete` and the `shown`/`hidden`/`total`
/// triple are the fields a reader has to consult before concluding that an empty
/// `items` means "nothing is there", and they are the fields a client could not
/// see declared anywhere before.
fn budgeted_envelope(items: &str) -> Value {
    json!({
        "type": "object",
        "properties": {
            "items": {"type": "array", "description": items},
            "shown": {"type": "integer", "description": "Entries present in `items`."},
            "hidden": {"type": "integer",
                "description": "Entries the token budget withheld. Non-zero means this answer is \
    a prefix, not a set."},
            "total": {"type": "integer",
                "description": "What a complete answer would have held: shown + hidden."},
            "truncated": {"type": "boolean",
                "description": "True when the budget cut the answer. An empty `items` with \
    `truncated` false is 'nothing matched'; with it true it is 'nothing fitted'."},
            "tokens_used": {"type": "integer"},
            "resolution": {"type": ["string", "object"],
                "description": "\"Available\", or {\"Unavailable\": {\"reason\": …}} when symbol \
    resolution could not run for this generation — in which case an empty answer says nothing about \
    the code."},
            "walk_incomplete": {"type": ["string", "null"],
                "description": "Present only when the *producer* stopped early — a depth cap, a \
    cancellation — as distinct from the budgeter trimming a complete set. Its presence means the \
    answer is partial by an unknown amount, which `hidden` cannot express."}
        },
        "required": ["items", "shown", "hidden", "total", "truncated", "tokens_used", "resolution"],
        "additionalProperties": true
    })
}

/// The shape a tool's `structuredContent` is promised to have.
///
/// The single owner of that promise: [`tool_specs`] publishes what this returns
/// and [`structured_content_violation`] checks against what this returns, so the
/// declaration and the enforcement are one object. Declaring an `outputSchema`
/// is not free — "Servers **MUST** provide structured results that conform to
/// this schema" — and a promise nothing checks is the failure mode this module
/// already refuses on the input side.
///
/// Every schema here sets `additionalProperties: true` and requires only fields
/// the Rust type emits unconditionally. That is deliberate: an over-tight schema
/// would make a client reject an answer that is correct, and the point of
/// declaring one is to make the incompleteness markers visible, not to freeze
/// the payload.
fn describe_output(cmd: &str) -> Value {
    match cmd {
        "status" => json!({
            "type": "object",
            "properties": {
                "generation_id": {"type": ["integer", "null"],
                    "description": "Null when no generation has ever been built."},
                "pending_count": {"type": "integer"},
                "node_count": {"type": "integer",
                    "description": "Zero means the index is not built. That is a different fact \
        from 'the symbol does not exist', and every other tool's empty answer should be read through it."},
                "edge_count": {"type": "integer"},
                "is_fresh": {"type": "boolean"},
                "degraded_reason": {"type": ["string", "null"],
                    "description": "Why the index is not trustworthy, or null when it is."},
                "quarantined_count": {"type": "integer",
                    "description": "Files the indexer could not take. Their symbols are absent \
        from every answer without being reported as missing."}
            },
            "required": ["generation_id", "pending_count", "node_count", "edge_count",
                "is_fresh", "degraded_reason", "quarantined_count"],
            "additionalProperties": true
        }),
        "search" => budgeted_envelope("Ranked symbol hits: name, file, kind, span and source."),
        "deps" => budgeted_envelope("Outbound edges from the target."),
        "impact" => budgeted_envelope("Symbols that reach the target, walked in reverse."),
        "trace" => budgeted_envelope("Call paths from the origin, or between the two endpoints."),
        "dead" => budgeted_envelope(
            "Dead-symbol reports, each carrying its own confidence and reason. A candidate list \
to verify, not a delete list.",
        ),
        "neighbors" => json!({
            "type": "object",
            "properties": {
                "neighbors": {"type": "array",
                    "description": "One entry per requested target, in the order asked."}
            },
            "required": ["neighbors"],
            "additionalProperties": true
        }),
        "clones" => json!({
            "type": "object",
            "properties": {
                "groups": {"type": "object",
                    "description": "A budgeted envelope of clone groups; read its `truncated` \
        before concluding the list is complete."},
                "signed_symbols": {"type": "integer",
                    "description": "Symbols that carried a body signature and could therefore be \
        compared."},
                "unsigned_symbols": {"type": "integer",
                    "description": "Symbols that could not be compared at all. `groups` empty \
        with this large means 'we could not look', not 'there is no duplication'."}
            },
            "required": ["groups", "signed_symbols", "unsigned_symbols"],
            "additionalProperties": true
        }),
        "preview" => json!({
            "type": "object",
            "properties": {
                "file_path": {"type": "string"},
                "parse_status": {"type": "string",
                    "description": "Clean, Partial, Fallback or Failed, from the proposed \
        buffer's parse."},
                "delta_available": {"type": "boolean",
                    "description": "False when the buffer did not parse well enough to diff. The \
        symbol list below is then not a delta."},
                "file_is_indexed": {"type": "boolean",
                    "description": "False means the caller graph has nothing to say about this \
        file, so an empty `broken_callers` is uninformative."},
                "compared_against": {"type": "string",
                    "description": "`disk`, `nothing` (no such file, so every symbol is an \
        addition) or `unreadable` (the comparison did not happen). The last two license opposite \
        conclusions and are kept distinct for that reason."},
                "degraded_reason": {"type": ["string", "null"]},
                "symbols": {"type": "array"},
                "bodies_not_compared": {"type": "integer",
                    "description": "Symbols whose bodies nothing compared. Not found unchanged — \
        not examined."},
                "ambiguous_callers": {"type": "integer",
                    "description": "Call edges below the confidence floor, and so absent from \
        `broken_callers`. Counted so 'no callers affected' cannot quietly mean 'none we would vouch for'."},
                "broken_callers": {"type": "object",
                    "description": "A budgeted envelope of calls this edit would break."}
            },
            "required": ["file_path", "parse_status", "delta_available", "file_is_indexed",
                "compared_against", "symbols", "broken_callers"],
            "additionalProperties": true
        }),
        other => unreachable!("command tag {other} has no output schema"),
    }
}

/// Why `value` does not satisfy the `outputSchema` `tool` published, if it does
/// not.
///
/// Deliberately narrow: it implements `type`, `required` and per-property
/// `type`, which is the entire vocabulary [`describe_output`] uses — and it
/// **refuses a schema that uses anything else** rather than passing it. A
/// validator that silently skips the keyword it does not know reports a
/// check that never ran as a check that passed, which is the one failure this
/// repository will not accept from a checker.
pub fn structured_content_violation(tool: &str, value: &Value) -> Option<String> {
    let Some((_, cmd)) = TOOLS.iter().find(|(name, _)| *name == tool) else {
        return Some(format!(
            "'{tool}' is not a tool this server declares, so there is no outputSchema to check \
this against; a result cannot be reported as conforming to a schema that does not exist"
        ));
    };
    let schema = describe_output(cmd);
    let object = schema.as_object()?;

    const UNDERSTOOD: &[&str] = &[
        "type",
        "properties",
        "required",
        "additionalProperties",
        "description",
    ];
    if let Some(unknown) = object
        .keys()
        .find(|key| !UNDERSTOOD.contains(&key.as_str()))
    {
        return Some(format!(
            "{tool}'s outputSchema uses '{unknown}', which this checker does not implement — so \
the result was not checked, and an unchecked result must not be reported as a conforming one"
        ));
    }

    if let Some(fault) = type_violation("the result", object.get("type"), value) {
        return Some(format!("{tool}: {fault}"));
    }
    let Some(members) = value.as_object() else {
        return Some(format!(
            "{tool}: the schema declares an object and the result is {}",
            kind_of(value)
        ));
    };
    for required in object
        .get("required")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let Some(required) = required.as_str() else {
            return Some(format!("{tool}: a non-string entry in `required`"));
        };
        if !members.contains_key(required) {
            return Some(format!("{tool}: required field '{required}' is absent"));
        }
    }
    for (property, rules) in object
        .get("properties")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        let Some(present) = members.get(property) else {
            continue;
        };
        if let Some(fault) = type_violation(property, rules.get("type"), present) {
            return Some(format!("{tool}: {fault}"));
        }
    }
    None
}

/// Whether `value` matches a `type` keyword that is a name or a list of names.
///
/// A `type` this function does not recognise is a violation, not a pass, for the
/// same reason the keyword check above is: an unrecognised constraint is an
/// unchecked one.
fn type_violation(what: &str, declared: Option<&Value>, value: &Value) -> Option<String> {
    let declared = declared?;
    let names: Vec<&str> = match declared {
        Value::String(name) => vec![name.as_str()],
        Value::Array(items) => items.iter().filter_map(Value::as_str).collect(),
        other => return Some(format!("{what}: `type` is {}, not a name", kind_of(other))),
    };
    if names.is_empty() {
        return Some(format!("{what}: `type` names nothing"));
    }
    for name in &names {
        let matched = match *name {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "boolean" => value.is_boolean(),
            "null" => value.is_null(),
            "integer" => value.is_i64() || value.is_u64(),
            "number" => value.is_number(),
            unknown => {
                return Some(format!(
                    "{what}: `type: {unknown}` is not a type this checker implements, so nothing \
was checked"
                ))
            }
        };
        if matched {
            return None;
        }
    }
    Some(format!(
        "{what} is {}, and the schema allows only {}",
        kind_of(value),
        names.join(" or ")
    ))
}

pub fn tool_specs() -> Vec<Value> {
    TOOLS
        .iter()
        .map(|(name, cmd)| {
            let (description, schema) = describe(cmd);
            json!({
                "name": name,
                "description": description,
                "inputSchema": schema,
                // Declared because this server emits `structuredContent` on
                // every successful call, and structured content a client cannot
                // validate is structured content it has to guess at. The
                // obligation this creates — "Servers MUST provide structured
                // results that conform to this schema" — is enforced in
                // `tool_success` rather than trusted.
                "outputSchema": describe_output(cmd),
                "annotations": {
                    // Every tool here reads the index and returns an answer.
                    // `preview` takes file content as an argument but writes
                    // nothing — it is a question about a hypothetical edit, not
                    // the edit. Annotating it read-only is a statement about
                    // what it does, and `dispatch` is where that stays true.
                    "readOnlyHint": true,
                    "destructiveHint": false,
                    // Not idempotent in the strict sense: the index moves as the
                    // repository changes, so the same question can get a
                    // different answer tomorrow. Claiming idempotence would tell
                    // a client it may cache indefinitely.
                    "idempotentHint": false,
                    "openWorldHint": false
                }
            })
        })
        .collect()
}

/// Translate an MCP tool call into the socket protocol's own command type.
///
/// Returns the serde error verbatim on failure. That message names the field and
/// the expected type, which is what an agent needs to correct its next call; a
/// generic "invalid arguments" would send it guessing.
///
/// Each failure is classified as a request fault or a tool-input fault (see
/// [`RpcError::is_tool_input_fault`]) at the point that knows which it is. The
/// two are carried to the client by different mechanisms, and a second switch
/// over messages further down would be a second place for that judgement to
/// live, and to drift.
pub fn to_ipc_command(name: &str, arguments: Option<&Value>) -> Result<IpcCommand, RpcError> {
    // A request fault, not a tool-input fault: "any errors in *finding* the
    // tool ... should be reported as an MCP error response". The model cannot
    // fix a name that is not in the list it was given — the runtime that holds
    // the list can.
    let cmd = TOOLS
        .iter()
        .find(|(tool, _)| *tool == name)
        .map(|(_, cmd)| *cmd)
        .ok_or_else(|| RpcError::new(codes::INVALID_PARAMS, format!("unknown tool '{name}'")))?;

    // `arguments` absent and `arguments: {}` are the same request: the tools
    // that take no arguments are called both ways by real clients.
    let mut object = match arguments {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(map)) => map.clone(),
        // Also a request fault. `CallToolRequestParams.arguments` is typed
        // `{[key: string]: unknown}`, so a string or an array here is a call
        // the runtime built wrongly, not a value the model chose wrongly.
        Some(other) => {
            return Err(RpcError::new(
                codes::INVALID_PARAMS,
                format!("tool arguments must be an object, got {}", kind_of(other)),
            ))
        }
    };

    // Unknown arguments are refused here, against the tool's own declared
    // schema, because serde will not refuse them.
    //
    // `IpcCommand` ignores fields it does not recognise — the right behaviour
    // for the socket protocol, where an older kernel must tolerate a newer
    // client's extra field. But every schema in `tool_specs` says
    // `additionalProperties: false`, and a declared constraint that nothing
    // enforces is worse than no constraint: a client that misspells `budget`
    // passes schema validation, has the typo silently dropped, and receives the
    // 2000-token default while believing it asked for more. The answer is
    // short, correct-looking, and not what was requested.
    //
    // Checked against the schema rather than a second hand-written field list,
    // so the promise and the enforcement cannot disagree.
    let (_, schema) = describe(cmd);
    let known = schema_properties(name);
    let mut unknown: Vec<&str> = object
        .keys()
        .map(String::as_str)
        .filter(|key| !known.iter().any(|known| known == key))
        .collect();
    if !unknown.is_empty() {
        // Sorted: the message is compared in tests and read by an agent, and
        // `Map`'s iteration order should not decide either.
        unknown.sort_unstable();
        // A tool-input fault from here on: the tool exists and the call is
        // well-formed, so what is wrong is a value the model chose and can
        // choose again.
        return Err(RpcError::tool_input(
            codes::INVALID_PARAMS,
            format!(
                "unknown argument{} for {name}: {}. Accepted: {}",
                if unknown.len() == 1 { "" } else { "s" },
                unknown.join(", "),
                if known.is_empty() {
                    "(none — this tool takes no arguments)".to_string()
                } else {
                    known.join(", ")
                }
            ),
        ));
    }

    // `cmd` is not a declared property of any tool, so the check above already
    // refuses it. Asserted rather than assumed: if a future tool ever declared a
    // `cmd` property, a client could steer this call at a different command than
    // the tool it named — and therefore run it under another tool's annotations,
    // which is what a host uses to decide whether to ask the user first.
    debug_assert!(
        !known.iter().any(|name| name == "cmd"),
        "a tool declaring a 'cmd' property would let a client rename the command it runs"
    );

    // Every *other* constraint the schema states is enforced here too, for the
    // same reason the unknown-key check exists: a declared constraint nothing
    // enforces is worse than no constraint, because a client-side validator
    // refuses what this server accepts and the two disagree about what a valid
    // call is. `validate_request` bounds only the upper end (budget, depth,
    // confidence range) and knows nothing of `minItems` or `minimum`, so
    // `targets: []` and `depth: 0` both passed while the published schema said
    // they could not.
    //
    // `targets: []` is the one that actually lies: it produced
    // `{"neighbors": []}` with `isError: false` and no incompleteness marker —
    // indistinguishable from "these symbols have no callers or callees".
    if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
        for (key, value) in object.iter() {
            let Some(rules) = properties.get(key) else {
                continue;
            };
            if let Err(reason) = check_constraints(key, value, rules) {
                return Err(RpcError::tool_input(codes::INVALID_PARAMS, reason));
            }
        }
    }

    object.insert("cmd".to_string(), Value::String(cmd.to_string()));

    serde_json::from_value(Value::Object(object))
        .map_err(|err| RpcError::tool_input(codes::INVALID_PARAMS, err.to_string()))
}

/// The argument names a tool declares, read from the tool's own schema.
///
/// Derived from [`describe`] — the same function [`tool_specs`] publishes — so
/// the promise on the wire and the check performed here are one object. A second
/// hand-maintained list here would be a list that can disagree with the schema,
/// and the disagreement would be silent in the direction that matters: a
/// property declared but not accepted, or accepted but not declared.
fn schema_properties(tool: &str) -> Vec<String> {
    let Some((_, cmd)) = TOOLS.iter().find(|(name, _)| *name == tool) else {
        return Vec::new();
    };
    let (_, schema) = describe(cmd);
    schema
        .get("properties")
        .and_then(Value::as_object)
        .map(|props| props.keys().cloned().collect())
        .unwrap_or_default()
}

/// Enforce one property's declared constraints against the value supplied.
///
/// Reads the rules out of the published schema rather than restating them, so a
/// bound can only be enforced if it was advertised and can only be advertised if
/// it is enforced. Constraints absent from a property are simply not checked —
/// this validates what the schema claims, and claims nothing itself.
fn check_constraints(key: &str, value: &Value, rules: &Value) -> Result<(), String> {
    if let Some(minimum) = rules.get("minimum").and_then(Value::as_f64) {
        // `as_f64` covers integers too, so one branch serves both declared
        // types. A non-numeric value here is left to serde, whose type error
        // names the expected type better than anything this could say.
        if let Some(actual) = value.as_f64() {
            if actual < minimum {
                return Err(format!("{key} must be at least {minimum}, got {actual}"));
            }
        }
    }
    if let Some(maximum) = rules.get("maximum").and_then(Value::as_f64) {
        if let Some(actual) = value.as_f64() {
            if actual > maximum {
                return Err(format!("{key} must be at most {maximum}, got {actual}"));
            }
        }
    }
    if let Some(items) = value.as_array() {
        if let Some(minimum) = rules.get("minItems").and_then(Value::as_u64) {
            if (items.len() as u64) < minimum {
                return Err(format!(
                    "{key} needs at least {minimum} entr{}, got {}. An empty list is refused \
rather than answered, because an answer about nothing reads exactly like an answer that found \
nothing.",
                    if minimum == 1 { "y" } else { "ies" },
                    items.len()
                ));
            }
        }
        if let Some(maximum) = rules.get("maxItems").and_then(Value::as_u64) {
            if (items.len() as u64) > maximum {
                return Err(format!(
                    "{key} accepts at most {maximum} entries, got {}",
                    items.len()
                ));
            }
        }
    }
    if let Some(text) = value.as_str() {
        if let Some(maximum) = rules.get("maxLength").and_then(Value::as_u64) {
            if (text.len() as u64) > maximum {
                return Err(format!(
                    "{key} is {} bytes, over the {maximum}-byte limit",
                    text.len()
                ));
            }
        }
    }
    if let Some(allowed) = rules.get("enum").and_then(Value::as_array) {
        if !allowed.contains(value) {
            let names: Vec<String> = allowed.iter().map(|v| v.to_string()).collect();
            return Err(format!(
                "{key} must be one of {}, got {value}",
                names.join(", ")
            ));
        }
    }
    Ok(())
}

fn kind_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Negotiate a protocol revision against the client's request.
///
/// An unrecognised or absent request gets our newest rather than an error: the
/// specification's guidance is to counter-offer, and a client that cannot accept
/// the counter-offer will say so by disconnecting.
fn negotiate(requested: Option<&str>) -> &'static str {
    match requested {
        Some(asked) => HANDSHAKE_PROTOCOL_VERSIONS
            .iter()
            .find(|known| **known == asked)
            .copied()
            .unwrap_or(LATEST_HANDSHAKE_VERSION),
        None => LATEST_HANDSHAKE_VERSION,
    }
}

fn initialize_result(params: Option<&Value>) -> Value {
    let requested = params
        .and_then(|p| p.get("protocolVersion"))
        .and_then(Value::as_str);
    json!({
        "protocolVersion": negotiate(requested),
        "capabilities": {
            // `listChanged: false` and meant literally: the tool list is built
            // from literals and cannot change while this process runs, so this
            // server never emits notifications/tools/list_changed. Advertising
            // true would promise a notification that never comes.
            "tools": {"listChanged": false}
        },
        "serverInfo": server_info(),
        "instructions": INSTRUCTIONS
    })
}

/// Who answered. One copy, reached two ways.
///
/// `initialize` publishes it as `serverInfo`, and [`complete`] publishes it in
/// every result's `_meta`. Both are needed and neither is redundant: the modern
/// era has no `initialize` at all, and `DiscoverResult` carries no `serverInfo`
/// field, so `_meta` is the only place a client that never handshook can learn
/// which build produced its answers.
fn server_info() -> Value {
    json!({
        "name": "devmap",
        "title": "Dev Map code intelligence",
        "version": env!("CARGO_PKG_VERSION")
    })
}

/// Protocol revisions this server serves under the modern, per-request era.
///
/// Kept separate from [`HANDSHAKE_PROTOCOL_VERSIONS`] because they are different
/// eras with different framing, not a longer version of the same list: a modern
/// request is self-contained and carries its protocol version in `_meta`, with
/// no `initialize` and no session id. The split is by era, not by transport —
/// the era is reached over HTTP here, and announced over stdio through
/// [`discover_result`], which is the probe the stdio binding tells a modern
/// client to send first.
pub const MODERN_PROTOCOL_VERSIONS: &[&str] = &["2026-07-28"];

/// `_meta` key carrying a modern request's protocol version.
///
/// Its *presence* is what selects the era, which is the rule the specification
/// gives a dual-era server: "A request carrying modern per-request `_meta` is
/// served statelessly according to this revision. An `initialize` request
/// selects legacy semantics."
pub const META_PROTOCOL_VERSION: &str = "io.modelcontextprotocol/protocolVersion";

/// `_meta` key carrying the capabilities a modern request may be answered with.
pub const META_CLIENT_CAPABILITIES: &str = "io.modelcontextprotocol/clientCapabilities";

/// Which of the two eras a request asked to be served under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Era {
    /// No version declared per-request. `initialize` negotiates one for the
    /// connection, and everything after it is served under that negotiation.
    Handshake,
    /// The request carried its own version, identity and capabilities. Nothing
    /// is remembered between requests, so nothing may be assumed from an
    /// earlier one.
    Modern,
}

/// Read the era off a request, refusing a modern envelope this server cannot honour.
///
/// This is the whole of the version negotiation for the modern era, and it lives
/// here — above the transports — because it is a property of the *request*, not
/// of the pipe it arrived on. Putting it in the HTTP module (where it was) meant
/// stdio never performed it: this server answers `server/discover` on stdio
/// precisely because the stdio binding tells a dual-era client to probe with it
/// first, so a modern client would be told "yes, modern", declare a version, and
/// have it ignored on every request after.
///
/// Three refusals, and they are distinct on purpose, because each sends the
/// client somewhere different:
///
/// * a version field of the wrong *shape* is `-32602` — fix the request;
/// * a version we do not serve is `-32022` with `data.supported` — retry with
///   one of these;
/// * a missing `clientCapabilities` is `-32602` — "A request missing any
///   required field is malformed; the server **MUST** reject it with JSON-RPC
///   error code `-32602`".
///
/// `supported` lists only the modern revisions, and that is not an omission.
/// It is the set of versions a client may legally *state in this field*; the
/// handshake revisions are reached by handshaking, which is a different
/// mechanism, and offering them here would invite a retry that cannot work.
/// The message says where they live instead.
fn classify_era(params: Option<&Value>) -> Result<Era, RpcError> {
    let meta = match params.and_then(|params| params.get("_meta")) {
        Some(meta) => meta,
        None => return Ok(Era::Handshake),
    };
    let stated = match meta.get(META_PROTOCOL_VERSION) {
        None => return Ok(Era::Handshake),
        Some(Value::String(stated)) => stated.as_str(),
        Some(other) => {
            return Err(RpcError::new(
                codes::INVALID_PARAMS,
                format!(
                    "_meta.{META_PROTOCOL_VERSION} must be a version string such as \"{}\", got {}",
                    MODERN_PROTOCOL_VERSIONS[0],
                    kind_of(other)
                ),
            ))
        }
    };

    if !MODERN_PROTOCOL_VERSIONS.contains(&stated) {
        let reachable = if HANDSHAKE_PROTOCOL_VERSIONS.contains(&stated) {
            format!(
                " {stated} is a handshake revision: it is reached by sending `initialize` with \
no per-request protocol version, not by declaring it in _meta."
            )
        } else {
            String::new()
        };
        return Err(RpcError::with_data(
            codes::UNSUPPORTED_PROTOCOL_VERSION,
            format!(
                "this server serves {} through per-request _meta; the request declared {stated}.\
{reachable}",
                MODERN_PROTOCOL_VERSIONS.join(", ")
            ),
            json!({"supported": MODERN_PROTOCOL_VERSIONS, "requested": stated}),
        ));
    }

    if meta.get(META_CLIENT_CAPABILITIES).is_none() {
        return Err(RpcError::new(
            codes::INVALID_PARAMS,
            format!(
                "_meta.{META_CLIENT_CAPABILITIES} is required on every request of this revision. \
An empty object declares no optional capabilities; omitting it is a different statement, because \
a stateless server has no earlier request to infer them from."
            ),
        ));
    }

    Ok(Era::Modern)
}

/// How long a client may cache `tools/list` and `server/discover`.
///
/// Five minutes, and the reasoning is the bound rather than the number: this
/// server never emits `notifications/tools/list_changed` and correctly
/// advertises `listChanged: false`, so an expiring TTL is the *only*
/// invalidation a cached list gets. The list is built from literals and cannot
/// change while the process runs, so the risk of a stale cache is zero today and
/// the TTL exists to bound the day that stops being true.
pub const CACHE_TTL_MS: u64 = 5 * 60 * 1000;

/// The `server/discover` payload.
///
/// The modern era's replacement for `initialize`: a client probes what this
/// server speaks without opening a session. `supportedVersions` lists only the
/// modern revisions, because those are the ones a caller selecting from this
/// list can then state in `_meta` — listing the handshake revisions would
/// advertise a negotiation that does not happen through this method.
///
/// Answered on stdio as well as over HTTP, and deliberately: the stdio binding
/// tells a client that supports both eras to probe with `server/discover`
/// *before any other request*, so refusing it there would make this server look
/// legacy to every modern client that followed the specification.
fn discover_result() -> Value {
    json!({
        "supportedVersions": MODERN_PROTOCOL_VERSIONS,
        "capabilities": {"tools": {"listChanged": false}},
        "instructions": INSTRUCTIONS,
        // Required fields: `DiscoverResult` extends `CacheableResult`. Whether a
        // given client reads them is a property of that client's revision, not a
        // reason to omit them.
        "ttlMs": CACHE_TTL_MS,
        "cacheScope": "private"
    })
}

/// Guidance handed to the model on connect. One copy, used by both eras.
const INSTRUCTIONS: &str = "Ask the code graph before reading files. Every answer is budgeted and \
reports what it withheld: check `truncated` and `walk_incomplete` before concluding that an \
empty list means nothing exists. When a result looks wrong or empty, call devmap_status — an \
unbuilt index answers 'nothing' to every question.";

/// Run one JSON-RPC method against the store.
///
/// `Ok(None)` means the method was a notification that this server accepts and
/// deliberately does not answer.
async fn call_tool(
    store: &Arc<StoreSlot>,
    params: Option<Value>,
    cancel: devmap_query::Cancel,
) -> Result<Value, RpcError> {
    let params =
        params.ok_or_else(|| RpcError::new(codes::INVALID_PARAMS, "tools/call requires params"))?;
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::new(codes::INVALID_PARAMS, "tools/call requires a string 'name'"))?
        .to_string();

    // Argument *value* faults are reported as tool errors, not JSON-RPC errors:
    // the agent that sent them is the one that must correct them, and a tool
    // error reaches the model while a protocol error reaches only the client
    // runtime. Faults in the *request* — an unknown tool, arguments that are not
    // an object — go the other way, for the same reason read in reverse: the
    // model cannot fix them and the runtime can. The classification is made
    // where the fault is raised; this is only the routing.
    let command = match to_ipc_command(&name, params.get("arguments")) {
        Ok(command) => command,
        Err(err) if err.is_tool_input_fault() => return Ok(tool_error(err.message)),
        Err(err) => return Err(err),
    };

    let request = IpcRequest {
        version: PROTOCOL_VERSION,
        command,
    };
    if let Err(reason) = validate_request(&request) {
        return Ok(tool_error(reason));
    }

    // The engine is synchronous and holds a mutex; running it on the runtime
    // thread would stall every other connection task for the duration. The
    // cancel flag is what actually stops the work when the timeout fires —
    // dropping a blocking task's handle only detaches it.
    // Resolved here, after validation and per call: the index may be built
    // by the daemon while this server is already running, so a missing store is
    // a retryable condition rather than a permanent one.
    let store = match store.get() {
        Ok(store) => store,
        Err(reason) => return Ok(tool_error(reason)),
    };

    // The flag is the caller's: a stdio session holds a second handle so a
    // `notifications/cancelled` can trip it while this call is still running.
    // Dropping a blocking task's handle only detaches it; the flag is the only
    // thing that actually stops the traversal.
    let worker_cancel = cancel.clone();
    let handle = tokio::task::spawn_blocking(move || dispatch(&store, request, &worker_cancel));

    match tokio::time::timeout(CALL_TIMEOUT, handle).await {
        Ok(Ok(Ok(value))) => Ok(tool_success(&name, value)),
        Ok(Ok(Err(err))) => Ok(tool_error(err.to_string())),
        Ok(Err(join)) => Err(RpcError::new(
            codes::INTERNAL_ERROR,
            format!("tool task failed: {join}"),
        )),
        Err(_) => {
            cancel.cancel();
            Ok(tool_error(format!(
                "tool '{name}' exceeded its {}s budget and was cancelled; no partial answer is \
being reported because a partial traversal cannot be distinguished from a complete one",
                CALL_TIMEOUT.as_secs()
            )))
        }
    }
}

/// A successful tool result.
///
/// The payload rides in `structuredContent` *and* as JSON text in `content`.
/// Both are required in practice: `structuredContent` is what a client parses,
/// and the text block is what a client that predates it — or one that only
/// forwards `content` to the model — actually shows. Sending only one of them
/// makes the answer invisible to half the clients in the field.
fn tool_success(name: &str, value: Value) -> Value {
    // Checked here, against the tool's own published `outputSchema`, because
    // declaring one makes conformance a MUST and a client is entitled to
    // validate. If this ever fails, the honest answer is the failure: a
    // validating client would reject the result anyway and the agent would be
    // left with a rejection and no reason, whereas this names the field.
    if let Some(violation) = structured_content_violation(name, &value) {
        return tool_error(format!(
            "{name} produced a result that does not satisfy the outputSchema it publishes: \
{violation}. The answer is withheld rather than sent, because a client validating against the \
declared schema would reject it and could not say why. This is a server defect, not a bad \
argument — the call itself was well-formed."
        ));
    }
    let text = serde_json::to_string(&value).unwrap_or_else(|err| {
        format!("{{\"error\":\"{name} result was not serializable: {err}\"}}")
    });
    let result = json!({
        "content": [{"type": "text", "text": text}],
        "structuredContent": value,
        "isError": false
    });
    if let Some(refusal) = oversized_result_refusal(name, &result) {
        return tool_error(refusal);
    }
    result
}

/// Why this result is too large to send, if it is.
///
/// Measured, not estimated: the frame is already built, so the number in the
/// refusal is the size of the thing that would have gone out rather than a
/// prediction. That matters because the message asks the caller to pick a
/// smaller budget, and a caller cannot scale down from a figure that was guessed.
///
/// Public for the same reason [`structured_content_violation`] is: the check the
/// server performs before emitting and the check a test asserts have to be one
/// function, or the guard and the behaviour drift.
pub fn oversized_result_refusal(tool: &str, result: &Value) -> Option<String> {
    let measured = match serde_json::to_vec(result) {
        Ok(bytes) => bytes.len(),
        // Unserializable is not "small enough". The result cannot be sent
        // either way, and reporting nothing here would let it reach the
        // transport to fail there with no attribution.
        Err(err) => {
            return Some(format!(
                "{tool} produced a result that could not be serialized at all ({err}), so its \
size could not be checked and it cannot be sent"
            ))
        }
    };
    if measured <= MAX_RESULT_BYTES {
        return None;
    }
    let fan_out = if tool == "devmap_neighbors" {
        format!(
            " devmap_neighbors spends its budget once per target and once per direction, so the \
number you passed was multiplied by up to {}; lower `budget`, or ask about fewer targets.",
            devmap_query::MAX_NEIGHBOR_TARGETS * 2
        )
    } else {
        " Lower `budget` and ask again.".to_string()
    };
    Some(format!(
        "{tool} produced {measured} bytes, over the {MAX_RESULT_BYTES}-byte limit on a single \
result. Nothing is being sent: this server refuses to write a frame larger than it would agree \
to read, and a truncated answer is worse than none because a cut list reads exactly like a \
complete one.{fan_out}"
    ))
}

/// A failed tool call.
///
/// `isError: true` with the reason in the text block, which is the shape that
/// reaches the model. The alternative — an empty success — is the defect this
/// repository keeps finding: it reads as "there is nothing", and the reader acts
/// on it.
fn tool_error(message: impl Into<String>) -> Value {
    json!({
        "content": [{"type": "text", "text": message.into()}],
        "isError": true
    })
}

fn rpc_error_frame(id: Option<Value>, code: i64, message: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id.unwrap_or(Value::Null),
        "error": {"code": code, "message": message.into()}
    })
}

/// The same frame, carrying whatever recovery data the error was raised with.
///
/// Separate from [`rpc_error_frame`] only because most refusals have no `data`
/// to carry and an `Option` at every call site would obscure the ones that do.
/// The `data` member is omitted rather than written as `null` when there is
/// none: `null` is a value, and a client reading `error.data` would have to
/// distinguish "no recovery information" from "recovery information that is
/// literally null".
fn rpc_error_frame_from(id: Option<Value>, error: &RpcError) -> Value {
    let mut frame = rpc_error_frame(id, error.code(), error.message());
    if let (Some(data), Some(object)) = (
        error.data(),
        frame.get_mut("error").and_then(Value::as_object_mut),
    ) {
        object.insert("data".to_string(), data.clone());
    }
    frame
}
/// In-flight requests, so a cancellation notification can reach one.
///
/// Keyed by the request id rendered as JSON text: ids are legally numbers *or*
/// strings, and `1` and `"1"` are different ids that must not collide. Rendering
/// through `Value::to_string` keeps them distinct (`1` vs `"1"`) without needing
/// `Value` to be `Hash`.
type InFlight = Arc<Mutex<std::collections::HashMap<String, devmap_query::Cancel>>>;

/// One stdio session: the store, and whatever it currently has in flight.
///
/// Exists because cancellation needs two things the old loop could not provide:
/// requests running concurrently (so a `notifications/cancelled` can be *read*
/// while the call it cancels is still running) and a handle on the running call.
/// Serially, a cancellation could only ever arrive after the answer had already
/// been written, which is why the notification was accepted and wired to
/// nothing.
struct Session {
    store: Arc<StoreSlot>,
    in_flight: InFlight,
}

impl Session {
    fn new(store: Arc<StoreSlot>) -> Self {
        Self {
            store,
            in_flight: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }

    /// Trip the cancel flag for `id`, if it is still running.
    ///
    /// Absent is normal, not an error: a cancellation that loses the race with
    /// completion is exactly the case the specification says to tolerate.
    fn cancel(&self, id: &Value) {
        if let Ok(map) = self.in_flight.lock() {
            if let Some(cancel) = map.get(&id.to_string()) {
                cancel.cancel();
            }
        }
    }

    /// Take a slot for `id`, or say why there is none.
    ///
    /// The bound is on concurrency, not on rate: a slot is held only while the
    /// request is actually running, so a client that waits for its answers never
    /// meets it however many questions it asks.
    fn register(&self, id: &Value, cancel: devmap_query::Cancel) -> Result<(), String> {
        let Ok(mut map) = self.in_flight.lock() else {
            return Err(
                "the in-flight table was poisoned by an earlier panic, so this session can no \
longer account for what it is running and cannot promise this request would be cancellable"
                    .to_string(),
            );
        };
        // Occupied means the client reused an id that has not been answered:
        // "The request ID **MUST NOT** match the ID of any other request the
        // sender has issued and not yet received a response for." Overwriting
        // was the previous behaviour and it broke cancellation silently — the
        // first request's flag was dropped from the table, so the cancellation
        // the client later sent for that id reached the wrong call, and the
        // first request became unstoppable with nothing saying so.
        if let Some(existing) = map.insert(id.to_string(), cancel) {
            // Put the original back: the *earlier* request is the one that owns
            // this id, and it is still running.
            map.insert(id.to_string(), existing);
            return Err(format!(
                "id {id} already names a request on this connection that has not been answered. \
Ids must be unique among a sender's outstanding requests; reusing one would make the two \
answers indistinguishable and would leave the first call uncancellable."
            ));
        }
        Ok(())
    }

    /// Give back the slot `id` held.
    fn release(&self, id: &Value) {
        if let Ok(mut map) = self.in_flight.lock() {
            map.remove(&id.to_string());
        }
    }
}

/// The `tools/list` payload, honouring the pagination contract.
///
/// This server returns its whole list in one page and never sets `nextCursor`,
/// so every cursor it is handed is one it did not issue. Ignoring it and
/// re-serving page one is the failure the pagination rules exist to prevent: a
/// client paging forward would receive the same nine tools believing them to be
/// the next nine, and would stop only because `nextCursor` was absent — having
/// double-counted the list without ever being told. "Invalid cursors SHOULD
/// result in an error with code -32602."
fn list_tools(params: Option<&Value>) -> Result<Value, RpcError> {
    if let Some(cursor) = params.and_then(|p| p.get("cursor")) {
        return Err(RpcError::new(
            codes::INVALID_PARAMS,
            format!(
                "cursor {cursor} was not issued by this server: tools/list returns every tool \
in a single page and never sets nextCursor, so there is no page after the first. Retry \
without a cursor."
            ),
        ));
    }
    // `ListToolsResult` extends `CacheableResult`, so the hint is a required
    // field of this result rather than an HTTP-layer decoration — and it is
    // written here, once, because `server/discover` already carried it on both
    // transports while `tools/list` carried it on only one. The same connection
    // answering one cacheable method with a hint and the other without is a
    // client-visible inconsistency with no reason behind it.
    Ok(json!({
        "tools": tool_specs(),
        "ttlMs": CACHE_TTL_MS,
        "cacheScope": "private"
    }))
}

/// Stamp the revision's required `resultType` on a result payload.
///
/// `Result.resultType`: "Servers implementing this protocol version MUST include
/// this field." It tells a client whether what it holds is the final answer
/// (`complete`) or a request for more input (`input_required`); this server
/// never asks for more input, so every result it produces is complete.
///
/// Applied at the one place every result passes through, and to the handshake
/// eras as well as the modern one. `Result` has always been
/// `{[key: string]: unknown}`, so an older client ignores the field, and the
/// specification tells clients to read an *absent* `resultType` as `"complete"` —
/// so its presence can never mean less than its absence. Stamping per method
/// instead would be nine chances to forget one, and the one forgotten would be
/// the one a modern client rejected.
///
/// A payload that already states its own type keeps it.
///
/// The same place stamps `_meta.io.modelcontextprotocol/serverInfo`, which the
/// specification asks for on "every result's `_meta`" for the same reason this
/// function exists at all: the modern era keeps no connection state, so anything
/// a client can only learn once is something it cannot learn.
fn complete(mut result: Value) -> Value {
    let Some(object) = result.as_object_mut() else {
        return result;
    };
    object
        .entry("resultType")
        .or_insert_with(|| json!("complete"));
    // Merged, not overwritten: a result that already carries `_meta` of its own
    // keeps it, and only the key this server owns is written.
    if let Some(meta) = object
        .entry("_meta")
        .or_insert_with(|| json!({}))
        .as_object_mut()
    {
        meta.entry("io.modelcontextprotocol/serverInfo")
            .or_insert_with(server_info);
    }
    result
}

/// Run one JSON-RPC method, with a cancellation flag the caller may trip.
///
/// Split from [`handle_method`] so the stdio session can hand in a flag it holds
/// a second handle on; every other caller wants the flag nobody will ever set.
///
/// A method that produces no payload of its own answers with an empty result
/// rather than "no result". Whether a frame is written at all is the transport's
/// decision and is made from the presence of an `id`, not from this — a
/// `notifications/initialized` that arrives carrying an id is a request, and a
/// request is owed a response frame whatever its method name suggests.
pub async fn handle_method_cancellable(
    store: &Arc<StoreSlot>,
    method: &str,
    params: Option<Value>,
    cancel: devmap_query::Cancel,
) -> Result<Value, RpcError> {
    // Before the method, because the answer to "is this a method I have" depends
    // on which era is asking, and because a request declaring a revision this
    // server does not serve must be refused whatever it went on to ask for.
    let era = classify_era(params.as_ref())?;

    let result = match method {
        // `initialize` under a modern envelope is not a method this server has.
        // The handshake was removed in the modern revision and `server/discover`
        // replaces it, so answering would hand a client that had just declared
        // `2026-07-28` a `protocolVersion` of `2025-11-25` — this server
        // reporting a negotiation that did not happen, over a mechanism with no
        // connection to negotiate over. `-32601` is what the HTTP binding maps
        // to `404`, which is exactly the "I do not have that method" the client
        // needs to see.
        "initialize" if era == Era::Modern => {
            return Err(RpcError::new(
                codes::METHOD_NOT_FOUND,
                format!(
                    "'initialize' is not a method of {}: that revision replaced the handshake \
with per-request metadata. Use 'server/discover' to learn what this server supports. To \
handshake instead, send 'initialize' without a per-request _meta.{META_PROTOCOL_VERSION}; this \
server also serves {}.",
                    MODERN_PROTOCOL_VERSIONS.join(", "),
                    HANDSHAKE_PROTOCOL_VERSIONS.join(", ")
                ),
            ))
        }
        "initialize" => initialize_result(params.as_ref()),
        // Accepted and acted on nowhere. `notifications/cancelled` is handled by
        // the session before it ever reaches this function; reaching here means
        // there was no session, so there is nothing in flight to stop.
        "notifications/initialized" | "notifications/cancelled" => json!({}),
        "ping" => json!({}),
        "server/discover" => discover_result(),
        "tools/list" => list_tools(params.as_ref())?,
        "tools/call" => call_tool(store, params, cancel).await?,
        other => {
            return Err(RpcError::new(
                codes::METHOD_NOT_FOUND,
                format!("method '{other}' is not supported by this server"),
            ))
        }
    };
    Ok(complete(result))
}

/// Run one JSON-RPC method against the store.
pub async fn handle_method(
    store: &Arc<StoreSlot>,
    method: &str,
    params: Option<Value>,
) -> Result<Value, RpcError> {
    handle_method_cancellable(store, method, params, devmap_query::Cancel::default()).await
}

/// Decode one frame and produce the response, or `None` for a notification.
///
/// Parsing happens in two stages, and that is the point. Decoding straight into
/// a struct made every structural fault a serde error, and every serde error was
/// reported as `-32700 Parse error` against a null id — for JSON that had parsed
/// perfectly well and whose id was sitting right there. A client correlating
/// responses to pending ids never resolved that id and waited forever.
///
/// Stage one establishes "is this JSON at all". Only stage two asks whether it
/// is a *well-formed request*, and by then the id is recoverable, so a malformed
/// request is reported as `-32600` against its own id.
async fn dispatch_value(session: &Arc<Session>, value: Value) -> Option<Value> {
    // JSON-RPC 2.0 §6: a batch is an Array of Request objects, answered with an
    // Array of the corresponding Responses. An empty Array is a single
    // `-32600`. Notifications inside a batch still get no entry, and a batch of
    // only notifications gets no response array at all.
    if let Value::Array(items) = value {
        if items.is_empty() {
            return Some(rpc_error_frame(
                None,
                codes::INVALID_REQUEST,
                "a batch must contain at least one request",
            ));
        }
        let mut responses = Vec::new();
        for item in items {
            // Sequential inside a batch: the members share one connection and
            // one store, and answering them concurrently would buy nothing
            // while making the response order depend on scheduling.
            // Through `dispatch_single`, not back through this function: a
            // batch member is a Request *object*. Recursing here treated a
            // nested array as another batch, so `[[[…]]]` came back as an
            // equally nested array of arrays wrapping one error — a frame no
            // client can match to a pending id, with the id itself buried N
            // levels down. It was also the only unbounded recursion on the
            // request path.
            if let Some(response) = dispatch_single(session, item).await {
                responses.push(response);
            }
        }
        return if responses.is_empty() {
            None
        } else {
            Some(Value::Array(responses))
        };
    }

    dispatch_single(session, value).await
}

/// Decode one request object. Never a batch — see [`dispatch_value`].
///
/// JSON-RPC 2.0 §6 defines a batch as "an Array of Request objects". It does not
/// nest, and keeping that fact in the type of this function is what stops a
/// nested array from being answered with a nested response.
async fn dispatch_single(session: &Arc<Session>, value: Value) -> Option<Value> {
    let Value::Object(object) = &value else {
        return Some(rpc_error_frame(
            None,
            codes::INVALID_REQUEST,
            if value.is_array() {
                "a batch member must be a request object; JSON-RPC 2.0 §6 batches do not nest"
                    .to_string()
            } else {
                format!(
                    "a request must be an object or a batch array, got {}",
                    kind_of(&value)
                )
            },
        ));
    };

    // Presence, not value. JSON-RPC 2.0 §4: "A Notification is a Request object
    // without an 'id' member." A frame that carries an `id` member is a request
    // and is owed a response, whatever that member turned out to contain —
    // decoding into `Option<Value>` folded "absent" and "null" together, so a
    // frame that was owed an answer got none and its client hung.
    //
    // *Whether the value is a legal id* is a separate question, answered below.
    let has_id = object.contains_key("id");
    let id = object.get("id").cloned().unwrap_or(Value::Null);

    // MCP narrows JSON-RPC's id, in identical words in every revision this
    // server speaks: "Requests MUST include a string or integer ID. Unlike base
    // JSON-RPC, the ID MUST NOT be `null`."
    //
    // The offending value is echoed rather than replaced with null. JSON-RPC's
    // own rule — "It MUST be the same as the value of the id member in the
    // Request Object" — is what lets the client resolve the call it made, and
    // the id here was read successfully; it is simply not one this protocol
    // permits. Answering `null` instead would refuse the request *and* hide
    // which request was refused, so a client with several in flight would learn
    // only that one of them had failed.
    if has_id {
        if let Some(fault) = id_fault(&id) {
            return Some(rpc_error_frame(
                Some(id),
                codes::INVALID_REQUEST,
                format!(
                    "a request id must be a string or an integer and must not be null; this one \
is {fault}. Sending it back so the call can be resolved, but it is not a legal MCP id and the \
request was not run."
                ),
            ));
        }
    }

    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return respond(
            has_id,
            rpc_error_frame(Some(id), codes::INVALID_REQUEST, "jsonrpc must be \"2.0\""),
        );
    }

    let Some(method) = object.get("method").and_then(Value::as_str) else {
        return respond(
            has_id,
            rpc_error_frame(
                Some(id),
                codes::INVALID_REQUEST,
                "request has no string 'method'",
            ),
        );
    };
    let method = method.to_string();
    let params = object.get("params").cloned();

    // Acted on here rather than in `handle_method`, because only the session
    // knows what is running.
    //
    // `!has_id` is part of the condition, not a detail of it. "Notifications
    // **MUST NOT** include an ID", so a frame that has one is not a
    // notification however its method is spelled — and this function already
    // applies that rule to every other method. `notifications/cancelled` was the
    // one exception, intercepted before the id was ever consulted, so a frame
    // carrying both an id and this method cancelled what it named *and* returned
    // nothing: the client kept an id it would never see resolved, on a
    // connection that was otherwise healthy. Carrying an id, it falls through
    // and is answered like any other request whose method produces no payload.
    if method == "notifications/cancelled" && !has_id {
        if let Some(target) = params.as_ref().and_then(|p| p.get("requestId")) {
            session.cancel(target);
        }
        return None;
    }

    // A frame with no id is a notification, and the only notifications this
    // protocol has are `notifications/*`. `notifications/cancelled` was handled
    // above; anything else that arrives without an id is not a notification this
    // server can act on, and dispatching it anyway was doing real work nobody
    // could ever see: a `tools/call` sent without an id ran the full query,
    // held the store lock for up to the call timeout, and threw the answer away,
    // because a notification must not be answered. The refusal is loud on
    // stderr — the one channel stdio leaves open for it — and silent on the
    // wire, which is where the specification requires silence.
    if !has_id && method != "notifications/initialized" {
        tracing::warn!(
            "discarded a frame with no id calling '{method}': the only notifications this \
server accepts are notifications/initialized and notifications/cancelled, and a request \
method sent as a notification would run work whose result cannot be returned"
        );
        return None;
    }

    let cancel = devmap_query::Cancel::default();
    if has_id {
        // Registering is also where the concurrency bound is applied, because
        // this is the point at which a request starts costing something: past
        // here it holds a map entry, a task, and — for `tools/call` — a slot in
        // the blocking pool for up to the call timeout. Unbounded, a peer that
        // pipelines without waiting chooses how much of this process it owns.
        //
        // Refused rather than queued, and refused *by name*. A client that is
        // shedding load can back off; a client whose requests were silently
        // queued behind a thousand others cannot tell overload from a hang.
        match session.register(&id, cancel.clone()) {
            Ok(()) => {}
            // The only way this fails is a reused id, which is the client's own
            // violation and something it must fix by renumbering. Capacity is
            // not refused here: the read loop applies backpressure instead, so
            // there is no "busy" for this to report.
            Err(refusal) => {
                return respond(
                    has_id,
                    rpc_error_frame(Some(id), codes::INVALID_REQUEST, refusal),
                );
            }
        }
    }

    let outcome = handle_method_cancellable(&session.store, &method, params, cancel.clone()).await;

    if has_id {
        session.release(&id);
    }

    // A cancelled request gets no response. The specification is explicit that
    // the receiver must not answer a request it was told to abandon, and an
    // answer here would also be a claim about a traversal that stopped early.
    if cancel.is_cancelled() {
        return None;
    }

    // A request whose method legitimately produces no payload still needs a
    // response frame, or the client waits forever for an id it will never see
    // again; `handle_method_cancellable` returns an empty result for those, so
    // there is one shape here rather than two.
    match outcome {
        Ok(result) => respond(
            has_id,
            json!({"jsonrpc": "2.0", "id": id, "result": result}),
        ),
        Err(err) => respond(has_id, rpc_error_frame_from(Some(id), &err)),
    }
}

/// Why `id` is not a legal MCP request id, if it is not.
///
/// "Integer" is meant literally: `1.5` is a JSON number and not an integer, and
/// a client that sent it would be matching responses against a value its own
/// JSON layer may well have rounded. An empty string is legal — the constraint
/// is on the type, not on the content.
pub(crate) fn id_fault(id: &Value) -> Option<&'static str> {
    match id {
        Value::String(_) => None,
        Value::Number(number) if number.is_i64() || number.is_u64() => None,
        Value::Number(_) => Some("a fractional number"),
        Value::Null => Some("null"),
        other => Some(kind_of(other)),
    }
}

/// A notification is never answered, not even to report that it failed.
fn respond(has_id: bool, frame: Value) -> Option<Value> {
    has_id.then_some(frame)
}

/// Process one decoded line, returning the frame to write back.
///
/// `None` means "write nothing": the line was a notification, and answering a
/// notification is a protocol violation the client is entitled to reject.
///
/// The session this creates has nothing else in flight, so a cancellation
/// arriving through it has nothing to cancel. That is correct for a one-shot
/// call; [`serve_streams`] holds a session across the whole connection.
pub async fn handle_line(store: &Arc<StoreSlot>, line: &str) -> Option<Value> {
    let session = Arc::new(Session::new(Arc::clone(store)));
    handle_line_in(&session, line).await
}

async fn handle_line_in(session: &Arc<Session>, line: &str) -> Option<Value> {
    if line.trim().is_empty() {
        return None;
    }
    match serde_json::from_str::<Value>(line) {
        Ok(value) => dispatch_value(session, value).await,
        // Only here is `-32700` correct, and only here is a null id forced: the
        // bytes were not JSON, so there is no id to recover.
        Err(err) => Some(rpc_error_frame(None, codes::PARSE_ERROR, err.to_string())),
    }
}

/// Serve MCP over stdio until the client closes its end.
///
/// Nothing but JSON-RPC frames may reach stdout on this transport — a stray
/// `println!` corrupts the stream and the client's next parse fails on our
/// bytes. Diagnostics go to stderr, which the client ignores.
pub async fn serve_stdio(store: Arc<StoreSlot>) -> anyhow::Result<()> {
    let stdin = BufReader::with_capacity(64 * 1024, tokio::io::stdin());
    serve_streams(store, stdin, tokio::io::stdout()).await
}

/// What one read of the wire produced.
enum Frame {
    /// A complete line, as raw bytes. Not yet known to be UTF-8.
    Line(Vec<u8>),
    /// A line longer than [`MAX_FRAME_BYTES`], refused and skipped without ever
    /// being held in memory. Carries how far it got before the limit.
    TooLarge(usize),
    Eof,
}

/// Read one newline-terminated frame, refusing to buffer past the limit.
///
/// The bound is applied *while* accumulating, not after. The previous version
/// read through `BufReader::lines()`, which grows its buffer to fit, and then
/// checked the length of a `String` that had already been allocated — so the
/// check reported the size of an allocation it had failed to prevent. The socket
/// transport's `read_frame` has always done this correctly; this now matches it.
///
/// An over-long frame is drained to the next newline rather than closing the
/// connection, so one bad frame costs its own response and not the session.
async fn read_frame<R>(reader: &mut BufReader<R>) -> std::io::Result<Frame>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut buffer: Vec<u8> = Vec::new();
    let mut overflowed: Option<usize> = None;
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Ok(match overflowed {
                Some(seen) => Frame::TooLarge(seen),
                None if buffer.is_empty() => Frame::Eof,
                // A final line with no trailing newline is still a frame.
                None => Frame::Line(buffer),
            });
        }
        match available.iter().position(|byte| *byte == b'\n') {
            Some(offset) => {
                if overflowed.is_none() {
                    buffer.extend_from_slice(&available[..offset]);
                }
                reader.consume(offset + 1);
                return Ok(match overflowed {
                    Some(seen) => Frame::TooLarge(seen + offset),
                    None => Frame::Line(buffer),
                });
            }
            None => {
                let taken = available.len();
                if overflowed.is_none() {
                    if buffer.len() + taken > MAX_FRAME_BYTES {
                        // Stop accumulating here, before the copy. Everything
                        // from now to the newline is counted and discarded.
                        overflowed = Some(buffer.len() + taken);
                        buffer = Vec::new();
                    } else {
                        buffer.extend_from_slice(available);
                    }
                } else if let Some(seen) = overflowed.as_mut() {
                    *seen += taken;
                }
                reader.consume(taken);
            }
        }
    }
}

/// The transport loop, over any reader and writer.
///
/// Split out from [`serve_stdio`] so tests drive it over in-memory pipes: a test
/// that has to spawn a process to exercise the protocol is a test nobody runs.
///
/// Requests run concurrently, each in its own task, with writes serialized
/// behind one lock. Serially, a single 30-second `impact` blocked every other
/// tool call on the connection, and a cancellation could not be read until the
/// call it cancelled had already been answered.
pub async fn serve_streams<R, W>(
    store: Arc<StoreSlot>,
    reader: BufReader<R>,
    writer: W,
) -> anyhow::Result<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    serve_streams_with_admission(
        store,
        reader,
        writer,
        Admission::new(MAX_IN_FLIGHT_REQUESTS),
    )
    .await
}

/// The transport loop under a caller-supplied ceiling on requests in flight.
///
/// The [`Admission`] is passed in rather than built here for the reason
/// `serve_http_on_with_admission` gives: one host process may run several
/// sessions that should share a budget, and the pool's counters — the
/// high-water mark, and how many requests were shed — are the only way to see a
/// ceiling doing its job. On this transport `shed()` must stay zero: the policy
/// is to wait (see [`MAX_IN_FLIGHT_REQUESTS`]), so a non-zero count here is a
/// bug rather than a busy server.
pub async fn serve_streams_with_admission<R, W>(
    store: Arc<StoreSlot>,
    reader: BufReader<R>,
    writer: W,
    admission: Admission,
) -> anyhow::Result<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let session = Arc::new(Session::new(store));
    let writer = Arc::new(tokio::sync::Mutex::new(writer));
    let mut reader = reader;
    let mut tasks = Vec::new();

    loop {
        let frame = match read_frame(&mut reader).await {
            Ok(frame) => frame,
            Err(err) => {
                // A transport-level read failure is the end of the session; a
                // *content* fault is not, and is handled below.
                for task in tasks {
                    let _ = task.await;
                }
                return Err(err.into());
            }
        };

        let bytes = match frame {
            Frame::Eof => break,
            Frame::TooLarge(seen) => {
                let refusal = rpc_error_frame(
                    None,
                    codes::INVALID_REQUEST,
                    // The limit is named before the observed size, deliberately.
                    // The refusal used to lead with the total, and that number
                    // was the size of an allocation this process had already
                    // made — the check ran after the buffering it claimed to
                    // prevent. It now leads with the bound that was enforced,
                    // and reports the overage as something counted while being
                    // discarded, which is what actually happens.
                    format!(
                        "request frame exceeded the limit of {MAX_FRAME_BYTES} bytes, which \
is the most this server will hold; the frame was discarded unbuffered past that point, and \
continued for {seen} bytes in total before terminating"
                    ),
                );
                write_frame(&writer, &refusal).await?;
                continue;
            }
            Frame::Line(bytes) => bytes,
        };

        // Invalid UTF-8 is one bad frame, not the end of the session. Reading
        // through `lines()` turned a single stray byte into `InvalidData`, which
        // the loop returned as an error — so one `0xFF` cost the client every
        // response for that request *and every request after it*. The socket
        // transport answers the same input with a structured refusal.
        let text = match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(err) => {
                let refusal = rpc_error_frame(
                    None,
                    codes::PARSE_ERROR,
                    format!("request frame is not valid UTF-8: {err}"),
                );
                write_frame(&writer, &refusal).await?;
                continue;
            }
        };

        // Backpressure, and nothing else. Reaping is not a bound — it only
        // removes what has already finished, and a peer writing faster than
        // tasks retire outgrows it without limit — so the permit is taken
        // *before* the spawn and held by the task until it retires. While this
        // await is held the loop is not reading the next frame, so the pressure
        // reaches the peer through the pipe rather than through this process's
        // heap.
        //
        // It waits rather than refusing on purpose: a bound that discards a
        // well-formed request from a client doing nothing wrong is data loss
        // wearing a good error message, and an agent host draining a plan
        // pipelines hundreds of requests as a matter of course. The wait is
        // bounded in the only way that matters — every admitted call is bounded
        // by `CALL_TIMEOUT`, so a permit is never held forever.
        let Some(admitted) = admission.admit().await else {
            // The pool is closed, which nothing here does. Ending the session
            // is the only honest answer: continuing would spawn unadmitted work
            // and put this loop back where the ceiling was added to fix it.
            break;
        };

        let session = Arc::clone(&session);
        let writer = Arc::clone(&writer);
        tasks.push(tokio::spawn(async move {
            let _admitted = admitted;
            if let Some(frame) = handle_line_in(&session, &text).await {
                let _ = write_frame(&writer, &frame).await;
            }
        }));
        // Finished handles are still reaped as we go, so a long session does not
        // accumulate a handle per request it ever served. This bounds the vector
        // and not the fan-out; the fan-out is the admission above.
        tasks.retain(|task| !task.is_finished());
    }

    for task in tasks {
        let _ = task.await;
    }
    Ok(())
}

async fn write_frame<W>(writer: &tokio::sync::Mutex<W>, frame: &Value) -> anyhow::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let mut payload = serde_json::to_vec(frame)?;
    payload.push(b'\n');
    // The lock spans the write and the flush, so two concurrent responses cannot
    // interleave their bytes into one unparseable line.
    let mut writer = writer.lock().await;
    writer.write_all(&payload).await?;
    // Flushed per frame. Buffered, a request/response protocol deadlocks: the
    // client waits for an answer sitting in our buffer, and we wait for its next
    // request.
    writer.flush().await?;
    Ok(())
}
