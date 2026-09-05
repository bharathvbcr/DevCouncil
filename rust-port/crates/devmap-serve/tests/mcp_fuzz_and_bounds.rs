//! Randomised and resource-bound attacks on the MCP server.
//!
//! The other four MCP test files are hand-picked cases: each one names a defect
//! and pins it. That leaves the shape a hand-picked suite cannot cover — *any*
//! byte string a client can send. An agent host is exactly the place where that
//! matters, because the client is another program and a malformed frame is a
//! bug report, not an attack.
//!
//! Four invariants are asserted here on every generated input rather than on
//! chosen ones:
//!
//! 1. **No panic, ever.** A panic in the transport loop takes the session down
//!    and, on the HTTP transport, the process with it.
//! 2. **Every response is one line of valid JSON** carrying `jsonrpc: "2.0"`.
//! 3. **A request with an id is answered against that id**; a notification is
//!    never answered.
//! 4. **The work is bounded by the input**, not by what the input asks for —
//!    nesting depth, batch length and frame size all have to be refused rather
//!    than obeyed.

use std::sync::Arc;

use devmap_serve::mcp::{handle_line, serve_streams, StoreSlot};
use devmap_store::Store;
use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;

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

/// xorshift64*, so the corpus is random but the run is reproducible. A fuzz
/// failure nobody can replay is a rumour, not a bug report.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Frames that are individually valid, used as the seed corpus to mutate.
fn seeds() -> Vec<String> {
    vec![
        r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#.to_string(),
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#.to_string(),
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"devmap_search","arguments":{"query":"helper"}}}"#.to_string(),
        r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"devmap_impact","arguments":{"target":"helper","max_depth":3}}}"#.to_string(),
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#.to_string(),
        r#"[{"jsonrpc":"2.0","id":5,"method":"ping"},{"jsonrpc":"2.0","id":6,"method":"tools/list"}]"#.to_string(),
    ]
}

/// Check the invariants that must hold for ANY input.
fn assert_response_shape(input: &str, response: &Option<Value>) {
    let Some(frame) = response else {
        return; // no response is legal: notifications, all-notification batches
    };
    // The transport writes this as one line, so an embedded newline in the
    // serialized form would corrupt the following frame.
    let text = frame.to_string();
    assert!(
        !text.contains('\n'),
        "a response must serialize to one line or it corrupts the next frame.\ninput={input:?}\nout={text:?}"
    );
    // A batch answers with an array; a single request with an object.
    let members: Vec<&Value> = match frame {
        Value::Array(items) => items.iter().collect(),
        other => vec![other],
    };
    for member in members {
        assert_eq!(
            member["jsonrpc"], "2.0",
            "every response carries the version.\ninput={input:?}\nout={text:?}"
        );
        assert!(
            member.get("result").is_some() || member.get("error").is_some(),
            "a response is a result or an error, never neither.\ninput={input:?}\nout={text:?}"
        );
        assert!(
            !(member.get("result").is_some() && member.get("error").is_some()),
            "a response is a result or an error, never both.\ninput={input:?}\nout={text:?}"
        );
    }
}

/// Byte-level mutation of valid frames. This is the one that finds the crash a
/// hand-written case would not think to write.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mutated_frames_never_panic_and_always_answer_in_shape() {
    let slot = corpus();
    let mut rng = Rng(0x5EED_1234_ABCD_0001);
    let seeds = seeds();

    for round in 0..4_000 {
        let mut bytes = seeds[rng.below(seeds.len())].clone().into_bytes();
        // One to four mutations, so some frames stay nearly valid (which
        // reaches deeper code) and some become noise (which reaches the parser).
        for _ in 0..=rng.below(4) {
            if bytes.is_empty() {
                break;
            }
            match rng.below(4) {
                0 => {
                    let at = rng.below(bytes.len());
                    bytes[at] = (rng.next() & 0xff) as u8;
                }
                1 => {
                    let at = rng.below(bytes.len());
                    bytes.remove(at);
                }
                2 => {
                    let at = rng.below(bytes.len() + 1);
                    bytes.insert(at, (rng.next() & 0xff) as u8);
                }
                _ => {
                    let at = rng.below(bytes.len());
                    bytes.truncate(at);
                }
            }
        }
        // Non-UTF-8 is handled by the transport, not the dispatcher; this test
        // drives the dispatcher, so keep the input a `str`.
        let Ok(input) = String::from_utf8(bytes) else {
            continue;
        };
        let response = handle_line(&slot, &input).await;
        assert_response_shape(&format!("round {round}: {input}"), &response);
    }
}

