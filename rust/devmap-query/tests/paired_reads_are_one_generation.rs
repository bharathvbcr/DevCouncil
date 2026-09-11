//! A count and the rows it measures must come from one generation.
//!
//! Three surfaces drew a list from one read and the number qualifying it from
//! another. Each read resolves "the latest generation" for itself, and the
//! daemon is designed to commit while clients query, so the two could describe
//! different corpora. `search` was fixed first (`shown=40 hidden=0 total=1`);
//! the kernel-store audit then found the same shape here:
//!
//! * `preview` listed callers above the confidence floor from one read and
//!   counted all callers from another, then subtracted. The subtraction is
//!   saturating, so when the newer generation held fewer callers the difference
//!   clamped to zero and `preview` reported that the floor had hidden nothing —
//!   "no ambiguous callers" and "I counted a different corpus" became the same
//!   answer, on the surface whose whole job is to say what an edit would break.
//! * `dead_symbols` attached a coverage disclosure derived from one read to
//!   rows from another. A disclosure saying the corpus was fully covered, over
//!   rows from a generation where it was not, is what promotes a finding from
//!   "look at this" to "safe to delete".
//!
//! `Store::callers_page` and `Store::dead_page` take both halves under one
//! pinned snapshot. These tests are deterministic: they do not race for the
//! interleaving, they check the property the pairing installs.

use devmap_extract::extract_file;
use devmap_extract::model::{Extraction, ExtractionEngine, ParseOutcome, SymbolKind};
use devmap_query::StoreQueryEngine;
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};
use std::path::PathBuf;

