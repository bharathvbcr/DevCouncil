//! Conformance against the published MCP specification, read rather than recalled.
//!
//! Every assertion here quotes the revision it comes from. The revisions this
//! server declares span two eras and the rules differ between them, so a test
//! that asserts "the spec says X" without naming which spec is a test that will
//! be wrong the next time either era moves.
//!
//! * Handshake era — `2024-11-05` … `2025-11-25`. An `initialize` exchange
//!   negotiates one version for the connection.
//! * Modern era — `2026-07-28`. No handshake: every request carries its own
//!   version, client identity and capabilities in `_meta`, and the server
//!   accepts or rejects each request on its own.
//!
//! What decides which era a request is served under is the presence of
//! `_meta["io.modelcontextprotocol/protocolVersion"]`, and that is the rule the
//! specification gives a dual-era server: "A request carrying modern
//! per-request `_meta` is served statelessly according to this revision. An
//! `initialize` request selects legacy semantics."

use std::sync::Arc;

use devmap_serve::mcp::{
    codes, handle_line, oversized_result_refusal, serve_streams, structured_content_violation,
    tool_specs, StoreSlot, MAX_IN_FLIGHT_REQUESTS, MODERN_PROTOCOL_VERSIONS, TOOL_NAMES,
};
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

/// A request body carrying the modern era's required `_meta`.
fn modern(method: &str, params: Value) -> String {
    let mut params = params;
    params["_meta"] = json!({
        "io.modelcontextprotocol/protocolVersion": MODERN_PROTOCOL_VERSIONS[0],
        "io.modelcontextprotocol/clientCapabilities": {},
    });
    json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string()
}

// ---------------------------------------------------------------------------
// Request ids
// ---------------------------------------------------------------------------

/// DEFECT: the `id` member is never type-checked.
///
/// Every revision this server declares says the same thing, in the same words:
/// "Requests **MUST** include a string or integer ID. Unlike base JSON-RPC, the
/// ID **MUST NOT** be `null`." (`2025-11-25` and `2026-07-28`, Base Protocol →
/// Requests.)
///
/// WRONG ANSWER A CALLER GETS: a request with `id: null` is answered with a
/// *successful result* carrying `id: null` — a response the client's own
/// conformance check must reject, for a call the server has already run. An
/// `id` that is an object or an array is worse: it is accepted, used as the key
/// of the in-flight cancellation map, and echoed back, so a peer chooses how
/// much of this server's memory one pending request costs.
#[tokio::test]
async fn an_id_that_is_not_a_string_or_integer_is_refused() {
    let store = corpus();
    let illegal = [
        (json!(null), "null"),
        (json!(1.5), "fractional"),
        (json!(true), "boolean"),
        (json!([]), "array"),
        (json!({"a": 1}), "object"),
    ];
    for (id, what) in illegal {
        let frame = json!({"jsonrpc": "2.0", "id": id, "method": "ping"}).to_string();
        let response = handle_line(&store, &frame)
            .await
            .unwrap_or_else(|| panic!("a frame carrying an id member must be answered: {frame}"));
        assert!(
            response.get("result").is_none(),
            "an id of type {what} is not a legal MCP request id and must not be answered with a \
             result the client is then obliged to reject: {response}"
        );
        assert_eq!(
            response["error"]["code"],
            json!(-32600),
            "a malformed id is an Invalid Request, not a parse error or a method fault: \
             {response}"
        );
        assert_eq!(
            response["id"], id,
            "the offending id comes back verbatim so the client can still resolve the call it \
             made: {response}"
        );
    }
}

/// The other half of the bound: what the specification *does* allow still works.
///
/// A bound that refuses more than it was asked to is the same defect wearing the
/// opposite sign. An empty string and a negative integer are both legal ids.
#[tokio::test]
async fn a_legal_id_is_still_served() {
    let store = corpus();
    for id in [
        json!(0),
        json!(-1),
        json!(9_007_199_254_740_993_i64),
        json!(""),
    ] {
        let frame = json!({"jsonrpc": "2.0", "id": id, "method": "ping"}).to_string();
        let response = handle_line(&store, &frame).await.expect("answered");
        assert!(
            response.get("result").is_some(),
            "{id} is a legal MCP request id and must be served: {response}"
        );
        assert_eq!(response["id"], id, "{response}");
    }
}

