//! Adversarial tests against `src/mcp.rs`.
//!
//! Every test here is written to FAIL against the code as it stands. Each one
//! names, in its own comment, the wrong answer a caller receives. A test in this
//! file that passes is not a finding and should be deleted, not celebrated.

use std::sync::Arc;

use devmap_serve::mcp::{handle_line, serve_streams, tool_specs, StoreSlot};
use devmap_store::Store;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

/// Mirrors `tests/mcp_protocol.rs::corpus`: a store with a real call graph, so
/// an empty answer means something.
fn corpus_store() -> Arc<Store> {
    let files = [
        ("core.py", "def helper(rows):\n    return sum(rows)\n"),
        (
            "caller.py",
            "from core import helper\n\n\ndef run(rows):\n    return helper(rows)\n",
        ),
        (
            "outer.py",
            "from caller import run\n\n\ndef outer(rows):\n    return run(rows)\n",
        ),
        (
            "top.py",
            "from outer import outer\n\n\ndef top(rows):\n    return outer(rows)\n",
        ),
    ];
    let extractions: Vec<_> = files
        .iter()
        .map(|(path, source)| devmap_extract::extract_file(path, source))
        .collect();
    let mut resolver = devmap_resolve::Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let analysis = devmap_analyze::analyze(&extractions, &resolution);
    let store = Store::open_in_memory().expect("in-memory store");
    store
        .save_generation(&extractions, &resolution, &analysis)
        .expect("generation");
    Arc::new(store)
}

fn corpus() -> Arc<StoreSlot> {
    Arc::new(StoreSlot::ready("in-memory", corpus_store()))
}

async fn call(store: &Arc<StoreSlot>, tool: &str, arguments: Value) -> Value {
    let frame = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": tool, "arguments": arguments}
    });
    handle_line(store, &frame.to_string())
        .await
        .expect("a request must produce a response frame")
}

/// Drive the socket transport with one IPC request frame and return its envelope.
async fn socket_request(store: Arc<Store>, request: Value) -> Value {
    let (mut client, server) = tokio::io::duplex(4 * 1024 * 1024);
    let task = tokio::spawn(devmap_serve::handle_stream(server, store));
    let mut payload = serde_json::to_vec(&request).unwrap();
    payload.push(b'\n');
    client.write_all(&payload).await.unwrap();
    let mut line = String::new();
    tokio::io::BufReader::new(&mut client)
        .read_line(&mut line)
        .await
        .unwrap();
    task.await.unwrap().expect("socket handler");
    serde_json::from_str(&line).expect("socket envelope is JSON")
}

// ---------------------------------------------------------------------------
// Claim 6: "A notification is never answered; a request always gets exactly one
// frame."
// ---------------------------------------------------------------------------

