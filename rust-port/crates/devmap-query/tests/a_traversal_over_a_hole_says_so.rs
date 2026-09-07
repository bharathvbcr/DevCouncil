//! K-A2, the traversal half: `impact`, `trace` and `neighbors` answer over the
//! same corpus `dead` does, and must be as honest about what was not read.
//!
//! The rule was closed twice already and both closures stopped short of the
//! walk. `dependencies` refuses a file whose parse failed, and `dead_symbols`
//! carries the analysis's `Partial` reason into `walk_incomplete` — so a
//! delete-this list over a corpus with a hole in it says it is a lower bound.
//! The traversal did neither. Measured on the fixture below, where `app.py` is
//! the only caller of `helper` and its extraction was refused:
//!
//! ```text
//! dead                 shown=2 walk_incomplete=Some("the analysis is partial: …")
//! deps(app.py)         Unavailable { reason: "app.py could not be parsed" }
//! impact(lib.py::helper)  shown=0 total=0 Available walk_incomplete=None
//! neighbors → callers     shown=0        Available walk_incomplete=None
//! ```
//!
//! `impact` is the answer an agent reads as "nothing calls this, the blast
//! radius is empty, it is safe to change or delete", and it is the one answer
//! of the three that said so with no qualification at all — over a generation
//! whose own analysis summary records that a file failed to parse. A check that
//! could not run reported what a check that ran and passed reports.
//!
//! The fixture is deliberately tiny and deterministic: the refusal is injected
//! rather than provoked, because provoking one means feeding the extractor a
//! file large or pathological enough to exhaust the 5-second parse budget, and
//! a test whose fixture depends on machine speed is a test that reports the
//! machine.

use devmap_extract::extract_file;
use devmap_extract::model::{Extraction, ExtractionEngine, ParseOutcome, SymbolKind};
use devmap_query::{Request, StoreQueryEngine};
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};

/// Shape an extraction exactly as `treesitter::refused_extraction` does: the
/// `File` node only, engine `Unavailable`, no imports, calls or wiring. Setting
/// `parse_outcome` alone would leave the call edges in place and the fixture
/// could not reproduce anything.
fn refuse(extraction: &mut Extraction) {
    extraction.parse_outcome = ParseOutcome::Failed {
        reason: "forced parse failure".to_string(),
    };
    extraction.engine = ExtractionEngine::Unavailable {
        requested_language: extraction.language.clone(),
    };
    extraction
        .symbols
        .retain(|symbol| symbol.kind == SymbolKind::File);
    extraction.imports.clear();
    extraction.calls.clear();
    extraction.wiring.clear();
}

fn corpus(refuse_the_caller: bool) -> Store {
    let mut extractions = vec![
        extract_file(
            "lib.py",
            "def helper():\n    return 1\n\n\ndef dangling():\n    return 2\n",
        ),
        extract_file(
            "app.py",
            "from lib import helper\n\n\ndef main():\n    return helper()\n",
        ),
    ];
    if refuse_the_caller {
        refuse(&mut extractions[1]);
    }
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
        token_budget: 2_000,
        min_confidence: 0.0,
        max_depth: 3,
    }
}

/// The one that matters: an empty blast radius over an unread corpus is a
/// lower bound, and has to say so.
#[test]
fn an_impact_over_a_corpus_with_an_unread_file_is_a_lower_bound() {
    let store = corpus(true);
    let engine = StoreQueryEngine::new(&store);

    let answer = engine.impact(request("lib.py::helper")).expect("impact");
    assert_eq!(
        answer.shown, 0,
        "the only caller lives in the file that was not read, so the list really is empty"
    );
    let reason = answer.walk_incomplete.as_deref().unwrap_or("");
    assert!(
        !reason.is_empty(),
        "impact reported an empty blast radius with resolution {:?} and no \
         qualification, over a generation whose analysis records a file that \
         failed to parse — the answer an agent reads as `safe to delete`",
        answer.resolution
    );
    assert!(
        reason.contains("failed to parse"),
        "the caveat must name what was not read, got: {reason}"
    );
}

/// The forward direction walks the same edges and inherits the same hole.
#[test]
fn a_trace_over_a_corpus_with_an_unread_file_is_a_lower_bound() {
    let store = corpus(true);
    let engine = StoreQueryEngine::new(&store);

    let answer = engine.trace(request("lib.py::helper")).expect("trace");
    assert!(
        answer
            .walk_incomplete
            .as_deref()
            .is_some_and(|reason| reason.contains("failed to parse")),
        "trace answered {:?} with walk_incomplete {:?}",
        answer.resolution,
        answer.walk_incomplete
    );
}