// ---------------------------------------------------------------------------
// Per-request protocol version — the modern era, on stdio
// ---------------------------------------------------------------------------

/// DEFECT: stdio never reads the protocol version a modern request declares.
///
/// `2026-07-28` → Versioning: "If the server does not implement the requested
/// version (whether the version is unknown to the server, or is a known version
/// the server has chosen not to support), it **MUST** respond with an
/// `UnsupportedProtocolVersionError` listing the versions it does support."
///
/// This server answers `server/discover` on stdio — deliberately, because the
/// stdio binding tells a dual-era client to probe with it before anything else.
/// So a modern client reaches stdio, is told this server is modern, and then
/// every version it declares is ignored: a request stating `1900-01-01` is
/// served exactly like one stating `2026-07-28`.
///
/// WRONG ANSWER A CALLER GETS: an answer computed under a revision the server
/// never agreed to, with no way for the client to discover the disagreement. The
/// HTTP transport gets this right; stdio, which is the transport agent hosts
/// actually launch, does not.
#[tokio::test]
async fn a_modern_request_naming_an_unserved_version_is_refused_with_recovery_data() {
    let store = corpus();
    let frame = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": {"_meta": {
            "io.modelcontextprotocol/protocolVersion": "1900-01-01",
            "io.modelcontextprotocol/clientCapabilities": {},
        }},
    })
    .to_string();
    let response = handle_line(&store, &frame).await.expect("answered");
    assert_eq!(
        response["error"]["code"],
        json!(-32022),
        "UnsupportedProtocolVersion, not a generic invalid-request and not silence: {response}"
    );
    assert_eq!(
        response["error"]["data"]["supported"],
        json!(MODERN_PROTOCOL_VERSIONS),
        "the refusal must carry the list the client is told to retry from; without it the \
         documented recovery path does not exist: {response}"
    );
    assert_eq!(
        response["error"]["data"]["requested"],
        json!("1900-01-01"),
        "and echo what was asked for: {response}"
    );
}

/// A handshake revision named through the *modern* mechanism is still refused,
/// and the refusal says where that revision does live.
///
/// `supportedVersions` is the set of versions a client may put in
/// `_meta["io.modelcontextprotocol/protocolVersion"]`, not the set of versions
/// this process can speak at all. Listing `2025-11-25` there would tell a client
/// it may state that version per-request, which is a negotiation the handshake
/// era does not have.
#[tokio::test]
async fn a_handshake_revision_named_through_modern_meta_is_refused_and_says_where_it_lives() {
    let store = corpus();
    let frame = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": {"_meta": {
            "io.modelcontextprotocol/protocolVersion": "2025-11-25",
            "io.modelcontextprotocol/clientCapabilities": {},
        }},
    })
    .to_string();
    let response = handle_line(&store, &frame).await.expect("answered");
    assert_eq!(response["error"]["code"], json!(-32022), "{response}");
    let message = response["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .to_ascii_lowercase();
    assert!(
        message.contains("initialize"),
        "a client that named a handshake revision has somewhere to go, and the refusal is the \
         only place it will be told: {message}"
    );
}

/// DEFECT: `clientCapabilities` is required by the revision and unread on stdio.
///
/// `2026-07-28` → `_meta`: `io.modelcontextprotocol/clientCapabilities` is
/// marked **Required** on every client request, and "A request missing any
/// required field is malformed; the server **MUST** reject it with JSON-RPC
/// error code `-32602` (Invalid params)."
#[tokio::test]
async fn a_modern_request_must_declare_its_client_capabilities() {
    let store = corpus();
    let frame = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": {"_meta": {
            "io.modelcontextprotocol/protocolVersion": MODERN_PROTOCOL_VERSIONS[0],
        }},
    })
    .to_string();
    let response = handle_line(&store, &frame).await.expect("answered");
    assert_eq!(
        response["error"]["code"],
        json!(-32602),
        "a missing required `_meta` field is Invalid params: {response}"
    );
}

