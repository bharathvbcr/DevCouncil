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
//! `ttlMs` / `cacheScope` are written by the shared dispatcher, not here: they
//! are required fields of `ListToolsResult` and `DiscoverResult` in this
//! revision. What is true of this transport is that they are always *read* —
//! the Python server sets them correctly and every version its handshake can
//! negotiate sieves them back out, so a client that reaches them is a client
//! that got here.
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

use crate::mcp::{handle_method, StoreSlot, META_PROTOCOL_VERSION, MODERN_PROTOCOL_VERSIONS};
// The codes are the shared ones. `-32020` and `-32022` are allocated by the
// specification out of a range it reserves for itself, so they belong with the
// rest of the taxonomy rather than in whichever module happened to need them
// first; a private copy here would be a second declaration of a number that only
// works if there is exactly one of it.
use crate::mcp::codes::{
    HEADER_MISMATCH, INTERNAL_ERROR, INVALID_PARAMS, INVALID_REQUEST, METHOD_NOT_FOUND,
    PARSE_ERROR, UNSUPPORTED_PROTOCOL_VERSION,
};

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

/// The one media type this endpoint reads and writes.
const JSON_MEDIA_TYPE: &str = "application/json";

/// Headers the revision requires a client to mirror out of the request body.
///
/// Lower-case because `Request::headers` is looked up case-insensitively but
/// these strings are also what the refusal messages name, and a client reading
/// "add MCP-Protocol-Version" should be able to search the spec for it.
const PROTOCOL_VERSION_HEADER: &str = "MCP-Protocol-Version";
const METHOD_HEADER: &str = "Mcp-Method";
const NAME_HEADER: &str = "Mcp-Name";

/// `-32021 MissingRequiredClientCapability`, named so [`status_for`] can map it.
///
/// This server never emits it — it needs no client capability to answer any of
/// its tools, and emitting it would name a requirement that does not exist. It
/// is in the table because the table is a mapping from *the specification's*
/// codes to statuses, and a code missing from it would fall through to `500`,
/// which is the wrong answer to give about a request the client could fix.
const MISSING_REQUIRED_CLIENT_CAPABILITY: i64 = -32021;

