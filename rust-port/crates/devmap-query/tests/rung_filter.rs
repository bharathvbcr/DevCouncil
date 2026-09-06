//! W2.3 — the resolution ladder, addressable from outside.
//!
//! The kernel had five rungs, abstention, receiver poisoning and honesty
//! invariants asserting that each rung claims only what its evidence entitles
//! it to. A caller could not ask for one. `min_confidence: f32` reached `deps`,
//! `neighbors`, `explore`, `affected` and `preview` — but a float is the wrong
//! handle for a named ladder, and `impact` and `trace`, the two queries a
//! refactor actually runs, took no threshold at all.
//!
//! Two properties are load-bearing here, and each is asserted in both
//! directions:
//!
//! * **The floor narrows.** `min_rung: deterministic` must drop the speculative
//!   edges, and no floor must return exactly what it returned before.
//! * **The narrowing is countable.** The histogram describes the population
//!   *before* the cut. A narrowed answer that cannot say how much it dropped is
//!   indistinguishable from a sparse graph, and the second reading is the one
//!   that gets a live symbol deleted.
//!
//! The fixture is built so the population is genuinely mixed: a same-file call
//! resolves deterministically, and a call to a name defined in two files
//! resolves ambiguously to both. If the resolver's confidences ever change, the
//! first test below fails with the measured distribution rather than letting
//! the rest pass vacuously over a single-rung population.

use devmap_extract::extract_file;
use devmap_query::rung::{Rung, RungHistogram};
use devmap_query::{Request, ResolutionAvailability, Response, StoreQueryEngine};
use devmap_resolve::model::ResolvedEdge;
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};

/// `main` calls `helper` in its own file — one answer, `DETERMINISTIC`.
/// `helper` calls `shared`, which two files define and no import binds — the
/// resolver cannot choose, so it records both at a low rung rather than picking
/// one. That is the mix the histogram has to describe.
fn corpus() -> Store {
    let extractions = vec![
        extract_file("lib_a.py", "def shared():\n    return 1\n"),
        extract_file("lib_b.py", "def shared():\n    return 2\n"),
        // Declares nothing and is touched by nothing, so `dependencies` over
        // it walks a real population of size zero — the "looked and found none"
        // case. `lib_a.py` cannot serve it (the ambiguous fan-out lands on it
        // as an inbound edge) and neither can a file with a function in it (a
        // declaration is itself an edge).
        extract_file("orphan.py", "# nothing here\n"),
        // Four same-file calls and one ambiguous one. The chain length is not
        // decoration: `the_floor_is_applied_before_the_budget` needs more
        // qualifying edges than a tight budget can pack, because that is the
        // only condition under which the two orderings disagree.
        extract_file(
            "app.py",
            concat!(
                "def a():\n    return 1\n\n\n",
                "def b():\n    return a()\n\n\n",
                "def c():\n    return b()\n\n\n",
                "def helper():\n    return shared()\n\n\n",
                "def main():\n    c()\n    return helper()\n",
            ),
        ),
    ];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let analysis = devmap_analyze::analyze(&extractions, &resolution);
    let store = Store::open_in_memory().expect("in-memory store");
    store
        .save_generation_with_opts(
            &extractions,
            &resolution,
            &analysis,
            GenerationWriteOpts::default(),
        )
        .expect("generation writes");
    store
}

fn request(query: &str) -> Request<String> {
    Request {
        query: query.to_string(),
        token_budget: 8_000,
        min_confidence: 0.0,
        max_depth: 5,
    }
}

fn histogram_of(response: &Response<ResolvedEdge>) -> RungHistogram {
    response
        .rungs
        .clone()
        .expect("an edge query must report its distribution")
}

/// The precondition every other test in this file depends on.
///
/// Without it, a fixture that happened to produce one rung would let
/// "filtering changed nothing" pass as "filtering works".
#[test]
fn the_fixture_spans_more_than_one_rung() {
    let store = corpus();
    let engine = StoreQueryEngine::new(&store);
    let answer = engine.trace(request("app.py::main")).expect("trace");
    let hist = histogram_of(&answer);
    assert!(
        hist.deterministic > 0 && hist.speculative > 0,
        "the fixture must span rungs for the filter tests to mean anything; \
         measured {hist:?} over {} edges",
        answer.total
    );
}