/// DEFECT: `initialize` is answered under an envelope that has no `initialize`.
///
/// The handshake was removed in `2026-07-28`; `server/discover` replaces it. A
/// request that declares `2026-07-28` in `_meta` and calls `initialize` is
/// currently answered with `protocolVersion: "2025-11-25"` — this server telling
/// a client that just stated one revision that the connection is now running
/// under a different one, over a mechanism that has no connection state to run
/// under.
#[tokio::test]
async fn initialize_is_not_a_method_of_the_modern_era() {
    let store = corpus();
    let frame = modern("initialize", json!({"protocolVersion": "2025-11-25"}));
    let response = handle_line(&store, &frame).await.expect("answered");
    assert_eq!(
        response["error"]["code"],
        json!(-32601),
        "the modern era has no `initialize`; answering it invents a negotiation: {response}"
    );
}

/// The regression the era rule must not cause: a handshake client is untouched.
///
/// A request with no `io.modelcontextprotocol/protocolVersion` in `_meta` is a
/// handshake-era request and is served as one — including `initialize` itself,
/// and including `server/discover`, which the stdio binding tells a dual-era
/// client to send as its very first frame, before it knows what to declare.
#[tokio::test]
async fn a_request_without_modern_meta_is_served_under_the_handshake_era() {
    let store = corpus();

    let response = handle_line(
        &store,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#,
    )
    .await
    .expect("answered");
    assert_eq!(
        response["result"]["protocolVersion"],
        json!("2025-06-18"),
        "a handshake request still negotiates a handshake revision: {response}"
    );

    let response = handle_line(
        &store,
        r#"{"jsonrpc":"2.0","id":2,"method":"server/discover"}"#,
    )
    .await
    .expect("answered");
    assert_eq!(
        response["result"]["supportedVersions"],
        json!(MODERN_PROTOCOL_VERSIONS),
        "the probe must answer even when the prober does not yet know what to declare: \
         {response}"
    );

    // An empty `_meta`, and a `_meta` carrying only keys this server does not
    // own, are both handshake-era requests: neither states a version.
    for meta in [
        json!({}),
        json!({"progressToken": 7, "com.example/tag": "x"}),
    ] {
        let frame = json!({"jsonrpc":"2.0","id":3,"method":"tools/list","params":{"_meta": meta}})
            .to_string();
        let response = handle_line(&store, &frame).await.expect("answered");
        assert!(
            response["result"]["tools"].is_array(),
            "a `_meta` that states no version states no era: {response}"
        );
    }
}

/// A modern request that *is* well-formed is served, not refused.
#[tokio::test]
async fn a_well_formed_modern_request_is_served() {
    let store = corpus();
    let response = handle_line(&store, &modern("tools/list", json!({})))
        .await
        .expect("answered");
    assert!(
        response["result"]["tools"].is_array(),
        "a conforming modern request must be answered: {response}"
    );
    assert_eq!(
        response["result"]["resultType"],
        json!("complete"),
        "{response}"
    );
}

/// A version stated as something other than a string is malformed, and saying so
/// is not the same as saying it is unsupported.
///
/// `-32602` (the field is the wrong shape) and `-32022` (the field is a version
/// we do not serve) send a client down different recovery paths: one is "fix the
/// request", the other is "retry with one of these". Collapsing them would tell a
/// client to retry a version it never successfully stated.
#[tokio::test]
async fn a_protocol_version_that_is_not_a_string_is_malformed_not_unsupported() {
    let store = corpus();
    for stated in [json!(20_260_728), json!(null), json!(["2026-07-28"])] {
        let frame = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": {"_meta": {"io.modelcontextprotocol/protocolVersion": stated}},
        })
        .to_string();
        let response = handle_line(&store, &frame).await.expect("answered");
        assert_eq!(
            response["error"]["code"],
            json!(-32602),
            "a version field of the wrong type is Invalid params, not an unsupported version: \
             {stated} -> {response}"
        );
    }
}

