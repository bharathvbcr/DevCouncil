//! The MCP 2.0 (`2026-07-28`) single-exchange HTTP transport.
//!
//! Driven over a real TCP socket on port 0 rather than by calling the handler
//! directly: the things most likely to be wrong here are envelope-level — status
//! codes, header handling, body limits — and none of them exist below the
//! socket. A test that calls `handle_request` in-process would pass while the
//! server crashed on every connection, which is exactly what happened once
//! during development (hyper panics at setup if a read timeout is configured
//! with no timer to drive it).

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use devmap_serve::mcp::StoreSlot;
use devmap_store::Store;
use serde_json::{json, Value};
use tokio::net::TcpListener;

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

/// Start a server on an ephemeral port and return its address.
///
/// Port 0, never a hardcoded port: a fixed port makes the suite fail on a
/// machine that happens to be using it, which reads as a code failure.
async fn start() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("addr").to_string();
    tokio::spawn(devmap_serve::mcp_http::serve_http_on(corpus(), listener));
    address
}

/// A minimal HTTP/1.1 client.
///
/// Hand-rolled rather than pulling in a client crate for a test: the requests are
/// one-shot, and building the bytes by hand is also what lets the malformed-input
/// tests below send things a client library would refuse to construct.
///
/// Returns the status, the header block and the body separately, because the
/// envelope tests below need each of the three and a helper that parsed the body
/// as JSON could not see a `202` with no body at all.
async fn exchange(address: &str, raw: &str) -> (u16, String, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(address)
        .await
        .expect("connect");
    stream.write_all(raw.as_bytes()).await.expect("write");
    stream.flush().await.expect("flush");
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.expect("read");
    let text = String::from_utf8_lossy(&response).to_string();

    let status: u16 = text
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or_else(|| panic!("no status line in response: {text}"));
    match text.split_once("\r\n\r\n") {
        Some((head, body)) => (status, head.to_string(), body.to_string()),
        None => (status, text, String::new()),
    }
}

async fn request(address: &str, raw: &str) -> (u16, Value) {
    let (status, _, body) = exchange(address, raw).await;
    let parsed = serde_json::from_str(&body)
        .unwrap_or_else(|err| panic!("body was not JSON ({err}): {body}"));
    (status, parsed)
}

/// The `_meta` fields the 2026-07-28 revision requires on every request.
///
/// Injected rather than written into each test body because they are not what
/// any of these tests is about: the revision replaced the `initialize` handshake
/// with per-request metadata, so a request without them is malformed for a
/// reason unrelated to whatever the test is probing. Existing values are kept —
/// the version-refusal test states its own and must keep stating it.
fn with_request_meta(body: &str) -> String {
    let Ok(mut parsed) = serde_json::from_str::<Value>(body) else {
        return body.to_string();
    };
    let Some(object) = parsed.as_object_mut() else {
        return body.to_string();
    };
    if let Some(params) = object
        .entry("params")
        .or_insert_with(|| json!({}))
        .as_object_mut()
    {
        if let Some(meta) = params
            .entry("_meta")
            .or_insert_with(|| json!({}))
            .as_object_mut()
        {
            meta.entry("io.modelcontextprotocol/protocolVersion")
                .or_insert_with(|| json!("2026-07-28"));
            meta.entry("io.modelcontextprotocol/clientCapabilities")
                .or_insert_with(|| json!({}));
        }
    }
    parsed.to_string()
}