/// Persist one generation over `files`, refusing to parse anything in `refuse`
/// so the analysis comes back degraded.
fn commit(store: &Store, files: &[(&str, &str)], refuse: &[&str]) {
    let mut extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    for ext in &mut extractions {
        if !refuse.contains(&ext.file_path.as_str()) {
            continue;
        }
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
    store
        .save_generation_with_opts(
            &extractions,
            &resolution,
            &analysis,
            GenerationWriteOpts::default(),
        )
        .unwrap();
}

const LIB: &str = "def helper():\n    return 1\n\n\ndef orphan():\n    return 2\n";
const APP: &str = "from lib import helper\n\n\ndef main():\n    return helper()\n";

#[test]
fn semantic_and_dependency_pages_pair_rows_with_their_own_analysis_under_writes() {
    use std::sync::{Arc, Barrier};
    let store = Arc::new(Store::open_in_memory().unwrap());
    commit(&store, &[("app.py", "def initial():\n    return 1\n")], &[]);
    let barrier = Arc::new(Barrier::new(2));
    let writer_store = Arc::clone(&store);
    let writer_barrier = Arc::clone(&barrier);
    let writer = std::thread::spawn(move || {
        writer_barrier.wait();
        for round in 0..100 {
            let mut source = "def root():\n    return 1\n".to_string();
            for i in 0..if round % 2 == 0 { 40 } else { 1 } {
                source.push_str(&format!("def member_{i}():\n    return root()\n"));
            }
            commit(&writer_store, &[("app.py", &source)], &[]);
            writer_store.prune_generations_except_latest(1).unwrap();
        }
    });
    barrier.wait();
    for _ in 0..300 {
        let symbols = store.all_symbols_page().unwrap().unwrap();
        assert_eq!(symbols.rows.len(), symbols.analysis.unwrap().total_symbols);
        assert_eq!(symbols.total as usize, symbols.rows.len());
        let file = store.file_edges("app.py", 0.0).unwrap().unwrap();
        assert_eq!(file.edges.len(), file.analysis.unwrap().total_edges);
        assert_eq!(file.file.path, "app.py");
    }
    writer.join().unwrap();
    assert_eq!(store.latest_generation_id().unwrap(), Some(101));
}

#[test]
fn explore_under_rebuilds_is_consistent_or_qualifies_every_part() {
    use std::sync::{Arc, Barrier};
    let store = Arc::new(Store::open_in_memory().unwrap());
    let generation = |store: &Store, label: &str| {
        let source = format!("class {label}:\n    def ping(self):\n        return 1\ndef enter_{label}():\n    return {label}().ping()\n");
        commit(store, &[("app.py", &source)], &[]);
    };
    generation(&store, "Even");
    let barrier = Arc::new(Barrier::new(2));
    let writer_store = Arc::clone(&store);
    let writer_barrier = Arc::clone(&barrier);
    let writer = std::thread::spawn(move || {
        writer_barrier.wait();
        for round in 0..100 {
            generation(&writer_store, if round % 2 == 0 { "Odd" } else { "Even" });
            writer_store.prune_generations_except_latest(1).unwrap();
        }
    });
    barrier.wait();
    let engine = StoreQueryEngine::new(&store);
    for _ in 0..300 {
        let report = engine.explore("ping", 5, 20_000, 0.0, 64).unwrap();
        assert_eq!(report.definitions.shown, 1, "{report:?}");
        let definition = &report.definitions.items[0];
        if report
            .definitions
            .walk_incomplete
            .as_deref()
            .is_some_and(|s| s.contains("index moved"))
        {
            for reason in [
                &report.blast_radius.layers.walk_incomplete,
                &definition.callers.walk_incomplete,
                &definition.callees.walk_incomplete,
            ] {
                assert!(
                    reason.as_deref().is_some_and(|s| s.contains("index moved")),
                    "{report:?}"
                );
            }
        } else {
            assert!(
                !definition.callers.items.is_empty(),
                "a stable answer must include its caller: {report:?}"
            );
            assert!(
                definition
                    .callers
                    .items
                    .iter()
                    .all(|e| e.target_symbol == definition.id),
                "{report:?}"
            );
        }
    }
    writer.join().unwrap();
    assert_eq!(store.latest_generation_id().unwrap(), Some(101));
}

#[test]
fn a_dead_page_reports_the_analysis_of_the_generation_it_listed() {
    let store = Store::open_in_memory().unwrap();
    // Generation 1: degraded, because `app.py` would not parse.
    commit(&store, &[("lib.py", LIB), ("app.py", APP)], &["app.py"]);
    // Generation 2: the same corpus, fully parsed.
    commit(&store, &[("lib.py", LIB), ("app.py", APP)], &[]);

    let page = store
        .dead_page(usize::MAX)
        .unwrap()
        .expect("the store holds a generation");

    assert_eq!(
        page.generation, 2,
        "the page must name the generation it read"
    );
    let analysis = page.analysis.expect("generation 2 stored its analysis");
    assert!(
        matches!(analysis.status, devmap_analyze::model::AnalysisStatus::Ok),
        "the disclosure must come from the generation whose rows are attached to \
         it — generation 1 was degraded and generation 2 is not, so reading the \
         older analysis beside the newer rows is visible here: {:?}",
        analysis.status
    );
    assert_eq!(
        analysis.total_files, 2,
        "the analysis must describe the corpus the rows came from"
    );
}

/// The OFF direction: a degraded generation must still report degraded.
///
/// Without this, a pairing that always read the newest *clean* analysis would
/// satisfy the test above while hiding every real coverage gap.
#[test]
fn a_dead_page_over_a_degraded_generation_still_says_so() {
    let store = Store::open_in_memory().unwrap();
    commit(&store, &[("lib.py", LIB), ("app.py", APP)], &["app.py"]);

    let page = store
        .dead_page(usize::MAX)
        .unwrap()
        .expect("a generation exists");
    let analysis = page.analysis.expect("analysis stored");
    assert!(
        matches!(
            analysis.status,
            devmap_analyze::model::AnalysisStatus::Partial { .. }
        ),
        "this generation could not parse app.py; the page must carry that: {:?}",
        analysis.status
    );

    // And the engine surfaces it on the findings themselves.
    let dead = StoreQueryEngine::new(&store).dead_symbols(10_000).unwrap();
    assert!(
        dead.walk_incomplete.is_some(),
        "a delete-this list computed over a partial analysis must say so: {dead:?}"
    );
}

#[test]
fn a_callers_page_cannot_report_fewer_callers_than_it_listed() {
    let store = Store::open_in_memory().unwrap();
    commit(&store, &[("lib.py", LIB), ("app.py", APP)], &[]);

    let page = store
        .callers_page(&["lib.py::helper".to_string()], "app.py", 0.0)
        .unwrap()
        .expect("a generation exists");
    assert_eq!(page.generation, 1);
    assert!(
        page.total_unfiltered >= page.callers.len(),
        "the unfiltered total is a superset of the floored list by construction; \
         if it is ever smaller the two came from different generations and \
         `preview`'s saturating subtraction would silently report that the \
         confidence floor excluded nothing. total={} listed={}",
        page.total_unfiltered,
        page.callers.len()
    );
}

/// An empty target list is a real answer, not an absent generation.
///
/// `None` means the store holds no generation at all. Collapsing the two would
/// make "nothing to ask about" and "nothing to ask" indistinguishable.
#[test]
fn an_empty_name_list_still_answers_from_a_generation() {
    let store = Store::open_in_memory().unwrap();
    commit(&store, &[("lib.py", LIB)], &[]);

    let page = store
        .callers_page(&[], "app.py", 0.0)
        .unwrap()
        .expect("a generation exists, so this is an answer and not `None`");
    assert_eq!(page.total_unfiltered, 0);
    assert!(page.callers.is_empty());

    let empty = Store::open_in_memory().unwrap();
    assert!(
        empty.callers_page(&[], "app.py", 0.0).unwrap().is_none(),
        "a store with no generation has nothing to answer from, which is a \
         different statement from answering zero"
    );
    assert!(empty.dead_page(usize::MAX).unwrap().is_none());
}

/// A scratch directory named for the test using it. No `tempfile`
/// dev-dependency exists in this workspace and this does not add one.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "devmap-paired-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The defect itself, reproduced deterministically against the API that still
/// has it.
///
/// This is the evidence the paired readers exist for, and it is the only shape
/// that can show it: the straddle needs a commit to land *between* two reads,
/// which cannot happen inside a single `dead_page` call. So the two unpaired
/// readers are driven directly, with a second handle committing between them —
/// no race, a forced schedule.
///
/// It passes before and after the fix, because it tests the old composition,
/// which still exists and is still unsafe to use as a pair. The structural
/// tests above are what hold `dead_page` to reading one generation; this is
/// what says why it had to.
#[test]
fn reading_the_analysis_and_the_rows_separately_can_straddle_two_generations() {
    let dir = scratch("straddle");
    let db = dir.join("index.sqlite");
    let writer = Store::open(&db).unwrap();
    let reader = Store::open(&db).unwrap();

    // Generation 1: degraded — `app.py` would not parse.
    commit(&writer, &[("lib.py", LIB), ("app.py", APP)], &["app.py"]);

    // The first half of the old pair.
    let analysis = reader
        .latest_analysis()
        .unwrap()
        .expect("generation 1 stored an analysis");
    assert!(
        matches!(
            analysis.status,
            devmap_analyze::model::AnalysisStatus::Partial { .. }
        ),
        "fixture precondition: generation 1 is degraded"
    );

    // A commit lands between the two reads. This is what the daemon does.
    commit(&writer, &[("lib.py", LIB), ("app.py", APP)], &[]);

    // The second half now reads a different generation.
    let paired = reader
        .dead_page(usize::MAX)
        .unwrap()
        .expect("a generation exists");
    assert_eq!(paired.generation, 2);
    assert!(
        matches!(
            paired.analysis.as_ref().unwrap().status,
            devmap_analyze::model::AnalysisStatus::Ok
        ),
        "generation 2 is clean"
    );

    // The two halves of the old composition disagree: a caller holding
    // `analysis` from before the commit would attach "this corpus was only
    // partially covered" to generation 2's rows — or, in the direction that
    // costs code, attach generation 2's clean bill of health to generation 1's
    // findings. `dead_page` cannot produce either, because it reads both halves
    // inside one pinned snapshot.
    assert!(
        !matches!(analysis.status, devmap_analyze::model::AnalysisStatus::Ok),
        "the separately-read analysis still describes generation 1 while the rows \
         describe generation 2 — that divergence is the defect, and it is exactly \
         what the paired reader removes"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
