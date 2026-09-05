//! Two surfaces that answered confidently from a check that had not run.
//!
//! Both were found by audit and both are the repository's Class A rule: a check
//! that could not run must never report what a check that ran and passed
//! reports. The failures are worth restating because neither looks like a bug in
//! its output — each returns a well-formed, plausible, *wrong* answer.
//!
//! * `preview` compared a buffer against a file it could not read and reported
//!   the result as though there had been no file at all, so a genuine symbol
//!   **removal disappeared** and the report came back clean.
//! * `QueryEngine::impact`/`trace` computed "the walk stopped at the depth cap;
//!   this is a lower bound, not the full blast radius" and then dropped it,
//!   publishing `walk_incomplete: None`. For `impact` that is the reading that
//!   gets a live symbol deleted: an incomplete blast radius and a small one look
//!   identical.
//! * `StoreQueryEngine::dead_symbols` published a delete-this list with no
//!   denominator at all — neither `AnalysisSummary::status` nor
//!   `unresolved_calls`, the field whose own documentation says it exists so a
//!   reader can tell "nothing calls this" from "we could not work out what
//!   this calls". A generation with 4,242 unattributed calls answered in the
//!   exact shape of one with none.

use devmap_extract::extract_file;
use devmap_extract::model::{Extraction, ExtractionEngine, ParseOutcome, SymbolKind};
use devmap_query::{QueryEngine, Request, StoreQueryEngine};
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};

/// A chain `a -> b -> c -> d`, four hops, so a depth-2 walk must stop short.
fn chain() -> Vec<devmap_extract::model::Extraction> {
    let files = [
        ("d.py", "def d():\n    return 1\n"),
        ("c.py", "from d import d\n\n\ndef c():\n    return d()\n"),
        ("b.py", "from c import c\n\n\ndef b():\n    return c()\n"),
        ("a.py", "from b import b\n\n\ndef a():\n    return b()\n"),
    ];
    files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect()
}

#[test]
fn a_depth_capped_impact_walk_reports_that_it_stopped() {
    let extractions = chain();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let engine = QueryEngine::new(&extractions, &resolution);

    let capped = engine.impact(Request {
        query: "d".to_string(),
        token_budget: 10_000,
        min_confidence: 0.0,
        max_depth: 1,
    });

    assert!(
        capped.walk_incomplete.is_some(),
        "a walk stopped by the depth cap must say so; publishing `None` presents a \
         lower bound as the full blast radius, which is the reading that gets a live \
         symbol deleted. Got: {capped:?}"
    );
    let reason = capped.walk_incomplete.as_deref().unwrap_or_default();
    assert!(
        reason.contains("depth"),
        "the reason must name what stopped the walk, got {reason:?}"
    );
}

/// The signal must be *absent* when the walk actually completed.
///
/// Without this, a fix that always sets `walk_incomplete` would pass the test
/// above while making the marker meaningless — every answer would carry it, and
/// a caller that cannot tell complete from incomplete is back where it started.
#[test]
fn a_complete_walk_does_not_claim_to_be_incomplete() {
    let extractions = chain();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let engine = QueryEngine::new(&extractions, &resolution);

    let complete = engine.impact(Request {
        query: "d".to_string(),
        token_budget: 10_000,
        min_confidence: 0.0,
        max_depth: 64,
    });

    assert!(
        complete.walk_incomplete.is_none(),
        "a walk that reached the end of the graph must not be marked incomplete, \
         got {:?}",
        complete.walk_incomplete
    );
}

#[test]
fn a_depth_capped_trace_walk_reports_that_it_stopped() {
    let extractions = chain();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let engine = QueryEngine::new(&extractions, &resolution);

    let capped = engine.trace(Request {
        query: "a".to_string(),
        token_budget: 10_000,
        min_confidence: 0.0,
        max_depth: 1,
    });

    assert!(
        capped.walk_incomplete.is_some(),
        "trace must carry the same signal impact does; got {capped:?}"
    );
}

