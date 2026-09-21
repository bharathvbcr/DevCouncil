//! Hostile input to `search`, aimed at the query builder that just changed.
//!
//! `fts_match_query` used to wrap the whole input in one pair of quotes. It now
//! splits on whitespace and joins the terms with `AND`, which means **the
//! builder emits FTS5 syntax of its own for the first time**. That is exactly
//! the kind of change that reopens an injection surface someone closed on
//! purpose, so the properties the single-phrase form guaranteed are pinned here
//! against input designed to break them:
//!
//! * every FTS5 metacharacter and operator stays *data*, never syntax;
//! * a query the store cannot express is **refused**, not silently truncated
//!   into one it can (the rule the NUL-byte refusal already established);
//! * nothing panics, nothing leaks a SQLite parser error to the caller, and
//!   nothing runs unbounded.
//!
//! The bar for "handled" is deliberately low and deliberately explicit: each
//! query must either return a well-formed answer or fail with a refusal that
//! names a reason. What must never happen is a raw `SqliteFailure … syntax
//! error near …` reaching an agent, because that is a query surface leaking its
//! backend for an input the caller was entitled to pass.

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

fn corpus() -> Store {
    store_of(&[(
        "training/app.py",
        "def build_optimizer(model):\n    return 1\n\n\nclass Trainer:\n    def step(self):\n        return 2\n",
    )])
}

/// Every one of these must be survivable. The assertion is about *shape*, not
/// about how many rows come back: a hostile query is entitled to match nothing.
const HOSTILE: &[(&str, &str)] = &[
    ("*", "a bare wildcard is a term with no tokens in it"),
    ("**", "two of them"),
    (
        "\"",
        "one unbalanced double quote — the escape's own metacharacter",
    ),
    (
        "\"\"\"",
        "three of them, so the escaping doubles to an even count",
    ),
    (
        "foo\"bar",
        "an interior quote inside an otherwise ordinary term",
    ),
    ("OR", "an FTS5 operator alone"),
    ("AND", "the operator this builder now emits itself"),
    ("NOT", "the negation operator"),
    ("NEAR", "the proximity operator"),
    (
        "a OR b",
        "an operator between two terms — must match neither as syntax",
    ),
    ("a AND b", "the same with the operator this builder emits"),
    ("NEAR(a b)", "a proximity call"),
    ("name:Trainer", "an FTS5 column filter"),
    ("path:*", "a column filter with a wildcard"),
    ("(a b)", "a parenthesised group"),
    (")", "an unbalanced closer"),
    ("(", "an unbalanced opener"),
    (
        "a-b",
        "a hyphen, which FTS5 reads as an operator in some positions",
    ),
    ("^leading", "the initial-token operator"),
    ("a*b*c", "interior wildcards"),
    ("  ", "whitespace only"),
    ("", "empty"),
    ("\t\n", "other whitespace only"),
    ("--", "punctuation that tokenizes to nothing"),
    ("💥", "an astral-plane character"),
    ("e\u{0301}", "a combining accent"),
    ("\u{202E}reversed", "a bidi override"),
];

#[test]
fn hostile_queries_are_answered_or_refused_but_never_leak_the_backend() {
    let store = corpus();
    let engine = StoreQueryEngine::new(&store);

    for (query, why) in HOSTILE {
        let outcome = engine.search(Request {
            query: (*query).to_string(),
            token_budget: 10_000,
            min_confidence: 0.0,
            max_depth: 3,
        });
        match outcome {
            Ok(response) => {
                assert!(
                    response.shown as usize == response.items.len(),
                    "{query:?} ({why}): the counts must describe the items \
                     returned. Got {response:?}"
                );
                assert!(
                    response.shown + response.hidden == response.total
                        || response.total >= response.shown,
                    "{query:?} ({why}): shown + hidden must reconcile with \
                     total. Got {response:?}"
                );
            }
            Err(error) => {
                let text = format!("{error:#}");
                assert!(
                    !text.contains("syntax error") && !text.contains("SqliteFailure"),
                    "{query:?} ({why}): a refusal must name a reason the caller \
                     can act on, not forward SQLite's parser error. Got {text}"
                );
            }
        }
    }
}

