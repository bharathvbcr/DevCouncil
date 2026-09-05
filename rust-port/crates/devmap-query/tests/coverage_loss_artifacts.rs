//! Extraction coverage loss has to reach the artifacts, and only when it happened.
//!
//! Three defects, one shape — a check that could not run reporting as one that
//! ran and passed:
//!
//! * **Q-1** `code_graph.json` rendered a lost-caller finding at
//!   `confidence: "extracted"` with `analysis_status: "ok"`, and
//!   `repo_map.json` said `graph_degraded: false`. Nothing counted
//!   `ParseOutcome::Failed` anywhere.
//! * **Q-2** `unwired_candidates` exempted tests, vendored, generated,
//!   re-export packages, launchers and entry roots — but not a file whose
//!   imports were never extracted, which cannot answer the question the list
//!   asks. `liveness_meta.unwired` then reported `{shown, total, truncated}`
//!   over that narrowed population with no note that it had been narrowed.
//! * **Q-5** `dead_symbol_candidates` took the first 200 in extraction order,
//!   so 250 findings at 0.4 declared before 5 at 0.9 crowded every confident
//!   one off the end. R7: rank before truncating.
//!
//! Each is asserted in both directions. Against code that reported degraded
//! unconditionally, or capped every tier unconditionally, the "clean corpus"
//! halves below fail.

use devmap_analyze::model::{AnalysisSummary, DeadSymbolReport};
use devmap_extract::extract_file;
use devmap_extract::model::{Extraction, ExtractionEngine, ParseOutcome, SymbolKind};
use devmap_query::code_graph::generate_code_graph_json;
use devmap_query::manifest::generate_manifest_with_edges;
use devmap_query::model::FreshnessInfo;
use devmap_resolve::Resolver;
use serde_json::Value;

fn freshness() -> FreshnessInfo {
    FreshnessInfo {
        head_sha: "abc123".to_string(),
        generation_id: 7,
        pending_count: 0,
        stamped: Default::default(),
    }
}

/// `lib.py::helper` is called from `app.py` and from nowhere else.
fn fixture() -> Vec<Extraction> {
    vec![
        extract_file("lib.py", "def helper():\n    return 1\n"),
        extract_file(
            "app.py",
            "from lib import helper\n\n\ndef main():\n    return helper()\n",
        ),
    ]
}

/// Shaped as `refused_extraction` builds one: the File node only, no imports,
/// no calls, engine `Unavailable`.
fn refuse(ext: &mut Extraction) {
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

/// Both artifacts of one build, rendered from one analysis.
fn artifacts(extractions: &[Extraction]) -> (Value, Value) {
    let mut resolver = Resolver::new();
    resolver.index_extractions(extractions);
    let resolution = resolver.resolve_all(extractions);
    let analysis = devmap_analyze::analyze(extractions, &resolution);

    let graph = generate_code_graph_json(
        extractions,
        &analysis,
        &resolution.edges,
        &freshness(),
        None,
    )
    .expect("graph renders");
    let (_, map) =
        generate_manifest_with_edges(extractions, &analysis, freshness(), &resolution.edges);
    (
        serde_json::from_str(&graph).unwrap(),
        serde_json::from_str(&map).unwrap(),
    )
}

fn strings(value: &Value) -> Vec<&str> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item.as_str().unwrap())
        .collect()
}