/// JSON-RPC error code to HTTP status.
///
/// Mirrors the reference SDK's `ERROR_CODE_HTTP_STATUS`. The spec makes the
/// `400`/`404` mapping a MUST, which is why this is a table rather than a blanket
/// `200 with an error body`: a client distinguishes "this server does not have
/// that method" from "that method failed" by status, and collapsing both to 200
/// makes an unimplemented method look like a broken one.
fn status_for(code: i64) -> StatusCode {
    match code {
        PARSE_ERROR
        | INVALID_REQUEST
        | INVALID_PARAMS
        | HEADER_MISMATCH
        | MISSING_REQUIRED_CLIENT_CAPABILITY
        | UNSUPPORTED_PROTOCOL_VERSION => StatusCode::BAD_REQUEST,
        METHOD_NOT_FOUND => StatusCode::NOT_FOUND,
        // -32603 and anything unrecognised: the request was well-formed and the
        // server failed to answer it, which is a 500 and not the client's fault.
        _ => StatusCode::INTERNAL_SERVER_ERROR,
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
        .header(header::CONTENT_TYPE, JSON_MEDIA_TYPE)
        // This endpoint is not for browsers. Saying so explicitly stops a page
        // from reading a response it managed to send.
        .header("X-Content-Type-Options", "nosniff")
        .body(Full::new(Bytes::from(bytes)))
        .unwrap_or_else(|_| Response::new(Full::new(Bytes::from_static(b"{}"))))
}

/// `202 Accepted`, empty, which is the whole of a notification's answer.
///
/// "If the server accepts it, the server MUST return HTTP status code
/// `202 Accepted` with no body." A JSON-RPC frame here would carry `id: null`
/// for a message that has no id, and the revision states the id "MUST NOT be
/// `null`" — a response the client is required to reject, for a message it was
/// never going to correlate.
fn accepted() -> Response<Full<Bytes>> {
    Response::builder()
        .status(StatusCode::ACCEPTED)
        .header("X-Content-Type-Options", "nosniff")
        .body(Full::new(Bytes::new()))
        .unwrap_or_else(|_| Response::new(Full::new(Bytes::new())))
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
/// so this check alone does not keep a browser out. Three other properties do,
/// and because the safety rests on their conjunction rather than on this
/// function, `a_browser_cannot_reach_this_endpoint_even_with_a_null_origin`
/// pins them:
///
/// 1. **POST only, and `application/json` only.** Those two together are what
///    make the request non-simple, so a browser must preflight with `OPTIONS`,
///    which is answered `405` — the real request never leaves the browser. The
///    method alone is not enough: a POST with `Content-Type: text/plain` is a
///    simple request and is sent without a preflight, which is why
///    [`content_type_is_json`] is load-bearing rather than tidy.
/// 2. **No CORS headers, ever.** Without `Access-Control-Allow-Origin` the
///    response is unreadable cross-origin even where a request does go out.
/// 3. **`Host` must not name a routable host on a loopback listener**
///    ([`host_is_acceptable`]) — the rebinding case that reaches here with
///    `Origin: null`.
///
/// Adding an `OPTIONS` handler, any `Access-Control-Allow-*` header, or a
/// relaxation of the content type would therefore reopen the hole this closes,
/// which is exactly what that test fails on.
fn origin_is_acceptable(request: &Request<Incoming>) -> bool {
    match request.headers().get(header::ORIGIN) {
        None => true,
        Some(origin) => matches!(origin.to_str(), Ok("null")),
    }
}

/// Refuse a request that reached a loopback listener under a routable name.
///
/// The second line against DNS rebinding, and the one that catches what
/// [`origin_is_acceptable`] cannot: a sandboxed iframe, a `data:` URL and a
/// `file://` page all send `Origin: null`, which that function accepts by
/// design. A page that resolved `evil.example` to 127.0.0.1 still has to send
/// `Host: evil.example`, and nothing can legitimately reach a loopback socket
/// under a public name — the name does not resolve to 127.0.0.1 for anyone whose
/// resolver has not been steered.
///
/// Scoped to loopback listeners on purpose. The module's contract is that the
/// bind address is the caller's to choose, and a deployment that binds a
/// routable interface has a real hostname that must keep working; refusing every
/// unfamiliar `Host` would break it for a threat it does not have.
///
/// IP literals pass. Rebinding needs a *name* for the browser to resolve, and a
/// caller that already knows the socket address is not being steered to it.
fn host_is_acceptable(request: &Request<Incoming>, local: Option<SocketAddr>) -> bool {
    if !local.is_some_and(|addr| addr.ip().is_loopback()) {
        return true;
    }
    let Some(host) = request.headers().get(header::HOST) else {
        // HTTP/1.1 requires a Host; hyper rejects a request without one before
        // this point, and HTTP/2 synthesises it from `:authority`. Absent here
        // means there is nothing to steer, so there is nothing to refuse.
        return true;
    };
    let Ok(host) = host.to_str() else {
        return false;
    };
    let name = strip_port(host);
    name.parse::<std::net::IpAddr>().is_ok()
        || name.eq_ignore_ascii_case("localhost")
        || name.to_ascii_lowercase().ends_with(".localhost")
}

/// The host portion of an authority, with the port and any IPv6 brackets gone.
fn strip_port(authority: &str) -> &str {
    if let Some(rest) = authority.strip_prefix('[') {
        // `[::1]:8080` — the colons inside the brackets are the address.
        return rest.split(']').next().unwrap_or(rest);
    }
    match authority.rsplit_once(':') {
        Some((host, _)) => host,
        None => authority,
    }
}

/// Whether the body's media type is the one this endpoint reads.
///
/// Enforcing this is half of what keeps a browser out, and the half the module
/// header claims without checking. A POST with `Content-Type: text/plain` is a
/// CORS-*simple* request: no preflight, so it is never stopped by the `OPTIONS`
/// refusal, and `Origin: null` from a sandboxed page reaches the handler and
/// runs. Requiring `application/json` is what makes every browser request
/// non-simple and therefore preflighted — and the preflight is answered `405`.
///
/// Absent counts as wrong. A `fetch` with a typeless `Blob` body sends no
/// `Content-Type` at all, so treating "absent" as "probably JSON" would leave
/// the same hole open under a different shape.
fn content_type_is_json(request: &Request<Incoming>) -> bool {
    request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            let essence = value.split(';').next().unwrap_or("").trim();
            essence.eq_ignore_ascii_case(JSON_MEDIA_TYPE)
        })
}

