//! The MCP 2.0 (`2026-07-28`) single-exchange HTTP transport.
//!
//! Driven over a real TCP socket on port 0 rather than by calling the handler
//! directly: the things most likely to be wrong here are envelope-level — status
//! codes, header handling, body limits — and none of them exist below the
//! socket. A test that calls `handle_request` in-process would pass while the
//! server crashed on every connection, which is exactly what happened once
//! during development (hyper panics at setup if a read timeout is configured
//! with no timer to drive it).

use std::sync::Arc;

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
async fn request(address: &str, raw: &str) -> (u16, Value) {
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
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .unwrap_or("");
    let parsed = serde_json::from_str(body)
        .unwrap_or_else(|err| panic!("body was not JSON ({err}): {body}"));
    (status, parsed)
}

fn post(body: &str) -> String {
    format!(
        "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
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
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(address)
        .await
        .expect("connect");
    stream.write_all(raw.as_bytes()).await.expect("write");
    stream.flush().await.expect("flush");
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.expect("read");
    String::from_utf8_lossy(&response).to_string()
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
