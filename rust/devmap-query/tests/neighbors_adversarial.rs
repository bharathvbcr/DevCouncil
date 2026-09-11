//! Adversarial inputs to the composed `neighbors` query.
//!
//! `neighbors_composition.rs` proves the composition equals the calls it
//! replaces on a fixture built to make that equality hold. This file attacks
//! the same surface from the other side: degenerate targets, hostile filter
//! values, ambiguous basenames, and the response invariants
//! `DevMapClient._budgeted` enforces on every side of every entry.
//!
//! The load-bearing rule under test is this repository's Class A rule — a check
//! that could not run must never report what a check that ran and passed
//! reports. `Response` has exactly two ways to say "I could not look":
//! `ResolutionAvailability::Unavailable` and `walk_incomplete`. Anything that
//! reaches a caller as `Available`, `truncated: false`, `items: []` is a
//! positive claim that the target has no neighbours in that direction.
//!
//! Tests whose names start with `defect_` fail against the code as it stands.
//! They are the proof, not a wish list; each names the input, the expected
//! answer and the answer actually produced.

use devmap_extract::extract_file;
use devmap_query::{Neighbors, Request, ResolutionAvailability, Response, StoreQueryEngine};
use devmap_resolve::model::ResolvedEdge;
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

const CORE: &str = "def helper(rows):\n    return sum(rows)\n";
const CALLER: &str = "from core import helper\n\n\ndef run(rows):\n    return helper(rows)\n";
/// A second `core.py`, in a package, with a same-named function. Ambiguous
/// basenames are the common case in real trees (`models.py`, `utils.py`,
/// `__init__.py`, `main.go`), not an exotic one.
const PKG_CORE: &str = "def helper(rows):\n    return len(rows)\n";
const PKG_USER: &str =
    "from pkg.core import helper\n\n\ndef pkg_run(rows):\n    return helper(rows)\n";
/// Tree-sitter recovers from this but records error ranges over it, so the
/// stored file is `ParseOutcome::Partial`.
const BROKEN: &str = "def (:\n  ???\n";