/// DEFECT: `{"id": null}` is a *request*, not a notification, and gets no frame.
///
/// JSON-RPC 2.0 §4: "A Notification is a Request object without an 'id' member."
/// An id that is present and null is a request; the spec's own error examples
/// use `"id": null` in responses precisely because null is a legal id value.
/// `RpcRequest::id` is `Option<Value>`, and serde folds an explicit `null` into
/// `None`, so this server cannot tell "no id member" from "id member set to
/// null" and silently drops the request.
///
/// WRONG ANSWER A CALLER GETS: nothing at all. A client that sends
/// `{"jsonrpc":"2.0","id":null,"method":"ping"}` waits forever for a response
/// that this server has decided never to write.
#[tokio::test]
async fn defect_id_present_but_null_is_mistaken_for_a_notification() {
    let store = corpus();
    let response = handle_line(&store, r#"{"jsonrpc":"2.0","id":null,"method":"ping"}"#).await;
    assert!(
        response.is_some(),
        "a request carrying an explicit null id must be answered; it is a request, \
         not a notification, and dropping it hangs the client"
    );
}

/// DEFECT: well-formed JSON with a bad `method` is reported as a *parse* error.
///
/// JSON-RPC 2.0 separates the two: `-32700 Parse error` is "Invalid JSON was
/// received by the server"; `-32600 Invalid Request` is "The JSON sent is not a
/// valid Request object". `handle_line` decodes straight into `RpcRequest`,
/// whose `method` field has no serde default, and routes *every* serde failure
/// — including a perfectly parsed object that simply lacks `method` — to
/// `PARSE_ERROR`. The comment there ("A frame we could not parse has no id we
/// can trust") describes only the first of the two cases it actually handles.
///
/// WRONG ANSWER A CALLER GETS: told its JSON was malformed when its JSON was
/// fine, and handed `id: null` for a request whose id was sitting decoded in the
/// same object. The spec permits a null id only "if there was an error in
/// detecting the id"; there was none here. A client correlating frames to
/// pending ids never resolves id 7 and hangs on a call the server already
/// abandoned. The same path swallows the id for a duplicate-key frame and for a
/// non-string `jsonrpc`.
#[tokio::test]
async fn defect_a_bad_method_field_is_reported_as_a_parse_error_against_a_null_id() {
    let store = corpus();
    for frame in [
        r#"{"jsonrpc":"2.0","id":7}"#,
        r#"{"jsonrpc":"2.0","id":7,"method":42}"#,
        r#"{"jsonrpc":"2.0","id":7,"method":null,"params":{}}"#,
    ] {
        let response = handle_line(&store, frame)
            .await
            .expect("a request must be answered");
        assert_eq!(
            response["error"]["code"],
            json!(-32600),
            "this JSON parsed cleanly; a missing or non-string method is Invalid Request, \
             not Parse error: {frame}"
        );
        assert_eq!(
            response["id"],
            json!(7),
            "the id was decoded successfully, so the response must carry it: {frame}"
        );
    }
}

// ---------------------------------------------------------------------------
// Claim 7: "MAX_FRAME_BYTES=1MB ... exists so that a peer which never sends a
// newline cannot grow this process's heap without bound."
// ---------------------------------------------------------------------------

/// DEFECT: the frame bound is applied *after* the whole line has been buffered.
///
/// The doc comment on `MAX_FRAME_BYTES` says it "exists so that a peer which
/// never sends a newline cannot grow this process's heap without bound", and the
/// comment at the check itself concedes that "`lines()` grows its buffer to fit"
/// — then checks `line.len()` on a `String` that has already been allocated.
/// The sibling socket path gets this right: `protocol::read_frame` returns
/// `TooLarge` the moment `payload.len() + take` would cross `MAX_REQUEST_BYTES`,
/// so it never holds more than the limit.
///
/// WRONG ANSWER A CALLER GETS: the refusal itself proves the allocation — the
/// server reports the exact number of bytes it buffered, and that number is
/// 4 MiB against a 1 MiB limit. A hostile or broken peer that never sends a
/// newline can make this process allocate as much as it likes.
#[tokio::test]
async fn defect_the_frame_bound_does_not_bound_the_buffer() {
    const OVERSIZE: usize = 4 * 1024 * 1024;
    let store = corpus();
    let (client, server) = tokio::io::duplex(64 * 1024);
    let (server_read, server_write) = tokio::io::split(server);
    let task = tokio::spawn(serve_streams(
        store,
        tokio::io::BufReader::new(server_read),
        server_write,
    ));

    let (client_read, mut client_write) = tokio::io::split(client);
    let writer = tokio::spawn(async move {
        // No newline anywhere: the peer never terminates the frame.
        let chunk = vec![b'a'; 64 * 1024];
        let mut sent = 0usize;
        while sent < OVERSIZE {
            client_write.write_all(&chunk).await.unwrap();
            sent += chunk.len();
        }
        client_write.shutdown().await.unwrap();
    });

    let mut lines = tokio::io::BufReader::new(client_read).lines();
    let frame = lines
        .next_line()
        .await
        .unwrap()
        .expect("the over-limit frame must be refused");
    writer.await.unwrap();
    task.await.unwrap().expect("transport loop");

    let value: Value = serde_json::from_str(&frame).unwrap();
    let message = value["error"]["message"].as_str().unwrap_or_default();
    let buffered: usize = message
        .split_whitespace()
        .find_map(|word| word.parse().ok())
        .expect("the refusal names the byte count it buffered");
    assert!(
        buffered <= 1024 * 1024,
        "the reader buffered {buffered} bytes before refusing a 1048576-byte limit; \
         the bound is applied after the allocation it claims to prevent"
    );
}

/// DEFECT: one non-UTF-8 byte kills the whole MCP session with no frame written.
///
/// `serve_streams` reads through `BufReader::lines()`, which returns
/// `ErrorKind::InvalidData` on invalid UTF-8, and the loop turns any read error
/// into `return Err(err.into())`. The socket transport handles the same input
/// by answering with a structured `invalid_request` envelope and staying alive
/// for the byte-level equivalent.
///
/// WRONG ANSWER A CALLER GETS: zero frames for the request it just sent, and
/// every subsequent request on the session silently unanswered — the stated
/// invariant "a request always gets exactly one frame" fails closed on the
/// process, not on the frame.
#[tokio::test]
async fn defect_invalid_utf8_tears_down_the_session_without_a_frame() {
    let store = corpus();
    let (client, server) = tokio::io::duplex(64 * 1024);
    let (server_read, server_write) = tokio::io::split(server);
    let task = tokio::spawn(serve_streams(
        store,
        tokio::io::BufReader::new(server_read),
        server_write,
    ));

    let (client_read, mut client_write) = tokio::io::split(client);
    // A lone 0xFF is not valid UTF-8. It is followed by a perfectly good ping.
    client_write.write_all(&[0xFFu8, b'\n']).await.unwrap();
    client_write
        .write_all(br#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#)
        .await
        .unwrap();
    client_write.write_all(b"\n").await.unwrap();
    client_write.shutdown().await.unwrap();

    let mut frames = Vec::new();
    let mut lines = tokio::io::BufReader::new(client_read).lines();
    while let Some(line) = lines.next_line().await.unwrap() {
        frames.push(line);
    }
    let outcome = task.await.unwrap();

    assert!(
        outcome.is_ok(),
        "a non-UTF-8 byte from the peer must not end the transport loop: {:?}",
        outcome.err()
    );
    assert_eq!(
        frames.len(),
        2,
        "the bad frame must be refused with a frame and the following ping answered; got {frames:?}"
    );
}

// ---------------------------------------------------------------------------
// Claim 3: "Unknown arguments are refused, against the tool's own declared
// schema" — and, by extension, the schema is what a client may believe.
// ---------------------------------------------------------------------------

/// DEFECT: `devmap_neighbors` publishes `depth.default = 1` and actually uses 3.
///
/// `describe("neighbors")` builds its depth property with `depth_prop(1)`, but
/// the field it feeds is `IpcCommand::Neighbors::depth`, which carries
/// `#[serde(default = "default_depth")]` — and `default_depth()` is 3. Every
/// other tool's declared default matches (`impact` and `trace` both declare 3),
/// so this is a single wrong literal, not a convention.
///
/// WRONG ANSWER A CALLER GETS: an agent that omits `depth` is told it asked for
/// immediate neighbours and is handed a three-hop transitive closure — more
/// rows, more tokens, and a "callers" list that includes symbols that do not
/// call the target at all. The reverse is worse: an agent that reads the schema,
/// sees the default is already 1, and therefore does *not* send `depth`, cannot
/// get the answer the schema described.
#[tokio::test]
async fn defect_neighbors_declares_a_depth_default_it_does_not_use() {
    let store = corpus();
    let declared = tool_specs()
        .into_iter()
        .find(|spec| spec["name"] == json!("devmap_neighbors"))
        .expect("devmap_neighbors is declared")["inputSchema"]["properties"]["depth"]["default"]
        .clone();

    let omitted = call(&store, "devmap_neighbors", json!({"targets": ["helper"]})).await;
    let explicit = call(
        &store,
        "devmap_neighbors",
        json!({"targets": ["helper"], "depth": declared}),
    )
    .await;

    assert_eq!(omitted["result"]["isError"], json!(false));
    assert_eq!(explicit["result"]["isError"], json!(false));
    assert_eq!(
        omitted["result"]["structuredContent"], explicit["result"]["structuredContent"],
        "omitting depth must give the same answer as sending the declared default {declared}; \
         it does not, so the published default is a lie"
    );
}

/// WAS A DEFECT, NOW THE GATE: a published default must be the applied default.
///
/// Originally this test named one instance — `devmap_preview` published
/// `min_confidence.default = 0.0` while `IpcCommand::Preview::min_confidence`
/// carries `#[serde(default = "default_preview_confidence")]`, i.e.
/// `PREVIEW_CALLER_MIN_CONFIDENCE` = 0.5. That is the "which callers would my
/// edit break" tool: a client told the filter was off, that omitted the field,
/// silently lost every caller below 0.5 from the breakage list — a shorter list
/// that reads exactly like "this edit breaks nothing".
///
/// It was one of two (`devmap_neighbors.depth` declared 1 and applied 3), so the
/// single-instance assertion was the wrong shape. This is now the class gate,
/// and it is behavioural rather than structural: for every declared default, the
/// answer with the field OMITTED must equal the answer with the field sent
/// EXPLICITLY at its declared value. Comparing the two numbers would only prove
/// two literals match; comparing the two answers proves the published contract
/// is the one a client actually gets.
#[tokio::test]
async fn every_published_default_is_the_default_that_is_applied() {
    let store = corpus();
    let mut checked = 0usize;

    for spec in tool_specs() {
        let tool = spec["name"].as_str().expect("tool name").to_string();
        // Minimum arguments that make each tool answerable, so a difference is
        // attributable to the defaulted field and not to a missing required one.
        let base = match tool.as_str() {
            "devmap_status" | "devmap_dead_symbols" | "devmap_clones" => json!({}),
            "devmap_search" => json!({"query": "helper"}),
            "devmap_dependencies" | "devmap_impact" => json!({"target": "core.py"}),
            "devmap_trace" => json!({"from": "helper"}),
            "devmap_neighbors" => json!({"targets": ["helper"]}),
            "devmap_preview" => {
                json!({"file": "core.py", "content": "def helper(rows):\n    return 0\n"})
            }
            other => panic!("tool {other} has no fixture in this gate"),
        };

        let properties = spec["inputSchema"]["properties"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        for (field, rules) in properties {
            let Some(declared) = rules.get("default") else {
                continue;
            };
            let mut explicit = base.clone();
            explicit
                .as_object_mut()
                .expect("object")
                .insert(field.clone(), declared.clone());

            let omitted = call(&store, &tool, base.clone()).await;
            let explicit = call(&store, &tool, explicit).await;
            checked += 1;

            assert_eq!(
                omitted["result"], explicit["result"],
                "{tool}: omitting `{field}` differs from sending its published default \
{declared}, so the published default is not the applied one"
            );
        }
    }

    assert!(
        checked >= 9,
        "this gate checked only {checked} declared defaults; it is not covering the surface"
    );
}

// ---------------------------------------------------------------------------
// Claim 8 / Class A: "'No index' and 'the symbol does not exist' are different
// facts and must not arrive looking alike."
// ---------------------------------------------------------------------------

/// DEFECT: every non-file at the db path is reported as "the index has not been
/// built", including conditions the check never distinguished.
///
/// `StoreSlot::get` maps `Store::open_existing(..) == Ok(None)` to a message
/// that makes a definite claim: "This is 'the index has not been built', not
/// 'the repository is empty'." But `open_existing` returns `Ok(None)` for
/// `!path.is_file()`, and `Path::is_file` is false for a directory and false for
/// any IO error it swallows (an unreadable parent directory, for one). The check
/// that would distinguish those was never run.
///
/// WRONG ANSWER A CALLER GETS: told to run `devmap build`, which will fail again
/// for the same unstated reason. A check that could not run is reporting what a
/// check that ran and found nothing reports.
#[tokio::test]
async fn defect_a_directory_at_the_db_path_is_reported_as_an_unbuilt_index() {
    let dir = std::env::temp_dir().join("devmap-mcp-adversarial-dir/devmap.sqlite");
    std::fs::create_dir_all(&dir).expect("make a directory where the store should be");
    let store = Arc::new(StoreSlot::new(&dir));
    let response = call(&store, "devmap_status", json!({})).await;
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    assert!(
        !text.contains("the index has not been built"),
        "the path is a directory, which is not 'the index has not been built'; \
         the message states a fact the check never established: {text}"
    );
}

// ---------------------------------------------------------------------------
// Claim 1: "There is exactly one dispatcher" — the two transports must agree.
// ---------------------------------------------------------------------------

/// Both transports, same question, diffed payload for payload.
#[tokio::test]
async fn the_two_transports_answer_the_same_question_identically() {
    let raw = corpus_store();
    let slot = Arc::new(StoreSlot::ready("in-memory", Arc::clone(&raw)));

    let cases: Vec<(&str, Value, Value)> = vec![
        ("devmap_status", json!({}), json!({"cmd": "status"})),
        (
            "devmap_search",
            json!({"query": "helper"}),
            json!({"cmd": "search", "query": "helper"}),
        ),
        (
            "devmap_impact",
            json!({"target": "helper"}),
            json!({"cmd": "impact", "target": "helper"}),
        ),
        (
            "devmap_dependencies",
            json!({"target": "caller.py"}),
            json!({"cmd": "deps", "target": "caller.py"}),
        ),
        (
            "devmap_trace",
            json!({"from": "top"}),
            json!({"cmd": "trace", "from": "top"}),
        ),
        (
            "devmap_neighbors",
            json!({"targets": ["helper", "run"]}),
            json!({"cmd": "neighbors", "targets": ["helper", "run"]}),
        ),
        ("devmap_dead_symbols", json!({}), json!({"cmd": "dead"})),
        ("devmap_clones", json!({}), json!({"cmd": "clones"})),
        (
            "devmap_preview",
            json!({"file": "core.py", "content": "def gone():\n    pass\n"}),
            json!({"cmd": "preview", "file": "core.py", "content": "def gone():\n    pass\n"}),
        ),
    ];

    for (tool, arguments, mut ipc) in cases {
        let mcp = call(&slot, tool, arguments.clone()).await;
        assert_eq!(
            mcp["result"]["isError"],
            json!(false),
            "{tool} failed over MCP: {mcp}"
        );
        ipc.as_object_mut()
            .unwrap()
            .insert("version".into(), json!(1));
        let envelope = socket_request(Arc::clone(&raw), ipc.clone()).await;
        assert_eq!(envelope["ok"], json!(true), "{tool} failed over the socket");
        assert_eq!(
            mcp["result"]["structuredContent"], envelope["result"],
            "{tool} answered differently on the two transports"
        );
    }
}

// ---------------------------------------------------------------------------
// Class A: "an empty list means we looked and found nothing", and the declared
// schema constraints that were supposed to make that true.
// ---------------------------------------------------------------------------

/// DEFECT: `devmap_neighbors` declares `targets.minItems: 1` and answers `[]`.
///
/// Nothing enforces the declared minimum. `validate_request` checks only the
/// *upper* bound (`MAX_NEIGHBOR_TARGETS`) and the per-entry length;
/// `engine.neighbors` likewise only rejects too many. An empty list walks
/// straight through to `{"neighbors": []}` with `isError: false` and no
/// disclosure of any kind — no `truncated`, no `walk_incomplete`, no reason.
///
/// The module's own argument-checking comment is the standard this fails: "a
/// declared constraint that nothing enforces is worse than no constraint".
///
/// WRONG ANSWER A CALLER GETS: an agent that built `targets` from a filter that
/// happened to match nothing sends `[]` and is told, in the shape of a
/// successful answer, that the neighbours are empty. It reads as "these symbols
/// have no callers and no callees" rather than "you asked about no symbols".
/// The same hole passes `budget: 0`, `depth: 0` and `min_nodes: 0` — all
/// declared `minimum: 1` — though those at least come back flagged.
#[tokio::test]
async fn defect_an_empty_targets_list_is_answered_instead_of_refused() {
    let store = corpus();
    let declared = tool_specs()
        .into_iter()
        .find(|spec| spec["name"] == json!("devmap_neighbors"))
        .expect("devmap_neighbors is declared")["inputSchema"]["properties"]["targets"]["minItems"]
        .clone();
    assert_eq!(
        declared,
        json!(1),
        "fixture assumption: minItems is declared"
    );

    let response = call(&store, "devmap_neighbors", json!({"targets": []})).await;
    let result = &response["result"];
    assert_eq!(
        result["isError"],
        json!(true),
        "an empty targets list violates the declared minItems:1 and must be refused; \
         answering it with an empty success is the exact shape this module says it prevents: \
         {result}"
    );
}

/// DEFECT: the declared `minimum: 1` on every numeric property is unenforced.
///
/// Separate from the case above because these three are the same hole in the
/// tools that a client is most likely to parameterise. A client-side JSON Schema
/// validator refuses these values; this server accepts every one of them, so the
/// published schema and the server disagree about what a legal call is.
#[tokio::test]
async fn defect_declared_numeric_minimums_are_never_enforced() {
    let store = corpus();
    for (tool, arguments, property) in [
        (
            "devmap_impact",
            json!({"target": "helper", "depth": 0}),
            "depth",
        ),
        (
            "devmap_search",
            json!({"query": "helper", "budget": 0}),
            "budget",
        ),
        ("devmap_clones", json!({"min_nodes": 0}), "min_nodes"),
    ] {
        let response = call(&store, tool, arguments.clone()).await;
        assert_eq!(
            response["result"]["isError"],
            json!(true),
            "{tool} accepted {property}=0 although its own schema declares minimum:1; \
             the schema is a promise nothing checks"
        );
    }
}

// ---------------------------------------------------------------------------
// Claim 6 again, against the revisions this server volunteers to speak.
// ---------------------------------------------------------------------------

/// DEFECT: JSON-RPC batches are unhandled, on a revision this server offers.
///
/// Batching is part of JSON-RPC 2.0 itself (§6: a client may send an Array of
/// Request objects and "the Server should respond with an Array containing the
/// corresponding Response objects"), and MCP's `2025-03-26` revision carried it
/// forward — it was `2025-06-18` that removed it again. This server's
/// `HANDSHAKE_PROTOCOL_VERSIONS` includes `2025-03-26` and `negotiate` agrees to
/// it verbatim when a client asks, so a client is entitled to batch. But
/// `handle_line` deserializes straight into a single `RpcRequest` struct, and
/// `handle_line`'s own signature (`Option<Value>`) cannot express more than one
/// response frame.
///
/// WRONG ANSWER A CALLER GETS: one `-32700 Parse error` frame with `id: null`
/// for a batch of two well-formed requests. Neither id is ever answered, and the
/// error code says the client sent malformed JSON when it sent exactly what the
/// revision this server just agreed to says it may send. Either batches are
/// handled or `2025-03-26` should not be offered; today it is offered and they
/// are not.
#[tokio::test]
async fn defect_a_batch_is_refused_on_a_revision_this_server_agrees_to_speak() {
    let store = corpus();
    let agreed = handle_line(
        &store,
        r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}"#,
    )
    .await
    .expect("initialize is answered");
    assert_eq!(
        agreed["result"]["protocolVersion"],
        json!("2025-03-26"),
        "fixture assumption: this server agrees to the batching revision"
    );

    let response = handle_line(
        &store,
        r#"[{"jsonrpc":"2.0","id":1,"method":"ping"},{"jsonrpc":"2.0","id":2,"method":"ping"}]"#,
    )
    .await
    .expect("a batch must be answered");
    let frames = response
        .as_array()
        .map(|batch| batch.len())
        .unwrap_or_else(|| {
            panic!("a batch must be answered with a batch of responses, got: {response}")
        });
    assert_eq!(frames, 2, "both requests in the batch must be answered");
}

/// DEFECT: `notifications/cancelled` is accepted, does nothing, and the
/// cancelled request is answered anyway.
///
/// `handle_method` matches `notifications/cancelled` alongside
/// `notifications/initialized` and returns `Ok(None)`, which is indistinguishable
/// from honouring it. Nothing is wired to the in-flight call's `Cancel`; in fact
/// `serve_streams` reads and dispatches strictly one line at a time, so the
/// cancellation cannot even be *read* until the call it cancels has finished.
///
/// WRONG ANSWER A CALLER GETS: the MCP cancellation flow says a receiver SHOULD
/// NOT send a response for a cancelled request. This server sends one. A client
/// that has already freed its pending-request slot for id 1 receives a result
/// for id 1 and must either drop a real answer or resurrect a call it cancelled.
#[tokio::test]
async fn defect_a_cancelled_request_is_still_answered() {
    let store = corpus();
    let session = [
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devmap_impact","arguments":{"target":"helper"}}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1,"reason":"user cancelled"}}"#,
    ]
    .join("\n");

    let (client, server) = tokio::io::duplex(256 * 1024);
    let (server_read, server_write) = tokio::io::split(server);
    let task = tokio::spawn(serve_streams(
        store,
        tokio::io::BufReader::new(server_read),
        server_write,
    ));
    let (client_read, mut client_write) = tokio::io::split(client);
    client_write.write_all(session.as_bytes()).await.unwrap();
    client_write.write_all(b"\n").await.unwrap();
    client_write.shutdown().await.unwrap();

    let mut frames: Vec<Value> = Vec::new();
    let mut lines = tokio::io::BufReader::new(client_read).lines();
    while let Some(line) = lines.next_line().await.unwrap() {
        frames.push(serde_json::from_str(&line).unwrap());
    }
    task.await.unwrap().expect("transport loop");

    assert!(
        !frames.iter().any(|frame| frame["id"] == json!(1)),
        "a request the client cancelled must not be answered; got {frames:?}"
    );
}

// ---------------------------------------------------------------------------
// Attacks that did NOT land. Kept so the next reader knows they were tried.
// ---------------------------------------------------------------------------

/// The lazy slot really does retry: a store that appears mid-session is picked
/// up on the next call, and a store that never appears keeps saying so.
#[tokio::test]
async fn unbroken_a_store_that_appears_mid_session_is_picked_up() {
    let dir = std::env::temp_dir().join(format!(
        "devmap-mcp-adversarial-late-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("devmap.sqlite");
    let slot = Arc::new(StoreSlot::new(&db));

    let before = call(&slot, "devmap_status", json!({})).await;
    assert_eq!(before["result"]["isError"], json!(true));

    {
        let files = [("core.py", "def helper(rows):\n    return sum(rows)\n")];
        let extractions: Vec<_> = files
            .iter()
            .map(|(path, source)| devmap_extract::extract_file(path, source))
            .collect();
        let mut resolver = devmap_resolve::Resolver::new();
        resolver.index_extractions(&extractions);
        let resolution = resolver.resolve_all(&extractions);
        let analysis = devmap_analyze::analyze(&extractions, &resolution);
        let store = Store::open(&db).unwrap();
        store
            .save_generation(&extractions, &resolution, &analysis)
            .unwrap();
    }

    let after = call(&slot, "devmap_status", json!({})).await;
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(
        after["result"]["isError"],
        json!(false),
        "the slot must reopen once the daemon builds the index: {after}"
    );
}

/// A client cannot steer a tool at another command, by any spelling tried:
/// a smuggled `cmd`, a case-variant tool name, a tool name that differs only by
/// a zero-width character, or arguments that are not an object.
///
/// Each case also states *which* refusal mechanism it must arrive by, because
/// the two are not interchangeable and the spec draws the line straight through
/// this table. A bad argument value is a tool error the model can correct; a
/// name that is not in the tool list, or a `tools/call` that does not satisfy
/// the `CallToolRequest` schema, is a protocol error — the model cannot fix a
/// name it was never offered, and a model handed "unknown tool" inside a tool
/// *result* sees a tool that ran and retries the same non-existent name.
#[tokio::test]
async fn unbroken_a_tool_cannot_be_steered_at_another_command() {
    /// How a refusal must reach the caller.
    #[derive(Clone, Copy, PartialEq, Debug)]
    enum By {
        /// `isError: true` inside the result — the model's to fix.
        ToolError,
        /// A JSON-RPC error — the runtime's to fix.
        ProtocolError,
    }
    for (tool, arguments, mechanism) in [
        (
            "devmap_search",
            json!({"query": "x", "cmd": "dead"}),
            By::ToolError,
        ),
        (
            "devmap_search",
            json!({"query": "x", "Cmd": "dead"}),
            By::ToolError,
        ),
        (
            "devmap_search",
            json!({"query": "x", "__proto__": {"cmd": "dead"}}),
            By::ToolError,
        ),
        ("DEVMAP_STATUS", json!({}), By::ProtocolError),
        ("devmap_status\u{200b}", json!({}), By::ProtocolError),
        ("devmap_dead_symbols", json!("dead"), By::ProtocolError),
    ] {
        let store = corpus();
        let response = call(&store, tool, arguments.clone()).await;
        match mechanism {
            By::ToolError => {
                assert_eq!(
                    response["result"]["isError"],
                    json!(true),
                    "{tool} accepted {arguments}"
                );
            }
            By::ProtocolError => {
                assert!(
                    response.get("result").is_none(),
                    "{tool} with {arguments} must not be answered with a tool result: {response}"
                );
                assert_eq!(
                    response["error"]["code"],
                    json!(-32602),
                    "{tool} with {arguments}: {response}"
                );
            }
        }
    }
}

/// Hostile JSON that is not a framing problem is refused as one frame and the
/// session survives: deep nesting, an embedded NUL, a bare newline, and a
/// `params` that is an array rather than an object.
#[tokio::test]
async fn unbroken_hostile_json_is_refused_without_taking_the_session_down() {
    let store = corpus();
    let deep = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{}{}}}"#,
        "[".repeat(400),
        "]".repeat(400)
    );
    for frame in [
        deep.as_str(),
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"pi\0ng\"}",
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":[1,2]}"#,
    ] {
        let response = handle_line(&store, frame)
            .await
            .expect("hostile input still gets one frame");
        assert!(
            response.get("error").is_some(),
            "hostile input must be refused: {frame}"
        );
    }
    assert!(
        handle_line(&store, "   ").await.is_none(),
        "a blank line is not a request and must not be answered"
    );
}
