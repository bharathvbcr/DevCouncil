//! Two ways `search` returned a confident zero it had not earned.
//!
//! Both were found by audit against a **fresh, undegraded** index —
//! `is_fresh: true`, `source_freshness: true`, `analyzer_freshness: true`, no
//! coverage gaps, nothing quarantined — which is what makes them the
//! repository's Class A shape rather than ordinary misses. Nothing about the
//! answer invited doubt.
//!
//! * **The conjunction that was really a phrase.** `fts_match_query` wrapped
//!   the *whole* input as one quoted FTS5 phrase, and a quoted phrase requires
//!   its tokens **adjacent and in order inside a single indexed column**.
//!
//!   The surprise is which queries that breaks, and it is not the obvious one:
//!   `build optimizer` *did* match `build_optimizer`, because the tokenizer
//!   splits the underscore and leaves the two words adjacent and in order. What
//!   silently returned zero was every query whose words are out of order or
//!   spread across columns. Measured on the corpus below, before the fix:
//!
//!   | query | before | after |
//!   |---|---|---|
//!   | `build optimizer` | 1 | 1 |
//!   | `optimizer build` | **0** | 1 |
//!   | `configure state` | **0** | 1 |
//!   | `optimizer app` | **0** | 2 |
//!
//!   So the failure was invisible to exactly the queries a person writes to
//!   check that search works, and hit the ones they write when they are
//!   describing a symbol rather than naming it.
//!
//! * **The zero that would not say what it covered.** An agent asked "where is
//!   the optimizer constructed?", got zero items and a clean `devmap_status`,
//!   and reported absence. ripgrep then found eight sites —
//!   `self.optimizer = torch.optim.AdamW(...)`, an attribute assignment to an
//!   externally-owned class, which is not a symbol of the repository at all.
//!   The index was right to hold nothing and wrong to imply nothing existed.
//!   The FTS columns are `name`, `qualified_name` and `path`; no file body is
//!   ever read, and now the empty answer says so.

use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
use devmap_query::{Request, StoreQueryEngine};
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};

fn store_of(files: &[(&str, &str)]) -> Store {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    let analysis = devmap_analyze::analyze(&extractions, &resolution);
    let store = Store::open_in_memory().unwrap();
    store
        .save_generation_with_opts(
            &extractions,
            &resolution,
            &analysis,
            GenerationWriteOpts::default(),
        )
        .unwrap();
    store
}

fn search(store: &Store, query: &str) -> devmap_query::Response<devmap_query::SymbolHit> {
    StoreQueryEngine::new(store)
        .search(Request {
            query: query.to_string(),
            token_budget: 10_000,
            min_confidence: 0.0,
            max_depth: 3,
        })
        .unwrap()
}

const CORPUS: &[(&str, &str)] = &[(
    "training/app.py",
    "import torch\n\
     \n\
     \n\
     def build_optimizer(model):\n\
     \x20   return torch.optim.AdamW(model.parameters())\n\
     \n\
     \n\
     class Trainer:\n\
     \x20   def configure_optimizer_state(self):\n\
     \x20       return 1\n\
     \x20   def __init__(self, model):\n\
     \x20       self.optimizer = torch.optim.AdamW(model.parameters())\n",
)];

/// The word-order bug, stated as the measurements that found it.
///
/// Each case here was measured at **0** against the unmodified store. The
/// adjacent-and-in-order case is kept as a control precisely because it passed
/// before the fix too — without it, this test would look like it proves more
/// than it does.
#[test]
fn a_multi_word_query_matches_the_words_and_not_one_literal_phrase() {
    let store = store_of(CORPUS);

    let control = search(&store, "build optimizer");
    assert!(
        control.total > 0,
        "control: adjacent and in order matched before the fix and must still: \
         {control:?}"
    );

    for (query, why) in [
        (
            "optimizer build",
            "the same two words in the other order name the same symbol; a \
             phrase query requires the written order and nothing else does",
        ),
        (
            "configure state",
            "`configure_optimizer_state` contains both words, with a third \
             between them — a phrase query requires adjacency",
        ),
        (
            "optimizer app",
            "one word is in the symbol name and one is in the path; a phrase \
             can never span two indexed columns",
        ),
    ] {
        let answer = search(&store, query);
        assert!(
            answer.total > 0,
            "`{query}` returned zero: {why}. Got {answer:?}"
        );
    }
}

/// The single-token path must be untouched by that change.
#[test]
fn a_single_token_query_is_unchanged() {
    let store = store_of(CORPUS);
    let hit = search(&store, "Trainer");
    assert!(
        hit.items.iter().any(|item| item.symbol_name == "Trainer"),
        "single-token search is the overwhelmingly common case and must \
         produce exactly what it produced before: {hit:?}"
    );
}

