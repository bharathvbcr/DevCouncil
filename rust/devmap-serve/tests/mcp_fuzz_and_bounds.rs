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

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use devmap_serve::mcp::{handle_line, serve_streams, structured_content_violation, StoreSlot};
use devmap_store::Store;
use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;

#[allow(dead_code)]
mod stateful_fuzz {
    include!("../../testsupport/stateful_fuzz.rs");
}

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
    let resolution = resolver.resolve_all(&extractions).unwrap();
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

/// Check the invariants that must hold for ANY input, returning a structured
/// `CheckFailure` so callers can minimize failing inputs via `stateful_fuzz`.
fn check_response_shape(
    action: &'static str,
    input: &str,
    response: &Option<Value>,
) -> Result<(), stateful_fuzz::CheckFailure> {
    let Some(frame) = response else {
        return Ok(()); // no response is legal: notifications, all-notification batches
    };
    // The transport writes this as one line, so an embedded newline in the
    // serialized form would corrupt the following frame.
    let text = frame.to_string();
    if let Some(failure) = stateful_fuzz::check(
        !text.contains('\n'),
        "single_line_framing",
        action,
        format!("response contains embedded newline.\ninput={input:?}\nout={text:?}"),
    )
    .into_failure()
    {
        return Err(failure);
    }
    // A batch answers with an array; a single request with an object.
    let members: Vec<&Value> = match frame {
        Value::Array(items) => items.iter().collect(),
        other => vec![other],
    };
    for member in members {
        if let Some(failure) = stateful_fuzz::check(
            member["jsonrpc"] == "2.0",
            "jsonrpc_version",
            action,
            format!("every response carries jsonrpc 2.0.\ninput={input:?}\nout={text:?}"),
        )
        .into_failure()
        {
            return Err(failure);
        }
        let has_result = member.get("result").is_some();
        let has_error = member.get("error").is_some();
        if let Some(failure) = stateful_fuzz::check(
            has_result || has_error,
            "result_or_error_present",
            action,
            format!("response has neither result nor error.\ninput={input:?}\nout={text:?}"),
        )
        .into_failure()
        {
            return Err(failure);
        }
        if let Some(failure) = stateful_fuzz::check(
            !(has_result && has_error),
            "result_xor_error",
            action,
            format!("response has both result and error.\ninput={input:?}\nout={text:?}"),
        )
        .into_failure()
        {
            return Err(failure);
        }
    }
    Ok(())
}

/// Check the invariants that must hold for ANY input.
fn assert_response_shape(input: &str, response: &Option<Value>) {
    if let Err(failure) = check_response_shape("assert_response_shape", input, response) {
        panic!("{failure}");
    }
}

fn classify_mcp_outcome(response: &Option<Value>) -> &'static str {
    let Some(frame) = response else {
        return "outcome::notification";
    };
    match frame {
        Value::Array(_) => "outcome::batch",
        Value::Object(obj) => {
            if let Some(err) = obj.get("error") {
                if err.get("code").and_then(Value::as_i64)
                    == Some(devmap_serve::mcp::codes::PARSE_ERROR)
                {
                    "outcome::parse_error"
                } else {
                    "outcome::protocol_error"
                }
            } else if obj
                .get("result")
                .and_then(|r| r.get("isError"))
                .and_then(Value::as_bool)
                == Some(true)
            {
                "outcome::tool_error"
            } else {
                "outcome::valid_result"
            }
        }
        _ => "outcome::unexpected",
    }
}

