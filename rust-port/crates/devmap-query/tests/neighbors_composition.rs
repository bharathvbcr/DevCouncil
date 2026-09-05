//! The composed `neighbors` query must answer exactly what the one-at-a-time
//! calls answered.
//!
//! `neighbors` exists for speed: the MCP `graph_query` view needed callers and
//! callees for each of the first few definitions a search returned, and asking
//! for them separately cost two round trips per definition — eleven for a
//! five-definition view. Under the CLI transport a round trip is a process
//! spawn, so that view stayed at ~1.1 s no matter how fast the store became.
//!
//! A faster query that answers something *different* is not an optimisation, it
//! is a regression with a good benchmark. So the load-bearing test here is
//! equivalence against the calls it replaces, not a timing.

use devmap_extract::extract_file;
use devmap_query::cancel::Cancel;
use devmap_query::{Request, StoreQueryEngine, MAX_NEIGHBOR_TARGETS};
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};
use std::time::{Duration, Instant};

const CORE: &str =
    "def helper(rows):\n    return sum(rows)\n\n\ndef unused(rows):\n    return rows\n";
const CALLER_A: &str = "from core import helper\n\n\ndef run_a(rows):\n    return helper(rows)\n";
const CALLER_B: &str =
    "from core import helper\n\n\ndef run_b(rows):\n    return helper(rows) + 1\n";

