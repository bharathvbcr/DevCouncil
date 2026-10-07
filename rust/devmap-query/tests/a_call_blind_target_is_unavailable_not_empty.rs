//! P2.7a, the traversal half: `dead` already prices a symbol in a call-blind
//! file with `CALL_BLIND_REASON`; `impact` and `trace` did not.
//!
//! A call-blind file is one a grammar read cleanly in a language this build has
//! no call extractor for — HCL, CFML. Every symbol in it has no inbound and no
//! outbound `Calls` edge *by construction*, and the corpus-level coverage
//! marker does not fire for it, because a `.tf` beside Python cannot hide a
//! Python caller (`language_can_reference`). Measured on the fixture below
//! before this test existed:
//!
//! ```text
//! impact(main.tf::variable.region)  shown=0 total=0 Available walk_incomplete=None
//! trace(main.tf::variable.region)   Unavailable { "… has no indexed traversal start" }
//! ```
//!
//! The first is the answer an agent reads as "nothing depends on this, safe to
//! change": a check that never ran, reported as one that ran and found nothing.
//! The second refuses, but for a reason that sends the reader looking for a
//! typo rather than telling them the extractor never looked.

use devmap_extract::extract_file;
use devmap_query::{Request, ResolutionAvailability, StoreQueryEngine};
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};

const CALL_BLIND_MARKER: &str = "no call extractor";

fn corpus() -> Store {
    let extractions = vec![
        extract_file(
            "lib.py",
            "def helper():\n    return 1\n\n\ndef main():\n    return helper()\n",
        ),
        extract_file(
            "main.tf",
            "variable \"region\" {\n  default = \"us-east-1\"\n}\n\n\
             output \"region_out\" {\n  value = var.region\n}\n",
        ),
    ];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
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
        token_budget: 2_000,
        min_confidence: 0.0,
        max_depth: 3,
    }
}

/// Whether an answer says, somewhere a reader acting on it will see, that the
/// extractor never looked for calls in this language.
fn names_the_blindness(resolution: &ResolutionAvailability, walk_incomplete: Option<&str>) -> bool {
    let refused = matches!(
        resolution,
        ResolutionAvailability::Unavailable { reason } if reason.contains(CALL_BLIND_MARKER)
    );
    refused || walk_incomplete.is_some_and(|reason| reason.contains(CALL_BLIND_MARKER))
}

/// The one that matters: an empty blast radius for a symbol whose language has
/// no call extractor is not a measurement, and must not be published as one.
#[test]
fn an_impact_on_a_call_blind_symbol_is_unavailable_rather_than_empty() {
    let store = corpus();
    let engine = StoreQueryEngine::new(&store);

    let answer = engine
        .impact(request("main.tf::variable.region"))
        .expect("impact");
    if answer.total == 0 {
        assert!(
            matches!(
                &answer.resolution,
                ResolutionAvailability::Unavailable { reason } if reason.contains(CALL_BLIND_MARKER)
            ),
            "impact on an HCL symbol answered an empty blast radius as {:?} with \
             walk_incomplete {:?} — the shape of a check that ran and found no \
             callers, for a language whose callers are never extracted",
            answer.resolution,
            answer.walk_incomplete
        );
    }
    assert!(
        names_the_blindness(&answer.resolution, answer.walk_incomplete.as_deref()),
        "impact must name the call-blindness, got {:?} / {:?}",
        answer.resolution,
        answer.walk_incomplete
    );
}

/// The forward direction is blind for the same reason: no call out of the file
/// was ever extracted. A refusal is right; a refusal that blames the query is
/// not.
#[test]
fn a_trace_on_a_call_blind_symbol_names_the_blindness() {
    let store = corpus();
    let engine = StoreQueryEngine::new(&store);

    let answer = engine
        .trace(request("main.tf::variable.region"))
        .expect("trace");
    assert!(
        names_the_blindness(&answer.resolution, answer.walk_incomplete.as_deref()),
        "trace on an HCL symbol answered {:?} with walk_incomplete {:?}",
        answer.resolution,
        answer.walk_incomplete
    );
}

/// A scoped trace's "no indexed path" is the one outcome it calls a claim about
/// the graph. With an endpoint in a call-blind file it is a claim about the
/// extractor, and must say so.
#[test]
fn a_scoped_trace_from_a_call_blind_symbol_does_not_claim_no_path() {
    let store = corpus();
    let engine = StoreQueryEngine::new(&store);

    let answer = engine
        .trace_between(Request {
            query: (
                "main.tf::output.region_out".to_string(),
                "lib.py::helper".to_string(),
            ),
            token_budget: 2_000,
            min_confidence: 0.0,
            max_depth: 3,
        })
        .expect("scoped trace");
    assert!(
        names_the_blindness(&answer.resolution, answer.walk_incomplete.as_deref()),
        "scoped trace from an HCL symbol answered {:?} / {:?}",
        answer.resolution,
        answer.walk_incomplete
    );
}

/// The banded form and the composition walk the same seeds through the same
/// kernel, and must not launder the caveat away.
#[test]
fn the_layered_impact_and_neighbors_carry_it_too() {
    let store = corpus();
    let engine = StoreQueryEngine::new(&store);

    let layered = engine
        .impact_layered(request("main.tf::variable.region"))
        .expect("impact --layers");
    assert!(
        names_the_blindness(
            &layered.edges.resolution,
            layered.edges.walk_incomplete.as_deref()
        ),
        "impact --layers edges answered {:?} / {:?}",
        layered.edges.resolution,
        layered.edges.walk_incomplete
    );

    let neighbors = engine
        .neighbors(&["main.tf::variable.region".to_string()], 2_000, 0.0, 3)
        .expect("neighbors");
    let callers = &neighbors[0].callers;
    assert!(
        names_the_blindness(&callers.resolution, callers.walk_incomplete.as_deref()),
        "neighbors → callers answered {:?} / {:?}",
        callers.resolution,
        callers.walk_incomplete
    );
}

/// The control. A marker that rides on every answer tells a reader nothing, so
/// a symbol in a language that *has* a call extractor must be answered exactly
/// as before: its real caller, and no call-blind caveat.
#[test]
fn a_symbol_in_a_language_with_call_extraction_is_not_qualified() {
    let store = corpus();
    let engine = StoreQueryEngine::new(&store);

    let answer = engine.impact(request("lib.py::helper")).expect("impact");
    assert!(
        matches!(answer.resolution, ResolutionAvailability::Available),
        "a Python symbol with a real caller must stay Available, got {:?}",
        answer.resolution
    );
    assert!(
        answer
            .items
            .iter()
            .any(|edge| edge.source_symbol.ends_with("main")),
        "the control's one real caller must still be found: {:?}",
        answer.items
    );
    assert!(
        !names_the_blindness(&answer.resolution, answer.walk_incomplete.as_deref()),
        "a call-blind caveat leaked onto a Python answer: {:?}",
        answer.walk_incomplete
    );
}
