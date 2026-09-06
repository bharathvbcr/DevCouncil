//! What a generation could not read, and what its edges were resolved by, are
//! rows rather than numbers.
//!
//! Two facts a generation used to carry only as a count, or not at all:
//!
//! - `discovery_refused_files` was a number in `analysis_json`, so the only way
//!   to maintain it across an incremental write was to carry it forward — and a
//!   number can only be replaced, never edited. It is now `COUNT(*)` over an
//!   inventory the writer supplies, and `save_generation` refuses a summary
//!   that disagrees with the rows beside it.
//! - the evidence tier behind an edge was not persisted at all, so a re-read
//!   edge carried none and the honesty invariant on the read path rested on the
//!   write-side constructor plus a round trip. It is now a column, and a row
//!   without one reads back as `ResolutionSource::Reconstructed` — a guess that
//!   says it is one.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, Extraction};
use devmap_resolve::model::{Resolution, ResolutionResult, ResolvedEdge};
use devmap_store::{
    DiscoveryRefusal, GenerationWriteOpts, ResolutionSource, Store, StoredResolutionKind,
};

fn tmp_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "devmap-coverage-{label}-{}-{stamp}-{seq}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn python(path: &str, source: &str) -> Extraction {
    extract_file(path, source)
}

fn refusal(path: &str) -> DiscoveryRefusal {
    DiscoveryRefusal {
        path: path.to_string(),
        reason: "1048577 bytes exceeds the 1048576 byte source ceiling, so extraction \
                 can never succeed"
            .to_string(),
    }
}