/// A persisted generation over `files`, as `devmap build` would write one.
fn store_of(files: &[(&str, &str)], refuse: &[&str]) -> Store {
    let mut extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    for ext in &mut extractions {
        if !refuse.contains(&ext.file_path.as_str()) {
            continue;
        }
        // Shaped as `refused_extraction` builds one: the File node only.
        ext.parse_outcome = ParseOutcome::Failed {
            reason: "forced parse failure".to_string(),
        };
        ext.engine = ExtractionEngine::Unavailable {
            requested_language: ext.language.clone(),
        };
        ext.symbols.retain(|sym| sym.kind == SymbolKind::File);
        ext.imports.clear();
        ext.calls.clear();
        ext.wiring.clear();
    }
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

/// A call the resolution ladder cannot attribute to anything, beside a symbol
/// nothing calls. The dead-symbol finding and the blind spot that could
/// contradict it live in the same generation, which is the whole point.
#[test]
fn dead_symbols_computed_over_unattributed_calls_say_so() {
    let store = store_of(
        &[
            ("lib.py", "def abandoned():\n    return 1\n"),
            (
                "app.py",
                "def run(thing):\n    return thing.mystery_method()\n",
            ),
        ],
        &[],
    );
    let dead = StoreQueryEngine::new(&store).dead_symbols(10_000).unwrap();

    assert!(
        dead.items.iter().any(|row| row.symbol_name == "abandoned"),
        "the finding itself must survive — hiding it would be its own lie: {:?}",
        dead.items
    );
    assert!(
        dead.walk_incomplete.is_some(),
        "this list is read as 'delete these', and it was computed from a call \
         graph with holes in it. A generation with unattributed calls must not \
         answer in the shape of one with none. Got {dead:?}"
    );
    let reason = dead.walk_incomplete.as_deref().unwrap_or_default();
    assert!(
        reason.contains("unattributed") || reason.contains("unresolved"),
        "the reason must name the unattributed calls, got {reason:?}"
    );
}

/// The status half. Corpus-level extraction loss already reaches
/// `AnalysisStatus::Partial` and `graph_degraded`; the dead-symbol query is the
/// surface that most needs it and was the one not carrying it.
#[test]
fn dead_symbols_from_a_degraded_analysis_say_so() {
    let store = store_of(
        &[
            ("lib.py", "def helper():\n    return 1\n"),
            (
                "app.py",
                "from lib import helper\n\n\ndef main():\n    return helper()\n",
            ),
        ],
        &["app.py"],
    );
    let dead = StoreQueryEngine::new(&store).dead_symbols(10_000).unwrap();

    let reason = dead.walk_incomplete.as_deref().unwrap_or_default();
    assert!(
        !reason.is_empty(),
        "the analysis this list came from is `partial`; the list must not \
         read as complete. Got {dead:?}"
    );
    assert!(
        reason.contains("partial"),
        "the reason must carry the analysis status, got {reason:?}"
    );
}

/// The OFF direction. A generation where every call resolved and the analysis
/// converged must answer clean — otherwise the marker is on every answer and a
/// caller is back to having no way to tell the two apart.
#[test]
fn dead_symbols_over_a_complete_analysis_do_not_claim_to_be_incomplete() {
    let store = store_of(
        &[
            ("lib.py", "def helper():\n    return 1\n"),
            (
                "app.py",
                "from lib import helper\n\n\ndef main():\n    return helper()\n",
            ),
        ],
        &[],
    );
    let dead = StoreQueryEngine::new(&store).dead_symbols(10_000).unwrap();

    assert!(
        dead.items.iter().any(|row| row.symbol_name == "main"),
        "nothing calls `main`, so the query still has an answer: {:?}",
        dead.items
    );
    assert!(
        dead.walk_incomplete.is_none(),
        "every call resolved and the analysis converged: {:?}",
        dead.walk_incomplete
    );
}

/// Q-8: the qualification has to carry a *denominator*, not just an adjective.
///
/// "The analysis is partial" tells a reader to be careful and nothing about how
/// careful. The audit's complaint was the missing number: a list read as
/// "delete these" needs to say how much of the corpus was actually searched for
/// callers, because one unread file is a different risk from two hundred.
///
/// The numbers reach here through `ExtractionCoverage::degraded_reason`, which
/// `analyze()` folds into `AnalysisStatus::Partial` and `dead_symbols` surfaces
/// on `walk_incomplete`. This pins the whole chain: a change anywhere along it
/// that drops the counts leaves the adjective behind and fails here.
#[test]
fn a_degraded_dead_symbol_list_names_how_much_of_the_corpus_was_read() {
    let store = store_of(
        &[
            ("lib.py", "def helper():\n    return 1\n"),
            (
                "app.py",
                "from lib import helper\n\n\ndef main():\n    return helper()\n",
            ),
        ],
        &["app.py"],
    );
    let dead = StoreQueryEngine::new(&store).dead_symbols(10_000).unwrap();
    let reason = dead
        .walk_incomplete
        .as_deref()
        .expect("a partial analysis must qualify its dead list");

    assert!(
        reason.contains("did not cover the whole corpus"),
        "the reason must name the coverage gap, got {reason:?}"
    );
    assert!(
        reason.contains("1 file(s) failed to parse"),
        "the reason must carry the count of unread files, got {reason:?}"
    );
    assert!(
        reason.contains("lower bound"),
        "the reason must say the list is a lower bound, got {reason:?}"
    );
    // And the counters still describe the page honestly alongside it.
    assert_eq!(dead.shown as usize, dead.items.len());
    assert_eq!(dead.total, dead.shown + dead.hidden);
}

/// Force a stored parse outcome, as the extractor would have recorded it.
///
/// `Fallback` is what the pattern scanner produces for a language with no
/// linked grammar (`devmap-extract/src/fallback.rs`): named declarations are
/// matched, and **no calls and no imports are extracted at all**. Reproducing
/// that here rather than relying on which grammars this build happens to link
/// keeps the test about the outcome flag, which is the thing every consumer
/// reads.
fn degrade(ext: &mut Extraction, outcome: ParseOutcome) {
    if matches!(outcome, ParseOutcome::Fallback { .. }) {
        ext.engine = ExtractionEngine::Unavailable {
            requested_language: ext.language.clone(),
        };
        ext.imports.clear();
        ext.calls.clear();
    }
    ext.parse_outcome = outcome;
}

fn store_of_extractions(extractions: Vec<Extraction>) -> Store {
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

/// `dependencies` called a pattern-recovered file's edge list complete.
///
/// Both engines special-cased `ParseOutcome::Failed` — a file that contributed
/// nothing — and let `Fallback` fall through to `resolution: Available` with no
/// caveat. But a fallback extraction contains no imports and no calls by
/// construction, so its dependency set is empty *because nothing looked*, and
/// that answer was byte-identical to the one given for a fully parsed file that
/// genuinely imports nothing. `preview` has always reported this file state;
/// `dependencies` is where a reader goes to ask what a file needs.
///
/// `Partial` is the same shape one tier up: tree-sitter parsed the file and
/// flagged error ranges, and a call inside an error region is invisible to
/// extraction — so the edge list is a lower bound there too.
#[test]
fn dependencies_over_a_degraded_parse_say_the_edge_list_is_a_lower_bound() {
    for outcome in [
        ParseOutcome::Fallback {
            reason: "no linked grammar for vb; declarations recovered by pattern".to_string(),
        },
        ParseOutcome::Partial {
            error_ranges: Vec::new(),
        },
    ] {
        let mut extractions = vec![
            extract_file("lib.py", "def helper():\n    return 1\n"),
            extract_file(
                "app.py",
                "from lib import helper\n\n\ndef run():\n    return helper()\n",
            ),
        ];
        degrade(&mut extractions[1], outcome.clone());
        let store = store_of_extractions(extractions.clone());

        let stored = StoreQueryEngine::new(&store)
            .dependencies(Request {
                query: "app.py".to_string(),
                token_budget: 10_000,
                min_confidence: 0.0,
                max_depth: 3,
            })
            .unwrap();
        assert!(
            stored.walk_incomplete.is_some(),
            "a {outcome:?} file's dependency list is a lower bound and must say so; got {stored:?}"
        );

        let mut resolver = Resolver::new();
        resolver.index_extractions(&extractions);
        let resolution = resolver.resolve_all(&extractions);
        let in_memory = QueryEngine::new(&extractions, &resolution).dependencies(Request {
            query: "app.py".to_string(),
            token_budget: 10_000,
            min_confidence: 0.0,
            max_depth: 3,
        });
        assert!(
            in_memory.walk_incomplete.is_some(),
            "the in-memory engine must carry the same caveat as the store-backed one; \
             got {in_memory:?}"
        );
    }
}

/// The caveat must not appear on a clean parse.
///
/// A marker that rides on every answer leaves a reader exactly where they
/// started, which is the failure mode `dead_symbol_coverage_gap` documents.
#[test]
fn dependencies_over_a_clean_parse_carry_no_coverage_caveat() {
    let extractions = vec![
        extract_file("lib.py", "def helper():\n    return 1\n"),
        extract_file(
            "app.py",
            "from lib import helper\n\n\ndef run():\n    return helper()\n",
        ),
    ];
    let store = store_of_extractions(extractions.clone());

    let stored = StoreQueryEngine::new(&store)
        .dependencies(Request {
            query: "app.py".to_string(),
            token_budget: 10_000,
            min_confidence: 0.0,
            max_depth: 3,
        })
        .unwrap();
    assert_eq!(
        stored.walk_incomplete, None,
        "a cleanly parsed file's dependency list is complete: {stored:?}"
    );
    assert!(
        !stored.items.is_empty(),
        "the fixture must actually produce edges, or the assertion above is vacuous"
    );
}

/// `impact` answered "nothing calls this" over a corpus it had not fully read.
///
/// `dead_symbols` has carried the analysis coverage on `walk_incomplete` since
/// Q-8, and it is the *safer* of the two surfaces: it is explicitly a
/// candidate list, and its exemption rules already drop symbols in unread
/// files. `impact` is the one a reader consults immediately before deleting or
/// changing a symbol — the MCP tool description says exactly that — and it
/// published `items: [], resolution: Available, walk_incomplete: None` on a
/// generation whose call extraction never covered the file that might hold the
/// caller. An empty blast radius and an unsearched one were the same answer.
///
/// `trace` is the same walk in the other direction and gets it from the same
/// place: there is one `traverse_over`.
#[test]
fn a_traversal_over_a_partly_read_corpus_says_so() {
    // `app.py` contributed no calls at all, so nothing in it can appear as a
    // caller of `helper` — which is precisely the file a reader would need to
    // have been searched before believing an empty answer.
    let store = store_of(
        &[
            ("lib.py", "def helper():\n    return 1\n"),
            (
                "app.py",
                "from lib import helper\n\n\ndef main():\n    return helper()\n",
            ),
        ],
        &["app.py"],
    );
    let engine = StoreQueryEngine::new(&store);
    let request = |query: &str| Request {
        query: query.to_string(),
        token_budget: 10_000,
        min_confidence: 0.0,
        max_depth: 3,
    };

    let blast = engine.impact(request("helper")).unwrap();
    let reason = blast
        .walk_incomplete
        .as_deref()
        .unwrap_or_else(|| panic!("impact over a partial analysis must qualify itself: {blast:?}"));
    assert!(
        reason.contains("did not cover the whole corpus"),
        "the qualification must carry the coverage numbers, not just an adjective: {reason:?}"
    );

    // `helper` has no forward edges, and `trace` says so outright with
    // `Unavailable` — an honest refusal, not a silent empty answer, so it is
    // not the case worth pinning. `lib.py` does have forward edges (it contains
    // `helper`), and that is where the same walk must carry the same signal.
    let forward = engine.trace(request("lib.py")).unwrap();
    assert!(
        !forward.items.is_empty(),
        "the fixture must give trace something to walk: {forward:?}"
    );
    assert!(
        forward.walk_incomplete.is_some(),
        "trace shares one traversal with impact and must carry the same signal: {forward:?}"
    );
}

/// The corpus caveat must not displace the walk's own stop reason.
///
/// Two independent qualifications — "the graph I walked has holes" and "I
/// stopped before the graph ran out" — and a reader deciding whether to delete
/// a symbol needs both. Assigning rather than composing would have silently
/// dropped whichever ran second.
#[test]
fn a_capped_walk_over_a_partly_read_corpus_reports_both_reasons() {
    let store = store_of(
        &[
            ("d.py", "def d():\n    return 1\n"),
            ("c.py", "from d import d\n\n\ndef c():\n    return d()\n"),
            ("b.py", "from c import c\n\n\ndef b():\n    return c()\n"),
            ("a.py", "from b import b\n\n\ndef a():\n    return b()\n"),
            ("unread.py", "def spare():\n    return 2\n"),
        ],
        &["unread.py"],
    );
    let capped = StoreQueryEngine::new(&store)
        .impact(Request {
            query: "d".to_string(),
            token_budget: 10_000,
            min_confidence: 0.0,
            max_depth: 1,
        })
        .unwrap();
    let reason = capped
        .walk_incomplete
        .as_deref()
        .unwrap_or_else(|| panic!("a capped walk must report its cap: {capped:?}"));
    assert!(
        reason.contains("depth"),
        "the walk's own stop reason must survive: {reason:?}"
    );
    assert!(
        reason.contains("did not cover the whole corpus"),
        "the corpus coverage gap must survive alongside it: {reason:?}"
    );
}

/// A complete analysis leaves a complete walk unqualified.
#[test]
fn a_complete_traversal_over_a_complete_corpus_claims_nothing() {
    let store = store_of(
        &[
            ("lib.py", "def helper():\n    return 1\n"),
            (
                "app.py",
                "from lib import helper\n\n\ndef main():\n    return helper()\n",
            ),
        ],
        &[],
    );
    let blast = StoreQueryEngine::new(&store)
        .impact(Request {
            query: "helper".to_string(),
            token_budget: 10_000,
            min_confidence: 0.0,
            max_depth: 5,
        })
        .unwrap();
    assert!(
        !blast.items.is_empty(),
        "the fixture must produce a real blast radius, or the assertion below is vacuous"
    );
    assert_eq!(
        blast.walk_incomplete, None,
        "a complete walk over a fully read corpus must claim nothing: {blast:?}"
    );
}
