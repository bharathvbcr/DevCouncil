//! Q-10: `search` re-scores a page the database cut by a different key.
//!
//! The store pages the FTS match with `ORDER BY bm25(...) LIMIT n`, where `n`
//! is what the token budget could conceivably show. `StoreQueryEngine::search`
//! then throws that ordering away and re-scores every row exact (1.0) / prefix
//! (0.95) / other (0.8) before sorting. So the key that decided *which* rows
//! exist and the key that decides which rows rank are different functions, and
//! the one row a caller searching for `alpha` actually wants — the symbol
//! literally named `alpha` — is dropped by the first before the second ever
//! sees it.
//!
//! R7's rule is rank, then truncate. This is the inverse, and the counts stay
//! honest about it (`total` comes from `count_search_symbols`), which is
//! exactly what makes it hard to notice: the answer says "201 matches, showing
//! 101" and every one of the 101 is a worse match than the one it omitted.

use devmap_extract::extract_file;
use devmap_query::{Request, Response, StoreQueryEngine, SymbolHit};
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};

/// Decoys carry the query token twice and sit on a short path, so bm25 ranks
/// every one of them above the exact match, which carries it once on a long
/// one. Nothing here depends on a *particular* bm25 score — only on the
/// database and the engine disagreeing about the ordering, which is the defect.
fn corpus(decoys: usize) -> Store {
    let mut body = String::new();
    for index in 0..decoys {
        body.push_str(&format!(
            "def alpha_alpha_{index:05}():\n    return 1\n\n\n"
        ));
    }
    let extractions = vec![
        extract_file("decoys.py", &body),
        extract_file(
            "zzzz/deeply/nested/target/module.py",
            "def alpha():\n    return 2\n",
        ),
    ];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
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

fn search(store: &Store, budget: u32) -> Response<SymbolHit> {
    StoreQueryEngine::new(store)
        .search(Request {
            query: "alpha".to_string(),
            token_budget: budget,
            min_confidence: 0.0,
            max_depth: 1,
        })
        .expect("search answers")
}

fn names(response: &Response<SymbolHit>) -> Vec<&str> {
    response
        .items
        .iter()
        .map(|hit| hit.symbol_name.as_str())
        .collect()
}

/// The defect: 200 decoys crowd the exact match out of the SQL page, so the
/// engine's own top-ranked hit is never scored at all.
#[test]
fn an_exact_match_outside_the_database_page_still_ranks_first() {
    let store = corpus(200);
    let response = search(&store, devmap_query::Budget::SEARCH);

    let names = names(&response);
    assert!(
        names.contains(&"alpha"),
        "the symbol named exactly `alpha` is the best hit this engine's own \
         scoring function can produce, and it was cut by a page ordered on a \
         different key before that function ran. Showing {} of {}: {:?}",
        response.shown,
        response.total,
        &names[..names.len().min(5)]
    );
    assert_eq!(
        names.first().copied(),
        Some("alpha"),
        "rank before truncating: the exact match must lead the answer"
    );
    assert!(
        (response.items[0].score - 1.0).abs() < f32::EPSILON,
        "the exact match must carry the exact-match score, got {}",
        response.items[0].score
    );
}

/// The counts must keep describing the whole match set, not the pool the
/// re-ranking drew from. A wider page must not turn into a wider `total`.
#[test]
fn widening_the_ranking_pool_does_not_inflate_the_reported_total() {
    let store = corpus(200);
    let response = search(&store, devmap_query::Budget::SEARCH);

    assert_eq!(
        response.total, 201,
        "201 symbols match `alpha`: 200 decoys and the exact match"
    );
    assert_eq!(response.shown as usize, response.items.len());
    assert_eq!(response.shown + response.hidden, response.total);
    assert_eq!(response.truncated, response.hidden > 0);
    assert!(
        response.tokens_used <= devmap_query::Budget::SEARCH,
        "the wider pool must not be spent on the caller's budget: {} > {}",
        response.tokens_used,
        devmap_query::Budget::SEARCH
    );
}

/// The OFF direction. A corpus small enough that the page holds every match
/// was never mis-ranked, and must not acquire an incompleteness marker: a
/// signal every answer carries is a signal no caller can act on.
#[test]
fn a_fully_ranked_search_does_not_claim_an_incomplete_ranking() {
    let store = corpus(3);
    let response = search(&store, devmap_query::Budget::SEARCH);

    assert_eq!(response.total, 4);
    assert_eq!(names(&response).first().copied(), Some("alpha"));
    assert!(
        response.walk_incomplete.is_none(),
        "every match was ranked, so nothing was left out of the ranking: {:?}",
        response.walk_incomplete
    );
    assert!(!response.truncated);
}

/// The ON direction for the pool itself. The pool is bounded — it has to be,
/// or one query walks the whole corpus — so when the match set outruns it the
/// ranking really is over a prefix, and saying so is the difference between
/// "these are the best matches" and "these are the best of the ones we looked
/// at".
#[test]
fn a_ranking_pool_smaller_than_the_match_set_says_the_ranking_is_partial() {
    // A tiny budget makes the pool tiny, so this needs no enormous fixture:
    // the pool is derived from the budget exactly as the page always was.
    let store = corpus(200);
    let response = search(&store, 40);

    assert_eq!(response.total, 201, "the count is over the whole match set");
    assert!(
        response.walk_incomplete.is_some(),
        "the engine ranked a bm25-ordered prefix of 201 matches and returned \
         the best of that prefix; presenting it as the best of the match set \
         is the same lie the depth-capped walk told. Got {:?}",
        response.walk_incomplete
    );
    let reason = response.walk_incomplete.as_deref().unwrap_or_default();
    assert!(
        reason.contains("rank") || reason.contains("ranked"),
        "the reason must name what was incomplete, got {reason:?}"
    );
}