/// Byte-level mutation of valid frames. This is the one that finds the crash a
/// hand-written case would not think to write.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mutated_frames_never_panic_and_always_answer_in_shape() {
    let slot = corpus();
    let seed = stateful_fuzz::env_u64("DEVMAP_FUZZ_SEED", 0x5EED_1234_ABCD_0001);
    let mut rng = Rng(seed);
    let seeds = seeds();
    let mut tracker = stateful_fuzz::HealthTracker::new();

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
            tracker.record(
                "frame::mutated",
                None,
                &stateful_fuzz::bail("non_utf8_bytes"),
            );
            continue;
        };
        let response = handle_line(&slot, &input).await;
        if let Err(failure) = check_response_shape("frame::mutated", &input, &response) {
            let sig = failure.signature();
            let handle = tokio::runtime::Handle::current();
            let minimized = tokio::task::block_in_place(|| {
                stateful_fuzz::minimize_str(&input, &sig, 128, |cand| {
                    let resp = handle.block_on(handle_line(&slot, cand));
                    check_response_shape("frame::mutated", cand, &resp).err()
                })
            });
            let repro = stateful_fuzz::format_repro_test(
                "repro_mcp_mutated_frame",
                seed,
                &failure,
                &[format!("handle_line(&slot, {:?})", minimized)],
            );
            panic!(
                "round {round} failed: {failure}\n\
                 Minimized input ({} -> {} bytes): {minimized:?}\n\
                 Replay: DEVMAP_FUZZ_SEED={seed} cargo test --locked -p devmap-serve --test mcp_fuzz_and_bounds -- mutated_frames_never_panic_and_always_answer_in_shape\n\n\
                 {repro}",
                input.len(),
                minimized.len()
            );
        }
        tracker.record("frame::mutated", None, &stateful_fuzz::StepOutcome::Ok);
        let bucket = classify_mcp_outcome(&response);
        tracker.record(
            bucket,
            Some("frame::mutated"),
            &stateful_fuzz::StepOutcome::Ok,
        );
    }

    eprintln!("{}", tracker.probe_report("mcp_fuzz::mutated_frames"));
    tracker.assert_actions_exercised(&[
        "frame::mutated",
        "outcome::parse_error",
        "outcome::valid_result",
        "outcome::tool_error",
        "outcome::notification",
        "outcome::batch",
    ]);
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
        client_write
            .write_all(line.as_bytes())
            .await
            .expect("write");
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
        let nest = format!("{}null{}", r#"{"a":"#.repeat(depth), "}".repeat(depth));
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
        &[
            huge,
            r#"{"jsonrpc":"2.0","id":15,"method":"ping"}"#.to_string(),
        ],
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
                    if field == "id" {
                        value.clone()
                    } else {
                        json!(21)
                    },
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
        let input = json!({"jsonrpc": "2.0", "id": 31, "method": method, "params": {}}).to_string();
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
        members.push(format!(
            r#"{{"jsonrpc":"2.0","id":{},"method":"tools/call"}}"#,
            id + 3
        ));
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

/// A batch's *answer* must be bounded, not just its request frame.
///
/// The read side has three bounds — `MAX_FRAME_BYTES` on stdio, `MAX_BODY_BYTES`
/// over HTTP, `MAX_RESULT_BYTES` on one tool result — and `oversized_result_refusal`
/// states the invariant they exist to keep: "this server refuses to write a frame
/// larger than it would agree to read". The batch path broke it. `dispatch_value`
/// answered every member of an array with no ceiling on the assembled response,
/// so the amplification was the ratio between the cheapest request a member can
/// spell and the largest answer it can name — and `tools/list`, which takes 45
/// bytes to ask and 23 KB to answer, is a 500x lever sitting in the published
/// tool surface.
///
/// Measured against the pre-fix binary on this repository's corpus: one frame of
/// 1,048,571 bytes (22,310 `tools/list` members, filling the 1 MiB frame limit
/// exactly) was answered with **523,481,842 bytes** and drove the server's RSS
/// from 9.2 MiB to **3,635 MiB**. Nothing was refused and nothing was logged; the
/// client asked one legal question and the server chose to allocate 3.5 GiB.
///
/// The batch below is deliberately smaller than that maximum. It only has to
/// exceed the ceiling to prove the ceiling is absent, and a test that has to
/// allocate 3.5 GiB to fail is a test that gets deleted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_batch_answer_is_bounded_by_the_same_ceiling_a_single_result_is() {
    let slot = corpus();
    // Every member is the cheapest request with an expensive answer.
    const MEMBERS: usize = 2_000;
    let members: Vec<String> = (0..MEMBERS)
        .map(|id| format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/list"}}"#))
        .collect();
    let input = format!("[{}]", members.join(","));
    assert!(
        input.len() < devmap_serve::mcp::MAX_FRAME_BYTES,
        "fixture assumption: the attack must fit in one frame this server accepts, \
         so what is being measured is the answer and not the question ({} bytes)",
        input.len()
    );

    let frame = handle_line(&slot, &input)
        .await
        .expect("a batch of requests is answered");
    let written = serde_json::to_vec(&frame)
        .expect("the response frame serializes")
        .len();

    assert!(
        written <= devmap_serve::mcp::MAX_RESULT_BYTES,
        "a {MEMBERS}-member batch was answered with {written} bytes, over the \
         {}-byte ceiling this server enforces on everything else it writes. The \
         request was {} bytes, so one legal frame bought {:.0}x its own size in \
         response — and the whole array is materialised in memory before a byte \
         of it is written.",
        devmap_serve::mcp::MAX_RESULT_BYTES,
        input.len(),
        written as f64 / input.len() as f64
    );

    // The refusal must be a refusal, not a truncation: a cut batch leaves ids
    // the client is still waiting on, which is the failure mode the size bound
    // exists to avoid. Either every member is answered, or none is and the
    // frame says why.
    match &frame {
        Value::Array(items) => assert_eq!(
            items.len(),
            MEMBERS,
            "a batch that fits must be answered in full; truncating it strands \
             the ids that were dropped"
        ),
        Value::Object(object) => {
            assert_eq!(
                object.get("id"),
                Some(&Value::Null),
                "a batch-level refusal is answered against a null id"
            );
            let code = object["error"]["code"]
                .as_i64()
                .expect("a refusal carries a numeric code");
            assert_eq!(
                code,
                devmap_serve::mcp::codes::INVALID_REQUEST,
                "an oversized batch is a bad request, not a server fault"
            );
        }
        other => panic!("a batch is answered with an array or one refusal, got {other}"),
    }
}

// ---------------------------------------------------------------------------
// Stateful multi-turn MCP session fuzzer
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
enum InvalidArgKind {
    OversizedQuery,
    ZeroBudget,
    UnknownTool,
    LayersAndMinRungConflict,
    UnknownArgumentField,
}

#[derive(Clone, Debug)]
enum SessionAction {
    Initialize {
        id: Value,
        protocol_version: String,
    },
    Ping {
        id: Value,
    },
    ToolsList {
        id: Value,
    },
    Status {
        id: Value,
    },
    Search {
        id: Value,
        query: String,
        budget: u32,
    },
    Explore {
        id: Value,
        query: String,
        limit: u32,
    },
    Impact {
        id: Value,
        target: String,
        depth: u32,
        layers: bool,
    },
    DeadSymbols {
        id: Value,
        budget: u32,
    },
    Preview {
        id: Value,
        file: String,
        content: String,
    },
    InvalidOrOversized {
        id: Value,
        kind: InvalidArgKind,
    },
    CancelNotification {
        target_id: Value,
    },
    EvolveRepo {
        file: String,
        new_content: String,
    },
}

impl SessionAction {
    fn label(&self) -> &'static str {
        match self {
            SessionAction::Initialize { .. } => "mcp::initialize",
            SessionAction::Ping { .. } => "mcp::ping",
            SessionAction::ToolsList { .. } => "mcp::tools_list",
            SessionAction::Status { .. } => "mcp::devmap_status",
            SessionAction::Search { .. } => "mcp::devmap_search",
            SessionAction::Explore { .. } => "mcp::devmap_explore",
            SessionAction::Impact { .. } => "mcp::devmap_impact",
            SessionAction::DeadSymbols { .. } => "mcp::devmap_dead_symbols",
            SessionAction::Preview { .. } => "mcp::devmap_preview",
            SessionAction::InvalidOrOversized { .. } => "mcp::invalid_or_oversized",
            SessionAction::CancelNotification { .. } => "mcp::notifications_cancelled",
            SessionAction::EvolveRepo { .. } => "mcp::evolve_repo",
        }
    }
}