/// The count a consumer reads is the size of the list it can ask for.
#[test]
fn the_refusal_count_is_derived_from_the_inventory_and_never_carried() {
    let store = Store::open_in_memory().unwrap();
    let extractions = vec![python("lib.py", "def helper():\n    return 1\n")];
    let mut resolver = devmap_resolve::Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);

    let with_one = devmap_analyze::analyze_with_discovery(
        &extractions,
        &resolution,
        devmap_analyze::DiscoveryCoverage::refused(1),
    );
    store
        .save_generation_with_opts(
            &extractions,
            &resolution,
            &with_one,
            GenerationWriteOpts {
                discovery_refusals: Some(vec![refusal("app.py")]),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        store
            .latest_discovery_refusals()
            .unwrap()
            .into_iter()
            .map(|refusal| refusal.path)
            .collect::<Vec<_>>(),
        vec!["app.py".to_string()],
        "the inventory names the path, which is what makes a refusal checkable"
    );
    assert_eq!(
        store
            .latest_analysis()
            .unwrap()
            .unwrap()
            .discovery_refused_files,
        Some(1)
    );

    // The corpus is repaired: a walk that refused nothing writes an empty
    // inventory, and the count follows it down.
    let with_none = devmap_analyze::analyze_with_discovery(
        &extractions,
        &resolution,
        devmap_analyze::DiscoveryCoverage::refused(0),
    );
    store
        .save_generation_with_opts(
            &extractions,
            &resolution,
            &with_none,
            GenerationWriteOpts {
                discovery_refusals: Some(Vec::new()),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(
        store.latest_discovery_refusals().unwrap().is_empty(),
        "nothing was refused by this walk"
    );
    assert_eq!(
        store
            .latest_analysis()
            .unwrap()
            .unwrap()
            .discovery_refused_files,
        Some(0),
        "`refused(0)` is a measurement and must survive as one"
    );

    // And the unmeasured case stays unmeasured rather than becoming a zero: a
    // caller that supplied its own corpus never walked a tree.
    let unmeasured = devmap_analyze::analyze(&extractions, &resolution);
    store
        .save_generation(&extractions, &resolution, &unmeasured)
        .unwrap();
    assert_eq!(
        store
            .latest_analysis()
            .unwrap()
            .unwrap()
            .discovery_refused_files,
        None,
        "nobody walked, so `Some(0)` would claim the corpus was seen in full"
    );
}

/// The guard that makes the derivation structural rather than a convention.
#[test]
fn a_summary_claiming_a_refusal_the_inventory_cannot_name_is_refused() {
    let store = Store::open_in_memory().unwrap();
    let extractions = vec![python("lib.py", "def helper():\n    return 1\n")];
    let mut resolver = devmap_resolve::Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let analysis = devmap_analyze::analyze_with_discovery(
        &extractions,
        &resolution,
        devmap_analyze::DiscoveryCoverage::refused(1),
    );

    let error = store
        .save_generation(&extractions, &resolution, &analysis)
        .expect_err(
            "a summary that caps the dead-code confidence for a refusal nobody can name \
             is the over-claim the inventory exists to end",
        );
    let rendered = error.to_string();
    assert!(
        rendered.contains("discovery refusal"),
        "the refusal must say which invariant broke: {rendered}"
    );
}

/// `status` names the files, and says how many it did not name.
#[test]
fn status_names_the_paths_behind_every_coverage_number() {
    let dir = tmp_dir("status-names");
    let db = dir.join("index.sqlite");
    let store = Store::open(&db).unwrap();

    // A `.proto` has no linked grammar in this build, so its declarations are
    // recovered by line pattern: names and spans, no calls. That is a real hole
    // in call coverage and `extraction_gaps` records it as one.
    let extractions = vec![
        python("lib.py", "def helper():\n    return 1\n"),
        python(
            "svc.proto",
            "syntax = \"proto3\";\nmessage Ping { string id = 1; }\n",
        ),
        // The same language with nothing the pattern scanner can recover: a
        // grammar was wanted, none is linked, and no declaration came back —
        // which `Extraction::is_parse_failure` calls a parse failure, unlike a
        // `.md` where no grammar was ever expected.
        python("empty.proto", "// nothing but a comment\n"),
    ];
    let mut resolver = devmap_resolve::Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let analysis = devmap_analyze::analyze_with_discovery(
        &extractions,
        &resolution,
        devmap_analyze::DiscoveryCoverage::refused(1),
    );
    store
        .save_generation_with_opts(
            &extractions,
            &resolution,
            &analysis,
            GenerationWriteOpts {
                discovery_refusals: Some(vec![refusal("vendor/parser.c")]),
                ..Default::default()
            },
        )
        .unwrap();

    let status = store.status(&db.display().to_string()).unwrap();
    let refused = &status.coverage_gaps.discovery_refused;
    assert_eq!(refused.total, 1);
    assert_eq!(refused.shown.len(), 1);
    assert!(!refused.truncated());
    assert_eq!(refused.shown[0].path, "vendor/parser.c");
    assert!(
        refused.shown[0].reason.contains("source ceiling"),
        "the verdict is what lets an operator tell a correct refusal from a \
         broken one: {:?}",
        refused.shown[0]
    );

    let failed = &status.coverage_gaps.parse_failed;
    assert_eq!(
        failed
            .shown
            .iter()
            .map(|row| row.path.as_str())
            .collect::<Vec<_>>(),
        vec!["empty.proto"],
        "the third number in `degraded_reason` needs a path too: {:?}",
        status.coverage_gaps
    );
    assert_eq!(failed.total, 1);

    let recovered = &status.coverage_gaps.pattern_recovered;
    assert_eq!(
        recovered
            .shown
            .iter()
            .map(|row| row.path.as_str())
            .collect::<Vec<_>>(),
        vec!["svc.proto"],
        "the count in `degraded_reason` has always been there; the path is what \
         was missing"
    );
    assert_eq!(recovered.total, 1);

    // The off direction: a file that parsed is in none of the lists.
    assert!(
        status
            .coverage_gaps
            .parse_failed
            .shown
            .iter()
            .chain(recovered.shown.iter())
            .all(|row| row.path != "lib.py"),
        "a file the grammar read is not a coverage gap: {:?}",
        status.coverage_gaps
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Item 2: the stored kind wins over what a reconstruction would guess.
///
/// The edge is `Structural` — the relation a Go package star edge asserts — and
/// its two endpoints share a file, so the only reconstruction a row supports
/// says `SameFile`. Before the column existed there was no third possibility:
/// the read path returned no resolution at all, and every honesty claim about a
/// re-read edge rested on the write-side constructor.
#[test]
fn a_stored_resolution_kind_beats_the_reconstruction_a_row_would_support() {
    let dir = tmp_dir("edge-resolution");
    let db = dir.join("index.sqlite");
    let store = Store::open(&db).unwrap();

    let extractions = vec![python(
        "geo/measure.py",
        "def area():\n    return 1\n\n\ndef perimeter():\n    return area()\n",
    )];
    let structural = Arc::new(Resolution::Structural {
        target_symbol: "area".to_string(),
        target_file: "geo/measure.py".to_string(),
    });
    let resolution = ResolutionResult {
        edges: vec![ResolvedEdge::resolved(
            "geo/measure.py".to_string(),
            "geo/measure.py".to_string(),
            "perimeter".to_string(),
            "area".to_string(),
            EdgeKind::MemberOf,
            Arc::clone(&structural),
            None,
        )],
        receiver_types: Default::default(),
        reexport_chains: Default::default(),
        unresolved: Vec::new(),
    };
    let analysis = devmap_analyze::analyze(&extractions, &resolution);
    store
        .save_generation(&extractions, &resolution, &analysis)
        .unwrap();

    let index = store
        .generation_edges()
        .unwrap()
        .expect("a generation was persisted");
    assert_eq!(index.len(), 1);
    let read = index.resolution(0);
    assert_eq!(
        read.source,
        ResolutionSource::Stored,
        "the generation was written by this kernel, so its evidence is on the row"
    );
    assert_eq!(
        read.kind,
        StoredResolutionKind::Structural,
        "the row's own file layout says `SameFile`, and the resolver said \
         `Structural`. The stored answer is the one that was measured"
    );
    assert_eq!(
        store.latest_edge_resolution_source().unwrap(),
        Some(ResolutionSource::Stored)
    );
    drop(index);

    // Now the pre-column case, made by clearing the very column that was added.
    // A reconstruction is allowed to be wrong; what it may not do is look like
    // a reading.
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute("UPDATE generation_edges SET resolution = NULL", [])
            .unwrap();
    }
    let reread = Store::open(&db).unwrap();
    let index = reread.generation_edges().unwrap().unwrap();
    let guessed = index.resolution(0);
    assert_eq!(
        guessed.source,
        ResolutionSource::Reconstructed,
        "a generation that stored no evidence must not answer as though it had"
    );
    assert_eq!(
        guessed.kind,
        StoredResolutionKind::SameFile,
        "which is exactly the wrong answer, and why the label matters"
    );
    assert_eq!(
        reread.latest_edge_resolution_source().unwrap(),
        Some(ResolutionSource::Reconstructed)
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The read-side half of the honesty invariant. `ResolvedEdge::resolved` makes
/// `confidence` a function of the resolution on the way in; nothing re-checked
/// the two on the way out, so a row whose confidence had been changed under a
/// stored kind read back as a measurement. Now the index counts such rows when
/// it is built, and the SQL count answers the same question for a process that
/// holds no index. A reconstructed kind is not judged: a guess about the row
/// cannot convict the row.
#[test]
fn a_stored_confidence_that_contradicts_its_stored_kind_is_counted_not_trusted() {
    let dir = tmp_dir("confidence-mismatch");
    let db = dir.join("devmap.sqlite");
    let store = Store::open(&db).unwrap();
    let extractions = vec![
        python("a.py", "def helper():\n    return 1\n"),
        python(
            "b.py",
            "from a import helper\n\n\ndef caller():\n    return helper()\n",
        ),
    ];
    let resolution = ResolutionResult {
        edges: vec![
            ResolvedEdge::resolved(
                "b.py".to_string(),
                "a.py".to_string(),
                "caller".to_string(),
                "helper".to_string(),
                EdgeKind::Calls,
                Arc::new(Resolution::ImportScoped {
                    target_symbol: "helper".to_string(),
                    target_file: "a.py".to_string(),
                    imported_from: "a".to_string(),
                }),
                None,
            ),
            ResolvedEdge::resolved(
                "a.py".to_string(),
                "a.py".to_string(),
                "helper".to_string(),
                "helper".to_string(),
                EdgeKind::Contains,
                Arc::new(Resolution::SameFile {
                    target_symbol: "helper".to_string(),
                    target_file: "a.py".to_string(),
                }),
                None,
            ),
        ],
        receiver_types: Default::default(),
        reexport_chains: Default::default(),
        unresolved: Vec::new(),
    };
    let analysis = devmap_analyze::analyze(&extractions, &resolution);
    store
        .save_generation(&extractions, &resolution, &analysis)
        .unwrap();
    let index = store.generation_edges().unwrap().expect("a generation");
    assert_eq!(index.len(), 2);
    assert_eq!(
        index.confidence_mismatches(),
        0,
        "a generation a correct writer produced has nothing to report"
    );
    assert_eq!(store.edge_confidence_mismatches().unwrap(), Some(0));
    drop(index);
    drop(store);

    // One row's confidence is moved under it while its stored kind stays
    // `ImportScoped`, which entitles 1.0.
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        let changed = conn
            .execute(
                "UPDATE generation_edges SET confidence = 0.2 WHERE resolution = 'ImportScoped'",
                [],
            )
            .unwrap();
        assert_eq!(
            changed, 1,
            "the fixture must have exactly one ImportScoped edge"
        );
    }
    let reread = Store::open(&db).unwrap();
    let index = reread.generation_edges().unwrap().expect("a generation");
    assert_eq!(
        index.confidence_mismatches(),
        1,
        "the row now claims 0.2 under evidence that entitles 1.0, and the index must say so"
    );
    assert_eq!(
        reread.edge_confidence_mismatches().unwrap(),
        Some(1),
        "the SQL count must agree with the index"
    );
    // The row itself is left as evidence: the count reports, it does not repair.
    let tampered: Vec<&devmap_store::StoredEdge> = index
        .edges()
        .iter()
        .filter(|edge| edge.source_symbol == "caller")
        .collect();
    assert_eq!(tampered.len(), 1);
    assert!((tampered[0].confidence - 0.2).abs() < 1e-6);
    assert_eq!(tampered[0].resolution.as_deref(), Some("ImportScoped"));
    drop(index);
    drop(reread);

    // Clearing the column turns the kind into a reconstruction, and a
    // reconstruction cannot convict the row: the count drops to zero and the
    // generation's source says why.
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute("UPDATE generation_edges SET resolution = NULL", [])
            .unwrap();
    }
    let reread = Store::open(&db).unwrap();
    let index = reread.generation_edges().unwrap().expect("a generation");
    assert_eq!(
        index.confidence_mismatches(),
        0,
        "a guess must not convict the row"
    );
    assert_eq!(reread.edge_confidence_mismatches().unwrap(), Some(0));
    assert_eq!(
        reread.latest_edge_resolution_source().unwrap(),
        Some(ResolutionSource::Reconstructed)
    );
    let _ = std::fs::remove_dir_all(&dir);
}
