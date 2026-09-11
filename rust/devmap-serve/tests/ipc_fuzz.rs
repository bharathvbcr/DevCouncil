//! Frame-level abuse of the IPC surface, driven through `handle_stream`.
//!
//! The in-crate tests reach `validate_request` and `dispatch` directly, so they
//! start from an `IpcRequest` that already parsed. Everything that can go wrong
//! *before* that — a frame that is not JSON, is JSON of the wrong shape, is
//! larger than the buffer bound, nests deeper than the parser's stack, carries
//! invalid UTF-8 or duplicate keys, or names a number no `u32` can hold — is
//! only reachable over the wire, which is what this file drives.
//!
//! Two properties are asserted on *every* exchange, hostile or not, because
//! they are what the caller depends on:
//!
//! 1. Exactly one newline-terminated JSON object comes back, carrying
//!    `protocol_version`. `DevMapClient._send_socket_request` raises on a
//!    response that is not newline-framed and on any trailing bytes after it.
//! 2. The connection task returns rather than hanging, panicking, or dropping
//!    the peer without a word.
//!
//! No fuzzer and no `proptest`: the corpus below is written out, so a failure
//! names the exact bytes and a rerun reproduces it.

use devmap_serve::{handle_stream, PROTOCOL_VERSION};
use devmap_store::Store;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const MAX_QUERY_BYTES: usize = 4 * 1024;
const MAX_TOKEN_BUDGET: u32 = devmap_query::MAX_TOKEN_BUDGET;
const MAX_TRAVERSAL_DEPTH: usize = devmap_query::MAX_TRAVERSAL_DEPTH;
const MAX_NEIGHBOR_TARGETS: usize = devmap_query::MAX_NEIGHBOR_TARGETS;

/// Every exchange in this file is bounded by this. A hang is a failure with a
/// name, not a test run that never ends.
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(20);

/// A store with a real call graph, so a request that survives validation has
/// something to answer with and an empty answer means something.
fn corpus() -> Arc<Store> {
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
    Arc::new(store)
}

/// Send `frame` verbatim, half-close, and read the whole reply.
///
/// The bytes are sent exactly as given — no newline is appended — so a test can
/// drive a truncated frame, a frame with an embedded NUL, or one that is not
/// valid UTF-8. The half-close is what `read_frame` treats as end-of-frame for
/// clients that do not send a trailing newline.
async fn exchange(store: Arc<Store>, frame: &[u8]) -> Vec<u8> {
    let (client, server) = tokio::io::duplex(64 * 1024);
    let task = tokio::spawn(handle_stream(server, store));
    let (mut reader, mut writer) = tokio::io::split(client);
    let written = frame.to_vec();
    // Shutting the write half is the half-close `read_frame` treats as
    // end-of-frame; without it a frame with no trailing newline would sit
    // unread until the request deadline, which is a different test.
    let pump = tokio::spawn(async move {
        let _ = writer.write_all(&written).await;
        let _ = writer.shutdown().await;
    });
    let mut reply = Vec::new();
    tokio::time::timeout(EXCHANGE_TIMEOUT, reader.read_to_end(&mut reply))
        .await
        .unwrap_or_else(|_| {
            panic!(
                "the daemon never answered a {}-byte frame within {EXCHANGE_TIMEOUT:?}",
                frame.len()
            )
        })
        .expect("reading the reply failed");
    let _ = tokio::time::timeout(EXCHANGE_TIMEOUT, pump)
        .await
        .expect("the writer task did not finish");
    let _ = tokio::time::timeout(EXCHANGE_TIMEOUT, task)
        .await
        .expect("the connection task did not return");
    reply
}

/// The two properties every reply must satisfy, plus the parsed envelope.
fn parse_envelope(label: &str, reply: &[u8]) -> Value {
    assert!(
        !reply.is_empty(),
        "{label}: the daemon closed the connection without an answer"
    );
    assert_eq!(
        reply.last().copied(),
        Some(b'\n'),
        "{label}: the reply is not newline-framed; the Python client raises on \
         exactly this. Got: {}",
        String::from_utf8_lossy(&reply[reply.len().saturating_sub(80)..])
    );
    let body = &reply[..reply.len() - 1];
    assert!(
        !body.contains(&b'\n'),
        "{label}: the reply carries more than one frame; the Python client \
         rejects trailing protocol data"
    );
    let value: Value = serde_json::from_slice(body).unwrap_or_else(|error| {
        panic!(
            "{label}: the reply is not JSON ({error}): {}",
            String::from_utf8_lossy(&body[..body.len().min(200)])
        )
    });
    assert_eq!(
        value["protocol_version"], PROTOCOL_VERSION,
        "{label}: every envelope must name the protocol version it speaks"
    );
    assert!(
        value["ok"].is_boolean(),
        "{label}: `ok` must be a boolean, got {:?}",
        value["ok"]
    );
    if value["ok"] == Value::Bool(false) {
        assert!(
            value["error"]["code"].is_string() && value["error"]["message"].is_string(),
            "{label}: a refusal must carry a code and a message, got {:?}",
            value["error"]
        );
        assert!(
            value["result"].is_null(),
            "{label}: a refusal must not also carry a result"
        );
    } else {
        assert!(
            !value["result"].is_null(),
            "{label}: a success must carry a result"
        );
        assert!(
            value["error"].is_null(),
            "{label}: a success must not also carry an error"
        );
    }
    value
}