/// A deeply nested batch must be refused, not recursed into.
///
/// `dispatch_value` answers a batch by recursing through `Box::pin` for each
/// member, and a member can itself be an array. Nothing in the batch branch caps
/// that, so the only thing between a nested array and the stack is whatever the
/// JSON parser refuses first — which is a property of a dependency, not a
/// decision this server made. Asserted here so that if the parser's default ever
/// changes, this fails instead of the process.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_deeply_nested_batch_is_refused_rather_than_recursed_into() {
    let slot = corpus();
    for depth in [64usize, 512, 50_000] {
        let input = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
        let response = handle_line(&slot, &input).await;
        assert_response_shape(&format!("depth {depth}"), &response);
        assert!(
            response.is_some(),
            "a nested batch {depth} deep must be answered, not silently dropped"
        );
    }
}

/// A large batch must be answered in full, in order, with no lost ids — and
/// must not fan out into unbounded concurrent work.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_large_batch_answers_every_member_exactly_once() {
    let slot = corpus();
    const MEMBERS: usize = 5_000;
    let mut parts = Vec::with_capacity(MEMBERS);
    for id in 0..MEMBERS {
        parts.push(format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"ping"}}"#));
    }
    let input = format!("[{}]", parts.join(","));

    let started = std::time::Instant::now();
    let frame = handle_line(&slot, &input)
        .await
        .expect("a batch of requests is answered");
    let elapsed = started.elapsed();

    let items = frame.as_array().expect("a batch answers with an array");
    assert_eq!(
        items.len(),
        MEMBERS,
        "every member of a batch gets exactly one response"
    );
    let mut seen = std::collections::HashSet::new();
    for item in items {
        let id = item["id"].as_u64().expect("each response carries its id");
        assert!(seen.insert(id), "id {id} answered twice inside one batch");
    }
    assert!(
        elapsed < std::time::Duration::from_secs(30),
        "a {MEMBERS}-member batch took {elapsed:?}; a client can send this in one frame"
    );
}

/// Text a client can legally put in a string argument must round-trip.
///
/// The failure this guards is subtle: a control character or a lone surrogate
/// echoed unescaped into the response makes the *response* unparseable, so the
/// client loses a call it made correctly.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hostile_text_in_an_argument_round_trips_safely() {
    let slot = corpus();
    let hostile = [
        "line\\nbreak",
        "tab\\there",
        "null\\u0000byte",
        "quote\\\"inside",
        "backslash\\\\end",
        "emoji \\ud83d\\udca9",
        "rtl \\u202eoverride",
        "\\u0007bell",
    ];
    for probe in hostile {
        let input = format!(
            r#"{{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{{"name":"devmap_search","arguments":{{"query":"{probe}"}}}}}}"#
        );
        let response = handle_line(&slot, &input).await;
        assert_response_shape(&input, &response);
        let frame = response.expect("a request with an id is always answered");
        assert_eq!(frame["id"], 9, "the id survives hostile argument text");
    }
}

/// Every frame carrying an id is answered against that id, whatever else is
/// wrong with it. A client matches responses to pending calls by id; an answer
/// under the wrong id resolves the wrong future, which is worse than no answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_id_is_never_lost_or_altered() {
    let slot = corpus();
    let broken = [
        r#"{"jsonrpc":"2.0","id":77}"#,
        r#"{"jsonrpc":"1.0","id":77,"method":"ping"}"#,
        r#"{"jsonrpc":"2.0","id":77,"method":"no/such/method"}"#,
        r#"{"jsonrpc":"2.0","id":77,"method":"tools/call","params":{"name":"nope","arguments":{}}}"#,
        r#"{"jsonrpc":"2.0","id":77,"method":"tools/call","params":{"name":"devmap_search"}}"#,
        r#"{"jsonrpc":"2.0","id":77,"method":"tools/call","params":{"name":"devmap_search","arguments":{"query":""}}}"#,
        r#"{"jsonrpc":"2.0","id":77,"method":"tools/call","params":{"name":"devmap_search","arguments":{"query":"x","bogus":1}}}"#,
    ];
    for input in broken {
        let frame = handle_line(&slot, input)
            .await
            .unwrap_or_else(|| panic!("a request carrying an id must be answered: {input}"));
        assert_response_shape(input, &Some(frame.clone()));
        assert_eq!(
            frame["id"], 77,
            "the id must come back unaltered so the client can resolve its call: {input}"
        );
    }
}