/// The headers a conforming 2026-07-28 client mirrors out of the body.
///
/// Derived from the body rather than hardcoded, because the server's job is to
/// check that the two agree: a helper that stated a fixed `Mcp-Method` would
/// send a mismatched request on every test that is not `ping`.
fn mirrored_headers(body: &str) -> Vec<(String, String)> {
    let parsed: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let version = parsed
        .pointer("/params/_meta/io.modelcontextprotocol~1protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or("2026-07-28");
    let mut headers = vec![("MCP-Protocol-Version".to_string(), version.to_string())];
    if let Some(method) = parsed.get("method").and_then(Value::as_str) {
        headers.push(("Mcp-Method".to_string(), method.to_string()));
    }
    if let Some(name) = parsed.pointer("/params/name").and_then(Value::as_str) {
        headers.push(("Mcp-Name".to_string(), name.to_string()));
    }
    headers
}

fn raw_post(body: &str, headers: &[(String, String)]) -> String {
    let mut head = String::from(
        "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\n",
    );
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    format!(
        "{head}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn post(body: &str) -> String {
    let body = with_request_meta(body);
    let headers = mirrored_headers(&body);
    raw_post(&body, &headers)
}

/// `server/discover` replaces `initialize` in the modern era, and this is the
/// one place `ttlMs`/`cacheScope` actually reach a client.
#[tokio::test]
async fn discover_advertises_the_modern_revision_with_live_cache_hints() {
    let address = start().await;
    let (status, body) = request(
        &address,
        &post(r#"{"jsonrpc":"2.0","id":1,"method":"server/discover"}"#),
    )
    .await;
    assert_eq!(status, 200);
    let result = &body["result"];
    assert_eq!(result["supportedVersions"], json!(["2026-07-28"]));
    assert_eq!(
        result["ttlMs"],
        json!(300_000),
        "the cache hint must reach the client on this transport; on the handshake \
         transports it is sieved out by every version they can negotiate"
    );
    assert_eq!(result["cacheScope"], json!("private"));
    assert_eq!(result["capabilities"]["tools"]["listChanged"], json!(false));
}

/// `tools/list` is in the spec's cacheable set, so it carries the hint too.
/// Nothing else does — an un-annotated result must stay uncacheable.
#[tokio::test]
async fn only_cacheable_methods_carry_cache_hints() {
    let address = start().await;

    let (_, listed) = request(
        &address,
        &post(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#),
    )
    .await;
    assert_eq!(listed["result"]["ttlMs"], json!(300_000));

    let (_, pinged) = request(
        &address,
        &post(r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#),
    )
    .await;
    assert!(
        pinged["result"].get("ttlMs").is_none(),
        "ping is not cacheable and must not be advertised as such"
    );
}

/// The spec makes the error-code-to-status mapping a MUST.
///
/// Collapsing everything to 200 would make "this server does not have that
/// method" indistinguishable from "that method failed", and a client picks its
/// retry behaviour from that difference.
#[tokio::test]
async fn error_codes_map_to_the_required_http_statuses() {
    let address = start().await;

    let (status, body) = request(
        &address,
        &post(r#"{"jsonrpc":"2.0","id":1,"method":"resources/list"}"#),
    )
    .await;
    assert_eq!(status, 404, "an unimplemented method is 404");
    assert_eq!(body["error"]["code"], json!(-32601));

    let (status, _) = request(&address, &post("{not json")).await;
    assert_eq!(status, 400, "a parse error is 400");

    let (status, body) = request(
        &address,
        &post(r#"{"jsonrpc":"1.0","id":1,"method":"ping"}"#),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(body["error"]["code"], json!(-32600));
}

/// A stated protocol version this transport does not serve is refused, not
/// answered. Answering would mean guessing which surface the client expects.
#[tokio::test]
async fn a_handshake_era_version_stated_here_is_refused() {
    let address = start().await;
    let (status, body) = request(
        &address,
        &post(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2025-11-25"}}}"#,
        ),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(
        body["error"]["code"],
        json!(-32022),
        "UNSUPPORTED_PROTOCOL_VERSION, not a generic invalid-request"
    );
    assert_eq!(
        body["id"],
        json!(1),
        "the id must survive so the client can match the failure"
    );
}

/// DNS-rebinding protection. A page that resolves a hostname to 127.0.0.1 and
/// POSTs from JavaScript would otherwise read the user's private code graph.
#[tokio::test]
async fn a_browser_origin_is_refused() {
    let address = start().await;
    let body = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
    let raw = format!(
        "POST / HTTP/1.1\r\nHost: localhost\r\nOrigin: https://evil.example\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let (status, _) = request(&address, &raw).await;
    assert_eq!(
        status, 403,
        "a request carrying a site Origin must be refused"
    );
}

/// A non-browser client sends no Origin, and must keep working.
#[tokio::test]
async fn an_absent_origin_is_accepted() {
    let address = start().await;
    let (status, _) = request(
        &address,
        &post(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#),
    )
    .await;
    assert_eq!(status, 200);
}

#[tokio::test]
async fn a_get_is_refused_and_says_which_transport_this_is() {
    let address = start().await;
    let (status, body) = request(
        &address,
        "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert_eq!(status, 405);
    let message = body["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("2026-07-28"),
        "the refusal must name the transport so a client reaching for SSE knows why: {message}"
    );
}

/// An oversized body is refused on the declared length, before it is buffered.
#[tokio::test]
async fn an_oversized_declared_body_is_refused() {
    let address = start().await;
    let raw = "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 2097152\r\nConnection: close\r\n\r\n{}";
    let (status, _) = request(&address, raw).await;
    assert_eq!(
        status, 413,
        "an over-limit Content-Length must be refused without buffering the body"
    );
}

/// THE transport-parity test.
///
/// The server's central claim is "three transports, one answer". If HTTP and
/// stdio can disagree on the same question, the claim is false and one of the
/// two is lying to an agent. Compared on the `result` payload only: the
/// envelopes legitimately differ (HTTP adds cache hints to cacheable methods).
#[tokio::test]
async fn http_and_stdio_answer_the_same_question_identically() {
    let address = start().await;
    let store = corpus();

    for (method, params) in [
        (
            "tools/call",
            json!({"name": "devmap_status", "arguments": {}}),
        ),
        (
            "tools/call",
            json!({"name": "devmap_impact", "arguments": {"target": "helper"}}),
        ),
        (
            "tools/call",
            json!({"name": "devmap_search", "arguments": {"query": "helper"}}),
        ),
        // An error path too: the two must agree on failure, not only success.
        (
            "tools/call",
            json!({"name": "devmap_search", "arguments": {"nonsuch": true}}),
        ),
    ] {
        let frame = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        let (_, over_http) = request(&address, &post(&frame.to_string())).await;
        let over_stdio = devmap_serve::mcp::handle_line(&store, &frame.to_string())
            .await
            .expect("stdio answers");

        assert_eq!(
            over_http["result"], over_stdio["result"],
            "the transports disagree on {method} {}",
            frame["params"]
        );
    }
}

/// The full response text, headers included.
///
/// `request` parses the body as JSON and throws the headers away, which is
/// precisely what the test below has to look at.
async fn raw_exchange(address: &str, raw: &str) -> String {
    let (_, head, body) = exchange(address, raw).await;
    format!("{head}\r\n\r\n{body}")
}

/// The two properties that keep a browser out, pinned together.
///
/// `origin_is_acceptable` lets `Origin: null` through, and `null` is what a
/// sandboxed iframe, a `data:` URL and a `file://` page all send — so the origin
/// check alone does not stop a browser. What stops it is that a JSON POST is a
/// non-simple request, so a browser must first preflight with `OPTIONS` and that
/// is refused; and that no `Access-Control-Allow-*` header is ever emitted, so
/// even a response that did come back is unreadable cross-origin.
///
/// Neither property is obviously load-bearing when read from its own call site,
/// which is how one of them gets removed — adding an `OPTIONS` handler to be
/// helpful, or a permissive CORS header to make some dashboard work, silently
/// reopens the DNS-rebinding hole the origin check exists to close. This test
/// fails on either.
#[tokio::test]
async fn a_browser_cannot_reach_this_endpoint_even_with_a_null_origin() {
    let address = start().await;

    let preflight = raw_exchange(
        &address,
        "OPTIONS / HTTP/1.1\r\nHost: localhost\r\nOrigin: https://evil.example\r\n\
         Access-Control-Request-Method: POST\r\n\
         Access-Control-Request-Headers: content-type\r\nConnection: close\r\n\r\n",
    )
    .await;
    let status: u16 = preflight
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or_else(|| panic!("no status line: {preflight}"));
    assert!(
        status == 405 || status == 403 || status == 400,
        "a CORS preflight must be refused, or the browser goes on to send the real \
         request; got {status}\n{preflight}"
    );

    // A sandboxed page's own origin. Accepted by design — the point is that the
    // response still tells the browser it may not be read.
    let null_origin = raw_exchange(
        &address,
        &format!(
            "POST / HTTP/1.1\r\nHost: localhost\r\nOrigin: null\r\n\
             Content-Type: application/json\r\nContent-Length: {}\r\n\
             Connection: close\r\n\r\n{}",
            r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#.len(),
            r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#
        ),
    )
    .await;

    for (label, response) in [("preflight", &preflight), ("null-origin", &null_origin)] {
        let headers = response
            .split_once("\r\n\r\n")
            .map_or(response.as_str(), |(head, _)| head);
        assert!(
            !headers.to_ascii_lowercase().contains("access-control-"),
            "this endpoint must never grant CORS, or a page can read the user's private \
             code graph. {label} response carried one:\n{headers}"
        );
    }
}

// ---------------------------------------------------------------------------
// Envelope conformance: the parts of `2026-07-28` that live in the HTTP layer
// and therefore cannot be checked anywhere else.
// ---------------------------------------------------------------------------

/// A notification is `202 Accepted` with no body.
///
/// "If the body is a JSON-RPC *notification*: If the server accepts it, the
/// server **MUST** return HTTP status code `202 Accepted` with no body."
///
/// Answering `200` with a JSON-RPC frame instead is not a cosmetic difference.
/// The frame this server sent carried `"id": null`, and the revision states the
/// id "**MUST NOT** be `null`" — so a client got a response object it is
/// required to reject, for a message it was never going to correlate.
#[tokio::test]
async fn a_notification_post_is_accepted_with_no_body() {
    let address = start().await;
    let (status, _, body) = exchange(
        &address,
        &post(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#),
    )
    .await;
    assert_eq!(status, 202, "a notification is 202, got {status}: {body}");
    assert!(
        body.trim().is_empty(),
        "a 202 carries no body; this one carried: {body}"
    );
}

/// The protocol version header is required on every request POST.
///
/// "Every POST request to the MCP endpoint **MUST** include an
/// `MCP-Protocol-Version` header", and a missing required standard header is
/// listed as a `HeaderMismatch` (`-32020`) validation failure. This server
/// serves one revision and nothing older, so it has no fallback to read a
/// header-less request under.
#[tokio::test]
async fn a_request_without_the_protocol_version_header_is_refused() {
    let address = start().await;
    let body = with_request_meta(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#);
    // Every mirrored header except the version one.
    let headers: Vec<(String, String)> = mirrored_headers(&body)
        .into_iter()
        .filter(|(name, _)| !name.eq_ignore_ascii_case("MCP-Protocol-Version"))
        .collect();
    let (status, response) = request(&address, &raw_post(&body, &headers)).await;
    assert_eq!(status, 400, "a missing required header is 400: {response}");
    assert_eq!(
        response["error"]["code"],
        json!(-32020),
        "HeaderMismatch names the missing header: {response}"
    );
    let message = response["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message
            .to_ascii_lowercase()
            .contains("mcp-protocol-version"),
        "the refusal must name the header the client has to add, got: {message}"
    );
}

/// Headers and body must agree, or an intermediary and this server are acting
/// on different requests.
///
/// "Servers that process the request body **MUST** reject requests where the
/// values specified in the headers do not match ... This prevents potential
/// security vulnerabilities when different components in the network rely on
/// different sources of truth (e.g., a load balancer routing on the header value
/// while the MCP server executes based on the body value)." A gateway that
/// allows `Mcp-Name: devmap_status` and forwards a body calling
/// `devmap_preview` is exactly that failure.
#[tokio::test]
async fn headers_that_contradict_the_body_are_refused() {
    let address = start().await;
    let body = with_request_meta(
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devmap_status","arguments":{}}}"#,
    );
    let cases = [
        ("MCP-Protocol-Version", "2025-11-25"),
        ("Mcp-Method", "tools/list"),
        ("Mcp-Name", "devmap_preview"),
    ];
    for (header, wrong) in cases {
        let headers: Vec<(String, String)> = mirrored_headers(&body)
            .into_iter()
            .map(|(name, value)| {
                if name.eq_ignore_ascii_case(header) {
                    (name, wrong.to_string())
                } else {
                    (name, value)
                }
            })
            .collect();
        let (status, response) = request(&address, &raw_post(&body, &headers)).await;
        assert_eq!(status, 400, "{header}: {response}");
        assert_eq!(
            response["error"]["code"],
            json!(-32020),
            "a header that disagrees with the body is HeaderMismatch, not a generic \
             invalid-request: {header} -> {response}"
        );
    }
}

/// An unsupported version must say what *is* supported.
///
/// `UnsupportedProtocolVersionError` carries a required
/// `data: { supported: string[], requested: string }`. Without it the client is
/// told no and given nothing to retry with — the spec's recovery path is
/// literally "use one of the versions in its advertised `supported` list".
#[tokio::test]
async fn an_unsupported_version_names_the_versions_that_would_work() {
    let address = start().await;
    let (status, response) = request(
        &address,
        &post(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2025-11-25"}}}"#,
        ),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(response["error"]["code"], json!(-32022), "{response}");
    assert_eq!(
        response["error"]["data"]["supported"],
        json!(["2026-07-28"]),
        "the error must list what the client can retry with: {response}"
    );
    assert_eq!(
        response["error"]["data"]["requested"],
        json!("2025-11-25"),
        "and echo what was asked for: {response}"
    );
}

/// A body that is not JSON is refused before it is dispatched.
///
/// Enforcing the content type is also half of what keeps a browser out. A POST
/// with `Content-Type: text/plain` is a CORS-*simple* request: no preflight, so
/// `Origin: null` from a sandboxed iframe or a `file://` page reaches the
/// handler and runs. The module's own safety argument — "a JSON body makes the
/// request non-simple, so a browser must preflight" — is only true if a
/// non-JSON content type is actually refused.
#[tokio::test]
async fn a_non_json_content_type_is_refused() {
    let address = start().await;
    let body = with_request_meta(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#);
    for content_type in [
        "text/plain",
        "application/x-www-form-urlencoded",
        "multipart/form-data",
    ] {
        let raw = raw_post(&body, &mirrored_headers(&body)).replace(
            "Content-Type: application/json",
            &format!("Content-Type: {content_type}"),
        );
        let (status, response) = request(&address, &raw).await;
        assert_eq!(
            status, 415,
            "{content_type} must be refused as an unsupported media type: {response}"
        );
    }
}

/// A client that cannot read what this endpoint sends is told so.
///
/// This transport answers with `application/json`. A request whose `Accept`
/// excludes it gets a body it said it could not parse — HTTP's answer to that is
/// `406`, and a client that asked for SSE needs to learn that this endpoint has
/// no stream rather than to receive JSON labelled as something it rejected.
#[tokio::test]
async fn an_accept_header_that_excludes_json_is_refused() {
    let address = start().await;
    let body = with_request_meta(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#);
    let raw = raw_post(&body, &mirrored_headers(&body)).replace(
        "Accept: application/json, text/event-stream",
        "Accept: text/event-stream",
    );
    let (status, response) = request(&address, &raw).await;
    assert_eq!(
        status, 406,
        "an Accept that excludes application/json must be refused: {response}"
    );
}

/// RFC 9110 §15.5.6: "The origin server MUST generate an Allow header field in a
/// 405 (Method Not Allowed) response".
///
/// Without it a client that got a 405 has to guess which verb to use, and the
/// spec's own backward-compatibility probe reaches this endpoint with GET.
#[tokio::test]
async fn a_405_states_which_method_is_allowed() {
    let address = start().await;
    for verb in ["GET", "DELETE", "OPTIONS", "PUT"] {
        let (status, head, _) = exchange(
            &address,
            &format!("{verb} / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"),
        )
        .await;
        assert_eq!(status, 405, "{verb}");
        let lowered = head.to_ascii_lowercase();
        assert!(
            lowered.contains("allow: post"),
            "a 405 must name the methods it allows; {verb} got:\n{head}"
        );
    }
}

/// DNS rebinding, second line.
///
/// The `Origin` check misses the case it was written for: a sandboxed iframe, a
/// `data:` URL and a `file://` page all send `Origin: null`, which this server
/// accepts. What is left to catch a rebound request is the `Host` header — a
/// page that resolved `evil.example` to 127.0.0.1 sends `Host: evil.example`,
/// and nothing can legitimately reach a loopback listener under a public name.
/// Checked only when the listener is on loopback, because a deployment that
/// binds a routable interface has a real hostname and this must not break it.
#[tokio::test]
async fn a_host_header_naming_a_public_name_is_refused_on_a_loopback_listener() {
    let address = start().await;
    let body = with_request_meta(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#);
    let raw =
        raw_post(&body, &mirrored_headers(&body)).replace("Host: localhost", "Host: evil.example");
    let (status, response) = request(&address, &raw).await;
    assert_eq!(
        status, 403,
        "a Host naming a routable name cannot legitimately reach 127.0.0.1: {response}"
    );

    // The loopback names a real client uses must keep working, or this check
    // costs more than it buys.
    for host in ["localhost", "127.0.0.1", &address, "[::1]:9"] {
        let raw = raw_post(&body, &mirrored_headers(&body))
            .replace("Host: localhost", &format!("Host: {host}"));
        let (status, response) = request(&address, &raw).await;
        assert_eq!(status, 200, "Host: {host} must be accepted: {response}");
    }
}

/// A batch is not a body this transport takes, and must say which fault it is.
///
/// "The body of the HTTP POST **MUST** be a single JSON-RPC *request* or
/// *notification*." Reporting an array as "request has no string 'method'"
/// sends the client looking for a missing field in a body that has no fields.
#[tokio::test]
async fn a_batch_body_is_refused_by_name() {
    let address = start().await;
    let batch = r#"[{"jsonrpc":"2.0","id":1,"method":"ping"}]"#;
    let (status, response) = request(
        &address,
        &raw_post(
            batch,
            &[("MCP-Protocol-Version".to_string(), "2026-07-28".to_string())],
        ),
    )
    .await;
    assert_eq!(status, 400, "{response}");
    let message = response["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .to_ascii_lowercase();
    assert!(
        message.contains("batch") || message.contains("single"),
        "the refusal must say a batch is not a valid body here, got: {message}"
    );
}

/// Every result this transport returns carries the revision's `resultType`.
///
/// Asserted here as well as on the dispatcher because this is the transport that
/// actually speaks `2026-07-28`, where the field is a MUST rather than an inert
/// extra.
#[tokio::test]
async fn every_http_result_carries_the_result_type() {
    let address = start().await;
    for body in [
        r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
        r#"{"jsonrpc":"2.0","id":1,"method":"server/discover"}"#,
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devmap_status","arguments":{}}}"#,
    ] {
        let (status, response) = request(&address, &post(body)).await;
        assert_eq!(status, 200, "{body}: {response}");
        assert_eq!(
            response["result"]["resultType"],
            json!("complete"),
            "no resultType for {body}: {response}"
        );
    }
}

/// The simple-request path a browser actually has.
///
/// `a_browser_cannot_reach_this_endpoint_even_with_a_null_origin` checks that a
/// preflight is refused. This checks the request that is never preflighted:
/// `fetch(url, {method:"POST", body:"..."})` sends `Content-Type: text/plain`
/// and a typeless `Blob` body sends none at all — both CORS-simple, both
/// delivered without asking. If either reached the handler, a sandboxed page
/// could run tool calls against the user's private index; the response would be
/// unreadable, but the calls would have happened.
#[tokio::test]
async fn a_cors_simple_post_never_reaches_the_handler() {
    let address = start().await;
    let body = with_request_meta(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#);
    let conformant = raw_post(&body, &mirrored_headers(&body));
    for simple in [
        conformant.replace("Content-Type: application/json", "Content-Type: text/plain"),
        conformant.replace(
            "Content-Type: application/json",
            "Content-Type: multipart/form-data; boundary=x",
        ),
        conformant.replace("Content-Type: application/json\r\n", ""),
    ] {
        let raw = simple.replace("Host: localhost", "Host: localhost\r\nOrigin: null");
        let (status, response) = request(&address, &raw).await;
        assert_eq!(
            status, 415,
            "a CORS-simple POST must be refused before dispatch: {response}"
        );
        assert!(
            response.get("result").is_none(),
            "nothing may run for a refused content type: {response}"
        );
    }
}

/// A chunked body that declares nothing and sends everything.
///
/// `read_body` checks `Content-Length` *and* the accumulating stream, and the
/// second check is the only one that fires here — a `Content-Length` check alone
/// lets this through and buffers the whole thing. The refusal must arrive rather
/// than the connection dying, or the client cannot tell a limit from a crash.
#[tokio::test]
async fn a_chunked_body_over_the_limit_is_refused_mid_stream() {
    let address = start().await;
    // 1 MB is the limit; 16 chunks of 128 KB crosses it without ever declaring
    // a length.
    let chunk = "x".repeat(128 * 1024);
    let mut raw = String::from(
        "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n\
         Accept: application/json\r\nMCP-Protocol-Version: 2026-07-28\r\n\
         Mcp-Method: ping\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
    );
    for _ in 0..16 {
        raw.push_str(&format!("{:x}\r\n{chunk}\r\n", chunk.len()));
    }
    raw.push_str("0\r\n\r\n");

    // Written from a task while the response is read here, because the server
    // answers before the body is finished and then closes: a client that
    // insisted on completing its write would take an EPIPE and never read the
    // refusal it was sent. That is what a real client does, and it is also the
    // only way to prove the refusal is *delivered* rather than merely decided.
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(&address)
        .await
        .expect("connect");
    let (mut reader, mut writer) = stream.split();
    let send = async move {
        // Errors are expected and are the point: the server stops reading.
        let _ = writer.write_all(raw.as_bytes()).await;
        let _ = writer.flush().await;
    };
    let mut response = Vec::new();
    let receive = reader.read_to_end(&mut response);
    let (_, read) = tokio::join!(send, receive);
    let _ = read;

    let text = String::from_utf8_lossy(&response).to_string();
    let status: u16 = text
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or_else(|| {
            panic!("the refusal must reach the client, not just be decided: {text:?}")
        });
    assert_eq!(
        status, 413,
        "an undeclared oversized body must be refused mid-stream: {text}"
    );
}

/// Header values that try to break the framing.
///
/// A `Mcp-Name` carrying CR/LF would split the response if it were ever echoed,
/// and a value that decodes to something other than the body's name must be a
/// mismatch rather than a pass. Neither may take the connection down: a refusal
/// the client can read is the point.
#[tokio::test]
async fn hostile_header_values_are_refused_without_dropping_the_connection() {
    let address = start().await;
    let body = with_request_meta(
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devmap_status","arguments":{}}}"#,
    );
    for hostile in [
        // Base64 of "devmap_preview": decodes to a different tool than the body
        // names, so the decode has to happen before the comparison.
        "=?base64?ZGV2bWFwX3ByZXZpZXc=?=",
        // Not base64 at all, inside the sentinel.
        "=?base64?!!!!?=",
        // Base64 of bytes that are not UTF-8.
        "=?base64?/w==?=",
        "devmap_status\u{200b}",
        "",
    ] {
        let headers: Vec<(String, String)> = mirrored_headers(&body)
            .into_iter()
            .map(|(name, value)| {
                if name.eq_ignore_ascii_case("Mcp-Name") {
                    (name, hostile.to_string())
                } else {
                    (name, value)
                }
            })
            .collect();
        let (status, response) = request(&address, &raw_post(&body, &headers)).await;
        assert_eq!(status, 400, "Mcp-Name {hostile:?}: {response}");
        assert_eq!(
            response["error"]["code"],
            json!(-32020),
            "Mcp-Name {hostile:?} must be a HeaderMismatch, not a silent pass: {response}"
        );
    }

    // The encoding is not decoration: a name that legitimately needs it must be
    // decoded and compared, and here it names a tool that does not exist — so
    // the answer is "unknown tool", not "header mismatch".
    let body = with_request_meta(
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devmap_søk","arguments":{}}}"#,
    );
    let headers: Vec<(String, String)> = mirrored_headers(&body)
        .into_iter()
        .map(|(name, value)| {
            if name.eq_ignore_ascii_case("Mcp-Name") {
                // Base64 of the UTF-8 bytes of `devmap_søk`.
                (name, "=?base64?ZGV2bWFwX3PDuGs=?=".to_string())
            } else {
                (name, value)
            }
        })
        .collect();
    let (status, response) = request(&address, &raw_post(&body, &headers)).await;
    assert_eq!(
        response["error"]["code"],
        json!(-32602),
        "an encoded name that matches the body must be accepted, leaving only the fact \
         that the tool does not exist (status {status}): {response}"
    );
}

/// `initialize` is not a method of the revision this endpoint serves.
///
/// The handshake was removed in `2026-07-28` and `server/discover` replaces it,
/// so a request that arrives with the modern envelope and calls `initialize` is
/// asking for a method this server does not have on this transport. It used to
/// be answered — with `protocolVersion: "2025-11-25"`, a revision this endpoint
/// refuses on every other request, reported to a client that had just declared
/// `2026-07-28` in the header and the body.
///
/// `404` and not `400`: "If the server does not implement the requested RPC
/// method, it **MUST** respond with `404 Not Found` and a JSON-RPC error with
/// code `-32601`."
#[tokio::test]
async fn initialize_is_refused_as_an_unknown_method_on_the_modern_transport() {
    let address = start().await;
    let (status, body) = request(
        &address,
        &post(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25"}}"#),
    )
    .await;
    assert_eq!(status, 404, "an unimplemented method is 404: {body}");
    assert_eq!(body["error"]["code"], json!(-32601), "{body}");
    assert_eq!(
        body["id"],
        json!(1),
        "the id must survive so the client can match the failure: {body}"
    );
    assert!(
        body["result"].is_null(),
        "no negotiation may be reported for a handshake this transport does not perform: {body}"
    );
}

/// The id narrowing reaches this transport too.
///
/// "Requests **MUST** include a string or integer ID. Unlike base JSON-RPC, the
/// ID **MUST NOT** be `null`." A body carrying an `id` member is a request, so
/// it is owed an answer; a body carrying an *illegal* id is owed a refusal,
/// addressed to the id it sent so the client can still resolve the call.
#[tokio::test]
async fn an_id_that_is_not_a_string_or_integer_is_refused_over_http() {
    let address = start().await;
    for illegal in [json!(null), json!(1.5), json!(true), json!([]), json!({})] {
        let body = json!({"jsonrpc": "2.0", "id": illegal, "method": "ping"}).to_string();
        let (status, response) = request(&address, &post(&body)).await;
        assert_eq!(status, 400, "{illegal}: {response}");
        assert_eq!(
            response["error"]["code"],
            json!(-32600),
            "a malformed id is an Invalid Request: {illegal} -> {response}"
        );
        assert!(
            response["result"].is_null(),
            "an illegal id must not be answered with a result: {illegal} -> {response}"
        );
    }

    // And a legal one still works, so the bound refuses only what it was meant to.
    let (status, response) = request(
        &address,
        &post(r#"{"jsonrpc":"2.0","id":"call-1","method":"ping"}"#),
    )
    .await;
    assert_eq!(status, 200, "{response}");
    assert_eq!(response["id"], json!("call-1"), "{response}");
}

/// Every code this server emits is one the specification allows it to emit.
///
/// "`-32000` to `-32019` — legacy. New codes **MUST NOT** be allocated in this
/// sub-range, and new implementations **SHOULD NOT** use codes from this
/// sub-range at all." and "`-32020` to `-32099` — reserved for the MCP
/// specification. Implementations **MUST NOT** emit any code from this
/// sub-range that is not defined by this specification."
///
/// A sweep rather than a spot check, because the failure this guards against is
/// a *new* refusal added later reaching for a spare-looking number. Both counts
/// are reported: a sweep that exercised fewer cases than it listed would be
/// claiming coverage it did not have.
#[tokio::test]
async fn no_response_carries_a_code_the_specification_reserves() {
    let address = start().await;
    let hostile: &[&str] = &[
        r#"{"jsonrpc":"2.0","id":1,"method":"resources/list"}"#,
        r#"{"jsonrpc":"2.0","id":1,"method":"prompts/list"}"#,
        r#"{"jsonrpc":"2.0","id":1,"method":"completion/complete"}"#,
        r#"{"jsonrpc":"2.0","id":1,"method":"subscriptions/listen"}"#,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#,
        r#"{"jsonrpc":"1.0","id":1,"method":"ping"}"#,
        r#"{"jsonrpc":"2.0","id":1}"#,
        r#"{"jsonrpc":"2.0","id":null,"method":"ping"}"#,
        r#"{"jsonrpc":"2.0","id":{},"method":"ping"}"#,
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{"cursor":"made-up"}}"#,
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call"}"#,
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"nope"}}"#,
        "{not json",
        "[]",
    ];
    // The two the specification defines, plus the standard JSON-RPC set.
    let allowed = [-32700, -32600, -32601, -32602, -32603, -32020, -32022];
    let mut inspected = 0usize;
    for raw in hostile {
        let (_, response) = request(&address, &post(raw)).await;
        let Some(code) = response["error"]["code"].as_i64() else {
            continue;
        };
        inspected += 1;
        assert!(
            allowed.contains(&code),
            "{code} is not a code this server may emit ({raw}): the JSON-RPC reserved range is \
             partitioned and {code} is either in the retired -32000..-32019 band or is an \
             undefined code in the range the specification keeps for itself"
        );
    }
    // An unsupported version, which is the one MCP-allocated code a client can
    // provoke, reached through its own body because `post` mirrors the headers.
    let (_, response) = request(
        &address,
        &post(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"1999-01-01"}}}"#,
        ),
    )
    .await;
    assert_eq!(response["error"]["code"], json!(-32022), "{response}");
    inspected += 1;
    assert_eq!(
        inspected,
        hostile.len() + 1,
        "{inspected} of {} hostile requests produced an error to inspect; the rest were answered \
         successfully, which this sweep cannot vouch for",
        hostile.len() + 1
    );
}

// ---------------------------------------------------------------------------
// `serve_http`: the entry point that binds
// ---------------------------------------------------------------------------
//
// Everything above starts from `serve_http_on`, which is handed a listener the
// test bound itself. `serve_http` is the one the CLI calls (`dev map serve
// --http`), and what it adds is exactly what a listener-first fixture cannot
// reach: it binds the caller's `SocketAddr`, and it fails if it cannot. A
// `serve_http` whose body were replaced with `Ok(())` would leave every test
// above green and every `--http` server dead on arrival.

/// The address a client dials to reach a server bound at `addr`.
///
/// A wildcard bind is every interface at once and is not itself a destination;
/// loopback is the interface every machine has.
fn dial(addr: SocketAddr) -> SocketAddr {
    if addr.ip().is_unspecified() {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), addr.port())
    } else {
        addr
    }
}

/// Every message the crate logged, in the order it logged them.
///
/// The bound address reaches nothing else. `serve_http` serves until the
/// process ends, so it never hands the address back, and its one `tracing` line
/// is the whole of what an operator — or a test — gets to see. Reading it is
/// also the only way to tell a bind that honoured a wildcard address from one
/// that quietly rewrote it to loopback: both answer on `127.0.0.1` identically.
///
/// Hand-rolled, for the same reason the HTTP client above is: `tracing` is
/// already a dependency of this crate, `tracing-subscriber` is not, and one
/// assertion is not a reason to make it one.
static LOG: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Pulls the formatted `message` field out of an event and leaves the rest.
struct Message(String);

impl tracing::field::Visit for Message {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }
}

struct CaptureLog;

impl tracing::Subscriber for CaptureLog {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        // Nothing here reads spans back, and an id is required to be non-zero.
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut message = Message(String::new());
        event.record(&mut message);
        LOG.lock()
            .expect("the capture holds no lock across a panic")
            .push(message.0);
    }

    fn enter(&self, _: &tracing::span::Id) {}

    fn exit(&self, _: &tracing::span::Id) {}
}

/// Install the capture, once for the whole test binary.
fn capture_log() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        tracing::subscriber::set_global_default(CaptureLog)
            .expect("nothing else in this test binary installs a subscriber");
    });
}

/// The line `serve_http` writes once it has bound `port`.
///
/// Matched on the port as well as the text because the tests in this file run
/// concurrently and several of them start a server; the port is what makes the
/// line this caller's. Polled rather than read once: the log is written by the
/// server task between binding and its first `accept`, and a client can connect
/// to a listening socket in that window.
async fn bound_log_line(port: u16) -> String {
    for _ in 0..400 {
        let found = LOG
            .lock()
            .expect("the capture holds no lock across a panic")
            .iter()
            .find(|line| {
                line.contains("listening on http://") && line.ends_with(&format!(":{port}"))
            })
            .cloned();
        if let Some(line) = found {
            return line;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("serve_http logged no bound address for port {port}");
}

/// Run the real `serve_http` on an ephemeral port of `ip`, and return the
/// address it was told to bind.
///
/// The port has to be chosen before the call, because `serve_http` takes an
/// address rather than a listener and never hands the bound one back: it serves
/// until the process ends, and the port it got only ever reaches a `tracing`
/// line. So this takes an ephemeral port from the kernel and gives it straight
/// back — never a hardcoded one, which would fail on a machine that happens to
/// be using it. That hand-back is a race with everything else on the machine,
/// so it is retried rather than assumed, and the server that comes up is
/// confirmed to be *this* one by the exchange each caller then performs
/// against it.
async fn start_serve_http(ip: IpAddr) -> SocketAddr {
    // Before the first spawn, or the line the server writes on the way up is
    // written to nobody.
    capture_log();
    for _ in 0..64 {
        let probe = TcpListener::bind(SocketAddr::new(ip, 0))
            .await
            .expect("an ephemeral port");
        let addr = probe.local_addr().expect("the probe's address");
        drop(probe);

        let server = tokio::spawn(devmap_serve::mcp_http::serve_http(corpus(), addr));
        let target = dial(addr);
        for _ in 0..200 {
            if server.is_finished() {
                // The port went to someone else between the probe and the
                // bind. Take another one rather than reporting a race as a
                // failure of the thing under test.
                break;
            }
            if tokio::net::TcpStream::connect(target).await.is_ok() {
                return addr;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        server.abort();
    }
    panic!("serve_http never came up on an ephemeral port of {ip}");
}

/// The entry point the CLI calls binds the address it is handed, and what
/// answers there is the dispatcher every other transport reaches.
///
/// Nothing else in this crate calls `serve_http`. What it does is bind, report
/// and delegate, and a regression in any of the three — an address that is not
/// the caller's, a listener that is dropped, a delegation that is dropped — is
/// invisible to every test that starts from a listener it bound itself.
#[tokio::test]
async fn serve_http_binds_the_address_it_is_handed_and_serves_the_shared_dispatcher() {
    let addr = start_serve_http(IpAddr::V4(Ipv4Addr::LOCALHOST)).await;
    assert!(
        addr.ip().is_loopback(),
        "the fixture asked for a loopback port and must have been given one"
    );

    let (status, body) = request(
        &dial(addr).to_string(),
        &post(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert!(
        body["result"]["tools"]
            .as_array()
            .is_some_and(|tools| !tools.is_empty()),
        "the port serve_http bound must answer with the same tool list the other \
         transports serve: {body}"
    );
}

/// `serve_http` binds what the caller asked for, loopback or not.
///
/// The module says so in as many words — "Loopback by default, and the bind
/// address is the caller's to choose" — and the default lives a layer up, in
/// `dev map serve --http`, which spells a bare `8080` out as `127.0.0.1:8080`
/// before it ever reaches here. `serve_http` itself has no opinion: handed
/// `0.0.0.0` it publishes a private repository's code index on every interface,
/// and nothing in this crate stops it.
///
/// Pinned because that is the kind of thing that gets "fixed" in passing. A
/// clamp added here would silently break a deployment that binds a routable
/// interface on purpose; this test is what makes whoever adds one say so out
/// loud and correct the contract the module documents.
#[tokio::test]
async fn serve_http_binds_a_non_loopback_address_when_the_caller_asks_for_one() {
    let addr = start_serve_http(IpAddr::V4(Ipv4Addr::UNSPECIFIED)).await;
    assert!(
        !addr.ip().is_loopback(),
        "0.0.0.0 is every interface, not the loopback one"
    );

    let (status, body) = request(
        &dial(addr).to_string(),
        &post(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#),
    )
    .await;
    assert_eq!(
        status, 200,
        "the wildcard bind was accepted and is serving the private index: {body}"
    );

    // Dialling it proves it is serving, not *where*: a server clamped to
    // 127.0.0.1 would answer the exchange above exactly as this one does, and
    // the clamp is the change worth catching. What the two cases cannot both
    // produce is the same log line, so that is what is read back.
    let line = bound_log_line(addr.port()).await;
    assert!(
        line.contains(&format!("http://{addr}")),
        "serve_http must report binding the address it was handed, not one it rewrote: \
         {line}"
    );
}

/// The rebinding guard is decided per connection, not per listener.
///
/// `host_is_acceptable` fires only when the socket a request arrived on is a
/// loopback one, and the address it reads is `stream.local_addr()` — this
/// connection's local address, filled in by the kernel — not the listener's. On
/// a wildcard bind the two disagree: the listener's address is `0.0.0.0`, which
/// is not loopback, while a connection that arrived over loopback reports
/// `127.0.0.1`.
///
/// Reading it once outside the accept loop looks like an obvious tidy-up and
/// would turn the guard off for every connection a wildcard server ever
/// accepts: no error, no log, just a `Host: evil.example` that starts being
/// answered. Every other `Host` test in this file starts from a `127.0.0.1`
/// listener, where the two addresses agree and the substitution is invisible.
#[tokio::test]
async fn the_rebinding_guard_reads_the_connection_not_the_wildcard_listener() {
    let addr = start_serve_http(IpAddr::V4(Ipv4Addr::UNSPECIFIED)).await;
    let target = dial(addr).to_string();
    let body = with_request_meta(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#);

    let rebound =
        raw_post(&body, &mirrored_headers(&body)).replace("Host: localhost", "Host: evil.example");
    let (status, response) = request(&target, &rebound).await;
    assert_eq!(
        status, 403,
        "this connection arrived over loopback, so a routable Host must still be refused \
         however wide the listener was bound: {response}"
    );

    // And the guard has not merely become "refuse everything": the same
    // connection under a loopback name is served.
    let (status, response) = request(&target, &raw_post(&body, &mirrored_headers(&body))).await;
    assert_eq!(status, 200, "Host: localhost must keep working: {response}");
}

/// A port that cannot be had is refused, naming the address that could not be
/// had rather than only the errno.
///
/// This is the one failure `serve_http` owns alone, and the message is the
/// whole of what the operator gets: `dev map serve --http 8080` normalises a
/// bare port into `127.0.0.1:8080` before the call, so "Address already in use
/// (os error 48)" on its own names nothing the user typed and nothing they can
/// go and look at. Failing silently is worse still — a server that returned
/// `Ok(())` here would exit zero having served nobody.
#[tokio::test]
async fn a_port_already_in_use_is_refused_and_names_the_address() {
    let held = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = held.local_addr().expect("the held address");

    let error = devmap_serve::mcp_http::serve_http(corpus(), addr)
        .await
        .expect_err("the port is held for the whole call, so the bind cannot succeed");

    let reported = format!("{error:#}");
    assert!(
        reported.contains(&addr.to_string()),
        "the failure must name the address it could not bind, got: {reported}"
    );
}

/// Every request-smuggling framing shape, and what actually happens to it.
///
/// This is a characterisation test, and it is one on purpose. The obvious guard
/// — refuse a request carrying both `Content-Length` and `Transfer-Encoding`,
/// which is what RFC 9112 §6.1 means by "ought to be handled as an error" —
/// **cannot be written at this layer**, and shipping it would have been shipping
/// a branch that can never be taken. Hyper resolves the ambiguity while parsing
/// the head and then erases the evidence: `proto/h1/role.rs:279-281` does
/// `headers.remove(header::CONTENT_LENGTH)` the moment a `Transfer-Encoding` is
/// seen, and the following arm `continue`s past every later `Content-Length`. By
/// the time `handle_request` is called the request has one framing header and
/// looks well-formed. That was verified by writing the guard, watching it never
/// fire, and reading the parser.
///
/// So what protects this endpoint is hyper's own normalisation, and the point of
/// this test is that the protection is *asserted* rather than assumed. A hyper
/// upgrade that changed any line below would otherwise change this server's
/// exposure with nothing saying so.
///
/// Measured against the release binary over a real socket, all six shapes:
///
/// | shape | answer |
/// |---|---|
/// | `Content-Length` + `Transfer-Encoding: chunked` | 200, the *chunked* body is what ran |
/// | …with a pipelined request trailing the chunked terminator | **one** response, not two |
/// | two `Content-Length`, different values | 400 |
/// | two `Content-Length`, identical values | 200 |
/// | `Transfer-Encoding: identity` (not chunked) | 400 |
/// | `Transfer-Encoding : chunked` (space before the colon) | 400 |
///
/// The row that matters is the second. A desync needs this server to leave bytes
/// in the buffer that something else reads as a request; it answers once and the
/// trailing bytes are never answered, so no response is split. The first row is
/// the RFC's own first sentence ("the Transfer-Encoding overrides the
/// Content-Length") and is a divergence only for an intermediary that reads it
/// the other way — which is a fact about that intermediary, in front of an
/// endpoint `--http` documents as loopback-by-default.
#[tokio::test]
async fn contradictory_body_framing_is_resolved_below_this_server_and_never_splits_a_response() {
    let address = start().await;

    let head = |method: &str| {
        format!(
            "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n\
             Accept: application/json\r\nMCP-Protocol-Version: 2026-07-28\r\n\
             Mcp-Method: {method}\r\n"
        )
    };
    let smuggled = with_request_meta(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#);
    let decoy = with_request_meta(r#"{"jsonrpc":"2.0","id":99,"method":"ping"}"#);

    // 1. Both framings, naming two different bodies. Exactly one runs, and it is
    //    the chunked one; the `Content-Length` reading is never executed.
    let raw = format!(
        "{}Content-Length: {}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n\
         {:x}\r\n{smuggled}\r\n0\r\n\r\n",
        head("tools/list"),
        decoy.len(),
        smuggled.len()
    );
    let (status, _, body) = exchange(&address, &raw).await;
    assert_eq!(
        status, 200,
        "hyper resolves this rather than refusing: {body}"
    );
    assert!(
        body.contains("\"tools\""),
        "the chunked framing is the one that wins, per RFC 9112 §6.1: {body}"
    );
    assert!(
        !body.contains("\"id\":99"),
        "the Content-Length reading must never also be answered — two answers to \
         one exchange is the response split itself: {body}"
    );

    // 2. The row that decides whether this is exploitable here: a complete second
    //    request pipelined after the chunked terminator. If the trailing bytes
    //    were answered, an intermediary reading `Content-Length` would have a
    //    request in flight that this server also answered separately.
    let trailing = with_request_meta(r#"{"jsonrpc":"2.0","id":77,"method":"ping"}"#);
    let second = format!(
        "{}Content-Length: {}\r\nConnection: close\r\n\r\n{trailing}",
        head("ping"),
        trailing.len()
    );
    let raw = format!(
        "{}Content-Length: {}\r\nTransfer-Encoding: chunked\r\n\r\n\
         {:x}\r\n{smuggled}\r\n0\r\n\r\n{second}",
        head("tools/list"),
        decoy.len(),
        smuggled.len()
    );
    let (_, _, _) = exchange(&address, &raw).await;
    let whole = {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut stream = tokio::net::TcpStream::connect(&address)
            .await
            .expect("connect");
        stream.write_all(raw.as_bytes()).await.expect("write");
        stream.flush().await.expect("flush");
        let mut buffer = Vec::new();
        let _ =
            tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut buffer)).await;
        String::from_utf8_lossy(&buffer).to_string()
    };
    assert_eq!(
        whole.matches("HTTP/1.1 ").count(),
        1,
        "one exchange must produce one response. Two would mean the bytes an \
         intermediary attributes to a second request were answered here as well, \
         which is the desync: {whole}"
    );
    assert!(
        !whole.contains("\"id\":77"),
        "the pipelined trailer must not be answered: {whole}"
    );

    // 3. The shapes hyper does refuse, pinned so a change is visible.
    let body = with_request_meta(r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#);
    for (shape, extra) in [
        (
            "two Content-Length headers with different values",
            format!(
                "Content-Length: {}\r\nContent-Length: {}\r\n",
                body.len(),
                body.len() + 10
            ),
        ),
        (
            "a Transfer-Encoding that is not chunked",
            "Transfer-Encoding: identity\r\n".to_string(),
        ),
        (
            "a header name with a space before its colon",
            format!(
                "Transfer-Encoding : chunked\r\nContent-Length: {}\r\n",
                body.len()
            ),
        ),
    ] {
        let raw = format!("{}{extra}Connection: close\r\n\r\n{body}", head("ping"));
        let (status, _, answered) = exchange(&address, &raw).await;
        assert_eq!(
            status, 400,
            "{shape} must stay refused; hyper's normalisation is what this \
             endpoint's framing safety rests on. Got {status}: {answered}"
        );
    }

    // 4. The positive control. Identical repeated Content-Length is one framing
    //    statement made twice and is served, so the refusals above are about
    //    disagreement rather than about repetition.
    let raw = format!(
        "{}Content-Length: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        head("ping"),
        body.len(),
        body.len()
    );
    let (status, parsed) = request(&address, &raw).await;
    assert_eq!(status, 200, "an unambiguous repeat is still one framing");
    assert_eq!(parsed["id"], json!(2));
}
