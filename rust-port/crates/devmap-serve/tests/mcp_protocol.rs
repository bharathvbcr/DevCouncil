//! Protocol-level tests for the native MCP server.
//!
//! These drive `handle_line` and `serve_streams` rather than a spawned process:
//! a test that has to launch a binary to check a parse error is a test that
//! nobody runs and that reports failures as exit codes.
//!
//! Two properties are asserted throughout, because they are what an agent host
//! depends on and what silently breaks:
//!
//! 1. **A request gets exactly one response frame; a notification gets none.**
//!    Answering a notification desynchronizes a client that is matching frames
//!    to ids, and failing to answer a request hangs it forever.
//! 2. **A call that could not run says so.** `isError: true` with a reason, never
//!    an empty success. An agent that reads `{"items": []}` concludes nothing
//!    calls the function and deletes it.

use std::sync::Arc;

use devmap_serve::mcp::{handle_line, serve_streams, to_ipc_command, tool_specs, StoreSlot};
use devmap_store::Store;
use serde_json::{json, Value};

/// A store with a real call graph, so an empty answer means something.
fn corpus() -> Arc<StoreSlot> {
    let files = [
        ("core.py", "def helper(rows):\n    return sum(rows)\n"),
        (
            "caller.py",
            "from core import helper\n\n\ndef run(rows):\n    return helper(rows)\n",
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
    Arc::new(StoreSlot::ready("in-memory", Arc::new(store)))
}

/// A slot pointing at a path where no index exists and none will appear.
fn absent_index() -> Arc<StoreSlot> {
    Arc::new(StoreSlot::new(
        std::env::temp_dir().join("devmap-mcp-no-such-index/devmap.sqlite"),
    ))
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

/// The drift guard.
///
/// Every declared tool must translate into a command the socket protocol
/// actually has. The two lists live in one array precisely so this cannot break,
/// and this test is what proves the array is doing that job — a tool that
/// appears in `tools/list` and cannot be called is worse than one that is
/// absent, because the agent plans around it and fails at the last step.
#[test]
fn every_declared_tool_maps_to_a_command() {
    let specs = tool_specs();
    assert_eq!(
        specs.len(),
        11,
        "the tool surface changed; update this count"
    );
    for spec in &specs {
        let name = spec["name"].as_str().expect("tool name");
        // Minimum viable arguments per tool: the required fields only. Anything
        // optional must have a serde default, which is the other half of the
        // claim this test makes.
        let arguments = match name {
            "devmap_status" | "devmap_dead_symbols" | "devmap_clones" => json!({}),
            "devmap_search" => json!({"query": "helper"}),
            "devmap_dependencies" | "devmap_impact" => json!({"target": "core.py"}),
            "devmap_trace" => json!({"from": "helper"}),
            "devmap_neighbors" => json!({"targets": ["helper"]}),
            "devmap_preview" => json!({"file": "core.py", "content": "def helper():\n    pass\n"}),
            "devmap_explore" => json!({"query": "helper"}),
            "devmap_affected_tests" => json!({"targets": ["helper"]}),
            other => panic!("tool {other} has no argument fixture in this test"),
        };
        to_ipc_command(name, Some(&arguments))
            .unwrap_or_else(|err| panic!("tool {name} does not deserialize: {}", err_text(&err)));
    }
}

fn err_text(err: &devmap_serve::mcp::RpcError) -> String {
    format!("{err:?}")
}

/// Every tool's declared schema must reject unknown fields.
///
/// Without `additionalProperties: false` a client can send a misspelled
/// argument, have the schema accept it, and get the default — which is a wrong
/// answer produced by a call that looked correct at every layer.
#[test]
fn every_tool_schema_refuses_unknown_arguments() {
    for spec in tool_specs() {
        let name = spec["name"].as_str().unwrap().to_string();
        assert_eq!(
            spec["inputSchema"]["additionalProperties"],
            json!(false),
            "{name} accepts unknown arguments"
        );
    }
}

/// The annotations must describe what the tools do.
///
/// A tool annotated `readOnlyHint: true` that writes is worse than an
/// unannotated one: a host may run it without asking. Every tool here routes to
/// a query, so every one is read-only — and this test is what makes adding a
/// writing tool break loudly instead of inheriting the wrong annotation.
#[test]
fn every_tool_is_annotated_read_only_and_non_destructive() {
    for spec in tool_specs() {
        let name = spec["name"].as_str().unwrap().to_string();
        let annotations = &spec["annotations"];
        assert_eq!(annotations["readOnlyHint"], json!(true), "{name}");
        assert_eq!(annotations["destructiveHint"], json!(false), "{name}");
        // Deliberately false: the index moves as the repository changes, so the
        // same question can get a different answer. Claiming idempotence invites
        // a client to cache an answer past the commit that invalidated it.
        assert_eq!(annotations["idempotentHint"], json!(false), "{name}");
        assert_eq!(annotations["openWorldHint"], json!(false), "{name}");
    }
}

/// R4 at the protocol edge: `tools/list` is a wire artifact and must not depend
/// on hash iteration order.
#[tokio::test]
async fn tools_list_is_byte_identical_across_calls() {
    let store = corpus();
    let frame = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
    let first = handle_line(&store, frame).await.expect("response");
    let second = handle_line(&store, frame).await.expect("response");
    assert_eq!(
        serde_json::to_string(&first).unwrap(),
        serde_json::to_string(&second).unwrap(),
        "tools/list must be byte-identical between calls"
    );
}

/// A notification carries no id, so it must produce no frame at all.
#[tokio::test]
async fn a_notification_is_never_answered() {
    let store = corpus();
    for frame in [
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}}"#,
        // Even a notification naming a method we do not have stays unanswered:
        // replying would hand the client a frame with a null id it cannot match.
        r#"{"jsonrpc":"2.0","method":"notifications/somethingUnknown"}"#,
    ] {
        assert!(
            handle_line(&store, frame).await.is_none(),
            "notification was answered: {frame}"
        );
    }
}

/// A request always gets a frame, even when the method produces no result.
#[tokio::test]
async fn every_request_gets_exactly_one_frame() {
    let store = corpus();
    let response = handle_line(&store, r#"{"jsonrpc":"2.0","id":7,"method":"ping"}"#)
        .await
        .expect("ping must be answered");
    assert_eq!(response["id"], json!(7));
    assert_eq!(response["jsonrpc"], json!("2.0"));
    assert!(response.get("result").is_some(), "ping must carry a result");
}

#[tokio::test]
async fn an_unparseable_frame_reports_parse_error_against_a_null_id() {
    let store = corpus();
    let response = handle_line(&store, "{not json")
        .await
        .expect("a parse failure must still be reported");
    assert_eq!(response["error"]["code"], json!(-32700));
    assert_eq!(
        response["id"],
        Value::Null,
        "an unparsed frame has no id we can trust, so it must be reported against null"
    );
}

#[tokio::test]
async fn a_wrong_jsonrpc_version_is_refused() {
    let store = corpus();
    let response = handle_line(&store, r#"{"jsonrpc":"1.0","id":1,"method":"ping"}"#)
        .await
        .expect("response");
    assert_eq!(response["error"]["code"], json!(-32600));
}

#[tokio::test]
async fn an_unknown_method_is_method_not_found() {
    let store = corpus();
    let response = handle_line(
        &store,
        r#"{"jsonrpc":"2.0","id":1,"method":"resources/list"}"#,
    )
    .await
    .expect("response");
    assert_eq!(response["error"]["code"], json!(-32601));
}

/// An unrecognised protocol revision gets a counter-offer, not a refusal, and
/// the counter-offer is one this transport can actually speak.
#[tokio::test]
async fn protocol_negotiation_counter_offers_a_reachable_revision() {
    let store = corpus();
    for (asked, expected) in [
        (Some("2025-06-18"), "2025-06-18"),
        // The modern revision is not reachable over an initialize handshake, so
        // agreeing to it here would be a promise this transport cannot keep.
        (Some("2026-07-28"), "2025-11-25"),
        (Some("not-a-version"), "2025-11-25"),
        (None, "2025-11-25"),
    ] {
        let params = match asked {
            Some(version) => json!({"protocolVersion": version}),
            None => json!({}),
        };
        let frame = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":params});
        let response = handle_line(&store, &frame.to_string())
            .await
            .expect("response");
        assert_eq!(
            response["result"]["protocolVersion"],
            json!(expected),
            "negotiating against {asked:?}"
        );
    }
}

/// The tool name must decide the command.
///
/// A client that could smuggle `cmd` through `arguments` would run one command
/// under another's name — and therefore under another's annotations, which is
/// what a host uses to decide whether to ask permission.
#[tokio::test]
async fn a_client_cannot_steer_a_tool_at_another_command() {
    let store = corpus();
    let response = call(
        &store,
        "devmap_search",
        json!({"query": "x", "cmd": "dead"}),
    )
    .await;
    assert_eq!(
        response["result"]["isError"],
        json!(true),
        "a smuggled cmd must be refused, not silently honoured"
    );
}

/// THE Class A test at the protocol edge.
///
/// With no index, every tool must report that fact. The failure this guards is
/// specific and has happened in this repository before: a query against an
/// absent or empty index returning `{"items": []}` with `isError: false`, which
/// an agent reads as "nothing calls this function" and acts on.
#[tokio::test]
async fn a_missing_index_is_an_error_not_an_empty_success() {
    let store = absent_index();
    for (tool, arguments) in [
        ("devmap_status", json!({})),
        ("devmap_search", json!({"query": "helper"})),
        ("devmap_impact", json!({"target": "core.py"})),
        ("devmap_dead_symbols", json!({})),
        ("devmap_neighbors", json!({"targets": ["helper"]})),
    ] {
        let response = call(&store, tool, arguments).await;
        let result = &response["result"];
        assert_eq!(
            result["isError"],
            json!(true),
            "{tool} reported success with no index behind it"
        );
        let text = result["content"][0]["text"].as_str().unwrap_or_default();
        assert!(
            text.contains("devmap build") || text.contains("dev map"),
            "{tool}'s error must name the command that fixes it, got: {text}"
        );
        assert!(
            result.get("structuredContent").is_none(),
            "{tool} attached a payload to a failure, which reads as a partial answer"
        );
    }
}

/// The socket path's bounds apply here too, because it is the same validator.
///
/// If MCP had its own validation these would drift, and the drift would show up
/// as one transport accepting work the other refuses.
#[tokio::test]
async fn the_shared_validator_bounds_apply_to_tool_calls() {
    let store = corpus();

    let too_many: Vec<String> = (0..devmap_query::MAX_NEIGHBOR_TARGETS + 1)
        .map(|i| format!("sym{i}"))
        .collect();
    let response = call(&store, "devmap_neighbors", json!({"targets": too_many})).await;
    assert_eq!(
        response["result"]["isError"],
        json!(true),
        "over-many targets must be refused, not trimmed to a short list that reads as complete"
    );

    // NaN reached the store as a filter that could not be evaluated and admitted
    // everything; it is refused at the boundary now, and must stay refused here.
    let response = call(
        &store,
        "devmap_dependencies",
        json!({"target": "core.py", "min_confidence": 2.0}),
    )
    .await;
    assert_eq!(
        response["result"]["isError"],
        json!(true),
        "an out-of-range confidence must be refused rather than clamped"
    );

    let response = call(
        &store,
        "devmap_impact",
        json!({"target": "core.py", "depth": 9_999}),
    )
    .await;
    assert_eq!(
        response["result"]["isError"],
        json!(true),
        "a depth past the traversal cap must be refused"
    );
}

/// A bad argument is the agent's mistake to fix, so it must reach the agent.
///
/// Reported as a JSON-RPC error it reaches only the client runtime; the model
/// sees a tool that failed for no stated reason and retries the same call.
#[tokio::test]
async fn argument_faults_are_tool_errors_not_protocol_errors() {
    let store = corpus();
    for arguments in [
        json!({}),                              // missing required field
        json!({"query": 42}),                   // wrong type
        json!({"query": "x", "nonsuch": true}), // unknown field
    ] {
        let response = call(&store, "devmap_search", arguments.clone()).await;
        assert!(
            response.get("error").is_none(),
            "argument fault surfaced as a protocol error for {arguments}"
        );
        assert_eq!(
            response["result"]["isError"],
            json!(true),
            "argument fault must be a tool error for {arguments}"
        );
    }
}

/// A name that is nearly a real tool is refused, never guessed at.
///
/// This test asserted `isError: true`, which was wrong: the specification puts
/// "any errors in *finding* the tool" on the protocol-error side, and
/// `an_unknown_tool_is_a_protocol_error_not_a_tool_error` below is the case
/// pinned against the spec text. What is left here is the property that case
/// does not cover — that a near-miss is refused rather than resolved to the
/// closest match. `devmap_serch` is one edit from `devmap_search`, and a server
/// that helpfully ran the neighbour would answer a question nobody asked, under
/// a name the caller believes it chose.
#[tokio::test]
async fn an_unknown_tool_name_is_reported_to_the_agent() {
    let store = corpus();
    for name in ["devmap_not_a_tool", "devmap_serch", "devmap_search "] {
        let response = call(&store, name, json!({"query": "helper"})).await;
        assert!(
            response.get("result").is_none(),
            "{name} was answered instead of refused: {response}"
        );
        let text = response["error"]["message"].as_str().unwrap_or_default();
        assert!(
            text.contains(name),
            "the error must name the tool that does not exist, got: {text}"
        );
    }
}

/// A successful answer must carry both encodings.
///
/// `structuredContent` is what a client parses; the text block is what a client
/// that only forwards `content` shows the model. Sending one without the other
/// makes the answer invisible to some clients.
#[tokio::test]
async fn a_successful_call_carries_both_structured_and_text_content() {
    let store = corpus();
    let response = call(&store, "devmap_status", json!({})).await;
    let result = &response["result"];
    assert_eq!(result["isError"], json!(false));
    let structured = &result["structuredContent"];
    assert!(
        structured["node_count"].as_u64().unwrap_or(0) > 0,
        "the fixture store has nodes; status must report them"
    );
    let text = result["content"][0]["text"].as_str().expect("text block");
    let reparsed: Value = serde_json::from_str(text).expect("text block must be the same JSON");
    assert_eq!(
        &reparsed, structured,
        "the two encodings must carry the same answer"
    );
}

/// End-to-end over the real transport loop.
///
/// This is the test that fails against a `Commands::Mcp` that builds its own
/// tokio runtime inside `#[tokio::main]` — that panics with "Cannot start a
/// runtime from within a runtime" and writes no frames at all.
#[tokio::test]
async fn the_transport_loop_answers_a_whole_session() {
    let store = corpus();
    let session = [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25"}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"devmap_impact","arguments":{"target":"helper"}}}"#,
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
    {
        use tokio::io::AsyncWriteExt;
        client_write.write_all(session.as_bytes()).await.unwrap();
        client_write.write_all(b"\n").await.unwrap();
        client_write.shutdown().await.unwrap();
    }

    let mut frames = Vec::new();
    {
        use tokio::io::AsyncBufReadExt;
        let mut lines = tokio::io::BufReader::new(client_read).lines();
        while let Some(line) = lines.next_line().await.unwrap() {
            frames.push(serde_json::from_str::<Value>(&line).expect("every frame is JSON"));
        }
    }
    task.await
        .unwrap()
        .expect("transport loop must exit cleanly");

    assert_eq!(
        frames.len(),
        3,
        "three requests and one notification must produce exactly three frames"
    );
    assert_eq!(frames[0]["id"], json!(1));
    assert_eq!(frames[1]["id"], json!(2));
    assert_eq!(frames[2]["id"], json!(3));
    assert_eq!(
        frames[2]["result"]["isError"],
        json!(false),
        "impact on an indexed symbol must succeed"
    );
}

// ---------------------------------------------------------------------------
// MCP 2026-07-28 conformance.
//
// This server answers `server/discover` with `supportedVersions:
// ["2026-07-28"]` on BOTH transports — the stdio binding's own backward
// compatibility section tells a modern client to probe with `server/discover`
// before anything else and to continue on the version it advertises. So the
// revision's result requirements are this dispatcher's requirements, not just
// the HTTP module's.
// ---------------------------------------------------------------------------

/// Every result must carry `resultType`.
///
/// Schema `Result.resultType`: "Servers implementing this protocol version MUST
/// include this field." It is what tells a client whether the result is the
/// final answer (`complete`) or a request for more input (`input_required`);
/// absent, a client that supports MRTR has to guess, and the guess it is told to
/// make is the one that ends the exchange.
///
/// Stamped on the handshake-era results too. `Result` has always been
/// `{[key: string]: unknown}`, so an older client ignores it, and the spec tells
/// clients to read an absent field as `"complete"` — so the field can never mean
/// less than its absence. One stamping point, no per-method exceptions to get
/// wrong.
#[tokio::test]
async fn every_result_carries_the_required_result_type() {
    let store = corpus();
    let frames = [
        r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
        r#"{"jsonrpc":"2.0","id":1,"method":"server/discover"}"#,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25"}}"#,
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devmap_status","arguments":{}}}"#,
        // The failure path is a result too, and the one a client is most likely
        // to mis-parse: `isError: true` is still `resultType: "complete"`.
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devmap_search","arguments":{"query":"x","nonsuch":1}}}"#,
        // A notification method reaching this dispatcher with an id still owes
        // the client a result frame, and that frame is a result like any other.
        r#"{"jsonrpc":"2.0","id":1,"method":"notifications/initialized"}"#,
    ];
    for frame in frames {
        let response = handle_line(&store, frame)
            .await
            .unwrap_or_else(|| panic!("a request must be answered: {frame}"));
        assert_eq!(
            response["result"]["resultType"],
            json!("complete"),
            "result carries no resultType for: {frame}\n{response}"
        );
    }
}

/// An unknown tool is a protocol error, not a tool error.
///
/// The two mechanisms are not interchangeable and the spec draws the line at
/// exactly this case: "any errors in _finding_ the tool ... should be reported
/// as an MCP error response", and the tools page lists "Unknown tool" as its
/// first example of a protocol error, with `-32602`.
///
/// The distinction has teeth. A tool error is fed back to the model to
/// self-correct; a model told "unknown tool 'devmap_serch'" *inside a tool
/// result* will retry the same non-existent tool, because from its side a tool
/// result means the tool ran. A protocol error reaches the client runtime, which
/// is the layer that owns the tool list and can re-read it.
#[tokio::test]
async fn an_unknown_tool_is_a_protocol_error_not_a_tool_error() {
    let store = corpus();
    let response = call(&store, "devmap_not_a_tool", json!({})).await;
    assert!(
        response.get("result").is_none(),
        "an unknown tool must not be answered with a tool result: {response}"
    );
    assert_eq!(
        response["error"]["code"],
        json!(-32602),
        "unknown tool is INVALID_PARAMS per the tools spec: {response}"
    );
    let message = response["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("devmap_not_a_tool"),
        "the error must name the tool that does not exist, got: {message}"
    );
    assert_eq!(response["id"], json!(1), "the id must survive: {response}");
}

/// `arguments` that is not an object fails the CallToolRequest schema.
///
/// `CallToolRequestParams.arguments` is typed `{ [key: string]: unknown }`, so a
/// string or an array there is a malformed request — the spec's second listed
/// protocol-error case — not a value the model chose badly. Reporting it as a
/// tool error tells the model its *arguments* were rejected when what was
/// rejected was the shape of the call its runtime built.
#[tokio::test]
async fn tool_arguments_that_are_not_an_object_are_a_protocol_error() {
    let store = corpus();
    for arguments in [json!("query=helper"), json!(["helper"]), json!(7)] {
        let response = call(&store, "devmap_search", arguments.clone()).await;
        assert!(
            response.get("result").is_none(),
            "arguments {arguments} is a malformed CallToolRequest, not a bad value: {response}"
        );
        assert_eq!(response["error"]["code"], json!(-32602), "{response}");
    }
}

/// A cursor this server never issued must be refused, not ignored.
///
/// `tools/list` is a paginated operation. This server returns its whole list in
/// one page and never sets `nextCursor`, so *every* cursor is one it did not
/// issue. Ignoring it re-serves page one: a client paging through would receive
/// the same nine tools again under the belief they were the next nine, and
/// stop only because `nextCursor` was absent — having double-counted the list
/// and never been told. "Invalid cursors SHOULD result in an error with code
/// -32602 (Invalid params)."
#[tokio::test]
async fn a_tools_list_cursor_this_server_never_issued_is_refused() {
    let store = corpus();
    for cursor in [json!("eyJwYWdlIjogMn0="), json!(""), json!(3), json!(null)] {
        let frame = json!({
            "jsonrpc": "2.0", "id": 4, "method": "tools/list",
            "params": {"cursor": cursor}
        });
        let response = handle_line(&store, &frame.to_string())
            .await
            .expect("a request must be answered");
        assert!(
            response.get("result").is_none(),
            "cursor {cursor} was silently ignored and page one was re-served: {response}"
        );
        assert_eq!(
            response["error"]["code"],
            json!(-32602),
            "an invalid cursor is INVALID_PARAMS: {response}"
        );
    }
}

/// The complete list says it is complete.
///
/// Counts never lie: `tools/list` returns every tool this server has, so it must
/// carry no `nextCursor` — a cursor here would promise a page that does not
/// exist, and its absence is the only signal a client has that it has seen
/// everything.
#[tokio::test]
async fn an_uncursored_tools_list_is_the_whole_list_and_says_so() {
    let store = corpus();
    let response = handle_line(&store, r#"{"jsonrpc":"2.0","id":5,"method":"tools/list"}"#)
        .await
        .expect("answered");
    let result = &response["result"];
    assert!(
        result.get("nextCursor").is_none(),
        "this server pages nothing; a nextCursor would name a page that does not exist: {result}"
    );
    assert_eq!(
        result["tools"].as_array().map(Vec::len),
        Some(tool_specs().len()),
        "the listed tools must be all of them, not a capped sample: {result}"
    );
}

/// Every result identifies the server that produced it.
///
/// "Servers SHOULD include the following `io.modelcontextprotocol/*` field in
/// every result's `_meta` ... to identify themselves without relying on any
/// prior connection state." On the modern transport that is not a nicety: there
/// is no `initialize`, and `DiscoverResult` has no `serverInfo` field of its
/// own, so `_meta` is the *only* place a client can learn what it is talking to.
/// Without it, "which devmap version answered this?" is unanswerable for every
/// caller that did not go through the handshake.
#[tokio::test]
async fn every_result_identifies_the_server_that_produced_it() {
    let store = corpus();
    for frame in [
        r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
        r#"{"jsonrpc":"2.0","id":1,"method":"server/discover"}"#,
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devmap_status","arguments":{}}}"#,
    ] {
        let response = handle_line(&store, frame).await.expect("answered");
        let info = &response["result"]["_meta"]["io.modelcontextprotocol/serverInfo"];
        assert_eq!(info["name"], json!("devmap"), "for {frame}: {response}");
        assert!(
            info["version"].as_str().is_some_and(|v| !v.is_empty()),
            "Implementation requires a version for {frame}: {response}"
        );
    }
}

/// `tools/list` carries its cache hint on every transport that serves it.
///
/// `ListToolsResult` extends `CacheableResult`, so `ttlMs` and `cacheScope` are
/// required fields of it — and this server advertises `2026-07-28` through
/// `server/discover` on stdio as well as over HTTP, which is the probe the stdio
/// binding tells a modern client to send first. `server/discover` already
/// carried them here; `tools/list` did not, so the same connection answered one
/// cacheable method with a hint and the other without.
#[tokio::test]
async fn tools_list_carries_its_cache_hint_on_the_shared_dispatcher() {
    let store = corpus();
    let response = handle_line(&store, r#"{"jsonrpc":"2.0","id":6,"method":"tools/list"}"#)
        .await
        .expect("answered");
    assert_eq!(response["result"]["ttlMs"], json!(300_000), "{response}");
    assert_eq!(
        response["result"]["cacheScope"],
        json!("private"),
        "{response}"
    );

    // And nothing else claims to be cacheable: an un-annotated result is
    // uncacheable, and saying otherwise would let a client serve a stale answer
    // about an index that has since moved.
    let pinged = handle_line(&store, r#"{"jsonrpc":"2.0","id":7,"method":"ping"}"#)
        .await
        .expect("answered");
    assert!(
        pinged["result"].get("ttlMs").is_none(),
        "ping is not cacheable and must not be advertised as such: {pinged}"
    );
}