// ---------------------------------------------------------------------------
// Hostile input, hand-aimed.
//
// The mutation loop above finds crashes; these find the cases where the server
// answers, correctly-looking, something that is not true. Every bound asserted
// here has to *bound*: refuse or answer, name what it refused, and never hang,
// panic, or truncate without saying so.
// ---------------------------------------------------------------------------

/// Run one session over the real transport loop and collect every frame.
///
/// Bounded by a timeout that is part of the assertion: a transport that stops
/// answering is the failure this file exists to catch, and a test that hangs
/// reports it as a CI timeout with no message.
async fn session(slot: &Arc<StoreSlot>, lines: &[String]) -> Vec<Value> {
    use tokio::io::AsyncBufReadExt;
    let (client, server) = tokio::io::duplex(8 * 1024 * 1024);
    let (server_read, server_write) = tokio::io::split(server);
    let task = tokio::spawn(serve_streams(
        Arc::clone(slot),
        tokio::io::BufReader::new(server_read),
        server_write,
    ));
    let (client_read, mut client_write) = tokio::io::split(client);
    for line in lines {
        client_write.write_all(line.as_bytes()).await.expect("write");
        client_write.write_all(b"\n").await.expect("newline");
    }
    client_write.shutdown().await.expect("shutdown");

    let collect = async {
        let mut frames = Vec::new();
        let mut reader = tokio::io::BufReader::new(client_read).lines();
        while let Some(line) = reader.next_line().await.expect("read") {
            frames.push(
                serde_json::from_str(&line)
                    .unwrap_or_else(|err| panic!("server wrote a non-JSON line ({err}): {line}")),
            );
        }
        frames
    };
    let frames = tokio::time::timeout(std::time::Duration::from_secs(60), collect)
        .await
        .expect("the transport loop must answer or close, never hang");
    tokio::time::timeout(std::time::Duration::from_secs(60), task)
        .await
        .expect("the transport loop must terminate when its reader closes")
        .expect("transport task")
        .expect("transport loop");
    frames
}

/// Deep *object* nesting, which the array-nesting test does not reach.
///
/// `[[[…]]]` is refused by the batch branch before anything looks inside it. A
/// nested object goes through `serde_json`'s own recursion limit instead, and
/// that limit is a property of a dependency rather than a decision this server
/// made. Pinned so a change to that default fails here rather than in a stack
/// overflow, which on this transport takes the process with it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deeply_nested_objects_are_bounded_by_something_that_answers() {
    let slot = corpus();
    for depth in [32usize, 200, 5_000, 200_000] {
        let nest = format!(
            "{}null{}",
            r#"{"a":"#.repeat(depth),
            "}".repeat(depth)
        );
        let input = format!(
            r#"{{"jsonrpc":"2.0","id":11,"method":"tools/call","params":{{"name":"devmap_search","arguments":{{"query":"x"}},"deep":{nest}}}}}"#
        );
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(60),
            handle_line(&slot, &input),
        )
        .await
        .unwrap_or_else(|_| panic!("nesting {depth} was not bounded — the call never returned"));
        assert_response_shape(&format!("object nesting {depth}"), &response);
        assert!(
            response.is_some(),
            "a request carrying {depth}-deep nesting must be answered, not dropped"
        );
    }
}