/// The composition must not launder the caveat away: both halves of every
/// entry carry it, because both halves are traversals over the same corpus.
#[test]
fn both_halves_of_a_composed_answer_carry_the_coverage_caveat() {
    let store = corpus(true);
    let engine = StoreQueryEngine::new(&store);

    let answers = engine
        .neighbors(&["lib.py::helper".to_string()], 2_000, 0.0, 3)
        .expect("neighbors");
    assert_eq!(answers.len(), 1);
    for (half, response) in [
        ("callers", &answers[0].callers),
        ("callees", &answers[0].callees),
    ] {
        assert!(
            response
                .walk_incomplete
                .as_deref()
                .is_some_and(|reason| reason.contains("failed to parse")),
            "the {half} half answered {:?} with walk_incomplete {:?}",
            response.resolution,
            response.walk_incomplete
        );
    }
}

/// The OFF direction, and the one that makes the other three mean anything: a
/// corpus the extractor read completely carries no caveat. A marker on every
/// answer leaves a caller exactly where it started.
#[test]
fn a_fully_read_corpus_carries_no_coverage_caveat() {
    let store = corpus(false);
    let engine = StoreQueryEngine::new(&store);

    let answer = engine.impact(request("lib.py::helper")).expect("impact");
    assert_eq!(answer.shown, 1, "app.py::main calls helper");
    assert_eq!(
        answer.walk_incomplete, None,
        "nothing was withheld and nothing failed to parse, so there is nothing to disclose"
    );

    let answers = engine
        .neighbors(&["lib.py::helper".to_string()], 2_000, 0.0, 3)
        .expect("neighbors");
    assert_eq!(answers[0].callers.walk_incomplete, None);
    assert_eq!(answers[0].callees.walk_incomplete, None);
}

/// A five-symbol chain, so a trace can be stopped by `--depth` short of the
/// target and the reason it gives can be read.
fn chain() -> Store {
    let extractions = vec![extract_file(
        "chain.py",
        "def e():\n    return 1\n\n\ndef d():\n    return e()\n\n\ndef c():\n    return d()\n\n\n\
         def b():\n    return c()\n\n\ndef a():\n    return b()\n",
    )];
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

fn trace(store: &Store, from: &str, to: &str, max_depth: usize) -> String {
    let response = StoreQueryEngine::new(store)
        .trace_between(Request {
            query: (from.to_string(), to.to_string()),
            token_budget: 2_000,
            min_confidence: 0.0,
            max_depth,
        })
        .expect("trace runs");
    match response.resolution {
        devmap_query::ResolutionAvailability::Unavailable { reason } => reason,
        other => panic!("{from}->{to} at depth {max_depth} must be Unavailable, got {other:?}"),
    }
}

/// The reason a stopped trace gives is one sentence a reader can act on.
///
/// The `format!` literal behind it had been re-wrapped without `\`
/// continuations, so the text the CLI printed carried runs of 26 spaces
/// inside the sentence: "stopped at depth 3 after                          visiting
/// 44 nodes". Read back from the release binary on a real store.
#[test]
fn a_stopped_trace_gives_its_reason_on_one_line() {
    let reason = trace(&chain(), "a", "e", 1);
    assert!(
        reason.contains("stopped at depth 1") && reason.contains("--depth"),
        "the reason must name the limit and the remedy: {reason}"
    );
    assert!(
        !reason.contains("  "),
        "the reason carries runs of whitespace from a broken literal: {reason:?}"
    );
}

/// A trace from a symbol to itself is not a search that ran out of budget.
///
/// `trace X X` walked the graph from `X` looking for `X`, never counted the
/// start as reached, and reported "stopped at depth 3 after visiting 44 nodes
/// without reaching the target; whether a path exists is unknown" — an
/// answer about the walk's budget for a question the walk cannot answer.
#[test]
fn a_trace_from_a_symbol_to_itself_says_so_instead_of_walking() {
    let reason = trace(&chain(), "a", "a", 3);
    assert!(
        reason.contains("same symbol"),
        "the reason must say the endpoints are the same symbol, not report a budget: {reason}"
    );
}