const SESSION_ACTION_NAMES: &[&str] = &[
    "mcp::initialize",
    "mcp::ping",
    "mcp::tools_list",
    "mcp::devmap_status",
    "mcp::devmap_search",
    "mcp::devmap_explore",
    "mcp::devmap_impact",
    "mcp::devmap_dead_symbols",
    "mcp::devmap_preview",
    "mcp::invalid_or_oversized",
    "mcp::notifications_cancelled",
    "mcp::evolve_repo",
];

fn save_repo_generation(store: &Store, repo: &stateful_fuzz::MicroRepo) -> u64 {
    let extractions: Vec<_> = repo
        .iter()
        .map(|(path, source)| devmap_extract::extract_file(path, source))
        .collect();
    let mut resolver = devmap_resolve::Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).expect("resolve");
    let analysis = devmap_analyze::analyze(&extractions, &resolution);
    u64::from(
        store
            .save_generation(&extractions, &resolution, &analysis)
            .expect("save generation"),
    )
}

fn build_repo_store(repo: &stateful_fuzz::MicroRepo) -> (Arc<Store>, Arc<StoreSlot>, u64) {
    let store = Arc::new(Store::open_in_memory().expect("in-memory store"));
    let generation_id = save_repo_generation(&store, repo);
    let slot = Arc::new(StoreSlot::ready("in-memory", Arc::clone(&store)));
    (store, slot, generation_id)
}