/// A store with a real call graph: two callers into one shared helper, plus a
/// function nothing calls, so "no callers" is a genuine answer under test and
/// not merely the absence of data.
fn fixture() -> Store {
    let extractions = vec![
        extract_file("core.py", CORE),
        extract_file("caller_a.py", CALLER_A),
        extract_file("caller_b.py", CALLER_B),
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

fn targets() -> Vec<String> {
    vec![
        "core.py".to_string(),
        "caller_a.py".to_string(),
        "caller_b.py".to_string(),
    ]
}

/// One composed call and six separate ones must produce the same answer.
///
/// Both directions are checked against the single call each replaces:
/// `impact` for callers, `trace` for callees. The composition deliberately does
/// *not* delegate to `dependencies` — see the note at the `trace` call below,
/// and `a_symbol_target_gets_its_own_callees_not_its_file_s`.
///
/// Compared field by field rather than by a single serialised blob, so a
/// failure names the direction and the counter that drifted instead of
/// printing two long JSON documents.
#[test]
fn a_composed_neighbors_answer_matches_the_calls_it_replaces() {
    let store = fixture();
    let engine = StoreQueryEngine::new(&store);
    let targets = targets();

    let composed = engine
        .neighbors(&targets, 2000, 0.0, 1)
        .expect("composed neighbors must answer");
    assert_eq!(
        composed.len(),
        targets.len(),
        "composed answer dropped or invented entries"
    );

    for (entry, target) in composed.iter().zip(targets.iter()) {
        assert_eq!(
            &entry.target, target,
            "entries must stay aligned with the request order"
        );

        let separate_callers = engine
            .impact(Request {
                query: target.clone(),
                token_budget: 2000,
                min_confidence: 0.0,
                max_depth: 1,
            })
            .expect("impact must answer");
        // `trace`, not `dependencies`. The composition used `dependencies`
        // for file targets until an adversarial sweep showed it answers the
        // wrong question for this field: its SQL matches
        // `sp.path = ?2 OR tp.path = ?2`, so a file's "outbound" edges included
        // edges pointing into it and the file appeared in its own callee list.
        // Both directions now resolve through the traversal, which is directed
        // by construction. On this fixture that took `core.py`'s callees from
        // 6 to 2 — the 4 dropped were inbound and structural edges that were
        // never callees.
        let separate_callees = engine
            .trace(Request {
                query: target.clone(),
                token_budget: 2000,
                min_confidence: 0.0,
                max_depth: 1,
            })
            .expect("trace must answer");

        for (direction, composed_side, separate_side) in [
            ("callers", &entry.callers, &separate_callers),
            ("callees", &entry.callees, &separate_callees),
        ] {
            assert_eq!(
                composed_side.shown, separate_side.shown,
                "{target} {direction}: shown differs"
            );
            assert_eq!(
                composed_side.hidden, separate_side.hidden,
                "{target} {direction}: hidden differs"
            );
            assert_eq!(
                composed_side.total, separate_side.total,
                "{target} {direction}: total differs"
            );
            assert_eq!(
                composed_side.truncated, separate_side.truncated,
                "{target} {direction}: truncated differs"
            );
            assert_eq!(
                composed_side.resolution, separate_side.resolution,
                "{target} {direction}: resolution differs — a composed answer \
                 that loses 'unavailable' reads as a confident empty one"
            );
            assert_eq!(
                serde_json::to_value(&composed_side.items).unwrap(),
                serde_json::to_value(&separate_side.items).unwrap(),
                "{target} {direction}: edge list differs"
            );
        }
    }
}

/// The fixture has to contain edges, or the equivalence above is a comparison
/// of two empty lists and proves nothing.
#[test]
fn the_equivalence_fixture_actually_carries_a_call_graph() {
    let store = fixture();
    let engine = StoreQueryEngine::new(&store);
    let composed = engine.neighbors(&targets(), 2000, 0.0, 1).unwrap();
    let edges: usize = composed
        .iter()
        .map(|entry| entry.callers.items.len() + entry.callees.items.len())
        .sum();
    assert!(
        edges > 0,
        "no target has any neighbour; the equivalence test would pass on two \
         empty lists and assert nothing"
    );
}

/// Over the fan-out bound the request is refused — never silently shortened.
///
/// Trimming would hand back a list that looks exactly like a complete answer
/// for a smaller question, which is the failure this codebase treats as a
/// class: a check that could not run must not report what a check that ran
/// and passed reports.
#[test]
fn too_many_targets_is_refused_rather_than_trimmed() {
    let store = fixture();
    let engine = StoreQueryEngine::new(&store);
    let over: Vec<String> = (0..=MAX_NEIGHBOR_TARGETS)
        .map(|index| format!("core.py::helper{index}"))
        .collect();

    let error = engine
        .neighbors(&over, 2000, 0.0, 1)
        .expect_err("an over-long target list must be refused")
        .to_string();
    assert!(
        error.contains(&MAX_NEIGHBOR_TARGETS.to_string())
            && error.contains(&over.len().to_string()),
        "the refusal must name the limit and what was asked for, got: {error}"
    );

    let at_limit: Vec<String> = over[..MAX_NEIGHBOR_TARGETS].to_vec();
    let answered = engine
        .neighbors(&at_limit, 2000, 0.0, 1)
        .expect("exactly the limit must still be answered");
    assert_eq!(
        answered.len(),
        MAX_NEIGHBOR_TARGETS,
        "the bound is off by one: the limit itself must be accepted"
    );
}

/// An unindexed target reports *why* it has no edges, in both directions.
///
/// The reason this matters: a caller asking "what calls this before I delete
/// it" must be able to tell "nothing calls it" from "I could not look."
#[test]
fn an_unindexed_target_is_unavailable_not_empty() {
    let store = fixture();
    let engine = StoreQueryEngine::new(&store);
    let composed = engine
        .neighbors(&["not/in/the/store.py".to_string()], 2000, 0.0, 1)
        .expect("an unindexed target is an answer, not a transport failure");

    let entry = &composed[0];
    assert!(
        matches!(
            entry.callees.resolution,
            devmap_query::ResolutionAvailability::Unavailable { .. }
        ),
        "an unindexed target's callees came back Available with an empty list, \
         which reads as 'this depends on nothing'"
    );
}

/// A cancelled composition stops partway through instead of running the whole
/// fan-out.
///
/// The flag is tripped *while* the loop runs, not before it: a pre-set flag is
/// caught by the first check whatever the checking strategy, so it would prove
/// far less than it looks like it proves.
///
/// What actually stops the work here is `impact`'s own traversal, which
/// consults the flag as it walks; the loop's per-target check only closes the
/// gap between sub-queries. This test was run against both the per-target
/// check and the `check_every(index)` form it replaced and passes on both, so
/// it pins the *property* — a cancelled composition stops partway through —
/// rather than one implementation of it.
///
/// It sizes itself against its own measured baseline: if the fixture is too
/// fast for "stopped early" to mean anything, it fails saying so rather than
/// passing vacuously.
#[test]
fn a_cancelled_composition_stops_partway_through_the_fan_out() {
    let store = wide_fixture();
    let targets: Vec<String> = (0..MAX_NEIGHBOR_TARGETS)
        .map(|index| format!("mod{index}.py"))
        .collect();

    let engine = StoreQueryEngine::new(&store);
    let started = Instant::now();
    engine
        .neighbors(&targets, 200_000, 0.0, 6)
        .expect("the uncancelled baseline must answer");
    let baseline = started.elapsed();
    assert!(
        baseline >= Duration::from_millis(150),
        "the fixture completes in {baseline:?}; too fast for 'stopped early' to \
         mean anything, so this test would assert nothing"
    );

    let cancel = Cancel::new();
    let engine = StoreQueryEngine::new(&store).with_cancel(cancel.clone());
    let flipper = {
        let cancel = cancel.clone();
        let after = baseline / 8;
        std::thread::spawn(move || {
            std::thread::sleep(after);
            cancel.cancel();
        })
    };
    let started = Instant::now();
    let outcome = engine.neighbors(&targets, 200_000, 0.0, 6);
    let elapsed = started.elapsed();
    flipper.join().expect("flipper panicked");

    assert!(
        outcome.is_err(),
        "a composition cancelled after {:?} ran all the way to completion",
        baseline / 8
    );
    assert!(
        elapsed < baseline / 2,
        "a cancelled composition took {elapsed:?} against an uncancelled \
         {baseline:?}; the loop is consulting the flag too rarely to matter"
    );
}

/// A flag already set before the call is refused immediately, too.
#[test]
fn a_composition_that_starts_cancelled_does_no_work() {
    let store = fixture();
    let cancel = Cancel::new();
    cancel.cancel();
    let engine = StoreQueryEngine::new(&store).with_cancel(cancel);
    assert!(
        engine.neighbors(&targets(), 2000, 0.0, 1).is_err(),
        "a composition ran with its cancel flag already set"
    );
}

/// A corpus wide enough that one `impact` traversal is real work: every module
/// calls into a shared hub, so a reverse walk from any of them fans across the
/// whole graph rather than terminating after a couple of edges.
fn wide_fixture() -> Store {
    let mut sources: Vec<(String, String)> = Vec::new();
    let mut hub = String::new();
    for index in 0..MAX_NEIGHBOR_TARGETS {
        hub.push_str(&format!(
            "def hub{index}(rows):\n    return sum(rows)\n\n\n"
        ));
    }
    sources.push(("hub.py".to_string(), hub));
    for module in 0..MAX_NEIGHBOR_TARGETS {
        let mut body = String::from("import hub\n\n\n");
        for func in 0..340 {
            body.push_str(&format!(
                "def f{module}_{func}(rows):\n    return hub.hub{}(rows)\n\n\n",
                func % MAX_NEIGHBOR_TARGETS
            ));
        }
        sources.push((format!("mod{module}.py"), body));
    }
    let extractions: Vec<_> = sources
        .iter()
        .map(|(path, body)| extract_file(path, body))
        .collect();
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

/// A symbol's callees are the symbol's own outbound calls.
///
/// `dependencies` resolves a file path, so for a symbol id it answered
/// `Unavailable: … is not indexed` — every time. The composed `query` view
/// therefore reported "callees: unknown" for every symbol-shaped query. Honest,
/// but the field never carried anything.
///
/// The two wrong ways to fix that are both ruled out here: reporting nothing
/// (the old behaviour), and answering with the *containing file's* outbound
/// edges, which makes a function appear to call every other function its file
/// touches — including itself.
#[test]
fn a_symbol_target_gets_its_own_callees_not_its_file_s() {
    let store = fixture();
    let engine = StoreQueryEngine::new(&store);

    let symbol = "caller_a.py::run_a".to_string();
    let composed = engine
        .neighbors(std::slice::from_ref(&symbol), 2000, 0.0, 1)
        .expect("a symbol target must be answerable");
    let callees = &composed[0].callees;

    assert!(
        matches!(
            callees.resolution,
            devmap_query::ResolutionAvailability::Available
        ),
        "a symbol's callees came back unavailable: {:?}",
        callees.resolution
    );

    let reached: Vec<String> = callees
        .items
        .iter()
        .map(|edge| serde_json::to_value(edge).unwrap()["target_symbol"].to_string())
        .collect();
    assert!(
        reached.iter().any(|name| name.contains("helper")),
        "run_a calls helper, but its callees are {reached:?}"
    );
    assert!(
        !reached.iter().any(|name| name.contains("run_b")),
        "run_b lives in a different file and is not called by run_a; \
         the answer is file-scoped, not symbol-scoped: {reached:?}"
    );

    // And the file that contains it still resolves the file-shaped way.
    let file = engine
        .neighbors(&["caller_a.py".to_string()], 2000, 0.0, 1)
        .expect("a file target must still be answerable");
    assert!(
        matches!(
            file[0].callees.resolution,
            devmap_query::ResolutionAvailability::Available
        ),
        "the file-shaped path regressed while fixing the symbol-shaped one"
    );
}