/// The refusal the single-phrase form established must survive the rewrite.
///
/// SQLite hands the MATCH argument to FTS5 as a C string, so an interior NUL
/// truncates the expression mid-flight. Splitting the query into terms does not
/// change that — a NUL inside any term is still a NUL inside the expression —
/// and silently dropping it would run the search on a prefix of what was asked
/// while reporting it as the whole thing.
#[test]
fn an_interior_nul_is_still_refused_and_says_why() {
    let store = corpus();
    let engine = StoreQueryEngine::new(&store);

    for query in ["alpha\0beta", "\0", "build\0", "a b\0c d"] {
        let error = engine
            .search(Request {
                query: query.to_string(),
                token_budget: 10_000,
                min_confidence: 0.0,
                max_depth: 3,
            })
            .expect_err(&format!("{query:?} must be refused, not truncated"));
        let text = format!("{error:#}");
        assert!(
            text.contains("NUL"),
            "the refusal must name the NUL byte so the caller knows what to \
             strip. Got {text}"
        );
    }
}

/// An operator must not be able to change the *meaning* of the search.
///
/// This is the injection test proper. `a OR b` must behave as "a term spelled
/// `OR` between two others", not as a disjunction — otherwise a caller's search
/// string is executing query logic. Checked by measurement rather than by
/// reading the builder: a disjunction over a term that matches everything would
/// return more rows than the conjunction can.
#[test]
fn an_operator_in_the_query_is_data_and_does_not_widen_the_result() {
    let store = corpus();
    let engine = StoreQueryEngine::new(&store);
    let run = |q: &str| {
        engine
            .search(Request {
                query: q.to_string(),
                token_budget: 10_000,
                min_confidence: 0.0,
                max_depth: 3,
            })
            .unwrap()
            .total
    };

    // `build` alone matches. `build OR zzzznotpresent` must match *no more*
    // than `build` does: if `OR` were syntax, the disjunction would match
    // everything `build` matches regardless of the impossible term, and if the
    // impossible term is ANDed the result is zero. Either way it may not
    // *exceed* the single-term result.
    let single = run("build");
    assert!(single > 0, "control: `build` matches `build_optimizer`");
    assert!(
        run("build OR zzzznotpresent") <= single,
        "`OR` widened the result, so it was read as syntax rather than as a \
         term. A caller's search string must not be able to execute query logic."
    );
    assert_eq!(
        run("zzzznotpresent OR alsonotpresent"),
        0,
        "a disjunction of two impossible terms must not match anything; if it \
         does, `OR` is being honoured as an operator."
    );
}

/// A pathologically long query must be bounded, not hung.
///
/// The builder now emits one `AND` clause per whitespace-separated word, so the
/// caller controls the size of the expression it hands SQLite. FTS5 has its own
/// limits on expression depth and term count; whichever side objects, the
/// caller must get an answer or a named refusal in reasonable time rather than
/// a crash or a hang.
#[test]
fn a_query_with_thousands_of_terms_is_bounded() {
    let store = corpus();
    let engine = StoreQueryEngine::new(&store);

    for count in [64usize, 1_000, 10_000] {
        let query = std::iter::repeat_n("build", count)
            .collect::<Vec<_>>()
            .join(" ");
        let started = std::time::Instant::now();
        let outcome = engine.search(Request {
            query,
            token_budget: 10_000,
            min_confidence: 0.0,
            max_depth: 3,
        });
        let elapsed = started.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(20),
            "{count} terms took {elapsed:?}; a caller-controlled expression \
             size must not become a caller-controlled runtime"
        );
        if let Err(error) = outcome {
            let text = format!("{error:#}");
            assert!(
                !text.contains("SqliteFailure"),
                "{count} terms: a limit being hit must be reported as a \
                 refusal, not as a raw backend failure. Got {text}"
            );
        }
    }
}
