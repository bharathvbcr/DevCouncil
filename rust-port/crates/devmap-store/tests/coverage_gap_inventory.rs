//! What a generation could not read is an inventory of rows, not a number.
//!
//! `discovery_refused_files` was a number in `analysis_json`, so the only way to
//! maintain it across an incremental write was to carry it forward — and a
//! number can only be replaced, never edited. It is now `COUNT(*)` over an
//! inventory the writer supplies, and `save_generation` refuses a summary that
//! disagrees with the rows beside it.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
use devmap_store::{DiscoveryRefusal, GenerationWriteOpts, Store};

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