// ---------------------------------------------------------------------------
// Output schemas
// ---------------------------------------------------------------------------

/// DEFECT: every tool returns `structuredContent` and none declares what shape it
/// is.
///
/// `2026-07-28` → Tools → Structured Content: structured content is "a JSON
/// value ... that conforms to the tool's `outputSchema` if one is defined", and
/// where one is defined "Clients **SHOULD** validate structured results against
/// this schema".
///
/// Undeclared, the field is unvalidatable: a client cannot check it, and the
/// incompleteness markers this server's whole honesty contract rests on —
/// `truncated`, `walk_incomplete`, `shown`/`hidden`/`total` — are invisible to
/// anything that reads the tool list to decide what a result will contain.
#[test]
fn every_tool_declares_the_output_schema_its_results_are_validated_against() {
    let specs = tool_specs();
    assert_eq!(
        specs.len(),
        TOOL_NAMES.len(),
        "the published list and the declared list must be the same list"
    );
    for spec in &specs {
        let name = spec["name"].as_str().unwrap_or("<unnamed>");
        let schema = &spec["outputSchema"];
        assert!(
            schema.is_object(),
            "{name} returns structuredContent and declares no outputSchema, so nothing can \
             check it: {spec}"
        );
        assert_eq!(
            schema["type"],
            json!("object"),
            "{name}: every tool here answers with a JSON object"
        );
        assert!(
            schema["properties"].is_object(),
            "{name}: a schema with no properties describes nothing"
        );
    }
}

/// The declaration is only worth having if the server is held to it.
///
/// "Servers **MUST** provide structured results that conform to this schema."
/// This runs every declared tool against a real index and checks the answer
/// against the schema that tool published — the same check the server applies
/// before it emits, reached through the same function, so the promise and the
/// enforcement cannot drift apart.
///
/// Both numbers are carried: a run that checked fewer tools than are declared is
/// reported as a gap rather than as a pass.
#[tokio::test]
async fn every_tool_result_conforms_to_the_output_schema_it_declared() {
    let store = corpus();
    let arguments: &[(&str, Value)] = &[
        ("devmap_status", json!({})),
        ("devmap_search", json!({"query": "helper"})),
        ("devmap_dependencies", json!({"target": "run"})),
        ("devmap_impact", json!({"target": "helper"})),
        ("devmap_trace", json!({"from": "run"})),
        ("devmap_neighbors", json!({"targets": ["helper"]})),
        ("devmap_dead_symbols", json!({})),
        ("devmap_clones", json!({})),
        (
            "devmap_preview",
            json!({"file": "core.py", "content": "def helper(rows):\n    return 0\n"}),
        ),
        ("devmap_explore", json!({"query": "helper"})),
        ("devmap_affected_tests", json!({"targets": ["helper"]})),
    ];
    assert_eq!(
        arguments.len(),
        TOOL_NAMES.len(),
        "this test must exercise every declared tool; {} declared, {} exercised",
        TOOL_NAMES.len(),
        arguments.len()
    );

    let mut checked = 0usize;
    for (tool, args) in arguments {
        let frame = json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": tool, "arguments": args},
        })
        .to_string();
        let response = handle_line(&store, &frame).await.expect("answered");
        let result = &response["result"];
        assert_eq!(
            result["isError"],
            json!(false),
            "{tool} could not run against the corpus, so this test proved nothing about its \
             schema: {response}"
        );
        let structured = &result["structuredContent"];
        assert!(
            !structured.is_null(),
            "{tool} answered without structuredContent: {response}"
        );
        if let Some(violation) = structured_content_violation(tool, structured) {
            panic!("{tool} answered outside its declared outputSchema: {violation}\n{response}");
        }
        checked += 1;
    }
    assert_eq!(
        checked,
        TOOL_NAMES.len(),
        "{checked} of {} tools were actually validated",
        TOOL_NAMES.len()
    );
}

