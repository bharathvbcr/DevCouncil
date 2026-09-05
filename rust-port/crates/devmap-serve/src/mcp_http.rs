//! MCP 2.0's modern transport: one self-contained HTTP POST per exchange.
//!
//! # Why this transport exists at all
//!
//! Protocol revision `2026-07-28` is not reachable over stdio. The `initialize`
//! handshake tops out at `2025-11-25` — verified in the reference SDK, whose
//! `HANDSHAKE_PROTOCOL_VERSIONS` deliberately excludes the modern revision
//! because it does not use a handshake at all. A modern request carries its own
//! protocol version, client info and capabilities in `_meta` and is answered in
//! the same exchange: no session id, no negotiation, nothing retained between
//! requests.
//!
//! That is why `ttlMs` / `cacheScope` are live here and inert on stdio. The
//! Python server sets them correctly and every version its transport can
//! negotiate sieves them back out; on this transport they reach the client.
//!
//! # Dispatch is shared
//!
//! Everything below the envelope is [`crate::mcp::handle_method`] — the same
//! function stdio calls, which in turn is the same `dispatch` the socket
//! protocol calls. This module owns the HTTP envelope and nothing else. Three
//! transports, one answer.
//!
//! # Binding
//!
//! Loopback by default, and the bind address is the caller's to choose. A code
//! index is a map of a private repository; serving it on a routable interface
//! publishes that map to the network. [`serve_http`] does not stop a caller from
//! binding elsewhere — that is a deployment decision — but it refuses
//! cross-origin browser requests regardless, because those are the ones the user
//! never intended to make.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{header, Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use serde_json::{json, Value};

use crate::mcp::{handle_method, StoreSlot, CACHE_TTL_MS, MODERN_PROTOCOL_VERSIONS};

/// Largest request body accepted, in bytes.
///
/// The same 1 MB the other two transports allow, for the same reason: `preview`
/// carries file content. Enforced against `Content-Length` *and* while
/// streaming, because a chunked request can claim nothing and send everything.
const MAX_BODY_BYTES: u64 = 1024 * 1024;

/// Ceiling on one exchange, end to end.
///
/// Longer than the 30s tool-call budget inside `handle_method` so that a call
/// which times out internally still gets to report that as a tool error, rather
/// than having the connection cut from under it and the agent left with a
/// transport failure it cannot interpret.
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(45);

/// How long a connection may stay idle before being dropped.
const IDLE_TIMEOUT: Duration = Duration::from_secs(120);

/// JSON-RPC error code to HTTP status.
///
/// Mirrors the reference SDK's `ERROR_CODE_HTTP_STATUS`. The spec makes the
/// `400`/`404` mapping a MUST, which is why this is a table rather than a blanket
/// `200 with an error body`: a client distinguishes "this server does not have
/// that method" from "that method failed" by status, and collapsing both to 200
/// makes an unimplemented method look like a broken one.
fn status_for(code: i64) -> StatusCode {
    match code {
        -32700 | -32600 | -32602 | -32020 | -32021 | -32022 => StatusCode::BAD_REQUEST,
        -32601 => StatusCode::NOT_FOUND,
        // -32603 and anything unrecognised: the request was well-formed and the
        // server failed to answer it, which is a 500 and not the client's fault.
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// Methods whose answers a client may cache, per the spec's cacheable set.
///
/// Only the two this server implements. A method not listed gets no cache
/// fields, which is the correct default: an un-annotated result is uncacheable.
fn cache_hint_for(method: &str) -> Option<(u64, &'static str)> {
    match method {
        "tools/list" | "server/discover" => Some((CACHE_TTL_MS, "private")),
        _ => None,
    }
}

fn json_response(status: StatusCode, body: &Value) -> Response<Full<Bytes>> {
    let bytes = serde_json::to_vec(body).unwrap_or_else(|_| {
        // Serializing a Value we just built cannot fail in practice; if it
        // somehow does, a fixed valid error body is better than a panic that
        // drops the connection with no explanation.
        br#"{"jsonrpc":"2.0","id":null,"error":{"code":-32603,"message":"response serialization failed"}}"#.to_vec()
    });
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        // This endpoint is not for browsers. Saying so explicitly stops a page
        // from reading a response it managed to send.
        .header("X-Content-Type-Options", "nosniff")
        .body(Full::new(Bytes::from(bytes)))
        .unwrap_or_else(|_| Response::new(Full::new(Bytes::from_static(b"{}"))))
}

fn rpc_error_body(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": code, "message": message.into()}
    })
}