/// Whether the client said it can read what this endpoint sends.
///
/// This transport answers with `application/json` and has no SSE stream, so a
/// client whose `Accept` excludes JSON would be handed a body it declared it
/// could not parse. An absent header is `*/*` per RFC 9110 and passes.
///
/// The most specific matching range decides, and `q=0` on it is a refusal rather
/// than a match — a client that writes `application/json;q=0` has said the one
/// thing this endpoint produces is unacceptable, and answering anyway would be
/// reading its header as decoration.
fn accepts_json(request: &Request<Incoming>) -> bool {
    let Some(accept) = request.headers().get(header::ACCEPT) else {
        return true;
    };
    let Ok(accept) = accept.to_str() else {
        return false;
    };
    if accept.trim().is_empty() {
        return true;
    }
    // (specificity, quality) of the best match so far. Specificity: 2 for
    // `application/json`, 1 for `application/*`, 0 for `*/*`.
    let mut best: Option<(u8, f32)> = None;
    for range in accept.split(',') {
        let mut parts = range.split(';');
        let essence = parts.next().unwrap_or("").trim().to_ascii_lowercase();
        let specificity = match essence.as_str() {
            JSON_MEDIA_TYPE => 2,
            "application/*" => 1,
            "*/*" => 0,
            _ => continue,
        };
        let quality = parts
            .filter_map(|parameter| {
                let (key, value) = parameter.split_once('=')?;
                key.trim().eq_ignore_ascii_case("q").then_some(value)
            })
            .next()
            .and_then(|value| value.trim().parse::<f32>().ok())
            .unwrap_or(1.0);
        if best.is_none_or(|(seen, _)| specificity > seen) {
            best = Some((specificity, quality));
        }
    }
    match best {
        Some((_, quality)) => quality > 0.0,
        None => false,
    }
}

/// Decode the `=?base64?…?=` sentinel a client uses for a header value that
/// cannot be written as plain ASCII.
///
/// Needed for correctness, not completeness. `Mcp-Name` mirrors `params.name`,
/// which is whatever the model asked for: a call to `devmap_søk` forces a
/// conforming client to encode the header, and a server that compared the
/// encoded form to the raw body value would answer `HeaderMismatch` for a
/// request whose only fault is that the tool does not exist. Wrong error, wrong
/// recovery.
///
/// Returns `None` for a malformed encoding rather than falling back to the raw
/// text: a value we could not decode is a value we did not check, and a check
/// that could not run must not report what a check that passed reports.
fn decode_header_value(value: &str) -> Option<String> {
    let Some(encoded) = value
        .strip_prefix("=?base64?")
        .and_then(|rest| rest.strip_suffix("?="))
    else {
        return Some(value.to_string());
    };
    let mut bytes = Vec::with_capacity(encoded.len() * 3 / 4);
    let mut accumulator: u32 = 0;
    let mut bits: u32 = 0;
    for byte in encoded.bytes() {
        let sextet = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            _ => return None,
        };
        accumulator = (accumulator << 6) | u32::from(sextet);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            bytes.push((accumulator >> bits) as u8);
        }
    }
    String::from_utf8(bytes).ok()
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

/// One mirrored header, with "present but unreadable" kept distinct from
/// "absent".
///
/// Collapsing the two would let a header carrying bytes we could not decode be
/// treated as a header the client never sent — a check that could not run
/// reporting what a check that ran and passed reports.
enum HeaderRead {
    Absent,
    Value(String),
    Undecodable,
}

impl HeaderRead {
    fn from(request: &Request<Incoming>, name: &str) -> Self {
        match request.headers().get(name) {
            None => Self::Absent,
            Some(raw) => match raw.to_str().ok().and_then(decode_header_value) {
                Some(value) => Self::Value(value),
                None => Self::Undecodable,
            },
        }
    }
}

/// The headers this revision requires a client to mirror out of the body.
///
/// Read before the body is consumed, because `read_body` takes the request.
struct MirroredHeaders {
    protocol_version: HeaderRead,
    method: HeaderRead,
    name: HeaderRead,
}