/// The validator has to be able to fail, or the test above is a tautology.
#[test]
fn the_output_schema_check_rejects_a_result_that_does_not_conform() {
    assert!(
        structured_content_violation("devmap_search", &json!({"items": []})).is_some(),
        "a `Response` envelope missing every count but `items` must be caught"
    );
    assert!(
        structured_content_violation("devmap_search", &json!([])).is_some(),
        "an array where the schema says object must be caught"
    );
    assert!(
        structured_content_violation("devmap_status", &json!({"node_count": "many"})).is_some(),
        "a count that is not a number must be caught"
    );
    assert!(
        structured_content_violation("no_such_tool", &json!({})).is_some(),
        "a tool with no declared schema cannot be reported as conforming to one"
    );
}

// ---------------------------------------------------------------------------
// Bounds
// ---------------------------------------------------------------------------

/// A pipeline deeper than the bound is slowed, never shed.
///
/// STRUCTURAL GUARD, not a reproduction: the defect it covers — `serve_streams`
/// spawning one unbounded task per frame, with nothing reaping them until they
/// finished on their own — is memory growth *inside* the server, and the wire
/// cannot show it. What this asserts is the part that is observable and that the
/// bound must not break: a pipeline four times the ceiling comes back complete,
/// every id exactly once and every one a result.
///
/// That last clause was decided by an existing test, not by taste. The first
/// version of this bound *shed* requests over the ceiling and named the ceiling
/// in the refusal, which read as honest; `concurrent_requests_never_interleave_or_lose_an_id`
/// then failed, because it pipelines 200 requests and requires 200 results. A
/// bound that discards a well-formed request from a client doing nothing wrong
/// is data loss wearing a good error message, so the bound became backpressure:
/// past the ceiling the read loop waits for the oldest task instead of reading
/// another frame.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pipeline_deeper_than_the_in_flight_bound_answers_every_id_exactly_once() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

    let store = corpus();
    let depth = MAX_IN_FLIGHT_REQUESTS * 4;
    let mut sent = String::new();
    for id in 0..depth {
        sent.push_str(
            &json!({
                "jsonrpc": "2.0", "id": id, "method": "tools/call",
                "params": {"name": "devmap_search", "arguments": {"query": "helper"}},
            })
            .to_string(),
        );
        sent.push('\n');
    }

    let (client, server) = tokio::io::duplex(16 * 1024 * 1024);
    let (server_read, server_write) = tokio::io::split(server);
    let task = tokio::spawn(serve_streams(
        store,
        tokio::io::BufReader::new(server_read),
        server_write,
    ));
    let (client_read, mut client_write) = tokio::io::split(client);
    client_write
        .write_all(sent.as_bytes())
        .await
        .expect("write");
    client_write.shutdown().await.expect("shutdown");

    let mut frames = Vec::new();
    let mut lines = tokio::io::BufReader::new(client_read).lines();
    while let Ok(Some(line)) =
        tokio::time::timeout(std::time::Duration::from_secs(60), lines.next_line())
            .await
            .expect("the server must not stop answering")
    {
        if line.trim().is_empty() {
            continue;
        }
        frames.push(serde_json::from_str::<Value>(&line).expect("every frame is JSON"));
    }
    let _ = tokio::time::timeout(std::time::Duration::from_secs(30), task).await;

    let mut seen: Vec<u64> = frames
        .iter()
        .map(|frame| frame["id"].as_u64().expect("every frame carries its id"))
        .collect();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(
        seen.len(),
        depth,
        "{} of {depth} pipelined requests were answered exactly once",
        seen.len()
    );

    let refused: Vec<&Value> = frames
        .iter()
        .filter(|frame| frame.get("error").is_some())
        .collect();
    assert!(
        refused.is_empty(),
        "{} of {depth} pipelined requests came back as errors; backpressure delays a request, \
         it does not discard one: {:?}",
        refused.len(),
        refused.first()
    );
    eprintln!("pipeline depth {depth} (bound {MAX_IN_FLIGHT_REQUESTS}): all answered, none shed");
}