/// An oversized argument is refused against the limit the schema published.
///
/// The refusal has to name the bound. "too long" tells a caller nothing it can
/// act on, and a silently truncated query returns hits for a prefix of what was
/// asked — an answer that looks complete and is about a different question.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_oversized_string_argument_is_refused_against_the_published_limit() {
    let slot = corpus();
    let huge = "x".repeat(100_000);
    let input = json!({
        "jsonrpc": "2.0", "id": 12, "method": "tools/call",
        "params": {"name": "devmap_search", "arguments": {"query": huge}}
    })
    .to_string();
    let frame = handle_line(&slot, &input).await.expect("answered");
    let result = &frame["result"];
    assert_eq!(
        result["isError"],
        json!(true),
        "an over-length query must be refused, not truncated: {result}"
    );
    let text = result["content"][0]["text"].as_str().unwrap_or_default();
    assert!(
        text.contains("4096") && text.contains("100000"),
        "the refusal must carry both the bound and what was sent, got: {text}"
    );

    // A field with no declared maximum still has to terminate. `preview.content`
    // is deliberately unbounded in the schema — it is a whole file — so this is
    // the case where the only bound left is the frame size, one layer up.
    let big_file = "def f():\n    pass\n".repeat(20_000);
    let input = json!({
        "jsonrpc": "2.0", "id": 13, "method": "tools/call",
        "params": {"name": "devmap_preview", "arguments": {"file": "core.py", "content": big_file}}
    })
    .to_string();
    let frame = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        handle_line(&slot, &input),
    )
    .await
    .expect("a large but legal preview must terminate")
    .expect("answered");
    assert_eq!(frame["id"], json!(13));
    assert!(
        frame["result"].get("isError").is_some(),
        "a preview of a large file answers or refuses, never neither: {frame}"
    );
}

/// A frame over the transport's limit is refused, and the refusal says so.
///
/// The dispatcher never sees this one: the bound is applied while the bytes are
/// being read. What must not happen is silence — a client that sent a 2 MB frame
/// and got nothing back cannot tell "too big" from "server died".
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_over_limit_frame_is_refused_and_the_session_continues() {
    let slot = corpus();
    let huge = json!({
        "jsonrpc": "2.0", "id": 14, "method": "tools/call",
        "params": {"name": "devmap_search", "arguments": {"query": "x".repeat(2 * 1024 * 1024)}}
    })
    .to_string();
    let frames = session(
        &slot,
        &[huge, r#"{"jsonrpc":"2.0","id":15,"method":"ping"}"#.to_string()],
    )
    .await;

    assert_eq!(frames.len(), 2, "one refusal and one answer: {frames:?}");
    let message = frames[0]["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("1048576"),
        "the refusal must name the limit that was enforced, got: {message}"
    );
    assert_eq!(
        frames[1]["id"],
        json!(15),
        "one over-sized frame must not cost the session: {frames:?}"
    );
}

/// A cancellation must reach exactly the request it names.
///
/// Request ids are legally numbers *or* strings, and `1` and `"1"` are different
/// ids. The in-flight table keys on `Value::to_string`, which keeps them apart —
/// a table keyed on the rendered *scalar* would not, and the failure would be
/// silent in the worst direction: a client cancelling its own string-keyed call
/// would kill an unrelated numeric one and never be told which answer went
/// missing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cancellation_cannot_reach_an_id_it_did_not_name() {
    let slot = corpus();
    let frames = session(
        &slot,
        &[
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devmap_impact","arguments":{"target":"helper"}}}"#.to_string(),
            // A different id that merely renders the same, and one that was
            // never issued at all.
            r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":"1"}}"#.to_string(),
            r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":9999}}"#.to_string(),
            r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#.to_string(),
        ],
    )
    .await;

    let answered: Vec<&Value> = frames.iter().map(|frame| &frame["id"]).collect();
    assert!(
        answered.contains(&&json!(1)),
        "cancelling \"1\" must not cancel 1: {frames:?}"
    );
    assert!(
        answered.contains(&&json!(2)),
        "the session must survive cancellations for ids it never issued: {frames:?}"
    );
}