impl MirroredHeaders {
    fn read(request: &Request<Incoming>) -> Self {
        Self {
            protocol_version: HeaderRead::from(request, PROTOCOL_VERSION_HEADER),
            method: HeaderRead::from(request, METHOD_HEADER),
            name: HeaderRead::from(request, NAME_HEADER),
        }
    }

    /// Check the headers against the body they claim to describe.
    ///
    /// `None` when they agree. Otherwise the status and JSON-RPC error body to
    /// send, already carrying the request's own id so the client can match the
    /// failure to the call it made.
    ///
    /// The mismatch case is a security requirement, not a tidiness one: "this
    /// prevents potential security vulnerabilities when different components in
    /// the network rely on different sources of truth (e.g., a load balancer
    /// routing on the header value while the MCP server executes based on the
    /// body value)". A gateway that authorised `Mcp-Name: devmap_status` and
    /// forwarded a body calling `devmap_preview` is exactly that.
    ///
    /// Header/body *agreement* only. Whether the version they agree on is one
    /// this server serves, and whether the body's `_meta` carries the other
    /// fields the revision requires, is decided by `classify_era` in the shared
    /// dispatcher — because those are properties of the request, not of HTTP,
    /// and while they lived here the stdio transport performed neither. The
    /// order still holds: a header that contradicts the body is answered before
    /// the version is judged, so a client is told its request is inconsistent
    /// before it is told to retry with a different version.
    fn check(&self, body: &Value, id: &Value, method: &str) -> Option<(StatusCode, Value)> {
        let mismatch = |message: String| {
            Some((
                StatusCode::BAD_REQUEST,
                rpc_error_body(id.clone(), HEADER_MISMATCH, message),
            ))
        };

        // The version header first: it is the header an intermediary reads to
        // decide whether header/body validation applies at all, so a request
        // without it cannot be checked by anything downstream either.
        let version = match &self.protocol_version {
            HeaderRead::Absent => {
                return mismatch(format!(
                    "every request POST must carry {PROTOCOL_VERSION_HEADER}; this transport \
serves only {} and has no earlier revision to read a header-less request under.",
                    MODERN_PROTOCOL_VERSIONS.join(", ")
                ))
            }
            HeaderRead::Undecodable => {
                return mismatch(format!("{PROTOCOL_VERSION_HEADER} is not readable text"))
            }
            HeaderRead::Value(value) => value.as_str(),
        };

        // Compared only when the body states a version at all. A body that
        // states none is not a *mismatch* — there is nothing to mismatch — it is
        // a request missing a required field, which the dispatcher answers with
        // `-32602` as the revision requires. Reporting it here as a header fault
        // would send the client to look at its headers, which are fine.
        if let Some(stated) = body
            .pointer("/params/_meta")
            .and_then(|meta| meta.get(META_PROTOCOL_VERSION))
            .and_then(Value::as_str)
        {
            if stated != version {
                return mismatch(format!(
                    "{PROTOCOL_VERSION_HEADER} is {version} but params._meta.\
{META_PROTOCOL_VERSION} is {stated}; the header and the body must state the same revision."
                ));
            }
        }

        match &self.method {
            HeaderRead::Absent => {
                return mismatch(format!(
                    "{METHOD_HEADER} is required on every request and must carry the body's \
method, which here is '{method}'"
                ))
            }
            HeaderRead::Undecodable => {
                return mismatch(format!("{METHOD_HEADER} is not readable text"))
            }
            HeaderRead::Value(value) if value != method => {
                return mismatch(format!(
                    "{METHOD_HEADER} is '{value}' but the body calls '{method}'"
                ))
            }
            HeaderRead::Value(_) => {}
        }

        // `Mcp-Name` mirrors `params.name`, and is required exactly where that
        // field exists. `tools/call` is the only such method this server has.
        let named = body.pointer("/params/name").and_then(Value::as_str)?;
        match &self.name {
            HeaderRead::Absent => mismatch(format!(
                "{NAME_HEADER} is required on a request carrying params.name, which here is \
'{named}'"
            )),
            HeaderRead::Undecodable => mismatch(format!(
                "{NAME_HEADER} is not readable text; a value that cannot be written as plain \
ASCII is carried as =?base64?<utf-8>?="
            )),
            HeaderRead::Value(value) if value != named => mismatch(format!(
                "{NAME_HEADER} is '{value}' but the body names '{named}'"
            )),
            HeaderRead::Value(_) => None,
        }
    }
}