/// DEFECT: `devmap_neighbors` declares a budget it multiplies by up to 32.
///
/// `budget_prop` describes the field as "Token budget for the answer", and for
/// every other tool that is what it is. `neighbors` hands the same number to
/// `traverse_over` once per target and once per direction, so a caller that asks
/// for 2000 tokens across 16 targets can be handed 64,000 — the same shape as
/// the depth default this tool already got wrong once
/// (`defect_neighbors_declares_a_depth_default_it_does_not_use`).
///
/// WRONG ANSWER A CALLER GETS: a budget is the one control an agent has over how
/// much of its context a tool call will consume, and this one is off by up to
/// 32x in the direction that overruns it. The engine's behaviour is not the
/// defect here — a per-target budget is the right semantics for a fan-out — the
/// declaration is, because it is the only thing the caller can read.
#[test]
fn the_neighbors_budget_declaration_states_the_fan_out_it_multiplies_by() {
    let specs = tool_specs();
    let neighbors = specs
        .iter()
        .find(|spec| spec["name"] == json!("devmap_neighbors"))
        .expect("devmap_neighbors is declared");
    let described = neighbors["inputSchema"]["properties"]["budget"]["description"]
        .as_str()
        .unwrap_or_default();
    let lowered = described.to_ascii_lowercase();
    assert!(
        lowered.contains("per target"),
        "the budget is spent once per target and the description does not say so: {described}"
    );
    assert!(
        lowered.contains("per direction") || lowered.contains("each direction"),
        "it is also spent once per direction, doubling again: {described}"
    );
    assert!(
        described.contains("32"),
        "a caller sizing its context needs the worst-case multiplier, not just the shape of \
         the fan-out: {described}"
    );

    // And the other tools must not have grown the disclosure by accident: for
    // them the budget really is the budget, and saying otherwise would be the
    // same defect pointing the other way.
    for spec in &specs {
        if spec["name"] == json!("devmap_neighbors") {
            continue;
        }
        let Some(description) = spec["inputSchema"]["properties"]["budget"]["description"].as_str()
        else {
            continue;
        };
        assert!(
            !description.to_ascii_lowercase().contains("per target"),
            "{} does not fan out, so its budget must not claim to: {description}",
            spec["name"]
        );
    }
}

/// The write side is bounded by the same rule the read side is.
///
/// STRUCTURAL GUARD, not a reproduction. The condition it covers is real and
/// measured — `devmap_neighbors` spends its budget once per target and once per
/// direction, so 16 targets at the declared maximum budget of 100,000 tokens is
/// 16 x 2 x 100,000 x 4 bytes = 12.8 MB of payload and roughly 26 MB of result,
/// against a 1 MB limit on anything this server will *read* — but producing it
/// needs an index with tens of thousands of edges, which is not something a test
/// can build cheaply. So the bound is asserted through the same function the
/// server calls before it emits, on a result built to be over it.
///
/// What must hold, and is checked here: the refusal is a refusal, it carries the
/// measured size and the limit, and it names the parameter to change. Never a
/// truncation — a cut answer reads exactly like a complete one.
#[test]
fn a_result_too_large_to_send_is_refused_with_its_measurements() {
    let small = json!({"content": [], "structuredContent": {"items": []}, "isError": false});
    assert!(
        oversized_result_refusal("devmap_search", &small).is_none(),
        "an ordinary answer must not be refused"
    );

    // Nine megabytes of payload, over the eight-megabyte ceiling.
    let filler = "x".repeat(9 * 1024 * 1024);
    let large = json!({
        "content": [{"type": "text", "text": ""}],
        "structuredContent": {"items": [filler]},
        "isError": false,
    });
    let refusal = oversized_result_refusal("devmap_neighbors", &large)
        .expect("a result over the ceiling must be refused, not sent");
    assert!(
        refusal.contains(&(8 * 1024 * 1024).to_string()),
        "the refusal must name the limit that was hit: {refusal}"
    );
    assert!(
        refusal.contains("budget"),
        "and the parameter the caller can change: {refusal}"
    );
    assert!(
        refusal.contains("per target"),
        "for the fan-out tool it must also name the multiplication, or the caller lowers the \
         budget by the wrong factor: {refusal}"
    );
    assert!(
        !refusal.to_ascii_lowercase().contains("truncat")
            || refusal.contains("truncated answer is worse"),
        "the answer is withheld, not cut: {refusal}"
    );
}