/// Q-1, the OFF direction: a corpus that was read completely says so, and
/// still reaches the top tier.
#[test]
fn a_fully_read_corpus_reports_ok_and_reaches_the_extracted_tier() {
    let mut extractions = fixture();
    extractions.push(extract_file(
        "orphan.py",
        "def never_called():\n    return 3\n",
    ));
    let (graph, map) = artifacts(&extractions);

    assert_eq!(graph["meta"]["devmap_rust"]["analysis_status"], "ok");
    assert_eq!(graph["meta"]["devmap_rust"]["parse_failed_files"], 0);
    assert_eq!(graph["meta"]["devmap_rust"]["regex_fallback_files"], 0);
    assert_eq!(map["graph_degraded"], false);
    assert_eq!(map["graph_degraded_reason"], "");

    let dead = graph["dead_code"].as_array().unwrap();
    let orphan = dead
        .iter()
        .find(|row| row["id"] == "orphan.py::never_called")
        .expect("a symbol nothing calls is still a finding");
    assert_eq!(
        orphan["confidence"], "extracted",
        "a complete scan must still reach the tier CLAUDE.md tells agents to \
         act on, or the fix has disabled dead-code detection: {orphan}"
    );
    assert_eq!(orphan["reason"], "no inbound call edges and not exported");
    assert!(
        !dead.iter().any(|row| row["id"] == "lib.py::helper"),
        "`helper` is called from app.py: {dead:?}"
    );
    // Every finding in a fully read corpus is confident: `orphan.py::never_called`
    // and `app.py::main`, which nothing calls either.
    assert_eq!(
        map["liveness_meta"]["dead_symbol"]["by_confidence"]["total"]["extracted"],
        dead.len(),
    );
    assert_eq!(
        map["liveness_meta"]["dead_symbol"]["by_confidence"]["total"]["ambiguous"],
        0
    );
}

/// Q-1, the ON direction: the audit's end-to-end wrong answer.
///
/// Before the fix, byte for byte:
/// `dead_code = [{"confidence":"extracted", "reason":"no inbound call edges
/// and not exported", "id":"lib.py::helper"}]`, `analysis_status = "ok"`,
/// `graph_degraded = false`.
#[test]
fn a_lost_caller_never_renders_at_the_top_confidence_tier() {
    let mut extractions = fixture();
    refuse(&mut extractions[1]);
    let (graph, map) = artifacts(&extractions);

    assert_eq!(
        graph["meta"]["devmap_rust"]["parse_failed_files"], 1,
        "the counter the audit found missing entirely"
    );
    let status = graph["meta"]["devmap_rust"]["analysis_status"]
        .as_str()
        .unwrap();
    assert!(
        status.starts_with("partial:"),
        "corpus-level extraction loss must reach `analysis_status`: {status}"
    );
    assert_eq!(map["graph_degraded"], true);
    assert!(map["graph_degraded_reason"]
        .as_str()
        .unwrap()
        .contains("did not cover the whole corpus"));

    let dead = graph["dead_code"].as_array().unwrap();
    let helper = dead
        .iter()
        .find(|row| row["id"] == "lib.py::helper")
        .expect("still listed — hiding it would be its own lie");
    assert_eq!(
        helper["confidence"], "ambiguous",
        "the reachability check could not read `helper`'s only caller: {helper}"
    );
    assert_ne!(helper["reason"], "no inbound call edges and not exported");

    // And the whole population is capped, not just the one symbol — nothing
    // can tell which findings the missing edges would have contradicted.
    assert!(dead.iter().all(|row| row["confidence"] == "ambiguous"));
    assert_eq!(
        map["liveness_meta"]["dead_symbol"]["by_confidence"]["total"]["extracted"],
        0
    );
}

/// Q-2, both directions: a file whose imports were never read cannot answer
/// "does anything import me", and the count of what was dropped travels with
/// the list.
#[test]
fn unwired_excludes_files_whose_imports_were_never_read_and_says_how_many() {
    // OFF: nothing excluded, and an unimported module is still reported.
    let clean = fixture();
    let (graph, map) = artifacts(&clean);
    assert_eq!(
        strings(&graph["unwired_candidates"]),
        ["app.py"],
        "app.py imports lib.py, so only app.py is unimported"
    );
    assert_eq!(map["liveness_meta"]["unwired"]["excluded_coverage_loss"], 0);
    assert_eq!(
        graph["meta"]["devmap_rust"]["unwired_excluded_coverage_loss"],
        0
    );
    assert_eq!(map["liveness_meta"]["unwired"]["total"], 1);

    // ON: the refused file drops out of the population and is counted.
    let mut lost = fixture();
    refuse(&mut lost[1]);
    let (graph, map) = artifacts(&lost);
    let unwired = strings(&graph["unwired_candidates"]);
    assert!(
        !unwired.contains(&"app.py"),
        "app.py's own imports were never extracted, so its unwired status is \
         not a finding: {unwired:?}"
    );
    assert_eq!(
        unwired,
        ["lib.py"],
        "lib.py now looks unimported because app.py's import was never read — \
         that is what `graph_degraded` and the excluded count are for: \
         {unwired:?}"
    );
    assert_eq!(
        map["liveness_meta"]["unwired"]["excluded_coverage_loss"], 1,
        "a narrowed population under a bare total is how 'we did not look' \
         comes to read as 'we looked and found nothing'"
    );
    assert_eq!(
        graph["meta"]["devmap_rust"]["unwired_excluded_coverage_loss"],
        1
    );
    assert_eq!(strings(&map["unwired_candidates"]), ["lib.py"]);
}

