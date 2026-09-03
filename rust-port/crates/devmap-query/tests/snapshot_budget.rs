//! `snapshots` must answer inside the budget it was given.
//!
//! `semantic_snapshots` budgeted at ten times the requested figure and then
//! reported the tokens it spent verbatim, so a 2,000-token request could come
//! back claiming `tokens_used: 20000`. The Python client enforces the budget as
//! a hard contract (`DevMapClient._budgeted` raises on any response that breaks
//! it), so an over-budget answer is not merely large — it is unreadable.

use devmap_extract::extract_file;
use devmap_query::{semantic_snapshots, Request};

/// One extraction per file, each with a handful of public symbols.
fn corpus(files: usize, symbols_per_file: usize) -> Vec<devmap_extract::model::Extraction> {
    (0..files)
        .map(|file| {
            let mut source = String::new();
            for symbol in 0..symbols_per_file {
                source.push_str(&format!(
                    "def sym_{file}_{symbol}():\n    return {symbol}\n"
                ));
            }
            extract_file(&format!("mod_{file:03}.py"), &source)
        })
        .collect()
}

fn request(budget: u32) -> Request<String> {
    Request {
        query: String::new(),
        token_budget: budget,
        min_confidence: 0.0,
        max_depth: 1,
    }
}

/// The three invariants a budgeted response has to satisfy at once.
#[test]
fn a_small_budget_is_honoured_exactly_and_accounted_for() {
    let extractions = corpus(40, 6);
    let budget = 200u32;
    let response = semantic_snapshots(&extractions, request(budget));

    assert_eq!(
        response.total, 40,
        "every matching file must be counted in `total`, budget or not"
    );
    assert!(
        response.tokens_used <= budget,
        "a {budget}-token request answered with tokens_used={} — the client \
         rejects any response over its budget",
        response.tokens_used
    );
    assert_eq!(
        response.shown + response.hidden,
        response.total,
        "shown ({}) + hidden ({}) must account for every one of {} files",
        response.shown,
        response.hidden,
        response.total
    );
    assert_eq!(
        response.truncated,
        response.hidden > 0,
        "`truncated` must mean exactly 'something was withheld' (hidden={})",
        response.hidden
    );
    assert_eq!(
        response.shown as usize,
        response.items.len(),
        "`shown` must count the items actually returned"
    );
}

/// The budget must bound the tokens the *content* costs, not a flat per-file
/// figure that ignores how many symbols each snapshot carries. A flat rate is
/// how a response comes to report a token count it did not spend.
#[test]
fn reported_tokens_track_the_symbols_actually_returned() {
    let extractions = corpus(4, 200);
    let response = semantic_snapshots(&extractions, request(10_000));
    assert!(response.shown > 0, "a 10,000-token budget must show files");

    let returned_symbols: usize = response
        .items
        .iter()
        .map(|snapshot| snapshot.symbols.len())
        .sum();
    assert!(
        returned_symbols > 0,
        "the fixture must actually return symbols"
    );
    assert!(
        response.tokens_used >= returned_symbols as u32,
        "tokens_used ({}) is below one token per returned symbol ({returned_symbols}) — \
         the cost function is not charging for the content it returned",
        response.tokens_used
    );
    assert!(response.tokens_used <= 10_000);
}

/// A budget that fits everything must not claim truncation.
#[test]
fn a_generous_budget_reports_a_complete_answer() {
    let extractions = corpus(3, 2);
    let response = semantic_snapshots(&extractions, request(100_000));
    assert_eq!(response.shown, response.total);
    assert_eq!(response.hidden, 0);
    assert!(!response.truncated);
    assert!(response.tokens_used <= 100_000);
}