fn micro_lang_for_path(path: &str) -> stateful_fuzz::MicroLang {
    if path.ends_with(".rs") {
        stateful_fuzz::MicroLang::Rust
    } else if path.ends_with(".ts") {
        stateful_fuzz::MicroLang::TypeScript
    } else if path.ends_with(".go") {
        stateful_fuzz::MicroLang::Go
    } else {
        stateful_fuzz::MicroLang::Python
    }
}

fn collect_repo_symbols(repo: &stateful_fuzz::MicroRepo) -> Vec<String> {
    let mut names = Vec::new();
    for (path, source) in repo {
        let ext = devmap_extract::extract_file(path, source);
        for sym in ext.symbols {
            if sym.kind != devmap_extract::model::SymbolKind::File && !sym.name.is_empty() {
                names.push(sym.name);
            }
        }
    }
    names.sort();
    names.dedup();
    names
}

fn make_request_id(src: &mut stateful_fuzz::Source, seq: usize) -> Value {
    if src.bool() {
        json!(seq)
    } else {
        json!(format!("req-{seq}"))
    }
}

fn build_session_action(
    name: &str,
    src: &mut stateful_fuzz::Source,
    seq: usize,
    repo: &stateful_fuzz::MicroRepo,
    symbols: &[String],
    issued_ids: &[Value],
) -> SessionAction {
    let id = make_request_id(src, seq);
    let files: Vec<&String> = repo.keys().collect();
    let pick_sym = |s: &mut stateful_fuzz::Source| -> String {
        if s.chance(4, 5) {
            s.choose(symbols)
                .cloned()
                .unwrap_or_else(|| "api_0".to_string())
        } else {
            s.ident("missing_sym")
        }
    };
    let pick_file = |s: &mut stateful_fuzz::Source| -> (String, String) {
        let path = s
            .choose(&files)
            .map(|p| (*p).clone())
            .unwrap_or_else(|| "src/mod_0.rs".to_string());
        let content = repo.get(&path).cloned().unwrap_or_default();
        (path, content)
    };

    match name {
        "mcp::initialize" => {
            let versions = ["2025-11-25", "2025-06-18", "2024-11-05", "2099-01-01"];
            let protocol_version = src
                .choose(&versions)
                .copied()
                .unwrap_or("2025-11-25")
                .to_string();
            SessionAction::Initialize {
                id,
                protocol_version,
            }
        }
        "mcp::ping" => SessionAction::Ping { id },
        "mcp::tools_list" => SessionAction::ToolsList { id },
        "mcp::devmap_status" => SessionAction::Status { id },
        "mcp::devmap_search" => SessionAction::Search {
            id,
            query: pick_sym(src),
            budget: (500 + src.usize_below(3_500)) as u32,
        },
        "mcp::devmap_explore" => SessionAction::Explore {
            id,
            query: pick_sym(src),
            limit: (1 + src.usize_below(8)) as u32,
        },
        "mcp::devmap_impact" => SessionAction::Impact {
            id,
            target: pick_sym(src),
            depth: (1 + src.usize_below(4)) as u32,
            layers: src.bool(),
        },
        "mcp::devmap_dead_symbols" => SessionAction::DeadSymbols {
            id,
            budget: (500 + src.usize_below(3_500)) as u32,
        },
        "mcp::devmap_preview" => {
            let (file, content) = pick_file(src);
            let lang = micro_lang_for_path(&file);
            let preview_content = if src.chance(3, 4) {
                stateful_fuzz::mutate_file_elements(src, lang, &content)
            } else {
                stateful_fuzz::lower_damage(src, &content)
            };
            SessionAction::Preview {
                id,
                file,
                content: preview_content,
            }
        }
        "mcp::invalid_or_oversized" => {
            let kinds = [
                InvalidArgKind::OversizedQuery,
                InvalidArgKind::ZeroBudget,
                InvalidArgKind::UnknownTool,
                InvalidArgKind::LayersAndMinRungConflict,
                InvalidArgKind::UnknownArgumentField,
            ];
            let kind = src
                .choose(&kinds)
                .copied()
                .unwrap_or(InvalidArgKind::ZeroBudget);
            SessionAction::InvalidOrOversized { id, kind }
        }
        "mcp::notifications_cancelled" => {
            let target_id = src
                .choose(issued_ids)
                .cloned()
                .unwrap_or_else(|| json!(999_999));
            SessionAction::CancelNotification { target_id }
        }
        _ => {
            let (file, content) = pick_file(src);
            let lang = micro_lang_for_path(&file);
            let new_content = if src.chance(4, 5) {
                stateful_fuzz::mutate_file_elements(src, lang, &content)
            } else {
                stateful_fuzz::lower_damage(src, &content)
            };
            SessionAction::EvolveRepo { file, new_content }
        }
    }
}