/// Refuse a request whose `Origin` names a site.
///
/// DNS-rebinding is the attack this closes: a page the user is browsing resolves
/// a hostname to 127.0.0.1 and then POSTs to this port from JavaScript, reading
/// the user's private code graph. A local MCP endpoint has no browser client, so
/// any request carrying a site `Origin` is one the user did not intend.
///
/// Absent origins pass — that is the non-browser client this exists for.
/// **`null` also passes, and `null` is not only a non-browser client:** a
/// sandboxed iframe, a `data:` URL and a `file://` page all send `Origin: null`,
/// so this check alone does not keep a browser out. Two other properties do, and
/// because the safety rests on their conjunction rather than on this function,
/// `a_browser_cannot_reach_this_endpoint_even_with_a_null_origin` pins both:
///
/// 1. **POST only.** A JSON body makes the request non-simple, so a browser must
///    preflight with `OPTIONS`, which is answered `405` — the real request never
///    leaves the browser.
/// 2. **No CORS headers, ever.** Without `Access-Control-Allow-Origin` the
///    response is unreadable cross-origin even where a request does go out.
///
/// Adding an `OPTIONS` handler or any `Access-Control-Allow-*` header would
/// therefore reopen the hole this closes, which is exactly what that test fails
/// on.
fn origin_is_acceptable(request: &Request<Incoming>) -> bool {
    match request.headers().get(header::ORIGIN) {
        None => true,
        Some(origin) => matches!(origin.to_str(), Ok("null")),
    }
}

/// Read the body, refusing anything over [`MAX_BODY_BYTES`].
///
/// Checked twice on purpose. `Content-Length` catches the honest oversized
/// request before a byte is buffered; the streaming accumulator catches the
/// chunked request that declares nothing and sends a gigabyte, which is the one
/// a `Content-Length` check alone lets through.
async fn read_body(request: Request<Incoming>) -> Result<Bytes, (StatusCode, String)> {
    if let Some(declared) = request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
    {
        if declared > MAX_BODY_BYTES {
            return Err((
                StatusCode::PAYLOAD_TOO_LARGE,
                format!("body declares {declared} bytes, over the {MAX_BODY_BYTES}-byte limit"),
            ));
        }
    }

    let mut body = request.into_body();
    let mut collected = Vec::new();
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|err| {
            (
                StatusCode::BAD_REQUEST,
                format!("request body could not be read: {err}"),
            )
        })?;
        if let Some(chunk) = frame.data_ref() {
            if collected.len() as u64 + chunk.len() as u64 > MAX_BODY_BYTES {
                return Err((
                    StatusCode::PAYLOAD_TOO_LARGE,
                    format!("body exceeded the {MAX_BODY_BYTES}-byte limit mid-stream"),
                ));
            }
            collected.extend_from_slice(chunk);
        }
    }
    Ok(Bytes::from(collected))
}