/// The honesty half, and the one that matters more.
///
/// The conjunction fix does **not** make this query find anything, and it must
/// not: `self.optimizer = torch.optim.AdamW(...)` is not a symbol, and inventing
/// a hit for it would be a fabrication. What changes is that zero stops
/// impersonating absence.
#[test]
fn an_empty_result_says_what_it_did_and_did_not_cover() {
    let store = store_of(CORPUS);
    let answer = search(&store, "optimizer AdamW step");

    assert_eq!(
        answer.total, 0,
        "precondition: no symbol is named for all three of these words"
    );
    let reason = answer.walk_incomplete.as_deref().unwrap_or_default();
    assert!(
        !reason.is_empty(),
        "a fresh index returning zero is the most confident-looking answer this \
         API produces. It must not be returned bare: an agent reading \
         `total: 0, truncated: false` on a clean `devmap_status` will report \
         absence, and did. Got {answer:?}"
    );
    assert!(
        reason.contains("file contents") || reason.contains("body"),
        "the disclosure has to name the actual boundary — that names and paths \
         are indexed and bodies are not — or a caller cannot tell which \
         questions to stop asking it. Got {reason:?}"
    );
    assert!(
        reason.contains("3 terms") || reason.contains("all 3"),
        "with several terms the conjunction is also a filter, and the caller \
         should be told how many had to match. Got {reason:?}"
    );
}

/// The half that stops the disclosure from becoming noise.
///
/// A qualification attached to every answer is a qualification nobody reads. It
/// fires on zero and only on zero.
#[test]
fn a_non_empty_result_carries_no_empty_result_disclosure() {
    let store = store_of(CORPUS);
    let answer = search(&store, "Trainer");

    assert!(answer.total > 0, "precondition: this matches");
    let reason = answer.walk_incomplete.as_deref().unwrap_or_default();
    assert!(
        !reason.contains("file contents"),
        "the scope note belongs on an empty answer, not on every answer: \
         {reason:?}"
    );
}

/// The semantic surface reaches zero differently and owes the same account.
///
/// It scores term overlap against `name` and `qualified_name` rather than
/// running an FTS conjunction, so the *mechanism* differs — but it reads no
/// file body either, and its own doc comment says "'Nothing matched' is an
/// answer". It is; it is also an answer whose scope the caller cannot see. A
/// fix applied to one search surface and not its sibling is how the two come to
/// answer the same corpus differently depending on which was asked.
#[test]
fn semantic_search_also_says_what_an_empty_answer_covers() {
    let store = store_of(CORPUS);
    let engine = StoreQueryEngine::new(&store);

    // Terms chosen to share nothing with any symbol name or path: semantic
    // search scores *overlap*, so a phrase containing a word like `optimizer`
    // is a hit, not a miss — measured, `where optimizer constructed` returned
    // 2. The precondition below is what caught that.
    let answer = engine
        .search_semantic("zzzznotpresent alsoabsent neitherhere", 10_000)
        .unwrap();
    assert_eq!(answer.total, 0, "precondition: nothing scores against this");
    let reason = answer.walk_incomplete.as_deref().unwrap_or_default();
    assert!(
        reason.contains("file contents"),
        "the semantic surface must disclose the same boundary as the keyword \
         one. Got {answer:?}"
    );
    assert!(
        !reason.contains("all 3 terms"),
        "semantic search has no all-terms-must-match rule, so borrowing the \
         conjunction sentence would be a false explanation. Got {reason:?}"
    );

    // Same gate as the keyword surface: a one-word miss is a completed check.
    let single = engine.search_semantic("zzzznotpresent", 10_000).unwrap();
    assert_eq!(
        single.walk_incomplete, None,
        "the two surfaces must agree about when an empty answer is worth \
         qualifying: {single:?}"
    );

    let hit = engine.search_semantic("Trainer", 10_000).unwrap();
    assert!(hit.total > 0, "precondition: this scores");
    assert!(
        !hit.walk_incomplete
            .as_deref()
            .unwrap_or_default()
            .contains("file contents"),
        "and it fires on zero only: {hit:?}"
    );
}

/// A one-word miss claims nothing, and that is deliberate.
///
/// The first version of this file asserted the opposite — that every miss owes
/// the scope note — and `search_over_a_complete_corpus_claims_nothing` in
/// `incomplete_answers_say_so` rejected it. That test is right and this one now
/// agrees with it: "no symbol is named `zzzznotpresent`" is a **completed
/// check** over a fully read corpus, and attaching a caveat to it would claim
/// an incompleteness that does not exist.
///
/// The cost of getting this wrong is not cosmetic. `walk_incomplete` is the
/// field a caller must believe when the corpus really does have holes in it,
/// and a qualification that fires on the most common outcome of all is one a
/// caller learns to skip.
#[test]
fn a_single_word_miss_claims_nothing() {
    let store = store_of(CORPUS);
    let answer = search(&store, "zzzznotpresent");

    assert_eq!(answer.total, 0, "precondition: nothing is named this");
    assert_eq!(
        answer.walk_incomplete, None,
        "a one-word miss over a complete corpus is a completed check and must \
         say so by staying silent: {answer:?}"
    );
}