/// ON: a floor removes what sits below it.
#[test]
fn a_deterministic_floor_drops_the_speculative_edges() {
    let store = corpus();
    let engine = StoreQueryEngine::new(&store);

    let open = engine.trace(request("app.py::main")).expect("trace");
    let floored = engine
        .trace_at_rung(request("app.py::main"), Some(Rung::Deterministic))
        .expect("trace at rung");

    assert!(
        floored.total < open.total,
        "the floor removed nothing: {} of {} survived",
        floored.total,
        open.total
    );
    for edge in &floored.items {
        assert!(
            edge.confidence.0 >= 1.0,
            "{} -> {} survived a deterministic floor at {}",
            edge.source_symbol,
            edge.target_symbol,
            edge.confidence.0
        );
    }
}

/// OFF: no floor is the behaviour that already existed.
///
/// The direction that keeps this parameter from being a silent breaking change
/// for every caller that predates it.
#[test]
fn no_floor_returns_exactly_what_it_returned_before() {
    let store = corpus();
    let engine = StoreQueryEngine::new(&store);

    let before = engine.trace(request("app.py::main")).expect("trace");
    let explicit_none = engine
        .trace_at_rung(request("app.py::main"), None)
        .expect("trace at no rung");
    let speculative = engine
        .trace_at_rung(request("app.py::main"), Some(Rung::Speculative))
        .expect("trace at the bottom rung");

    assert_eq!(before.total, explicit_none.total);
    assert_eq!(before.shown, explicit_none.shown);
    assert_eq!(
        before.total, speculative.total,
        "the bottom rung admits everything, so it must filter nothing"
    );
    assert_eq!(histogram_of(&explicit_none).filtered_out, 0);
    assert_eq!(histogram_of(&speculative).filtered_out, 0);
}

/// The histogram describes the population, not the survivors.
///
/// This is the whole reason the field exists: a caller who narrows must be able
/// to tell a filtered answer from a sparse graph.
#[test]
fn the_histogram_reports_what_the_filter_removed() {
    let store = corpus();
    let engine = StoreQueryEngine::new(&store);

    let open = histogram_of(&engine.trace(request("app.py::main")).expect("trace"));
    let floored = engine
        .trace_at_rung(request("app.py::main"), Some(Rung::Deterministic))
        .expect("trace at rung");
    let hist = histogram_of(&floored);

    assert_eq!(
        hist.total(),
        open.total(),
        "the histogram must count the population the floor was applied to, \
         not the slice that survived it"
    );
    assert!(hist.filtered_out > 0, "{hist:?}");
    assert_eq!(
        hist.total() - hist.filtered_out,
        floored.total as usize,
        "every edge is either returned or counted as filtered out"
    );
}

/// `high` is the boundary a float comparison could not be trusted at.
///
/// `Confidence::HIGH` is `0.9`, which SQLite REAL cannot round-trip. The
/// integer floor is what makes `>= high` mean the same thing after a store
/// round trip as before one — this asserts it over an actual round trip rather
/// than in unit-test memory.
#[test]
fn the_high_floor_survives_a_store_round_trip() {
    let store = corpus();
    let engine = StoreQueryEngine::new(&store);
    let answer = engine
        .trace_at_rung(request("app.py::main"), Some(Rung::High))
        .expect("trace at rung");
    for edge in &answer.items {
        assert!(
            devmap_extract::model::confidence_millis(edge.confidence.0) >= 700,
            "{} survived a high floor at {}",
            edge.target_symbol,
            edge.confidence.0
        );
    }
}

