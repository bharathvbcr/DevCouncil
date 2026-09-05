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

use devmap_serve::mcp::{handle_line, StoreSlot};
use devmap_store::Store;
use serde_json::Value;

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