/// Every code this server can emit sits where the specification allows it.
///
/// `2026-07-28` partitions the JSON-RPC implementation-defined band:
/// `-32000`..`-32019` is retired ("new implementations **SHOULD NOT** use codes
/// from this sub-range at all"), and `-32020`..`-32099` belongs to the
/// specification ("Implementations **MUST NOT** emit any code from this
/// sub-range that is not defined by this specification"). Anything an
/// implementation invents for itself **SHOULD** be "allocated outside the
/// JSON-RPC reserved range (`-32768` to `-32000`)".
///
/// This server invents nothing: every refusal it produces is a standard
/// JSON-RPC code or one of the two the specification defines. The guard is here
/// for the next refusal someone adds, because reaching for a spare-looking
/// `-32000` would be silently non-conforming — nothing would fail, and a client
/// would read a number the specification says carries no agreed meaning.
#[test]
fn the_codes_this_server_emits_stay_out_of_the_reserved_bands() {
    // The two the specification does define, used only with their meanings.
    assert_eq!(codes::HEADER_MISMATCH, -32020);
    assert_eq!(codes::UNSUPPORTED_PROTOCOL_VERSION, -32022);

    // And the guard that keeps a future refusal out of the specification's band.
    for retired in [-32000_i64, -32002, -32019] {
        assert!(
            codes::is_reserved_and_undefined(retired),
            "{retired} is in the retired sub-range and must be rejected"
        );
    }
    for undefined in [-32021_i64, -32023, -32099] {
        assert!(
            codes::is_reserved_and_undefined(undefined),
            "{undefined} is in the range the specification keeps for itself and does not define \
             for us"
        );
    }
    for allowed in [
        codes::HEADER_MISMATCH,
        codes::UNSUPPORTED_PROTOCOL_VERSION,
        codes::PARSE_ERROR,
        codes::INVALID_REQUEST,
        codes::METHOD_NOT_FOUND,
        codes::INVALID_PARAMS,
        codes::INTERNAL_ERROR,
    ] {
        assert!(
            !codes::is_reserved_and_undefined(allowed),
            "{allowed} is a code this server legitimately emits"
        );
    }
}