async fn refusal(store: Arc<Store>, label: &str, frame: &[u8]) -> String {
    let value = parse_envelope(label, &exchange(store, frame).await);
    assert_eq!(
        value["ok"],
        Value::Bool(false),
        "{label}: was accepted; envelope {value}"
    );
    value["error"]["code"].as_str().unwrap().to_string()
}

async fn accepted(store: Arc<Store>, label: &str, frame: &[u8]) -> Value {
    let value = parse_envelope(label, &exchange(store, frame).await);
    assert_eq!(
        value["ok"],
        Value::Bool(true),
        "{label}: was refused; envelope {value}"
    );
    value["result"].clone()
}

// ---------------------------------------------------------------------------
// Frames that are not requests
// ---------------------------------------------------------------------------

/// Bytes that are not a well-formed request are refused with a structured
/// error, one frame, every time.
///
/// The corpus covers empty and whitespace frames, every JSON scalar in the
/// top-level position, unbalanced and truncated documents, a bare newline,
/// interior NUL bytes, invalid UTF-8, a UTF-8 BOM, and two requests packed into
/// one frame. Coverage is accounted for at the end: nothing here is a sample
/// reported as a sweep.
#[tokio::test]
async fn malformed_frames_are_refused_one_envelope_at_a_time() {
    let store = corpus();
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("bare newline", b"\n".to_vec()),
        ("whitespace only", b"   \t  \n".to_vec()),
        ("open brace", b"{\n".to_vec()),
        ("close brace", b"}\n".to_vec()),
        ("empty array", b"[]\n".to_vec()),
        ("null", b"null\n".to_vec()),
        ("bare string", b"\"status\"\n".to_vec()),
        ("bare number", b"123\n".to_vec()),
        ("bare true", b"true\n".to_vec()),
        ("truncated mid-key", br#"{"version":1,"cmd":"stat"#.to_vec()),
        (
            "trailing garbage after object",
            br#"{"version":1,"cmd":"status"} trailing"#.to_vec(),
        ),
        (
            "two requests in one frame",
            br#"{"version":1,"cmd":"status"}{"version":1,"cmd":"status"}"#.to_vec(),
        ),
        ("single quotes", b"{'version':1,'cmd':'status'}\n".to_vec()),
        (
            "trailing comma",
            br#"{"version":1,"cmd":"status",}"#.to_vec(),
        ),
        (
            "interior NUL",
            b"{\"version\":1,\"cmd\":\"sta\0tus\"}\n".to_vec(),
        ),
        ("raw NUL only", b"\0\n".to_vec()),
        (
            "invalid utf-8 in the middle",
            [
                b"{\"version\":1,\"cmd\":\"".as_ref(),
                &[0xff, 0xfe, 0x80],
                b"\"}\n".as_ref(),
            ]
            .concat(),
        ),
        (
            "lone surrogate escape",
            br#"{"version":1,"cmd":"search","query":"\ud800"}"#.to_vec(),
        ),
        (
            "utf-8 BOM before the object",
            [b"\xef\xbb\xbf".as_ref(), br#"{"version":1,"cmd":"status"}"#].concat(),
        ),
        ("comment syntax", b"// hi\n".to_vec()),
    ];

    let mut refused = 0usize;
    let mut accepted_anyway: Vec<&str> = Vec::new();
    for (label, frame) in &cases {
        let value = parse_envelope(label, &exchange(Arc::clone(&store), frame).await);
        if value["ok"] == Value::Bool(false) {
            let code = value["error"]["code"].as_str().unwrap();
            assert!(
                matches!(code, "invalid_request" | "incomplete_request"),
                "{label}: refused with an unexpected code {code:?}; a malformed \
                 frame must not be reported as a query or parameter failure"
            );
            refused += 1;
        } else {
            accepted_anyway.push(label);
        }
    }
    assert!(
        accepted_anyway.is_empty(),
        "these malformed frames were accepted: {accepted_anyway:?}"
    );
    assert_eq!(
        refused,
        cases.len(),
        "coverage accounting is wrong: {refused} refused of {} swept",
        cases.len()
    );
    assert_eq!(cases.len(), 20, "the malformed-frame corpus changed size");
}

/// A peer that closes without sending anything gets a structured refusal, not
/// silence — and a peer that half-closes after a complete object is served.
#[tokio::test]
async fn a_closed_and_a_half_closed_peer_are_told_apart() {
    let store = corpus();
    let code = refusal(Arc::clone(&store), "empty frame", b"").await;
    assert_eq!(
        code, "incomplete_request",
        "a peer that sent nothing must be told the frame was incomplete"
    );

    // No trailing newline: the half-close ends the frame.
    let result = accepted(
        store,
        "half-closed status",
        br#"{"version":1,"cmd":"status"}"#,
    )
    .await;
    assert!(
        result["node_count"].is_number(),
        "a half-closed complete frame must still be served: {result}"
    );
}

/// A deeply nested payload is refused by the parser rather than overflowing the
/// stack.
///
/// `IpcRequest` uses `#[serde(flatten)]`, which buffers the whole document into
/// `serde_json::Value`-shaped content before the enum is matched, so the
/// nesting is *walked* even though no field wants it. A recursion limit that
/// did not apply here would be a daemon crash — and under the release profile,
/// which sets `panic = "abort"`, a stack overflow takes the process down with
/// no envelope for the caller.
#[tokio::test]
async fn a_deeply_nested_payload_is_refused_rather_than_overflowing_the_stack() {
    let store = corpus();
    let nested = |depth: usize| {
        let mut frame = String::from(r#"{"version":1,"cmd":"status","pad":"#);
        frame.push_str(&"[".repeat(depth));
        frame.push_str(&"]".repeat(depth));
        frame.push('}');
        assert!(
            frame.len() <= MAX_REQUEST_BYTES,
            "depth {depth} exceeds the frame bound, so the size check would \
             answer instead of the parser"
        );
        frame
    };

    // Shallow enough that no plausible parser limit is in play: still served.
    for depth in [1usize, 8, 64, 100] {
        let value = parse_envelope(
            &format!("nesting depth {depth}"),
            &exchange(Arc::clone(&store), nested(depth).as_bytes()).await,
        );
        assert_eq!(
            value["ok"],
            Value::Bool(true),
            "an unremarkable nesting depth of {depth} stopped being served, so \
             the refusals below prove nothing about depth: {value}"
        );
    }

    // Far past any parser limit, up to a frame that is nearly the whole 1 MiB
    // ceiling: refused with an envelope, not a stack overflow. Under the
    // release profile `panic = "abort"`, an overflow here would take the
    // daemon down without answering anyone.
    for depth in [1_000usize, 50_000, 500_000] {
        let value = parse_envelope(
            &format!("nesting depth {depth}"),
            &exchange(Arc::clone(&store), nested(depth).as_bytes()).await,
        );
        assert_eq!(
            value["ok"],
            Value::Bool(false),
            "nesting depth {depth} was accepted; if the parser's recursion \
             limit was lifted, one frame can crash this daemon"
        );
        assert_eq!(
            value["error"]["code"], "invalid_request",
            "a frame the parser refused must be reported as an unparseable \
             request: {value}"
        );
    }

    // The daemon is still serving afterwards, which a crashed one would not be.
    let after = accepted(
        store,
        "after the deep frames",
        br#"{"version":1,"cmd":"status"}"#,
    )
    .await;
    assert!(after["node_count"].is_number());
}

/// The frame bound is exclusive, and both sides of it behave.
///
/// One byte under the ceiling is served; one byte over is refused as
/// `request_too_large`. The in-crate test drives a frame far past the bound,
/// which passes just as well against a `>=` or an `==`.
#[tokio::test]
async fn the_frame_size_bound_is_exclusive_on_both_sides() {
    let store = corpus();
    // `{"version":1,"cmd":"search","query":"x…"}` — pad the query so the whole
    // frame lands exactly on a chosen length. The query itself is far past
    // MAX_QUERY_BYTES, so the *accepted* frame is still refused by
    // `validate_request`; that is the point, it proves the frame was read
    // rather than cut.
    let build = |total: usize| {
        let prefix = br#"{"version":1,"cmd":"search","query":""#;
        let suffix = br#""}"#;
        let pad = total - prefix.len() - suffix.len();
        [prefix.as_ref(), "x".repeat(pad).as_bytes(), suffix.as_ref()].concat()
    };

    let at_limit = build(MAX_REQUEST_BYTES);
    assert_eq!(at_limit.len(), MAX_REQUEST_BYTES);
    let code = refusal(Arc::clone(&store), "frame exactly at the bound", &at_limit).await;
    assert_eq!(
        code, "invalid_parameters",
        "a frame exactly at the ceiling must be read and then judged on its \
         contents, not refused for its size"
    );

    let over = build(MAX_REQUEST_BYTES + 1);
    assert_eq!(over.len(), MAX_REQUEST_BYTES + 1);
    let code = refusal(store, "frame one byte over the bound", &over).await;
    assert_eq!(
        code, "request_too_large",
        "one byte past the ceiling must be refused for its size"
    );
}

// ---------------------------------------------------------------------------
// Requests that parse but should not be served
// ---------------------------------------------------------------------------

/// Version handling: only the exact protocol version is served, and a version
/// of the wrong JSON type is a parse failure rather than a mismatch.
#[tokio::test]
async fn only_the_exact_protocol_version_is_served() {
    let store = corpus();
    for wrong in ["0", "2", "99", "4294967295"] {
        let frame = format!(r#"{{"version":{wrong},"cmd":"status"}}"#);
        let code = refusal(
            Arc::clone(&store),
            &format!("version {wrong}"),
            frame.as_bytes(),
        )
        .await;
        assert_eq!(
            code, "request_failed",
            "version {wrong} must be refused by dispatch"
        );
    }
    for malformed in ["-1", "1.5", "\"1\"", "null", "4294967296"] {
        let frame = format!(r#"{{"version":{malformed},"cmd":"status"}}"#);
        let code = refusal(
            Arc::clone(&store),
            &format!("version {malformed}"),
            frame.as_bytes(),
        )
        .await;
        assert_eq!(
            code, "invalid_request",
            "version {malformed} is not a u32 and must fail to parse"
        );
    }
    let result = accepted(store, "version 1", br#"{"version":1,"cmd":"status"}"#).await;
    assert!(
        result["generation_id"].is_number(),
        "the matching version stopped being served, so the refusals above prove \
         nothing"
    );
}

/// An unknown or mis-typed `cmd` is refused, never guessed at.
#[tokio::test]
async fn an_unknown_command_is_refused_rather_than_guessed() {
    let store = corpus();
    for frame in [
        r#"{"version":1,"cmd":"drop_store"}"#,
        r#"{"version":1,"cmd":"STATUS"}"#,
        r#"{"version":1,"cmd":"neighbours"}"#,
        r#"{"version":1,"cmd":""}"#,
        r#"{"version":1,"cmd":null}"#,
        r#"{"version":1,"cmd":1}"#,
        r#"{"version":1,"cmd":["status"]}"#,
        r#"{"version":1}"#,
    ] {
        let code = refusal(Arc::clone(&store), frame, frame.as_bytes()).await;
        assert_eq!(
            code, "invalid_request",
            "{frame} was not refused as unparseable"
        );
    }
}

/// Numbers outside their field's type are refused at the parser, and numbers
/// inside the type but outside the policy bound are refused at validation.
///
/// The distinction matters: the first is `invalid_request` (this is not a
/// request), the second is `invalid_parameters` (this is a request I will not
/// serve). A client retries those differently.
#[tokio::test]
async fn out_of_range_numbers_are_refused_at_the_right_layer() {
    let store = corpus();

    // Not representable in the field's type.
    for (label, frame) in [
        (
            "budget 2^32",
            r#"{"version":1,"cmd":"status_x","budget":4294967296}"#,
        ),
        (
            "negative budget",
            r#"{"version":1,"cmd":"search","query":"a","budget":-1}"#,
        ),
        (
            "float budget",
            r#"{"version":1,"cmd":"search","query":"a","budget":1.5}"#,
        ),
        (
            "budget 2^32",
            r#"{"version":1,"cmd":"search","query":"a","budget":4294967296}"#,
        ),
        (
            "budget 1e20",
            r#"{"version":1,"cmd":"search","query":"a","budget":1e20}"#,
        ),
        (
            "negative depth",
            r#"{"version":1,"cmd":"impact","target":"core.py","depth":-1}"#,
        ),
        (
            "depth 2^64",
            r#"{"version":1,"cmd":"impact","target":"core.py","depth":18446744073709551616}"#,
        ),
        (
            "min_confidence as a string",
            r#"{"version":1,"cmd":"deps","target":"core.py","min_confidence":"0.5"}"#,
        ),
        (
            "targets not an array",
            r#"{"version":1,"cmd":"neighbors","targets":"core.py"}"#,
        ),
        (
            "targets of numbers",
            r#"{"version":1,"cmd":"neighbors","targets":[1,2]}"#,
        ),
        (
            "targets of nulls",
            r#"{"version":1,"cmd":"neighbors","targets":[null]}"#,
        ),
    ] {
        let code = refusal(Arc::clone(&store), label, frame.as_bytes()).await;
        assert_eq!(
            code, "invalid_request",
            "{label} ({frame}) must fail to parse, not fail validation"
        );
    }

    // Representable, but past the policy bound.
    for (label, frame) in [
        (
            "budget past the ceiling",
            format!(
                r#"{{"version":1,"cmd":"search","query":"a","budget":{}}}"#,
                MAX_TOKEN_BUDGET + 1
            ),
        ),
        (
            "depth past the ceiling",
            format!(
                r#"{{"version":1,"cmd":"impact","target":"core.py","depth":{}}}"#,
                MAX_TRAVERSAL_DEPTH + 1
            ),
        ),
        (
            "depth at usize::MAX",
            r#"{"version":1,"cmd":"impact","target":"core.py","depth":18446744073709551615}"#
                .to_string(),
        ),
        (
            "min_confidence just over one",
            r#"{"version":1,"cmd":"deps","target":"core.py","min_confidence":1.0000001}"#
                .to_string(),
        ),
        (
            "min_confidence negative",
            r#"{"version":1,"cmd":"deps","target":"core.py","min_confidence":-0.0001}"#.to_string(),
        ),
    ] {
        let code = refusal(Arc::clone(&store), label, frame.as_bytes()).await;
        assert_eq!(
            code, "invalid_parameters",
            "{label} ({frame}) must be refused as a parameter, not as a parse \
             failure"
        );
    }
}

/// JSON has no NaN or Infinity literal, but `1e400` overflows to infinity in
/// every conforming parser — and that is the one way a non-finite
/// `min_confidence` can reach the daemon over the wire.
///
/// It matters because the engine's two confidence filters disagree completely
/// about NaN: `Store::latest_edges` admits everything (`NaN as i64` saturates
/// to 0) while `Store::latest_edges_for_file` admits nothing (SQLite treats NaN
/// as NULL). A non-finite value that got past `validate_request` would produce
/// an `Available` empty `callees` beside a fully populated `callers` — which is
/// exactly what `devmap neighbors --min-confidence nan` does today, because the
/// CLI has no equivalent of this check.
#[tokio::test]
async fn a_float_that_overflows_to_infinity_cannot_reach_the_confidence_filter() {
    let store = corpus();
    for literal in ["1e400", "-1e400", "1e999", "1e-400", "-1e-400"] {
        let frame = format!(
            r#"{{"version":1,"cmd":"neighbors","targets":["core.py"],"min_confidence":{literal}}}"#
        );
        let value = parse_envelope(
            literal,
            &exchange(Arc::clone(&store), frame.as_bytes()).await,
        );
        if literal.contains("e-") {
            // 1e-400 underflows to +/-0.0, which is a legal confidence.
            assert_eq!(
                value["ok"],
                Value::Bool(true),
                "{literal} underflows to zero and is a legal confidence: {value}"
            );
            let entries = value["result"]["neighbors"].as_array().unwrap();
            assert!(
                entries[0]["callees"]["total"].as_u64().unwrap() > 0,
                "{literal} was accepted but behaved like a filter that admits \
                 nothing: {value}"
            );
        } else {
            assert_eq!(
                value["ok"],
                Value::Bool(false),
                "{literal} overflows to a non-finite float and was served; the \
                 engine's two confidence filters disagree completely about \
                 NaN/inf, so this would answer with a populated `callers` beside \
                 an `Available` empty `callees`: {value}"
            );
            // Refused at the parser (serde_json rejects an out-of-range float
            // outright) or at validation (`min_confidence must be finite`) —
            // either is a refusal, and which one it is has been unstable across
            // serde_json versions, so both are accepted here.
            let code = value["error"]["code"].as_str().unwrap();
            assert!(
                matches!(code, "invalid_request" | "invalid_parameters"),
                "{literal}: refused as {code:?}, which is neither a parse nor a \
                 parameter failure: {value}"
            );
        }
    }
}

/// Duplicate keys must not let a bound be set twice and enforced once.
///
/// `IpcRequest` flattens an internally-tagged enum, so the document is buffered
/// key by key before any field is claimed — a path where serde's usual
/// duplicate-field detection could plausibly be lost. It is not: every case
/// below is refused outright, naming the duplicated field. That is the
/// strongest available answer, and pinning it is what keeps a future
/// last-wins/first-wins resolution from quietly appearing between the value
/// `validate_request` judged and the value `dispatch` used.
///
/// Each pairing is driven both ways round, so a rule that only holds when the
/// legal value comes first cannot pass.
#[tokio::test]
async fn duplicate_keys_cannot_smuggle_a_value_past_validation() {
    let store = corpus();
    let over = MAX_TOKEN_BUDGET + 1;

    for (label, frame) in [
        (
            "legal budget then illegal",
            format!(
                r#"{{"version":1,"cmd":"search","query":"helper","budget":10,"budget":{over}}}"#
            ),
        ),
        (
            "illegal budget then legal",
            format!(
                r#"{{"version":1,"cmd":"search","query":"helper","budget":{over},"budget":10}}"#
            ),
        ),
        (
            "short query then oversized",
            format!(
                r#"{{"version":1,"cmd":"search","query":"a","query":"{}"}}"#,
                "x".repeat(MAX_QUERY_BYTES + 1)
            ),
        ),
        (
            "oversized query then short",
            format!(
                r#"{{"version":1,"cmd":"search","query":"{}","query":"a"}}"#,
                "x".repeat(MAX_QUERY_BYTES + 1)
            ),
        ),
        (
            "short target list then over-long",
            format!(
                r#"{{"version":1,"cmd":"neighbors","targets":["core.py"],"targets":[{}]}}"#,
                vec!["\"core.py\""; MAX_NEIGHBOR_TARGETS + 1].join(",")
            ),
        ),
        (
            "over-long target list then short",
            format!(
                r#"{{"version":1,"cmd":"neighbors","targets":[{}],"targets":["core.py"]}}"#,
                vec!["\"core.py\""; MAX_NEIGHBOR_TARGETS + 1].join(",")
            ),
        ),
        (
            "duplicate version",
            r#"{"version":1,"cmd":"status","version":99}"#.to_string(),
        ),
        (
            "duplicate cmd",
            r#"{"version":1,"cmd":"status","cmd":"dead"}"#.to_string(),
        ),
    ] {
        let value = parse_envelope(label, &exchange(Arc::clone(&store), frame.as_bytes()).await);
        assert_eq!(
            value["ok"],
            Value::Bool(false),
            "{label}: a duplicated key was resolved to one of its two values \
             and served. Whichever value won, `validate_request` and `dispatch` \
             must have seen the same one — re-check the bound before relaxing \
             this: {value}"
        );
        assert_eq!(
            value["error"]["code"], "invalid_request",
            "{label}: a duplicated key is a malformed request, not a bad \
             parameter: {value}"
        );
        assert!(
            value["error"]["message"]
                .as_str()
                .unwrap()
                .contains("duplicate field"),
            "{label}: the refusal must name what was duplicated: {value}"
        );
    }

    // The same requests with the duplicate removed are served, so the refusals
    // above are about the duplication and not about the commands.
    for frame in [
        r#"{"version":1,"cmd":"search","query":"helper","budget":10}"#,
        r#"{"version":1,"cmd":"status"}"#,
        r#"{"version":1,"cmd":"neighbors","targets":["core.py"]}"#,
    ] {
        let result = accepted(Arc::clone(&store), frame, frame.as_bytes()).await;
        assert!(!result.is_null());
    }
}

/// Fields nobody declared are ignored, which is what makes a newer client's
/// request serviceable by an older daemon.
///
/// The cost of that is pinned here rather than hidden: a *misspelled* field is
/// indistinguishable from a field this daemon predates, so `"budgt": 25` is
/// silently dropped and the caller is served against the 2000-token default it
/// did not ask for. The same shape `Clones` refuses explicitly for its `kind`
/// string.
#[tokio::test]
async fn unknown_fields_are_ignored_including_misspelled_ones() {
    let store = corpus();

    let tight = accepted(
        Arc::clone(&store),
        "spelled correctly",
        br#"{"version":1,"cmd":"deps","target":"core.py","budget":25}"#,
    )
    .await;
    let default = accepted(
        Arc::clone(&store),
        "field omitted",
        br#"{"version":1,"cmd":"deps","target":"core.py"}"#,
    )
    .await;
    assert_ne!(
        tight["shown"], default["shown"],
        "the requested budget makes no difference on this fixture, so a dropped \
         field would be invisible and this test would assert nothing"
    );
    assert_eq!(tight["truncated"], Value::Bool(true));

    let misspelled = accepted(
        Arc::clone(&store),
        "misspelled field",
        br#"{"version":1,"cmd":"deps","target":"core.py","budgt":25}"#,
    )
    .await;
    assert_eq!(
        misspelled["shown"], default["shown"],
        "a misspelled `budget` now changes the answer; if unknown fields became \
         an error, this characterisation should be retired"
    );
    assert_eq!(
        misspelled["truncated"],
        Value::Bool(false),
        "the caller asked for a 25-token answer, was served a 2000-token one, \
         and is told it was not truncated"
    );

    let padded = accepted(
        store,
        "unknown field alongside good ones",
        br#"{"version":1,"cmd":"status","future_field":{"a":[1,2,3]}}"#,
    )
    .await;
    assert!(
        padded["node_count"].is_number(),
        "an unknown field must not stop a valid request being served"
    );
}

// ---------------------------------------------------------------------------
// `neighbors` over the wire
// ---------------------------------------------------------------------------

fn neighbors_frame(targets: &[&str], extra: &str) -> Vec<u8> {
    let list = targets
        .iter()
        .map(|target| serde_json::to_string(target).unwrap())
        .collect::<Vec<_>>()
        .join(",");
    format!(r#"{{"version":1,"cmd":"neighbors","targets":[{list}]{extra}}}"#).into_bytes()
}

/// The fan-out bound is exclusive over the wire, and the refusal names both
/// numbers.
#[tokio::test]
async fn the_neighbors_fan_out_bound_is_exclusive_over_the_wire() {
    let store = corpus();
    let at_limit = vec!["core.py"; MAX_NEIGHBOR_TARGETS];
    let result = accepted(
        Arc::clone(&store),
        "exactly the fan-out bound",
        &neighbors_frame(&at_limit, ""),
    )
    .await;
    assert_eq!(
        result["neighbors"].as_array().unwrap().len(),
        MAX_NEIGHBOR_TARGETS,
        "the bound must be inclusive of the limit itself"
    );

    let over = vec!["core.py"; MAX_NEIGHBOR_TARGETS + 1];
    let value = parse_envelope(
        "one past the fan-out bound",
        &exchange(Arc::clone(&store), &neighbors_frame(&over, "")).await,
    );
    assert_eq!(value["ok"], Value::Bool(false));
    assert_eq!(value["error"]["code"], "invalid_parameters");
    let message = value["error"]["message"].as_str().unwrap();
    assert!(
        message.contains(&MAX_NEIGHBOR_TARGETS.to_string())
            && message.contains(&over.len().to_string()),
        "the refusal must name the limit and the ask, got {message:?}"
    );
    assert!(
        value["result"].is_null(),
        "a refused fan-out must not carry a partial list of the first \
         {MAX_NEIGHBOR_TARGETS}"
    );
}

/// An oversized target anywhere in the list is refused, and the empty list is
/// served as the unambiguous request for nothing that it is.
#[tokio::test]
async fn a_neighbors_list_is_bounded_entry_by_entry_over_the_wire() {
    let store = corpus();
    let long = "q".repeat(MAX_QUERY_BYTES + 1);
    let at_limit = "q".repeat(MAX_QUERY_BYTES);

    for position in [0usize, MAX_NEIGHBOR_TARGETS / 2, MAX_NEIGHBOR_TARGETS - 1] {
        let mut targets = vec!["core.py"; MAX_NEIGHBOR_TARGETS];
        targets[position] = long.as_str();
        let code = refusal(
            Arc::clone(&store),
            &format!("oversized target at index {position}"),
            &neighbors_frame(&targets, ""),
        )
        .await;
        assert_eq!(
            code, "invalid_parameters",
            "an oversized target buried at index {position} rode in"
        );
    }

    let mut targets = vec!["core.py"; MAX_NEIGHBOR_TARGETS];
    targets[MAX_NEIGHBOR_TARGETS - 1] = at_limit.as_str();
    accepted(
        Arc::clone(&store),
        "target exactly at the entry bound",
        &neighbors_frame(&targets, ""),
    )
    .await;

    let empty = accepted(store, "empty target list", &neighbors_frame(&[], "")).await;
    assert_eq!(
        empty["neighbors"].as_array().unwrap().len(),
        0,
        "an empty list must answer with an empty list"
    );
}

/// A zero budget or depth is refused on the socket exactly as on the CLI.
///
/// Read back from the release binary: `search` with `budget: 0` answered
/// `ok: true, shown: 0, hidden: 26` over the socket while `devmap search
/// --budget 0` is refused ("a zero token budget returns an empty result that
/// cannot be told apart from a complete one"); `depth: 0` walked nothing and
/// answered an empty radius. The sweep above used to include both zeros and
/// asserted they were *served*; they are refusals, and the reason names the
/// parameter.
#[tokio::test]
async fn a_zero_budget_or_depth_is_refused_on_the_socket_as_on_the_cli() {
    let store = corpus();
    let cases: [(&str, &[u8], &str); 4] = [
        (
            "search budget 0",
            br#"{"version":1,"cmd":"search","query":"helper","budget":0}"#,
            "budget",
        ),
        (
            "impact depth 0",
            br#"{"version":1,"cmd":"impact","target":"core.py::helper","depth":0}"#,
            "depth",
        ),
        (
            "neighbors depth 0",
            br#"{"version":1,"cmd":"neighbors","targets":["core.py::helper"],"depth":0}"#,
            "depth",
        ),
        (
            "trace budget 0",
            br#"{"version":1,"cmd":"trace","from":"caller.py::main","to":"core.py::helper","budget":0}"#,
            "budget",
        ),
    ];
    for (label, frame, parameter) in cases {
        let value = parse_envelope(label, &exchange(Arc::clone(&store), frame).await);
        assert_eq!(value["ok"], Value::Bool(false), "{label}: {value}");
        assert_eq!(
            value["error"]["code"], "invalid_parameters",
            "{label}: a zero is a parameter refusal, not a request-shape one: {value}"
        );
        let message = value["error"]["message"].as_str().unwrap_or("");
        assert!(
            message.contains(parameter) && message.contains("at least 1"),
            "{label}: the refusal must name the parameter and its floor: {message:?}"
        );
    }
}

/// Every `neighbors` response the daemon will serve satisfies the invariants
/// `DevMapClient._budgeted` enforces on both directions of every entry.
///
/// The client raises on a violation, so a break here is a production error and
/// not a cosmetic one. Swept over hostile target shapes, the budget's floor and
/// ceiling, and depths on either side of the traversal clamp, with the coverage
/// accounted for. Zero is not in either list: a zero budget or depth is refused
/// before dispatch, on the socket as on the CLI — pinned in
/// `a_zero_budget_or_depth_is_refused_on_the_socket_as_on_the_cli` below.
#[tokio::test]
async fn every_served_neighbors_response_survives_the_client_s_invariants() {
    let store = corpus();
    let shapes = [
        "core.py",
        "core.py::helper",
        "caller.py",
        "",
        "   ",
        "::",
        "../core.py",
        "/etc/passwd",
        "nope.py",
        "\u{1F600}.py",
    ];
    let budgets = [1u32, 25, 2000, MAX_TOKEN_BUDGET];
    let depths = [1usize, MAX_TRAVERSAL_DEPTH];

    let mut checked = 0usize;
    let mut refused = 0usize;
    for budget in budgets {
        for depth in depths {
            let frame = neighbors_frame(
                &shapes,
                &format!(r#","budget":{budget},"depth":{depth},"min_confidence":0.0"#),
            );
            let value = parse_envelope(
                &format!("budget {budget} depth {depth}"),
                &exchange(Arc::clone(&store), &frame).await,
            );
            if value["ok"] == Value::Bool(false) {
                refused += 1;
                continue;
            }
            let entries = value["result"]["neighbors"].as_array().unwrap();
            assert_eq!(
                entries.len(),
                shapes.len(),
                "budget {budget} depth {depth}: {} entries for {} targets; the \
                 client zips these against its request and raises on a length \
                 mismatch",
                entries.len(),
                shapes.len()
            );
            for (entry, requested) in entries.iter().zip(shapes.iter()) {
                assert_eq!(
                    entry["target"].as_str().unwrap(),
                    *requested,
                    "the echoed target must be byte-identical to the request"
                );
                for side in ["callers", "callees"] {
                    let response = &entry[side];
                    let shown = response["shown"].as_u64().expect("shown");
                    let hidden = response["hidden"].as_u64().expect("hidden");
                    let total = response["total"].as_u64().expect("total");
                    let tokens = response["tokens_used"].as_u64().expect("tokens_used");
                    let truncated = response["truncated"].as_bool().expect("truncated");
                    let items = response["items"].as_array().expect("items");
                    let label = format!("{requested:?} {side} @ budget {budget} depth {depth}");
                    assert_eq!(shown as usize, items.len(), "{label}: shown != items");
                    assert_eq!(shown + hidden, total, "{label}: shown + hidden != total");
                    assert_eq!(truncated, hidden > 0, "{label}: truncated != (hidden > 0)");
                    assert!(
                        tokens <= budget as u64,
                        "{label}: spent {tokens} of {budget}"
                    );
                    checked += 1;
                }
            }
        }
    }
    let expected = budgets.len() * depths.len();
    assert_eq!(
        checked / (shapes.len() * 2) + refused,
        expected,
        "coverage accounting is wrong: {} served + {refused} refused != \
         {expected} combinations",
        checked / (shapes.len() * 2)
    );
    assert_eq!(refused, 0, "no combination in this sweep should be refused");
    assert_eq!(
        checked,
        expected * shapes.len() * 2,
        "{checked} sides checked; expected {}",
        expected * shapes.len() * 2
    );
}

// ---------------------------------------------------------------------------
// Load
// ---------------------------------------------------------------------------

/// Many connections at once, half of them hostile, all answered.
///
/// Each accepted connection buffers up to `MAX_REQUEST_BYTES` before anything
/// is validated, and the store is shared behind one mutex. This drives more
/// simultaneous exchanges than the daemon's own `MAX_CONCURRENT_CONNECTIONS`
/// admits at a time — `handle_stream` is called directly here, so the cap is
/// not in play and the store itself is what has to hold — and checks that every
/// one of them gets a well-formed envelope rather than a hang, a panic or a
/// poisoned mutex.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hundred_simultaneous_exchanges_are_all_answered() {
    let store = corpus();
    let targets = vec!["core.py", "core.py::helper", "caller.py"];
    let good = neighbors_frame(&targets, r#","budget":2000,"depth":3"#);
    let hostile: Vec<Vec<u8>> = vec![
        b"{".to_vec(),
        b"\n".to_vec(),
        br#"{"version":9,"cmd":"status"}"#.to_vec(),
        neighbors_frame(&vec!["core.py"; MAX_NEIGHBOR_TARGETS + 1], ""),
    ];

    let tasks: Vec<_> = (0..100)
        .map(|index| {
            let store = Arc::clone(&store);
            let frame = if index % 2 == 0 {
                good.clone()
            } else {
                hostile[(index / 2) % hostile.len()].clone()
            };
            tokio::spawn(async move { (index, exchange(store, &frame).await) })
        })
        .collect();

    let mut served = 0usize;
    let mut refused = 0usize;
    for task in tasks {
        let (index, reply) = tokio::time::timeout(EXCHANGE_TIMEOUT, task)
            .await
            .expect("a connection never finished under load")
            .expect("a connection task panicked");
        let value = parse_envelope(&format!("connection {index}"), &reply);
        if value["ok"] == Value::Bool(true) {
            assert_eq!(
                value["result"]["neighbors"].as_array().unwrap().len(),
                targets.len(),
                "connection {index} served a short fan-out under load"
            );
            served += 1;
        } else {
            refused += 1;
        }
    }
    assert_eq!(served + refused, 100, "connections went missing under load");
    assert_eq!(served, 50, "the well-formed half must all be served");
    assert_eq!(refused, 50, "the hostile half must all be refused");

    // The store is still usable afterwards: a poisoned mutex or a wedged
    // connection would show up here rather than in the counts above.
    let after = accepted(store, "after the load", br#"{"version":1,"cmd":"status"}"#).await;
    assert!(after["node_count"].as_u64().unwrap() > 0);
}