fn store_of(files: &[(&str, &str)]) -> Store {
    let extractions: Vec<_> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
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

/// Two files, one call between them. The smallest fixture with a real edge.
fn simple() -> Store {
    store_of(&[("core.py", CORE), ("caller.py", CALLER)])
}

/// The same graph, plus a package that shadows `core.py`'s basename and has
/// its own `helper` with its own caller. Nothing in here calls anything in the
/// top-level files, so any edge crossing between the two halves is a
/// mis-attribution and not a property of the corpus.
fn shadowed() -> Store {
    store_of(&[
        ("core.py", CORE),
        ("caller.py", CALLER),
        ("pkg/core.py", PKG_CORE),
        ("pkg/user.py", PKG_USER),
    ])
}

fn edge_ids(response: &Response<ResolvedEdge>) -> Vec<String> {
    response
        .items
        .iter()
        .map(|edge| {
            format!(
                "{} -> {} [{:?}]",
                edge.source_symbol, edge.target_symbol, edge.edge_kind
            )
        })
        .collect()
}

/// Every invariant `DevMapClient._budgeted` enforces, in the same order it
/// enforces them. A violation here is a client-side exception in production,
/// so it is checked on every response this file produces rather than only
/// where a break is suspected.
fn assert_client_invariants(label: &str, response: &Response<ResolvedEdge>, budget: u32) {
    assert_eq!(
        response.shown as usize,
        response.items.len(),
        "{label}: shown={} but {} items were returned",
        response.shown,
        response.items.len()
    );
    assert_eq!(
        response.shown + response.hidden,
        response.total,
        "{label}: shown {} + hidden {} != total {}",
        response.shown,
        response.hidden,
        response.total
    );
    assert_eq!(
        response.truncated,
        response.hidden > 0,
        "{label}: truncated={} with hidden={}",
        response.truncated,
        response.hidden
    );
    assert!(
        response.tokens_used <= budget,
        "{label}: spent {} tokens of a {budget} budget",
        response.tokens_used
    );
    if matches!(
        response.resolution,
        ResolutionAvailability::Unavailable { .. }
    ) {
        assert!(
            response.items.is_empty() && response.total == 0,
            "{label}: an Unavailable response still carried {} of {} items",
            response.items.len(),
            response.total
        );
    }
}

fn assert_entry_invariants(entry: &Neighbors, budget: u32) {
    assert_client_invariants(&format!("{} callers", entry.target), &entry.callers, budget);
    assert_client_invariants(&format!("{} callees", entry.target), &entry.callees, budget);
}

// ---------------------------------------------------------------------------
// Proven defects
// ---------------------------------------------------------------------------

/// A confidence filter that cannot be evaluated must not answer "nothing".
///
/// `min_confidence: NaN` is reachable from the engine API and from
/// `devmap neighbors --min-confidence nan` (clap parses `nan` into an `f32`
/// and the CLI runs no bounds check; only `validate_request` on the IPC path
/// rejects it). The two halves of the answer then disagree about what the
/// filter meant, because the same documented comparison has two
/// implementations:
///
/// * `Store::latest_edges` filters in Rust — `(NaN * 1000.0).round() as i64`
///   saturates to `0`, so every edge is admitted.
/// * `Store::latest_edges_for_file` filters in SQL — SQLite stores NaN as
///   NULL, `confidence >= NULL` is NULL, so no edge is admitted.
///
/// The result is a fully populated `callers` beside an empty `callees` that
/// says `Available`: "this file depends on nothing", asserted by a comparison
/// that never ran.
#[test]
fn an_unevaluable_confidence_is_refused_and_a_real_one_filters_both_sides() {
    // Was `defect_a_non_finite_confidence_answers_available_and_empty`, which
    // bundled three values that turned out to be two different problems, and
    // whose code contradicted its own message: it named "either a refusal, or
    // Unavailable" as acceptable, then `expect()`ed an `Ok`. Rewritten to assert
    // the two properties rather than one implementation of them.
    //
    // NaN is genuinely un-evaluable: `admits` in Rust saturates it to 0 and
    // lets every edge through, while SQLite stores it as NULL so `>= NULL`
    // lets none through. It is refused at the store boundary now.
    //
    // 2.0 and infinity are *not* un-evaluable — nothing has confidence that
    // high, so an empty answer is a filter that ran and matched nothing, which
    // is a real result. What made them look like a defect was a separate bug:
    // `neighbors` hardcoded `min_confidence: 0.0` on the callers side, so one
    // composed answer reported every caller while reporting no callees. The two
    // halves disagreed about what the filter meant.
    let store = simple();
    let engine = StoreQueryEngine::new(&store);

    let honest = engine
        .neighbors(&["core.py".to_string()], 2000, 0.0, 1)
        .unwrap();
    assert!(
        honest[0].callees.total > 0 && honest[0].callers.total > 0,
        "fixture has no edges on one side at min_confidence 0, so the filter \
         assertions below would hold vacuously"
    );

    // 1. Un-evaluable: refused, not guessed.
    assert!(
        engine
            .neighbors(&["core.py".to_string()], 2000, f32::NAN, 1)
            .is_err(),
        "NaN was answered rather than refused; whichever of the two \
         implementations ran, the caller cannot tell the filter never did"
    );

    // 2. Evaluable but impossible: both sides agree, and both are empty.
    for threshold in [2.0f32, f32::INFINITY] {
        let answer = engine
            .neighbors(&["core.py".to_string()], 2000, threshold, 1)
            .expect("a finite, evaluable threshold is a real question");
        assert_entry_invariants(&answer[0], 2000);
        assert_eq!(
            (answer[0].callers.total, answer[0].callees.total),
            (0, 0),
            "min_confidence {threshold:?} filtered one direction and not the \
             other, so the two halves of one answer disagree about what the \
             filter meant"
        );
    }

    // 3. And a threshold the data actually straddles still filters both sides.
    let strict = engine
        .neighbors(&["core.py".to_string()], 2000, 0.95, 1)
        .expect("a normal threshold must answer");
    assert!(
        strict[0].callers.total <= honest[0].callers.total
            && strict[0].callees.total <= honest[0].callees.total,
        "raising the threshold grew an edge list"
    );
}

/// Callers and callees must be scoped to the *same* target.
///
/// `impact`/`trace` resolve a path through `query_match::path_matches`, which
/// accepts any file whose path *ends with* `/<query>`. `dependencies` resolves
/// it through `Store::latest_edges_for_file`, which is exact string equality on
/// the stored path. When two files share a basename the composed answer merges
/// one file's outbound edges with two files' inbound edges, and nothing in the
/// response says so.
///
/// The fixture's two halves are disjoint: nothing under `pkg/` calls anything
/// at the top level. So an edge from `pkg/user.py` appearing in the answer for
/// `core.py` is not a property of the corpus.
#[test]
fn an_ambiguous_basename_scopes_both_directions_the_same_way() {
    // Was `defect_an_ambiguous_basename_mixes_two_files_into_one_answer`.
    //
    // The composition used to mix two resolvers: `impact` matches a path by
    // suffix (`path_matches`: `file == query || file.ends_with("/{query}")`)
    // while `dependencies` matched exactly. With `core.py` and `pkg/core.py`
    // both indexed, one composed answer described *two different files* — its
    // callers were `pkg/core.py`'s and its callees were `core.py`'s. "What
    // calls core.py before I delete it" answered with a file that does not.
    //
    // Both directions now resolve through the traversal, so the two halves
    // cannot disagree about which file the target names. This test asserts that
    // coherence. It does **not** assert the scope is narrow: `impact`'s suffix
    // matching is pre-existing and unchanged, so an ambiguous basename still
    // answers about every file that ends with it. That over-breadth is a real
    // and separate issue — recorded in STATUS.md — but an over-broad answer to
    // one coherent question is a different failure from two answers to two
    // different questions.
    let store = shadowed();
    let engine = StoreQueryEngine::new(&store);

    let answer = engine
        .neighbors(&["core.py".to_string()], 20_000, 0.0, 1)
        .expect("an indexed file must be answerable");
    assert_entry_invariants(&answer[0], 20_000);

    let files_of = |response: &devmap_query::Response<devmap_resolve::model::ResolvedEdge>| {
        let mut seen: Vec<String> = response
            .items
            .iter()
            .flat_map(|edge| [edge.source_file.clone(), edge.target_file.clone()])
            .filter(|file| file.ends_with("core.py"))
            .collect();
        seen.sort();
        seen.dedup();
        seen
    };

    let caller_files = files_of(&answer[0].callers);
    let callee_files = files_of(&answer[0].callees);
    assert!(
        !caller_files.is_empty() && !callee_files.is_empty(),
        "one side of the fixture is empty, so this comparison asserts nothing: \
         callers={caller_files:?} callees={callee_files:?}"
    );
    assert_eq!(
        caller_files, callee_files,
        "the two halves of one composed answer describe different files: \
         callers touch {caller_files:?} while callees touch {callee_files:?}"
    );
}

/// `Neighbors::callees` is documented as "Outbound edges — what this target
/// reaches". For a file target it also carries the inbound edges, because
/// `Store::latest_edges_for_file` matches `sp.path = ?2 OR tp.path = ?2`.
///
/// The consequence at the consumer: `graph_cmd.py` renders a callee node from
/// each item's `target_file`/`target_symbol`, so `core.py`'s callee list
/// contains `core.py` — a file listed as its own dependency. That is the same
/// "symbols appeared to call themselves" shape the symbol-scoped path was
/// changed to avoid; it survives on the file-scoped path.
#[test]
fn a_file_targets_callees_are_outbound_only() {
    let store = simple();
    let engine = StoreQueryEngine::new(&store);
    let answer = engine
        .neighbors(&["core.py".to_string()], 20_000, 0.0, 1)
        .expect("an indexed file must be answerable");
    assert_entry_invariants(&answer[0], 20_000);

    let inbound: Vec<String> = answer[0]
        .callees
        .items
        .iter()
        .filter(|edge| edge.source_file != "core.py")
        .map(|edge| {
            format!(
                "{}::{} -> {}::{}",
                edge.source_file, edge.source_symbol, edge.target_file, edge.target_symbol
            )
        })
        .collect();

    let also_callers: Vec<&String> = inbound
        .iter()
        .filter(|id| {
            answer[0].callers.items.iter().any(|edge| {
                *id == &format!(
                    "{}::{} -> {}::{}",
                    edge.source_file, edge.source_symbol, edge.target_file, edge.target_symbol
                )
            })
        })
        .collect();

    assert!(
        inbound.is_empty(),
        "neighbors(\"core.py\").callees contains {} edge(s) that originate \
         elsewhere and point *into* core.py: {inbound:?}. {} of them are byte \
         for byte the same edges returned as `callers`, so one composed answer \
         reports the same edge in both directions. Expected: callees carries \
         only edges whose source is the target.",
        inbound.len(),
        also_callers.len()
    );
}

// ---------------------------------------------------------------------------
// Characterisations: behaviour that holds, pinned so a change is visible
// ---------------------------------------------------------------------------

/// `min_confidence` reaches only one of the two directions.
///
/// `StoreQueryEngine::neighbors` hardcodes `min_confidence: 0.0` into its
/// `impact` call (engine.rs) and threads the caller's value only into
/// `dependencies`/`trace`. That matches what a separate `impact` call would
/// have answered — the IPC and CLI `impact` commands take no confidence at all
/// — so the composition is faithful. But the doc comment on `neighbors` says
/// each target gets the calls it would have got "with the same budget and
/// confidence", and the IPC `Neighbors` command exposes a `min_confidence`
/// field that visibly does nothing to half the response.
///
/// Pinned rather than asserted-against: if the parameter ever starts reaching
/// `impact`, this test says so, and the composition-equivalence test next door
/// will need updating in the same change.
#[test]
fn min_confidence_filters_both_directions_of_one_answer() {
    // Was `min_confidence_is_applied_to_callees_only`, a characterisation of a
    // bug: `neighbors` hardcoded `min_confidence: 0.0` into its `impact` call,
    // so the caller's filter reached only half the answer.
    //
    // That test was also vacuous for its own claim. It compared thresholds 0.0
    // and 1.0 on the callers side, and every edge in this fixture has
    // confidence >= 1.0 — so it passed whether or not the filter was applied.
    // Only its callees half, which used 1.1, could tell the difference. A
    // threshold has to actually straddle the data to prove anything.
    let store = simple();
    let engine = StoreQueryEngine::new(&store);

    let permissive = engine
        .neighbors(&["core.py".to_string()], 20_000, 0.0, 1)
        .unwrap();
    assert!(
        permissive[0].callers.total > 0 && permissive[0].callees.total > 0,
        "fixture is empty on one side, so the comparisons below assert nothing"
    );

    // 1.1 is above every confidence the resolver assigns, so a direction that
    // honours the filter must come back empty and one that ignores it must not.
    let strict = engine
        .neighbors(&["core.py".to_string()], 20_000, 1.1, 1)
        .unwrap();
    assert_eq!(
        strict[0].callers.total, 0,
        "min_confidence 1.1 left {} callers; the inbound direction is ignoring \
         the filter, so one composed answer reports every caller while \
         reporting no callees",
        strict[0].callers.total
    );
    assert_eq!(
        strict[0].callees.total, 0,
        "min_confidence 1.1 left {} callees",
        strict[0].callees.total
    );
}

/// An indexed symbol with no outbound edges reports its callees `Unavailable`.
///
/// `helper` is in the store and calls nothing. `trace` builds its start set
/// from edges that have the target on the *source* side, finds none, and
/// returns `Unavailable: … has no indexed traversal start` — "I could not
/// look" for a symbol the store knows about and could answer for.
///
/// This errs in the safe direction, so it is characterised rather than filed
/// as a break. It is still an inconsistency worth seeing: an indexed *file*
/// with no callers answers `Available` with an empty list, and the same
/// underlying situation ("nothing found") reaches the caller two different
/// ways depending on the target's shape. `dev map query`'s callee column is
/// blank for exactly the leaf symbols where "calls nothing" is the useful
/// answer.
#[test]
fn a_leaf_symbol_reports_unavailable_callees_where_a_leaf_file_reports_empty() {
    let store = simple();
    let engine = StoreQueryEngine::new(&store);

    let symbol = engine
        .neighbors(&["core.py::helper".to_string()], 2000, 0.0, 1)
        .unwrap();
    assert!(
        store.latest_symbol_names_by_file().unwrap()["core.py"].contains("helper"),
        "the fixture must actually index `helper`, or 'unavailable for an \
         indexed symbol' is not what is being observed"
    );
    assert!(
        matches!(
            symbol[0].callees.resolution,
            ResolutionAvailability::Unavailable { .. }
        ),
        "an indexed leaf symbol now answers {:?} for its callees; if that is \
         the intended fix, it is the honest one and this test should be \
         retired",
        symbol[0].callees.resolution
    );

    // The file-shaped equivalent of "nothing found" is a confident empty.
    let file = engine
        .neighbors(&["caller.py".to_string()], 2000, 0.0, 1)
        .unwrap();
    assert!(
        matches!(
            file[0].callers.resolution,
            ResolutionAvailability::Available
        ) && file[0].callers.items.is_empty(),
        "the file-shaped half of this asymmetry changed: {:?}",
        file[0].callers.resolution
    );
}

/// A `Partial` parse answers `Available` with whatever survived.
///
/// `dependencies` refuses only `ParseOutcome::Failed`. A file tree-sitter
/// recovered from — error ranges recorded, symbols possibly missed — answers
/// `Available` with an empty edge list and no incompleteness marker, which
/// reads as "this file depends on nothing". `Response` has a carrier for
/// exactly this (`walk_incomplete`) and nothing sets it from the parse
/// outcome.
///
/// Characterised, not filed: partial extraction results are the design, and
/// the honest fix (a parse-incompleteness carrier) is a wider change than this
/// query. The test exists so the gap is visible and dated.
#[test]
fn a_partially_parsed_file_reports_that_it_has_no_traversal_start() {
    // Was `a_partially_parsed_file_answers_available_with_no_incompleteness_marker`,
    // a characterisation of a Class A gap: a file tree-sitter had only partly
    // recovered answered `Available` with an empty list and nothing set on
    // `walk_incomplete` — "this depends on nothing", from a file whose contents
    // were never fully read. `dependencies` refused only `ParseOutcome::Failed`.
    //
    // Routing callees through the traversal closed it as a side effect: a file
    // with no usable node reports `Unavailable` naming the reason, which is the
    // same answer a file with no grammar at all already gave. The test that
    // recorded the gap now asserts the signal, as its own failure message
    // instructed.
    // A file tree-sitter can only partly recover: the `def` opens a body that
    // never closes, so extraction reports `ParseOutcome::Partial` rather than
    // `Failed`, which is the case `dependencies` used to answer confidently.
    let store = store_of(&[
        ("broken.py", BROKEN),
        (
            "user.py",
            "import broken\n\n\ndef run():\n    return broken.widget()\n",
        ),
    ]);
    let engine = StoreQueryEngine::new(&store);

    let answer = engine
        .neighbors(&["broken.py".to_string()], 2000, 0.0, 1)
        .expect("a partially parsed file is an answer, not a transport failure");
    assert_entry_invariants(&answer[0], 2000);

    match &answer[0].callees.resolution {
        ResolutionAvailability::Unavailable { reason } => assert!(
            !reason.trim().is_empty(),
            "the refusal carries no reason, so a caller learns only that it failed"
        ),
        ResolutionAvailability::Available => panic!(
            "a partially parsed file reported Available with {} callees and \
             walk_incomplete={:?} — a positive claim about a file that was \
             never fully read",
            answer[0].callees.total, answer[0].callees.walk_incomplete
        ),
    }
}

/// The default depth reports its own walk as incomplete.
///
/// Both the CLI (`--depth 1`) and `DevMapClient.neighbors` (`depth: int = 1`)
/// default to depth 1, and at that depth the reverse walk is routinely capped:
/// `walk_incomplete` is `Some(...)` while `truncated` is `false` and `hidden`
/// is `0`, because the budget did not withhold anything — the walk did.
///
/// Use a real two-hop chain. The original one-hop fixture only looked capped
/// because the stop probe counted excluded reverse containment edges.
#[test]
fn the_default_depth_marks_its_walk_incomplete_rather_than_truncated() {
    let store = store_of(&[
        ("core.py", CORE),
        ("caller.py", CALLER),
        (
            "outer.py",
            "from caller import run\n\ndef outer(rows):\n    return run(rows)\n",
        ),
    ]);
    let engine = StoreQueryEngine::new(&store);
    let answer = engine
        .neighbors(&["core.py".to_string()], 20_000, 0.0, 1)
        .unwrap();
    let callers = &answer[0].callers;

    assert!(
        !callers.truncated && callers.hidden == 0,
        "the budget withheld something at this size, so the walk cap is not \
         what this test is observing"
    );
    assert!(
        callers.walk_incomplete.is_some(),
        "a depth-1 reverse walk over this fixture reported itself complete; \
         the only remaining signal for a capped walk would be gone"
    );

    let complete = engine
        .neighbors(&["core.py".into()], 20_000, 0.0, 2)
        .unwrap();
    assert_eq!(complete[0].callers.walk_incomplete, None);
    assert!(complete[0]
        .callers
        .items
        .iter()
        .any(|edge| edge.source_symbol.ends_with("::outer")));
    assert!(!callers
        .items
        .iter()
        .any(|edge| edge.source_symbol.ends_with("::outer")));

    // Depth 0 is accepted by `validate_request` (0 <= MAX_TRAVERSAL_DEPTH) and
    // by the CLI. It visits nothing, and says so only through this field.
    let none = engine
        .neighbors(&["core.py::helper".to_string()], 20_000, 0.0, 0)
        .unwrap();
    assert!(
        none[0].callers.items.is_empty()
            && matches!(
                none[0].callers.resolution,
                ResolutionAvailability::Available
            )
            && none[0].callers.walk_incomplete.is_some(),
        "depth 0 returned {:?} / walk_incomplete {:?}; an empty Available \
         answer with no incompleteness marker would be a confident 'nothing \
         calls this' from a walk that never took a step",
        none[0].callers.resolution,
        none[0].callers.walk_incomplete
    );
}

// ---------------------------------------------------------------------------
// Degenerate and hostile target shapes
// ---------------------------------------------------------------------------

/// Every degenerate target shape either answers honestly or says it could not
/// look — and every response, whichever it is, satisfies the client's
/// invariants.
///
/// The sweep is exhaustive over the list below and reports both halves of its
/// own coverage: nothing here is a sample presented as complete.
#[test]
fn every_degenerate_target_shape_keeps_the_response_invariants() {
    let store = shadowed();
    let engine = StoreQueryEngine::new(&store);

    // (target, must_be_unavailable_in_both_directions)
    let shapes: &[(&str, bool)] = &[
        ("", true),
        ("   ", true),
        ("\t\n", true),
        ("::", true),
        ("::helper", true),
        ("core.py::", true),
        ("core.py::helper::extra", true),
        ("/etc/passwd", true),
        ("/", true),
        ("../core.py", true),
        ("../../../../../../etc/passwd", true),
        ("./core.py", true),
        ("pkg/../core.py", true),
        ("nul\0byte.py", true),
        ("core.py\0", true),
        ("\u{1F600}\u{1F600}.py", true),
        ("\u{202e}core.py", true),
        ("C:\\Windows\\system32\\core.py", true),
        ("core.py ", false), // trimmed by traversal matching, exact for deps
        ("core.py", false),
        ("core.py::helper", false),
        ("pkg/core.py", false),
    ];

    let mut unavailable_in_both = 0usize;
    let mut answered = 0usize;
    for (target, expect_unavailable) in shapes {
        for budget in [0u32, 25, 20_000, u32::MAX] {
            for depth in [0usize, 1, 64, usize::MAX] {
                let answer = engine
                    .neighbors(&[(*target).to_string()], budget, 0.0, depth)
                    .unwrap_or_else(|error| {
                        panic!("target {target:?} budget {budget} depth {depth} failed: {error}")
                    });
                assert_eq!(answer.len(), 1, "target {target:?} lost its entry");
                assert_eq!(
                    &answer[0].target, target,
                    "the echoed target must be byte-identical to the request; \
                     the client aligns answers by it"
                );
                assert_entry_invariants(&answer[0], budget);
            }
        }

        let answer = engine
            .neighbors(&[(*target).to_string()], 20_000, 0.0, 1)
            .unwrap();
        let both_unavailable = matches!(
            answer[0].callers.resolution,
            ResolutionAvailability::Unavailable { .. }
        ) && matches!(
            answer[0].callees.resolution,
            ResolutionAvailability::Unavailable { .. }
        );
        if both_unavailable {
            unavailable_in_both += 1;
        } else {
            answered += 1;
        }
        assert_eq!(
            both_unavailable, *expect_unavailable,
            "target {target:?}: expected both directions unavailable = \
             {expect_unavailable}, got callers {:?} / callees {:?}. A shape \
             that starts answering `Available` with an empty list is claiming \
             the target exists and has no neighbours.",
            answer[0].callers.resolution, answer[0].callees.resolution
        );
    }

    assert_eq!(
        unavailable_in_both + answered,
        shapes.len(),
        "coverage accounting is wrong: {unavailable_in_both} refused + \
         {answered} answered != {} swept",
        shapes.len()
    );
    assert_eq!(
        shapes.len(),
        22,
        "the sweep list changed size; update the count so a silently shortened \
         list cannot be read as the same coverage"
    );
}

/// A path that leaves the repository is refused by name, not read from disk.
///
/// `preview` had exactly this hole (`preview_containment.rs`). `neighbors`
/// never opens a file — it only matches strings against stored paths — so the
/// escape has nothing to reach. This pins that: the refusal must quote the
/// target back, and a file that genuinely exists outside the corpus must still
/// be unknown to it.
#[test]
fn a_target_outside_the_corpus_is_unknown_rather_than_read() {
    let store = simple();
    let engine = StoreQueryEngine::new(&store);
    // Exists on every unix box this runs on; the store has never heard of it.
    for escape in ["/etc/hosts", "../../etc/hosts", "/etc/passwd"] {
        let answer = engine
            .neighbors(&[escape.to_string()], 2000, 0.0, 1)
            .unwrap();
        for (side, response) in [
            ("callers", &answer[0].callers),
            ("callees", &answer[0].callees),
        ] {
            match &response.resolution {
                ResolutionAvailability::Unavailable { reason } => assert!(
                    reason.contains(escape),
                    "{escape} {side}: the refusal must name what was refused, \
                     got {reason:?}"
                ),
                other => {
                    panic!("{escape} {side}: answered {other:?} for a path outside the corpus")
                }
            }
            assert!(response.items.is_empty(), "{escape} {side} returned items");
        }
    }
}

/// An empty target list is an unambiguous request for nothing.
#[test]
fn an_empty_target_list_answers_with_an_empty_list() {
    let store = simple();
    let engine = StoreQueryEngine::new(&store);
    let answer = engine
        .neighbors(&[], 2000, 0.0, 1)
        .expect("asking about nothing is a valid question");
    assert!(
        answer.is_empty(),
        "got {} entries for no targets",
        answer.len()
    );
}

/// Duplicate targets each get their own entry, in order, with equal answers.
///
/// The client zips its request against the response and rejects a mismatch, so
/// de-duplicating server-side would break alignment for every caller that
/// happens to ask twice. Sixteen copies also sits exactly on the fan-out bound.
#[test]
fn duplicate_targets_stay_aligned_and_answer_identically() {
    let store = simple();
    let engine = StoreQueryEngine::new(&store);
    let targets = vec!["core.py".to_string(); devmap_query::MAX_NEIGHBOR_TARGETS];

    let answer = engine.neighbors(&targets, 20_000, 0.0, 1).unwrap();
    assert_eq!(
        answer.len(),
        targets.len(),
        "duplicates were collapsed; the client's zip against its request would \
         mis-attribute every answer after the first"
    );
    let first = edge_ids(&answer[0].callers);
    assert!(!first.is_empty(), "fixture has no callers to compare");
    for (index, entry) in answer.iter().enumerate() {
        assert_eq!(entry.target, "core.py", "entry {index} lost its target");
        assert_eq!(
            edge_ids(&entry.callers),
            first,
            "entry {index} answered differently from entry 0 for the same target"
        );
        assert_entry_invariants(entry, 20_000);
    }
}

/// A mixed batch of every shape at once, at the fan-out bound, still answers
/// one aligned entry per target.
///
/// Individually each shape is covered above; batched is where an early
/// `Unavailable` could drop an entry, shift the list, or abort the fan-out.
#[test]
fn a_full_batch_of_mixed_shapes_answers_one_entry_per_target_in_order() {
    let store = shadowed();
    let engine = StoreQueryEngine::new(&store);
    let targets: Vec<String> = [
        "",
        "core.py",
        "/etc/passwd",
        "core.py::helper",
        "../core.py",
        "pkg/core.py",
        "::",
        "caller.py",
        "\u{1F600}.py",
        "pkg/user.py::pkg_run",
        "core.py::",
        "pkg/core.py::helper",
        "nul\0.py",
        "caller.py::run",
        "   ",
        "pkg/user.py",
    ]
    .iter()
    .map(|target| (*target).to_string())
    .collect();
    assert_eq!(
        targets.len(),
        devmap_query::MAX_NEIGHBOR_TARGETS,
        "this batch is meant to sit exactly on the fan-out bound"
    );

    let answer = engine.neighbors(&targets, 20_000, 0.0, 3).unwrap();
    assert_eq!(answer.len(), targets.len());
    for (entry, requested) in answer.iter().zip(targets.iter()) {
        assert_eq!(&entry.target, requested, "the batch reordered its answers");
        assert_entry_invariants(entry, 20_000);
    }

    let answered = answer
        .iter()
        .filter(|entry| matches!(entry.callers.resolution, ResolutionAvailability::Available))
        .count();
    assert!(
        answered >= 6,
        "only {answered} of {} entries produced an Available callers side; the \
         batch is not exercising the answering path",
        targets.len()
    );
}

/// One target past the bound is refused, and the refusal is total: no partial
/// answer comes back alongside the error.
///
/// `neighbors_composition.rs` pins the boundary itself. What is added here is
/// that the *first* sixteen are not computed and handed back with an error on
/// the side — a caller that ignored the error would otherwise read a complete
/// list for a smaller question.
#[test]
fn an_over_long_batch_produces_no_partial_answer() {
    let store = simple();
    let engine = StoreQueryEngine::new(&store);
    let mut over = vec!["core.py".to_string(); devmap_query::MAX_NEIGHBOR_TARGETS];
    over.push("caller.py".to_string());

    let error = engine
        .neighbors(&over, 20_000, 0.0, 1)
        .expect_err("one past the bound must be refused");
    assert!(
        error.to_string().contains("17") && error.to_string().contains("16"),
        "the refusal must name both the limit and the ask, got: {error}"
    );
    // Nothing partial is reachable: `Result` carries no answer. Assert the
    // shorter list still works, so the refusal is the bound and not a
    // permanently broken engine.
    assert_eq!(
        engine
            .neighbors(&over[..devmap_query::MAX_NEIGHBOR_TARGETS], 20_000, 0.0, 1)
            .unwrap()
            .len(),
        devmap_query::MAX_NEIGHBOR_TARGETS
    );
}

/// Extreme budgets and depths neither overflow nor break the invariants.
///
/// `budget_take` accumulates `u32` token costs and `traverse` clamps depth with
/// `min(64)`; `usize::MAX` depth and `u32::MAX` budget are both reachable from
/// the CLI, which applies no bounds of its own.
#[test]
fn extreme_budgets_and_depths_neither_overflow_nor_lie() {
    let store = shadowed();
    let engine = StoreQueryEngine::new(&store);
    let targets = vec![
        "core.py".to_string(),
        "core.py::helper".to_string(),
        "pkg/user.py".to_string(),
    ];

    let mut cases = 0usize;
    for budget in [0u32, 1, 24, 25, 26, u32::MAX] {
        for depth in [0usize, 1, 63, 64, 65, usize::MAX] {
            let answer = engine.neighbors(&targets, budget, 0.0, depth).unwrap();
            for entry in &answer {
                assert_entry_invariants(entry, budget);
            }
            cases += 1;
        }
    }
    assert_eq!(cases, 36, "the budget x depth matrix changed size");

    // Depth past the internal clamp answers exactly as the clamp does, rather
    // than differently or not at all.
    let clamped = engine.neighbors(&targets, 20_000, 0.0, 64).unwrap();
    let absurd = engine.neighbors(&targets, 20_000, 0.0, usize::MAX).unwrap();
    for (a, b) in clamped.iter().zip(absurd.iter()) {
        assert_eq!(
            edge_ids(&a.callers),
            edge_ids(&b.callers),
            "depth {} and usize::MAX disagree for {}",
            64,
            a.target
        );
    }
}

/// A composed answer is not guaranteed to come from one generation.
///
/// Each target's `impact` reads the process-wide edge cache keyed on the
/// latest generation id, and each target's `dependencies` runs a fresh SQL
/// query; a generation committed part-way through the fan-out is therefore
/// visible to later targets and not earlier ones. This test races commits
/// against composed reads and asserts what must hold regardless: no panic, no
/// poisoned store, and every response internally consistent.
///
/// It is self-validating on the race itself — if the writer never advanced the
/// generation while readers were in flight, the test fails rather than passing
/// on an interleaving that never happened. Any observed cross-generation mix is
/// reported by name.
#[test]
fn a_composed_read_racing_a_commit_never_straddles_silently() {
    const GENERATIONS: usize = 12;
    let store = Arc::new(Store::open_in_memory().unwrap());

    // Generation k has exactly one caller symbol, `run_k`, so any answer that
    // names two different k values came from two generations.
    let commit = |store: &Store, k: usize| {
        let caller =
            format!("from core import helper\n\n\ndef run_{k}(rows):\n    return helper(rows)\n");
        let extractions = vec![
            extract_file("core.py", CORE),
            extract_file("caller.py", &caller),
        ];
        let mut resolver = Resolver::new();
        resolver.index_extractions(&extractions);
        let resolution = resolver.resolve_all(&extractions);
        let analysis = devmap_analyze::analyze(&extractions, &resolution);
        store
            .save_generation_with_opts(
                &extractions,
                &resolution,
                &analysis,
                GenerationWriteOpts::default(),
            )
            .unwrap();
    };
    commit(&store, 0);

    let stop = Arc::new(AtomicUsize::new(0));
    let writer = {
        let store = Arc::clone(&store);
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            for k in 1..GENERATIONS {
                commit(&store, k);
                stop.store(k, Ordering::SeqCst);
                std::thread::yield_now();
            }
        })
    };

    let reads = Arc::new(AtomicUsize::new(0));
    let mixed = Arc::new(AtomicUsize::new(0));
    let undisclosed = Arc::new(AtomicUsize::new(0));
    let readers: Vec<_> = (0..4)
        .map(|_| {
            let store = Arc::clone(&store);
            let stop = Arc::clone(&stop);
            let reads = Arc::clone(&reads);
            let mixed = Arc::clone(&mixed);
            let undisclosed = Arc::clone(&undisclosed);
            std::thread::spawn(move || {
                let engine = StoreQueryEngine::new(&store);
                let targets = vec!["core.py::helper".to_string(), "caller.py".to_string()];
                while stop.load(Ordering::SeqCst) < GENERATIONS - 1 {
                    let answer = engine.neighbors(&targets, 20_000, 0.0, 2).unwrap();
                    reads.fetch_add(1, Ordering::SeqCst);
                    let mut seen: Vec<usize> = Vec::new();
                    for entry in &answer {
                        assert_entry_invariants(entry, 20_000);
                        for side in [&entry.callers, &entry.callees] {
                            for edge in &side.items {
                                for symbol in [&edge.source_symbol, &edge.target_symbol] {
                                    if let Some(tail) = symbol.rsplit("::run_").next() {
                                        if tail != symbol.as_str() {
                                            if let Ok(k) = tail.parse::<usize>() {
                                                if !seen.contains(&k) {
                                                    seen.push(k);
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if seen.len() > 1 {
                        mixed.fetch_add(1, Ordering::SeqCst);
                        // A straddle is tolerable; a *silent* straddle is not.
                        // The composition retries once and, if the index moves
                        // again, marks every direction — so a caller can always
                        // tell a mixed answer from a coherent one.
                        let disclosed = answer.iter().any(|entry| {
                            [&entry.callers, &entry.callees].iter().any(|side| {
                                side.walk_incomplete
                                    .as_deref()
                                    .is_some_and(|note| note.contains("generation"))
                            })
                        });
                        if !disclosed {
                            undisclosed.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                }
            })
        })
        .collect();

    writer.join().expect("the writer thread panicked");
    for reader in readers {
        reader.join().expect("a reader thread panicked");
    }

    assert!(
        reads.load(Ordering::SeqCst) >= GENERATIONS,
        "only {} composed reads completed against {GENERATIONS} generations; \
         the readers never overlapped the writer, so this test asserted nothing \
         about a race",
        reads.load(Ordering::SeqCst)
    );
    assert_eq!(
        store.latest_generation_id().unwrap(),
        Some(GENERATIONS as u32),
        "the writer did not commit every generation, so the race window was \
         not what this test claims to have exercised"
    );
    assert_eq!(
        undisclosed.load(Ordering::SeqCst),
        0,
        "{} of {} composed answers carried symbols from more than one \
         generation *without saying so*. `neighbors` fans out over 4 \
         sub-queries, each taking and releasing the store lock on its own, so a \
         build committing mid-fan-out can leave one answer describing two \
         snapshots. That is tolerable; a caller unable to detect it is not. The \
         composition retries once and then marks every direction's \
         `walk_incomplete`, so a mixed answer is always distinguishable from a \
         coherent one.",
        undisclosed.load(Ordering::SeqCst),
        reads.load(Ordering::SeqCst)
    );
}

/// The engine's own `impact`/`dependencies` are unchanged by anything above.
///
/// Every defect test here compares the two halves of a composed answer; if
/// both halves regressed together the comparisons would pass. This asserts the
/// underlying single-target calls still answer, so a broken engine cannot make
/// this file green.
#[test]
fn the_underlying_single_target_calls_still_answer() {
    let store = simple();
    let engine = StoreQueryEngine::new(&store);
    let impact = engine
        .impact(Request {
            query: "core.py".to_string(),
            token_budget: 20_000,
            min_confidence: 0.0,
            max_depth: 1,
        })
        .unwrap();
    let deps = engine
        .dependencies(Request {
            query: "core.py".to_string(),
            token_budget: 20_000,
            min_confidence: 0.0,
            max_depth: 1,
        })
        .unwrap();
    assert!(impact.total > 0 && deps.total > 0, "the fixture went empty");
    assert_client_invariants("impact", &impact, 20_000);
    assert_client_invariants("dependencies", &deps, 20_000);
}