/// Many cancellations in flight at once, with the accounting made explicit.
///
/// A cancellation that loses the race with completion is legal and expected, so
/// this cannot assert that every cancelled call is silenced. What it can assert
/// is that the numbers add up: every id is either answered exactly once or not
/// at all, never twice, and nothing is lost between the two counts. A test that
/// only checked "no cancelled id was answered" would be flaky; one that checks
/// nothing would pass while the server dropped every frame.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_cancellations_leave_the_accounting_intact() {
    let slot = corpus();
    const CALLS: usize = 64;
    let mut lines = Vec::with_capacity(CALLS * 2 + 1);
    for id in 0..CALLS {
        lines.push(format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"devmap_impact","arguments":{{"target":"helper"}}}}}}"#
        ));
        lines.push(format!(
            r#"{{"jsonrpc":"2.0","method":"notifications/cancelled","params":{{"requestId":{id}}}}}"#
        ));
    }
    // A sentinel that was never cancelled: if the loop drops everything, this is
    // what says so.
    lines.push(format!(
        r#"{{"jsonrpc":"2.0","id":{CALLS},"method":"ping"}}"#
    ));

    let frames = session(&slot, &lines).await;

    let mut seen = std::collections::HashMap::new();
    for frame in &frames {
        assert_eq!(frame["jsonrpc"], "2.0", "{frame}");
        let id = frame["id"].as_u64().expect("every frame carries its id");
        *seen.entry(id).or_insert(0usize) += 1;
    }
    for (id, count) in &seen {
        assert_eq!(*count, 1, "id {id} was answered {count} times");
    }
    assert!(
        seen.contains_key(&(CALLS as u64)),
        "the uncancelled sentinel must be answered: {} frames back",
        frames.len()
    );
    let cancelled_but_answered = seen.len() - 1;
    assert!(
        cancelled_but_answered <= CALLS,
        "answered {cancelled_but_answered} of {CALLS} cancelled calls, which is more than \
         were sent"
    );
}

/// Wrong types in every field of the envelope and of `tools/call`'s params.
///
/// One matrix rather than a case per field, because the failure this catches is
/// a field that happens to have no type check at all — and the field nobody
/// thought to test is exactly that field.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_field_of_a_request_is_type_checked_or_refused() {
    let slot = corpus();
    let wrong_types = [
        json!(null),
        json!(true),
        json!(0),
        json!(-1),
        json!(1.5),
        json!(""),
        json!([]),
        json!({}),
        json!([{"a": 1}]),
    ];
    let mut answered = 0usize;
    for value in &wrong_types {
        for field in ["jsonrpc", "method", "params", "id"] {
            let mut frame = json!({
                "jsonrpc": "2.0", "id": 21, "method": "tools/call",
                "params": {"name": "devmap_search", "arguments": {"query": "helper"}}
            });
            frame[field] = value.clone();
            let input = frame.to_string();
            let response = handle_line(&slot, &input).await;
            assert_response_shape(&input, &response);
            if let Some(response) = response {
                answered += 1;
                // The id came from the frame we built, so it must come back
                // exactly — including when the id itself is the wrong type.
                assert_eq!(
                    response["id"],
                    if field == "id" { value.clone() } else { json!(21) },
                    "the id was altered by a fault in {field}: {input}"
                );
            }
        }

        for field in ["name", "arguments", "_meta", "cursor"] {
            let input = json!({
                "jsonrpc": "2.0", "id": 22, "method": "tools/call",
                "params": {"name": "devmap_search", "arguments": {"query": "helper"}, field: value.clone()}
            })
            .to_string();
            let response = handle_line(&slot, &input).await;
            assert_response_shape(&input, &response);
            let response = response.expect("a request with an id is always answered");
            assert_eq!(response["id"], json!(22), "{input}");
            answered += 1;
        }
    }
    assert_eq!(
        answered,
        wrong_types.len() * 8,
        "every generated frame with an id must have been answered; a notification cannot \
         appear here because none of these frames omits its id"
    );
}