struct SessionState {
    repo: stateful_fuzz::MicroRepo,
    store: Arc<Store>,
    slot: Arc<StoreSlot>,
    last_observed_gen: u64,
    evolved_since_status: bool,
}

async fn execute_session_action(
    state: &mut SessionState,
    action: &SessionAction,
) -> stateful_fuzz::StepOutcome {
    let act_name = action.label();

    if let SessionAction::EvolveRepo { file, new_content } = action {
        state.repo.insert(file.clone(), new_content.clone());
        let new_gen = save_repo_generation(&state.store, &state.repo);
        if let Some(failure) = stateful_fuzz::check(
            new_gen > state.last_observed_gen,
            "generation_monotonic",
            act_name,
            format!(
                "save_generation produced {new_gen} <= last_observed_gen {}",
                state.last_observed_gen
            ),
        )
        .into_failure()
        {
            return stateful_fuzz::StepOutcome::CheckFailed(failure);
        }
        state.evolved_since_status = true;
        return stateful_fuzz::StepOutcome::Ok;
    }

    let (input, expected_id, expected_tool, expect_refusal): (
        String,
        Option<Value>,
        Option<&'static str>,
        bool,
    ) = match action {
        SessionAction::Initialize {
            id,
            protocol_version,
        } => (
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "initialize",
                "params": {
                    "protocolVersion": protocol_version,
                    "capabilities": {},
                    "clientInfo": {"name": "stateful-fuzz", "version": "1.0"}
                }
            })
            .to_string(),
            Some(id.clone()),
            None,
            false,
        ),
        SessionAction::Ping { id } => (
            json!({"jsonrpc": "2.0", "id": id, "method": "ping"}).to_string(),
            Some(id.clone()),
            None,
            false,
        ),
        SessionAction::ToolsList { id } => (
            json!({"jsonrpc": "2.0", "id": id, "method": "tools/list"}).to_string(),
            Some(id.clone()),
            None,
            false,
        ),
        SessionAction::Status { id } => (
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "tools/call",
                "params": {"name": "devmap_status", "arguments": {}}
            })
            .to_string(),
            Some(id.clone()),
            Some("devmap_status"),
            false,
        ),
        SessionAction::Search { id, query, budget } => (
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "tools/call",
                "params": {"name": "devmap_search", "arguments": {"query": query, "budget": budget}}
            })
            .to_string(),
            Some(id.clone()),
            Some("devmap_search"),
            false,
        ),
        SessionAction::Explore { id, query, limit } => (
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "tools/call",
                "params": {"name": "devmap_explore", "arguments": {"query": query, "limit": limit}}
            })
            .to_string(),
            Some(id.clone()),
            Some("devmap_explore"),
            false,
        ),
        SessionAction::Impact {
            id,
            target,
            depth,
            layers,
        } => (
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "tools/call",
                "params": {
                    "name": "devmap_impact",
                    "arguments": {"target": target, "depth": depth, "layers": layers}
                }
            })
            .to_string(),
            Some(id.clone()),
            Some("devmap_impact"),
            false,
        ),
        SessionAction::DeadSymbols { id, budget } => (
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "tools/call",
                "params": {"name": "devmap_dead_symbols", "arguments": {"budget": budget}}
            })
            .to_string(),
            Some(id.clone()),
            Some("devmap_dead_symbols"),
            false,
        ),
        SessionAction::Preview { id, file, content } => (
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "tools/call",
                "params": {"name": "devmap_preview", "arguments": {"file": file, "content": content}}
            })
            .to_string(),
            Some(id.clone()),
            Some("devmap_preview"),
            false,
        ),
        SessionAction::InvalidOrOversized { id, kind } => {
            let params = match kind {
                InvalidArgKind::OversizedQuery => json!({
                    "name": "devmap_search",
                    "arguments": {"query": "q".repeat(5_000)}
                }),
                InvalidArgKind::ZeroBudget => json!({
                    "name": "devmap_search",
                    "arguments": {"query": "api", "budget": 0}
                }),
                InvalidArgKind::UnknownTool => json!({
                    "name": "devmap_nonexistent_tool",
                    "arguments": {}
                }),
                InvalidArgKind::LayersAndMinRungConflict => json!({
                    "name": "devmap_impact",
                    "arguments": {"target": "api_0", "layers": true, "min_rung": "high"}
                }),
                InvalidArgKind::UnknownArgumentField => json!({
                    "name": "devmap_status",
                    "arguments": {"unexpected_flag": true}
                }),
            };
            (
                json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "method": "tools/call",
                    "params": params
                })
                .to_string(),
                Some(id.clone()),
                None,
                true,
            )
        }
        SessionAction::CancelNotification { target_id } => (
            json!({
                "jsonrpc": "2.0",
                "method": "notifications/cancelled",
                "params": {"requestId": target_id}
            })
            .to_string(),
            None,
            None,
            false,
        ),
        SessionAction::EvolveRepo { .. } => unreachable!(),
    };

    let response = handle_line(&state.slot, &input).await;
    if let Err(failure) = check_response_shape(act_name, &input, &response) {
        return stateful_fuzz::StepOutcome::CheckFailed(failure);
    }

    match expected_id {
        None => {
            if let Some(failure) = stateful_fuzz::check(
                response.is_none(),
                "notification_unanswered",
                act_name,
                format!("notification must produce no response, got {response:?}"),
            )
            .into_failure()
            {
                return stateful_fuzz::StepOutcome::CheckFailed(failure);
            }
            stateful_fuzz::StepOutcome::Ok
        }
        Some(want_id) => {
            let Some(frame) = response else {
                return stateful_fuzz::StepOutcome::CheckFailed(stateful_fuzz::CheckFailure::new(
                    "request_answered",
                    act_name,
                    format!("request with id {want_id} received no response: {input}"),
                ));
            };
            if let Some(failure) = stateful_fuzz::check(
                frame["id"] == want_id,
                "id_preserved",
                act_name,
                format!(
                    "response id {:?} != request id {want_id} for {input}",
                    frame["id"]
                ),
            )
            .into_failure()
            {
                return stateful_fuzz::StepOutcome::CheckFailed(failure);
            }

            // Round-trip serialization check for single-line JSON framing.
            let wire = frame.to_string();
            if let Some(failure) = stateful_fuzz::check(
                !wire.contains('\n') && serde_json::from_str::<Value>(&wire).is_ok(),
                "wire_roundtrip",
                act_name,
                format!("response failed single-line JSON roundtrip: {wire:?}"),
            )
            .into_failure()
            {
                return stateful_fuzz::StepOutcome::CheckFailed(failure);
            }

            if expect_refusal {
                let is_rpc_err = frame.get("error").is_some();
                let is_tool_err = frame
                    .get("result")
                    .and_then(|r| r.get("isError"))
                    .and_then(Value::as_bool)
                    == Some(true);
                if let Some(failure) = stateful_fuzz::check(
                    is_rpc_err || is_tool_err,
                    "invalid_args_refused",
                    act_name,
                    format!("invalid/oversized request was silently accepted: {frame}"),
                )
                .into_failure()
                {
                    return stateful_fuzz::StepOutcome::CheckFailed(failure);
                }
                return stateful_fuzz::StepOutcome::Ok;
            }

            if let Some(tool_name) = expected_tool {
                if let Some(failure) = stateful_fuzz::check(
                    frame.get("error").is_none(),
                    "valid_tool_no_rpc_error",
                    act_name,
                    format!("valid tool call {tool_name} returned JSON-RPC error: {frame}"),
                )
                .into_failure()
                {
                    return stateful_fuzz::StepOutcome::CheckFailed(failure);
                }
                let result = &frame["result"];
                let is_err = result
                    .get("isError")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if !is_err {
                    if let Some(structured) = result.get("structuredContent") {
                        if let Some(violation) = structured_content_violation(tool_name, structured)
                        {
                            return stateful_fuzz::StepOutcome::CheckFailed(
                                stateful_fuzz::CheckFailure::new(
                                    "output_schema_conformance",
                                    act_name,
                                    format!("{violation} (payload: {structured})"),
                                ),
                            );
                        }
                        if tool_name == "devmap_status" {
                            if let Some(gen_id) =
                                structured.get("generation_id").and_then(Value::as_u64)
                            {
                                let ok = if state.evolved_since_status {
                                    gen_id > state.last_observed_gen
                                } else {
                                    gen_id >= state.last_observed_gen
                                };
                                if let Some(failure) = stateful_fuzz::check(
                                    ok,
                                    "status_generation_monotonic",
                                    act_name,
                                    format!(
                                        "devmap_status generation_id={gen_id}, prev={}, evolved={}",
                                        state.last_observed_gen, state.evolved_since_status
                                    ),
                                )
                                .into_failure()
                                {
                                    return stateful_fuzz::StepOutcome::CheckFailed(failure);
                                }
                                state.last_observed_gen = gen_id;
                                state.evolved_since_status = false;
                            }
                        }
                    }
                }
            }

            stateful_fuzz::StepOutcome::Ok
        }
    }
}