/// Q-5: rank, then truncate.
///
/// 255 non-exempt findings in `analyze()`'s extraction order — 250 at 0.4
/// first, then 5 at 0.9. Before the fix the emitted 200 were all 0.4 and the
/// list contained no confident finding at all, under counts
/// (`{shown: 200, total: 255, truncated: true}`) that were arithmetically
/// correct about a selection that was not.
#[test]
fn dead_symbol_candidates_rank_by_confidence_before_truncating() {
    let mut dead_symbols: Vec<DeadSymbolReport> = (0..250)
        .map(|i| DeadSymbolReport {
            symbol_name: format!("low{i:04}"),
            file_path: "a_low.py".to_string(),
            confidence: 0.4,
            is_exempt: false,
            exemption_reason: Some("only_ambiguous_callers".to_string()),
        })
        .collect();
    dead_symbols.extend((0..5).map(|i| DeadSymbolReport {
        // `z_` so file order alone can never float these to the front: only a
        // confidence-keyed sort puts them there.
        symbol_name: format!("high{i}"),
        file_path: "z_high.py".to_string(),
        confidence: 0.9,
        is_exempt: false,
        exemption_reason: None,
    }));
    // An exempt finding must stay out of the list and out of both histograms.
    dead_symbols.push(DeadSymbolReport {
        symbol_name: "exempted".to_string(),
        file_path: "a_low.py".to_string(),
        confidence: 0.3,
        is_exempt: true,
        exemption_reason: Some("Exported or exempt".to_string()),
    });

    let analysis = AnalysisSummary {
        // No discovery step ran over this hand-built corpus, so there is no
        // refusal count to report. `None` says that; `0` would claim a walk.
        discovery_refused_files: None,
        total_files: 2,
        total_symbols: 255,
        total_edges: 0,
        dead_symbols,
        communities: Vec::new(),
        status: devmap_analyze::model::AnalysisStatus::Ok,
        unresolved_calls: 0,
        clone_coverage: Default::default(),
    };
    let extractions = vec![
        extract_file("a_low.py", "def x(): pass\n"),
        extract_file("z_high.py", "def y(): pass\n"),
    ];
    let (_, map) = generate_manifest_with_edges(&extractions, &analysis, freshness(), &[]);
    let map: Value = serde_json::from_str(&map).unwrap();

    let candidates = strings(&map["dead_symbol_candidates"]);
    assert_eq!(candidates.len(), 200, "the cap still holds");
    for index in 0..5 {
        assert_eq!(
            candidates[index],
            format!("z_high.py::high{index}"),
            "every confident finding must survive the cut, ranked first: \
             {:?}",
            &candidates[..8]
        );
    }

    let meta = &map["liveness_meta"]["dead_symbol"];
    assert_eq!(meta["shown"], 200);
    assert_eq!(meta["total"], 255, "the exempt finding is not a candidate");
    assert_eq!(meta["truncated"], true);
    assert_eq!(meta["by_confidence"]["total"]["extracted"], 5);
    assert_eq!(meta["by_confidence"]["total"]["inferred"], 250);
    assert_eq!(meta["by_confidence"]["total"]["ambiguous"], 0);
    assert_eq!(
        meta["by_confidence"]["shown"]["extracted"], 5,
        "equal shown/total `extracted` counts are what prove the truncation \
         ranked before it cut"
    );
    assert_eq!(meta["by_confidence"]["shown"]["inferred"], 195);

    // Determinism: the same analysis renders the same list twice.
    let (_, again) = generate_manifest_with_edges(&extractions, &analysis, freshness(), &[]);
    let again: Value = serde_json::from_str(&again).unwrap();
    assert_eq!(
        map["dead_symbol_candidates"],
        again["dead_symbol_candidates"]
    );
}