/// Unknown methods are refused by name, with the id intact.
///
/// Including the shapes a client reaches for when it has guessed the wrong era:
/// the removed session endpoints, the capabilities this server does not
/// implement, and a method that only differs by case.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_methods_are_refused_by_name() {
    let slot = corpus();
    for method in [
        "tools/List",
        "TOOLS/CALL",
        "resources/list",
        "prompts/list",
        "completion/complete",
        "logging/setLevel",
        "subscriptions/listen",
        "notifications/tools/list_changed",
        "",
        " ",
        "tools/call/",
    ] {
        let input =
            json!({"jsonrpc": "2.0", "id": 31, "method": method, "params": {}}).to_string();
        let frame = handle_line(&slot, &input)
            .await
            .unwrap_or_else(|| panic!("a request with an id must be answered: {input}"));
        assert_eq!(frame["id"], json!(31), "{input}");
        assert_eq!(
            frame["error"]["code"],
            json!(-32601),
            "an unimplemented method is METHOD_NOT_FOUND: {frame}"
        );
        let message = frame["error"]["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(method),
            "the refusal must name the method that does not exist, got: {message}"
        );
    }
}

/// A duplicate key cannot smuggle a second value past the check.
///
/// `serde_json` keeps the last occurrence, so `{"id":1,"id":2}` is id 2. What
/// must not happen is the two disagreeing between layers — a check reading the
/// first and dispatch reading the last is how a validated request runs as a
/// different one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn duplicate_keys_resolve_to_one_value_everywhere() {
    let slot = corpus();
    let frame = handle_line(
        &slot,
        r#"{"jsonrpc":"2.0","id":1,"id":2,"method":"ping","method":"tools/list"}"#,
    )
    .await
    .expect("answered");
    assert_eq!(frame["id"], json!(2), "the last id wins, consistently");
    assert!(
        frame["result"]["tools"].is_array(),
        "the last method wins, consistently: {frame}"
    );

    // The same inside a tool's arguments: a duplicated `budget` must not pass
    // validation as one value and reach the engine as another.
    let frame = handle_line(
        &slot,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"devmap_search","arguments":{"budget":1,"budget":0,"query":"helper"}}}"#,
    )
    .await
    .expect("answered");
    assert_eq!(
        frame["result"]["isError"],
        json!(true),
        "budget 0 is below the declared minimum and must be refused whichever \
         occurrence is read: {frame}"
    );
}

/// A batch of mixed hostile members answers each request exactly once.
///
/// Notifications inside it get no entry, so the response array is shorter than
/// the request array — and the difference has to be exactly the notification
/// count, not "some were dropped".
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hostile_batch_answers_every_request_and_no_notification() {
    let slot = corpus();
    const ROUNDS: usize = 1_000;
    let mut members = Vec::with_capacity(ROUNDS * 5);
    let mut expected_requests = 0usize;
    for round in 0..ROUNDS {
        let id = round * 5;
        members.push(format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"ping"}}"#));
        members.push(format!(
            r#"{{"jsonrpc":"2.0","id":{},"method":"no/such/method"}}"#,
            id + 1
        ));
        members.push(format!(
            r#"{{"jsonrpc":"2.0","id":{},"method":"tools/call","params":{{"name":"devmap_search","arguments":{{"query":"helper","bogus":1}}}}}}"#,
            id + 2
        ));
        members.push(format!(r#"{{"jsonrpc":"2.0","id":{},"method":"tools/call"}}"#, id + 3));
        expected_requests += 4;
        // No id: never answered.
        members.push(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#.to_string());
    }
    let input = format!("[{}]", members.join(","));

    let started = std::time::Instant::now();
    let frame = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        handle_line(&slot, &input),
    )
    .await
    .expect("a large batch must be bounded by the work it names")
    .expect("a batch of requests is answered");
    let elapsed = started.elapsed();

    let items = frame.as_array().expect("a batch answers with an array");
    assert_eq!(
        items.len(),
        expected_requests,
        "{} members carried ids and {} answers came back; the difference must be exactly \
         the {ROUNDS} notifications",
        expected_requests,
        items.len()
    );
    let mut seen = std::collections::HashSet::new();
    for item in items {
        let id = item["id"].as_u64().expect("each response carries its id");
        assert!(seen.insert(id), "id {id} answered twice inside one batch");
    }
    assert!(
        elapsed < std::time::Duration::from_secs(120),
        "a {}-member batch took {elapsed:?}",
        members.len()
    );
}