/// A reused in-flight id is refused, and refused as the client's own violation.
///
/// STRUCTURAL GUARD with an observed-count disclosure, not a deterministic
/// reproduction. The condition needs two requests carrying one id to be running
/// at the same instant, and nothing on the wire can force that interleaving — so
/// this drives a same-id pipeline and asserts the *classification* of whatever
/// refusals occur, then reports how many it actually saw. A run that observed
/// none has checked nothing about the refusal, and says so rather than passing
/// quietly as if it had.
///
/// What is asserted unconditionally: every frame comes back under the id it was
/// sent with, and no frame is a duplicate-id refusal wearing the overload code
/// (which would tell a client to wait when what it must do is renumber).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reused_in_flight_id_is_the_clients_fault_and_says_so() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

    let store = corpus();
    let depth = 128;
    let mut sent = String::new();
    for _ in 0..depth {
        sent.push_str(
            &json!({
                "jsonrpc": "2.0", "id": "same", "method": "tools/call",
                "params": {"name": "devmap_search", "arguments": {"query": "helper"}},
            })
            .to_string(),
        );
        sent.push('\n');
    }

    let (client, server) = tokio::io::duplex(16 * 1024 * 1024);
    let (server_read, server_write) = tokio::io::split(server);
    let task = tokio::spawn(serve_streams(
        store,
        tokio::io::BufReader::new(server_read),
        server_write,
    ));
    let (client_read, mut client_write) = tokio::io::split(client);
    client_write
        .write_all(sent.as_bytes())
        .await
        .expect("write");
    client_write.shutdown().await.expect("shutdown");

    let mut frames = Vec::new();
    let mut lines = tokio::io::BufReader::new(client_read).lines();
    while let Ok(Some(line)) =
        tokio::time::timeout(std::time::Duration::from_secs(60), lines.next_line())
            .await
            .expect("the server must not stop answering")
    {
        if line.trim().is_empty() {
            continue;
        }
        frames.push(serde_json::from_str::<Value>(&line).expect("every frame is JSON"));
    }
    let _ = tokio::time::timeout(std::time::Duration::from_secs(30), task).await;

    assert_eq!(
        frames.len(),
        depth,
        "{} of {depth} frames came back; every request is owed exactly one",
        frames.len()
    );
    let mut duplicates = 0usize;
    for frame in &frames {
        assert_eq!(frame["id"], json!("same"), "the id must be echoed: {frame}");
        let Some(code) = frame["error"]["code"].as_i64() else {
            continue;
        };
        if code == codes::INVALID_REQUEST {
            let message = frame["error"]["message"].as_str().unwrap_or_default();
            if message.contains("already names a request") {
                duplicates += 1;
                assert!(
                    message.contains("unique"),
                    "the refusal must say what rule was broken: {frame}"
                );
            }
        }
        assert_eq!(
            code,
            codes::INVALID_REQUEST,
            "the only refusal a distinct-method pipeline can produce here is the reused id, and \
             it is the client's own violation to fix — not a condition to wait out: {frame}"
        );
    }
    eprintln!(
        "same-id pipeline of {depth}: {duplicates} collisions observed and classified; 0 would \
         mean this run exercised the refusal not at all"
    );
}

/// DEFECT: a cancellation carrying an id is acted on and never answered.
///
/// "Notifications **MUST NOT** include an ID", so a frame that has one is not a
/// notification however its method is spelled — and this module states the rule
/// itself, in `handle_method_cancellable`: "a `notifications/initialized` that
/// arrives carrying an id is a request, and a request is owed a response frame
/// whatever its method name suggests."
///
/// `notifications/cancelled` was the one method that did not obey it. It is
/// intercepted before the id is ever consulted, so a frame carrying both an id
/// and that method cancels whatever it names *and* returns nothing.
///
/// WRONG ANSWER A CALLER GETS: none, forever. The client holds an id it will
/// never see resolved, on a connection that is otherwise healthy — the exact
/// hang this file's sibling `every_request_gets_exactly_one_frame` exists to
/// prevent, reachable through the one method that skipped the check.
#[tokio::test]
async fn a_cancellation_that_carries_an_id_is_a_request_and_is_answered() {
    let store = corpus();
    let response = handle_line(
        &store,
        r#"{"jsonrpc":"2.0","id":42,"method":"notifications/cancelled","params":{"requestId":1}}"#,
    )
    .await
    .expect("a frame carrying an id must be answered, whatever its method is called");
    assert_eq!(response["id"], json!(42), "{response}");

    // And a real cancellation — which has no id — is still silent and still
    // acted on, or this fix would have traded a hang for a broken cancellation.
    assert!(
        handle_line(
            &store,
            r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}}"#,
        )
        .await
        .is_none(),
        "a well-formed cancellation is a notification and must not be answered"
    );
}