async fn handle_request(
    store: Arc<StoreSlot>,
    request: Request<Incoming>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    if !origin_is_acceptable(&request) {
        return Ok(json_response(
            StatusCode::FORBIDDEN,
            &rpc_error_body(
                Value::Null,
                -32600,
                "cross-origin requests are refused: this endpoint serves a private code index \
and has no browser client",
            ),
        ));
    }

    if request.method() != Method::POST {
        // The modern transport is POST-only. A GET here is usually a client
        // reaching for the legacy SSE stream, so the message says which
        // transport this is rather than only which verb was wrong.
        return Ok(json_response(
            StatusCode::METHOD_NOT_ALLOWED,
            &rpc_error_body(
                Value::Null,
                -32600,
                "this endpoint speaks the MCP 2026-07-28 single-exchange transport: one \
JSON-RPC request per POST. It has no SSE stream and no session.",
            ),
        ));
    }

    let body = match read_body(request).await {
        Ok(body) => body,
        Err((status, message)) => {
            return Ok(json_response(
                status,
                &rpc_error_body(Value::Null, -32600, message),
            ))
        }
    };

    let text = match std::str::from_utf8(&body) {
        Ok(text) => text,
        Err(err) => {
            return Ok(json_response(
                StatusCode::BAD_REQUEST,
                &rpc_error_body(
                    Value::Null,
                    -32700,
                    format!("body is not valid UTF-8: {err}"),
                ),
            ))
        }
    };

    let parsed: Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(err) => {
            return Ok(json_response(
                StatusCode::BAD_REQUEST,
                &rpc_error_body(Value::Null, -32700, err.to_string()),
            ))
        }
    };

    // The id is recovered before dispatch so that an error can be reported
    // against it. A client matching responses to ids cannot use a frame whose id
    // is null, so losing the id turns a reportable failure into an unmatchable
    // one.
    let id = parsed.get("id").cloned().unwrap_or(Value::Null);
    let method = match parsed.get("method").and_then(Value::as_str) {
        Some(method) => method.to_string(),
        None => {
            return Ok(json_response(
                StatusCode::BAD_REQUEST,
                &rpc_error_body(id, -32600, "request has no string 'method'"),
            ))
        }
    };
    if parsed.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Ok(json_response(
            StatusCode::BAD_REQUEST,
            &rpc_error_body(id, -32600, "jsonrpc must be \"2.0\""),
        ));
    }

    // A modern request states its protocol version in `_meta`. An absent one is
    // accepted as the era's only revision; a *stated* one that we do not speak is
    // refused rather than answered, because answering it would mean guessing
    // which surface the client expects and being wrong silently.
    if let Some(stated) = parsed
        .get("params")
        .and_then(|p| p.get("_meta"))
        .and_then(|m| m.get("io.modelcontextprotocol/protocolVersion"))
        .and_then(Value::as_str)
    {
        if !MODERN_PROTOCOL_VERSIONS.contains(&stated) {
            return Ok(json_response(
                StatusCode::BAD_REQUEST,
                &rpc_error_body(
                    id,
                    // UNSUPPORTED_PROTOCOL_VERSION
                    -32022,
                    format!(
                        "this transport speaks {}; the request stated {stated}. Handshake \
revisions are served over stdio instead.",
                        MODERN_PROTOCOL_VERSIONS.join(", ")
                    ),
                ),
            ));
        }
    }

    let params = parsed.get("params").cloned();
    let outcome = match tokio::time::timeout(
        EXCHANGE_TIMEOUT,
        handle_method(&store, &method, params),
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(_) => {
            return Ok(json_response(
                StatusCode::GATEWAY_TIMEOUT,
                &rpc_error_body(
                    id,
                    -32603,
                    format!(
                        "exchange exceeded {}s and was abandoned",
                        EXCHANGE_TIMEOUT.as_secs()
                    ),
                ),
            ))
        }
    };

    match outcome {
        Ok(result) => {
            // A notification over this transport still gets a body: there is no
            // stream to leave silent, and an empty 200 is what a POST-per-exchange
            // client is waiting on.
            let mut result = result.unwrap_or_else(|| json!({}));
            if let Some((ttl, scope)) = cache_hint_for(&method) {
                if let Some(object) = result.as_object_mut() {
                    object.insert("ttlMs".to_string(), json!(ttl));
                    object.insert("cacheScope".to_string(), json!(scope));
                }
            }
            Ok(json_response(
                StatusCode::OK,
                &json!({"jsonrpc": "2.0", "id": id, "result": result}),
            ))
        }
        Err(err) => Ok(json_response(
            status_for(err.code()),
            &rpc_error_body(id, err.code(), err.message()),
        )),
    }
}

/// Serve the modern transport on `addr` until the process ends.
///
/// Returns the bound address before serving, so a caller binding port 0 can
/// learn which port it got — a test that has to guess a free port is a test that
/// fails on a busy machine.
pub async fn serve_http(store: Arc<StoreSlot>, addr: SocketAddr) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let bound = listener.local_addr()?;
    tracing::info!("MCP 2.0 (2026-07-28) listening on http://{bound}");
    serve_http_on(store, listener).await
}

/// Serve on an already-bound listener.
pub async fn serve_http_on(
    store: Arc<StoreSlot>,
    listener: tokio::net::TcpListener,
) -> anyhow::Result<()> {
    loop {
        let (stream, _peer) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(err) => {
                // One failed accept is not a reason to stop serving everyone
                // else; fd exhaustion is transient under load.
                tracing::warn!("MCP HTTP accept failed: {err}");
                continue;
            }
        };
        let store = Arc::clone(&store);
        tokio::spawn(async move {
            let io = TokioIo::new(stream);
            let service = service_fn(move |request| handle_request(Arc::clone(&store), request));
            let connection = http1::Builder::new()
                // hyper panics at connection setup if a timeout is configured
                // with no timer to drive it — "timeout `header_read_timeout`
                // set, but no timer set". Not a warning and not a fallback: every
                // accepted connection dies. Verified by running it.
                .timer(TokioTimer::new())
                .header_read_timeout(IDLE_TIMEOUT)
                .serve_connection(io, service);
            if let Err(err) = connection.await {
                tracing::debug!("MCP HTTP connection ended: {err}");
            }
        });
    }
}