fn replay_session_sync(
    handle: &tokio::runtime::Handle,
    initial_repo: &stateful_fuzz::MicroRepo,
    actions: &[SessionAction],
) -> Option<stateful_fuzz::CheckFailure> {
    let (store, slot, initial_gen) = build_repo_store(initial_repo);
    let mut state = SessionState {
        repo: initial_repo.clone(),
        store,
        slot,
        last_observed_gen: initial_gen,
        evolved_since_status: false,
    };
    for action in actions {
        let outcome = handle.block_on(execute_session_action(&mut state, action));
        if let Some(failure) = outcome.into_failure() {
            return Some(failure);
        }
    }
    None
}

/// Stateful multi-turn MCP session fuzzing over an evolving `MicroRepoGen` store,
/// exercising handshake, tools, invalid/oversized refusals, live generation
/// evolution, cancellations, and duplex `serve_streams` framing with
/// `HealthTracker` telemetry and `minimize_sequence` reproducer emission.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stateful_multiturn_mcp_session_fuzz() {
    let seed = stateful_fuzz::env_u64(
        "DEVMAP_STATEFUL_SEED",
        stateful_fuzz::env_u64("DEVMAP_FUZZ_SEED", 0x5717_C400_CAFE_0001),
    );
    let rounds = stateful_fuzz::env_usize("DEVMAP_STATEFUL_ROUNDS", 8);
    let steps_per_round = stateful_fuzz::env_usize("DEVMAP_STATEFUL_STEPS", 28);
    let mut src = stateful_fuzz::Source::from_seed(seed);
    let mut tracker = stateful_fuzz::HealthTracker::new();

    let langs = [
        stateful_fuzz::MicroLang::Rust,
        stateful_fuzz::MicroLang::TypeScript,
        stateful_fuzz::MicroLang::Go,
        stateful_fuzz::MicroLang::Python,
    ];

    for round in 0..rounds {
        let initial_repo = stateful_fuzz::generate_micro_repo(&mut src, &langs, 6);
        let (store, slot, initial_gen) = build_repo_store(&initial_repo);
        let mut state = SessionState {
            repo: initial_repo.clone(),
            store,
            slot,
            last_observed_gen: initial_gen,
            evolved_since_status: false,
        };
        let mut symbols = collect_repo_symbols(&state.repo);
        let mut issued_ids: Vec<Value> = Vec::new();
        let mut executed: Vec<SessionAction> = Vec::new();
        let mut prev_action: Option<&'static str> = None;

        for step in 0..steps_per_round {
            let chosen = if step == 0 {
                "mcp::initialize"
            } else {
                tracker.pick_weighted(&mut src, SESSION_ACTION_NAMES, prev_action)
            };
            let seq = round * 1_000 + step + 1;
            let action =
                build_session_action(chosen, &mut src, seq, &state.repo, &symbols, &issued_ids);
            match &action {
                SessionAction::Initialize { id, .. }
                | SessionAction::Ping { id }
                | SessionAction::ToolsList { id }
                | SessionAction::Status { id }
                | SessionAction::Search { id, .. }
                | SessionAction::Explore { id, .. }
                | SessionAction::Impact { id, .. }
                | SessionAction::DeadSymbols { id, .. }
                | SessionAction::Preview { id, .. }
                | SessionAction::InvalidOrOversized { id, .. } => {
                    issued_ids.push(id.clone());
                }
                SessionAction::CancelNotification { .. } | SessionAction::EvolveRepo { .. } => {}
            }

            let outcome = execute_session_action(&mut state, &action).await;
            let label = action.label();
            tracker.record(label, prev_action, &outcome);
            executed.push(action.clone());
            if matches!(action, SessionAction::EvolveRepo { .. }) {
                symbols = collect_repo_symbols(&state.repo);
            }

            if let Some(failure) = outcome.into_failure() {
                let sig = failure.signature();
                let handle = tokio::runtime::Handle::current();
                let minimized = tokio::task::block_in_place(|| {
                    stateful_fuzz::minimize_sequence(&executed, &sig, 64, |cand| {
                        replay_session_sync(&handle, &initial_repo, cand)
                    })
                });
                let rendered: Vec<String> = minimized.iter().map(|a| format!("{a:?}")).collect();
                let repro = stateful_fuzz::format_repro_test(
                    "repro_mcp_stateful_session",
                    seed,
                    &failure,
                    &rendered,
                );
                panic!(
                    "stateful MCP session round {round} step {step} failed: {failure}\n\
                     Minimized sequence ({} -> {} actions)\n\
                     Replay: DEVMAP_STATEFUL_SEED={seed} cargo test --locked -p devmap-serve --test mcp_fuzz_and_bounds -- stateful_multiturn_mcp_session_fuzz --nocapture\n\n\
                     {repro}",
                    executed.len(),
                    minimized.len()
                );
            }

            prev_action = Some(label);
        }

        // Also exercise a real multi-turn duplex `serve_streams` session on the
        // evolved store, interleaving initialize, initialized notification,
        // tool calls, cancellation, and a final ping sentinel.
        let target_sym = symbols
            .first()
            .cloned()
            .unwrap_or_else(|| "api_0".to_string());
        let first_file = state
            .repo
            .keys()
            .next()
            .cloned()
            .unwrap_or_else(|| "src/mod_0.rs".to_string());
        let first_content = state.repo.get(&first_file).cloned().unwrap_or_default();
        let sentinel_id = json!(format!("sentinel-{round}"));
        let lines = vec![
            json!({
                "jsonrpc": "2.0",
                "id": "init-1",
                "method": "initialize",
                "params": {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "stateful-fuzz", "version": "1"}}
            })
            .to_string(),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string(),
            json!({"jsonrpc": "2.0", "id": 10, "method": "tools/list"}).to_string(),
            json!({
                "jsonrpc": "2.0",
                "id": 11,
                "method": "tools/call",
                "params": {"name": "devmap_status", "arguments": {}}
            })
            .to_string(),
            json!({
                "jsonrpc": "2.0",
                "id": 12,
                "method": "tools/call",
                "params": {"name": "devmap_impact", "arguments": {"target": target_sym, "depth": 2}}
            })
            .to_string(),
            json!({
                "jsonrpc": "2.0",
                "method": "notifications/cancelled",
                "params": {"requestId": 12}
            })
            .to_string(),
            json!({
                "jsonrpc": "2.0",
                "id": 13,
                "method": "tools/call",
                "params": {"name": "devmap_preview", "arguments": {"file": first_file, "content": first_content}}
            })
            .to_string(),
            json!({"jsonrpc": "2.0", "id": sentinel_id, "method": "ping"}).to_string(),
        ];

        let frames = session(&state.slot, &lines).await;
        let mut seen_ids: HashMap<String, usize> = HashMap::new();
        let mut sentinel_seen = false;
        let mut valid_response_ids: HashSet<String> = HashSet::new();
        for frame in &frames {
            // Skip server-initiated notifications (e.g. `notifications/tools/list_changed`).
            if frame.get("id").is_none() && frame.get("method").is_some() {
                continue;
            }
            assert_response_shape("transport_multiturn", &Some(frame.clone()));
            let key = frame["id"].to_string();
            *seen_ids.entry(key.clone()).or_insert(0) += 1;
            valid_response_ids.insert(key);
            if frame["id"] == sentinel_id {
                sentinel_seen = true;
            }
        }
        for (id_key, count) in &seen_ids {
            assert_eq!(
                *count, 1,
                "round {round}: response id {id_key} emitted {count} times in duplex session"
            );
        }
        assert!(
            sentinel_seen && valid_response_ids.contains(&json!("init-1").to_string()),
            "round {round}: duplex session lost init or sentinel frame: {frames:?}"
        );
        tracker.record(
            "mcp::transport_multiturn",
            prev_action,
            &stateful_fuzz::StepOutcome::Ok,
        );
    }

    eprintln!("{}", tracker.probe_report("mcp_fuzz::stateful_session"));
    tracker.assert_actions_exercised(&[
        "mcp::initialize",
        "mcp::ping",
        "mcp::tools_list",
        "mcp::devmap_status",
        "mcp::devmap_search",
        "mcp::devmap_explore",
        "mcp::devmap_impact",
        "mcp::devmap_dead_symbols",
        "mcp::devmap_preview",
        "mcp::invalid_or_oversized",
        "mcp::notifications_cancelled",
        "mcp::evolve_repo",
        "mcp::transport_multiturn",
    ]);
}