async fn handle_request(
    store: Arc<StoreSlot>,
    local: Option<SocketAddr>,
    request: Request<Incoming>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    if !origin_is_acceptable(&request) {
        return Ok(json_response(
            StatusCode::FORBIDDEN,
            &rpc_error_body(
                Value::Null,
                INVALID_REQUEST,
                "cross-origin requests are refused: this endpoint serves a private code index \
and has no browser client",
            ),
        ));
    }

    if !host_is_acceptable(&request, local) {
        return Ok(json_response(
            StatusCode::FORBIDDEN,
            &rpc_error_body(
                Value::Null,
                INVALID_REQUEST,
                "this listener is on loopback and the request named a routable host: nothing \
can legitimately reach 127.0.0.1 under a public name, so this is a rebound request",
            ),
        ));
    }

    if request.method() != Method::POST {
        // The modern transport is POST-only. A GET here is usually a client
        // reaching for the legacy SSE stream, so the message says which
        // transport this is rather than only which verb was wrong.
        //
        // `Allow` because RFC 9110 §15.5.6 makes it a MUST on a 405, and because
        // without it a client that got here has to guess.
        let body = rpc_error_body(
            Value::Null,
            INVALID_REQUEST,
            "this endpoint speaks the MCP 2026-07-28 single-exchange transport: one \
JSON-RPC request per POST. It has no SSE stream and no session.",
        );
        let mut response = json_response(StatusCode::METHOD_NOT_ALLOWED, &body);
        response
            .headers_mut()
            .insert(header::ALLOW, header::HeaderValue::from_static("POST"));
        return Ok(response);
    }

    if !content_type_is_json(&request) {
        return Ok(json_response(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            &rpc_error_body(
                Value::Null,
                INVALID_REQUEST,
                format!(
                    "this endpoint reads only {JSON_MEDIA_TYPE}. Requiring it is also what \
forces a browser to preflight, and the preflight is refused."
                ),
            ),
        ));
    }

    if !accepts_json(&request) {
        return Ok(json_response(
            StatusCode::NOT_ACCEPTABLE,
            &rpc_error_body(
                Value::Null,
                INVALID_REQUEST,
                format!(
                    "this endpoint answers with {JSON_MEDIA_TYPE} and has no SSE stream; the \
request's Accept header excludes it."
                ),
            ),
        ));
    }

    let headers = MirroredHeaders::read(&request);

    let body = match read_body(request).await {
        Ok(body) => body,
        Err((status, message)) => {
            return Ok(json_response(
                status,
                &rpc_error_body(Value::Null, INVALID_REQUEST, message),
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
                    PARSE_ERROR,
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
                &rpc_error_body(Value::Null, PARSE_ERROR, err.to_string()),
            ))
        }
    };

    // "The body of the HTTP POST MUST be a single JSON-RPC request or
    // notification." Named as the fault it is: an array reported as "no string
    // 'method'" sends a client looking for a missing field in a body that has no
    // fields, and batching is a thing the client may well believe it can do —
    // the stdio transport here does accept it.
    if !parsed.is_object() {
        return Ok(json_response(
            StatusCode::BAD_REQUEST,
            &rpc_error_body(
                Value::Null,
                INVALID_REQUEST,
                if parsed.is_array() {
                    "a batch is not a valid body for this transport: the MCP 2026-07-28 \
Streamable HTTP binding takes a single JSON-RPC request or notification per POST. Send one \
request per POST."
                } else {
                    "the body must be a single JSON-RPC request or notification object"
                },
            ),
        ));
    }

    // The id is recovered before dispatch so that an error can be reported
    // against it. A client matching responses to ids cannot use a frame whose id
    // is null, so losing the id turns a reportable failure into an unmatchable
    // one.
    //
    // Presence, not value: a body with no `id` member is a notification, and a
    // notification is answered `202` with nothing in it.
    let is_request = parsed.get("id").is_some();
    let id = parsed.get("id").cloned().unwrap_or(Value::Null);

    // The same narrowing stdio applies, for the same reason and in the same
    // words: "Requests MUST include a string or integer ID. Unlike base
    // JSON-RPC, the ID MUST NOT be `null`." Checked here too rather than only in
    // the dispatcher because the id is read by this layer — it is what the error
    // body is addressed to — so this layer is where an unusable one has to be
    // caught. The offending value is echoed so the client can still resolve the
    // call, exactly as on stdio.
    if is_request {
        if let Some(fault) = crate::mcp::id_fault(&id) {
            return Ok(json_response(
                StatusCode::BAD_REQUEST,
                &rpc_error_body(
                    id,
                    INVALID_REQUEST,
                    format!(
                        "a request id must be a string or an integer and must not be null; this \
one is {fault}. The request was not run."
                    ),
                ),
            ));
        }
    }

    if parsed.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Ok(json_response(
            StatusCode::BAD_REQUEST,
            &rpc_error_body(id, INVALID_REQUEST, "jsonrpc must be \"2.0\""),
        ));
    }
    let method = match parsed.get("method").and_then(Value::as_str) {
        Some(method) => method.to_string(),
        None => {
            return Ok(json_response(
                StatusCode::BAD_REQUEST,
                &rpc_error_body(id, INVALID_REQUEST, "request has no string 'method'"),
            ))
        }
    };

    // Only for requests. The revision states outright that "header requirements
    // for notification POSTs are not defined by this revision", and the `_meta`
    // protocol fields are specified for *client requests* — a notification that
    // carried neither would be refused for a rule that was never written.
    if is_request {
        if let Some((status, body)) = headers.check(&parsed, &id, &method) {
            return Ok(json_response(status, &body));
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
                    INTERNAL_ERROR,
                    format!(
                        "exchange exceeded {}s and was abandoned",
                        EXCHANGE_TIMEOUT.as_secs()
                    ),
                ),
            ))
        }
    };

    match (outcome, is_request) {
        // "If the server accepts it, the server MUST return HTTP status code
        // 202 Accepted with no body."
        (Ok(_), false) => Ok(accepted()),
        // No cache hint is written here. `tools/list` and `server/discover`
        // carry their own `ttlMs`/`cacheScope` out of the shared dispatcher,
        // where they are required fields of `ListToolsResult` and
        // `DiscoverResult` rather than an HTTP decoration. This module used to
        // insert the same two values a second time on top of the ones
        // `discover_result` had already written — a second owner writing the
        // same field, which is one edit away from two owners writing different
        // ones.
        (Ok(result), true) => Ok(json_response(
            StatusCode::OK,
            &json!({"jsonrpc": "2.0", "id": id, "result": result}),
        )),
        // "If the server cannot accept it, it MUST return an HTTP error status
        // code. The HTTP response body MAY comprise a JSON-RPC error response
        // that has no `id`." An unknown notification method is the case: 202
        // would claim this server acted on something it discarded.
        (Err(err), false) => Ok(json_response(
            status_for(err.code()),
            &with_error_data(
                json!({"jsonrpc": "2.0", "error": {"code": err.code(), "message": err.message()}}),
                &err,
            ),
        )),
        (Err(err), true) => Ok(json_response(
            status_for(err.code()),
            &with_error_data(rpc_error_body(id, err.code(), err.message()), &err),
        )),
    }
}

/// Attach the error's recovery data to the frame, when it has any.
///
/// `UnsupportedProtocolVersionError` is why this exists: `data.supported` is the
/// client's entire documented recovery path, and dropping it on the way out of
/// the dispatcher would turn a refusal the client can act on into one it can
/// only report. Omitted rather than written as `null` when there is none, so
/// "no recovery information" and "recovery information that is null" stay
/// distinguishable.
fn with_error_data(mut frame: Value, error: &crate::mcp::RpcError) -> Value {
    if let (Some(data), Some(object)) = (
        error.data(),
        frame.get_mut("error").and_then(Value::as_object_mut),
    ) {
        object.insert("data".to_string(), data.clone());
    }
    frame
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
        // The address this connection was accepted *on*, not the peer's. It is
        // what tells `host_is_acceptable` whether the listener is on loopback,
        // and therefore whether a `Host` naming a routable name could ever be
        // legitimate. Read per connection because a caller may bind anywhere.
        let local = stream.local_addr().ok();
        let store = Arc::clone(&store);
        tokio::spawn(async move {
            let io = TokioIo::new(stream);
            let service =
                service_fn(move |request| handle_request(Arc::clone(&store), local, request));
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
