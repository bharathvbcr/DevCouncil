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
//! A tool call that could not run returns `isError: true`. It never returns an
//! empty success. This is the repository's Class A rule at the protocol edge: a
//! check that could not run must not report what a check that ran and passed
//! reports, because an agent reading `[]` deletes the function that list was
//! supposed to protect.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use devmap_store::Store;
use serde_json::{json, Map, Value};
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};

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

/// JSON-RPC 2.0 reserved error codes, plus the one MCP adds.
mod codes {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL_ERROR: i64 = -32603;
}

/// A failure with the JSON-RPC code it should be reported under.
#[derive(Debug)]
pub struct RpcError {
    code: i64,
    message: String,
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

    fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
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
                    "budget": budget_prop(2000),
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

pub fn tool_specs() -> Vec<Value> {
    TOOLS
        .iter()
        .map(|(name, cmd)| {
            let (description, schema) = describe(cmd);
            json!({
                "name": name,
                "description": description,
                "inputSchema": schema,
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
pub fn to_ipc_command(name: &str, arguments: Option<&Value>) -> Result<IpcCommand, RpcError> {
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
        return Err(RpcError::new(
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
                return Err(RpcError::new(codes::INVALID_PARAMS, reason));
            }
        }
    }

    object.insert("cmd".to_string(), Value::String(cmd.to_string()));

    serde_json::from_value(Value::Object(object))
        .map_err(|err| RpcError::new(codes::INVALID_PARAMS, err.to_string()))
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
        "serverInfo": {
            "name": "devmap",
            "title": "Dev Map code intelligence",
            "version": env!("CARGO_PKG_VERSION")
        },
        "instructions": INSTRUCTIONS
    })
}

/// Protocol revisions reachable over the modern single-exchange HTTP transport.
///
/// Kept separate from [`HANDSHAKE_PROTOCOL_VERSIONS`] because they are different
/// eras with different framing, not a longer version of the same list: a modern
/// request is a self-contained POST carrying its protocol version in `_meta`,
/// with no `initialize` and no session id.
pub const MODERN_PROTOCOL_VERSIONS: &[&str] = &["2026-07-28"];

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
/// modern revisions, because that is what a caller reaching this method over
/// this transport can actually use — listing the handshake revisions would
/// advertise versions this endpoint does not serve.
fn discover_result() -> Value {
    json!({
        "supportedVersions": MODERN_PROTOCOL_VERSIONS,
        "capabilities": {"tools": {"listChanged": false}},
        "instructions": INSTRUCTIONS,
        // Live here, unlike on the handshake transports. The Python server
        // configures these correctly and they are sieved out on every version
        // stdio can negotiate; on this transport they reach the client.
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

    // Argument faults are reported as tool errors, not JSON-RPC errors: the
    // agent that sent them is the one that must correct them, and a tool error
    // reaches the model while a protocol error reaches only the client runtime.
    let command = match to_ipc_command(&name, params.get("arguments")) {
        Ok(command) => command,
        Err(err) => return Ok(tool_error(err.message)),
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
    let text = serde_json::to_string(&value).unwrap_or_else(|err| {
        format!("{{\"error\":\"{name} result was not serializable: {err}\"}}")
    });
    json!({
        "content": [{"type": "text", "text": text}],
        "structuredContent": value,
        "isError": false
    })
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
}

/// Run one JSON-RPC method, with a cancellation flag the caller may trip.
///
/// Split from [`handle_method`] so the stdio session can hand in a flag it holds
/// a second handle on; every other caller wants the flag nobody will ever set.
pub async fn handle_method_cancellable(
    store: &Arc<StoreSlot>,
    method: &str,
    params: Option<Value>,
    cancel: devmap_query::Cancel,
) -> Result<Option<Value>, RpcError> {
    match method {
        "initialize" => Ok(Some(initialize_result(params.as_ref()))),
        // Accepted and answered nowhere. `notifications/cancelled` is acted on
        // by the session before it ever reaches this function; reaching here
        // means there was no session, so there is nothing in flight to stop.
        "notifications/initialized" | "notifications/cancelled" => Ok(None),
        "ping" => Ok(Some(json!({}))),
        "server/discover" => Ok(Some(discover_result())),
        "tools/list" => Ok(Some(json!({"tools": tool_specs()}))),
        "tools/call" => call_tool(store, params, cancel).await.map(Some),
        other => Err(RpcError::new(
            codes::METHOD_NOT_FOUND,
            format!("method '{other}' is not supported by this server"),
        )),
    }
}

/// Run one JSON-RPC method against the store.
pub async fn handle_method(
    store: &Arc<StoreSlot>,
    method: &str,
    params: Option<Value>,
) -> Result<Option<Value>, RpcError> {
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
    // without an 'id' member." `id: null` is a request with a null id — legal,
    // and the specification's own error examples use it. Decoding into
    // `Option<Value>` folded the two together, so a legal request got no
    // response at all and its client hung.
    let has_id = object.contains_key("id");
    let id = object.get("id").cloned().unwrap_or(Value::Null);

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
    if method == "notifications/cancelled" {
        if let Some(target) = params.as_ref().and_then(|p| p.get("requestId")) {
            session.cancel(target);
        }
        return None;
    }

    let cancel = devmap_query::Cancel::default();
    if has_id {
        if let Ok(mut map) = session.in_flight.lock() {
            map.insert(id.to_string(), cancel.clone());
        }
    }

    let outcome = handle_method_cancellable(&session.store, &method, params, cancel.clone()).await;

    if has_id {
        if let Ok(mut map) = session.in_flight.lock() {
            map.remove(&id.to_string());
        }
    }

    // A cancelled request gets no response. The specification is explicit that
    // the receiver must not answer a request it was told to abandon, and an
    // answer here would also be a claim about a traversal that stopped early.
    if cancel.is_cancelled() {
        return None;
    }

    match outcome {
        Ok(Some(result)) => respond(
            has_id,
            json!({"jsonrpc": "2.0", "id": id, "result": result}),
        ),
        // A request whose method legitimately produces no result still needs a
        // response frame, or the client waits forever for an id it will never
        // see again.
        Ok(None) => respond(has_id, json!({"jsonrpc": "2.0", "id": id, "result": {}})),
        Err(err) => respond(has_id, rpc_error_frame(Some(id), err.code, err.message)),
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

        let session = Arc::clone(&session);
        let writer = Arc::clone(&writer);
        tasks.push(tokio::spawn(async move {
            if let Some(frame) = handle_line_in(&session, &text).await {
                let _ = write_frame(&writer, &frame).await;
            }
        }));
        // Finished tasks are reaped as we go so a long session does not
        // accumulate a handle per request it ever served.
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