/// The floor is applied before the budget, not after.
///
/// The two orderings agree on almost everything, because edges are sorted by
/// confidence descending — a tight budget packs the deterministic ones first
/// either way, so the *items* look identical. They disagree on `total`:
/// filtering the packed slice makes `total` count what fit rather than what
/// qualified, and a caller reading `total: 2` when four edges cleared the floor
/// has been told the blast radius is half its real size. That is the same
/// "capped sample presented as complete coverage" defect this pass removes
/// elsewhere, so it is asserted here on `total`, not on `items`.
///
/// It also matters for cost: budgeting rows that are about to be discarded
/// spends the caller's tokens on nothing.
#[test]
fn the_floor_is_applied_before_the_budget() {
    let store = corpus();
    let engine = StoreQueryEngine::new(&store);

    let open = engine.trace(request("app.py::main")).expect("trace");
    let qualifying = histogram_of(&open).deterministic;
    assert!(
        qualifying >= 2,
        "the fixture must hold more qualifying edges than the tight budget \
         below can pack, or the two orderings cannot disagree; got {qualifying}"
    );

    // 25 tokens an edge, so this packs one fewer than qualifies.
    let mut tight = request("app.py::main");
    tight.token_budget = 25 * (qualifying as u32 - 1);
    let floored = engine
        .trace_at_rung(tight, Some(Rung::Deterministic))
        .expect("trace at rung");

    assert!(
        floored.truncated,
        "the budget must actually bite for this test to discriminate"
    );
    assert_eq!(
        floored.total as usize, qualifying,
        "`total` must count every edge that cleared the floor, not the subset \
         the budget packed"
    );
    assert!(floored.shown < floored.total);

    let hist = histogram_of(&floored);
    assert_eq!(
        hist.total() - hist.filtered_out,
        floored.total as usize,
        "the histogram counts the pre-budget population on both sides"
    );
}

/// `dependencies` takes the same floor, over rows the store filtered by
/// confidence first. Both filters apply; only the rung's cut is countable.
#[test]
fn dependencies_takes_the_same_floor() {
    let store = corpus();
    let engine = StoreQueryEngine::new(&store);

    let open = engine.dependencies(request("app.py")).expect("deps");
    let floored = engine
        .dependencies_at_rung(request("app.py"), Some(Rung::Deterministic))
        .expect("deps at rung");

    assert!(open.total > floored.total, "{} vs {}", open.total, floored.total);
    let hist = histogram_of(&floored);
    assert_eq!(hist.total(), histogram_of(&open).total());
    assert_eq!(hist.total() - hist.filtered_out, floored.total as usize);
}

/// Measured-and-empty and never-measured are different answers.
///
/// A query that walked a real population and found nothing reports an empty
/// histogram: it looked. A query that could not start reports no histogram at
/// all: `RungHistogram::default()` there would claim zero edges were observed,
/// which is a count standing in for a check that never ran — the failure this
/// whole pass exists to remove, reproduced inside the field built to prevent
/// it.
#[test]
fn an_unmeasured_population_reports_no_histogram_and_an_empty_one_reports_zero() {
    let store = corpus();
    let engine = StoreQueryEngine::new(&store);

    // Measured: the file is indexed and parsed, and holds no outbound edges.
    let measured = engine.dependencies(request("orphan.py")).expect("deps");
    assert!(
        matches!(measured.resolution, ResolutionAvailability::Available { .. }),
        "{:?}",
        measured.resolution
    );
    assert_eq!(measured.total, 0, "`orphan.py` has no edges in either direction");
    assert_eq!(
        histogram_of(&measured),
        RungHistogram::default(),
        "an answer that looked and found nothing reports zeroes"
    );

    // Not measured: no traversal start exists, so no edges were ever examined.
    let unmeasured = engine
        .trace_at_rung(request("orphan.py"), Some(Rung::Deterministic))
        .expect("trace at rung");
    assert!(
        matches!(unmeasured.resolution, ResolutionAvailability::Unavailable { .. }),
        "{:?}",
        unmeasured.resolution
    );
    assert!(
        unmeasured.rungs.is_none(),
        "an answer that never walked must not publish a distribution of zero,          which reads as `I looked and found none`: got {:?}",
        unmeasured.rungs
    );
}
