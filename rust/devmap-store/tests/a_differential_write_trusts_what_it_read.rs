//! A differential write trusts the rows it read, not the caller's word about them.
//!
//! `save_generation_timed`'s carry-forward is an optimisation carrying a
//! correctness obligation: every row it declines to rewrite, it must instead be
//! able to reuse *exactly*. Four of its decisions are guards against a caller
//! whose `affected_paths` / `deleted_paths` / extraction slice do not agree with
//! each other, and each one was reachable by mutation with the whole suite --
//! `devmap-store`, `devmap-cli` and `devmap-serve`, 101 test binaries -- still
//! green. A guard nothing exercises is indistinguishable from a guard that is
//! wrong, so each test below names the mutation it refuses.
//!
//! The guards are not hypothetical. The two writers disagree about what a
//! differential write even contains: `devmap build` hands over an extraction for
//! every scanned file (`extract_scanned_for_generation` maps the whole tree,
//! reusing cached payloads), while the daemon hands over only the files it
//! re-read (`daemon.rs`: `if full_rebuild { &extractions } else { &fresh }`).
//! Every carry-forward below therefore has to be right for both shapes, and the
//! partial shape is the one no test reached.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use devmap_analyze::analyze;
use devmap_extract::model::EdgeKind;
use devmap_extract::{extract_file, Extraction};
use devmap_resolve::model::{Resolution, ResolutionResult, ResolvedEdge};
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};

/// A unique directory per test.
///
/// The pid and a sequence join the timestamp because two tests starting inside
/// one nanosecond tick otherwise collide on macOS, where the clock's resolution
/// is coarser than its units.
fn tmp_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "devmap-differential-{label}-{}-{stamp}-{seq}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Extract, resolve and analyse a corpus, the way a cold build does.
fn pipeline(
    files: &[(&str, &str)],
) -> (
    Vec<Extraction>,
    ResolutionResult,
    devmap_analyze::AnalysisSummary,
) {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, body)| extract_file(path, body))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    let analysis = analyze(&extractions, &resolution);
    (extractions, resolution, analysis)
}

/// A declarative language with no linked grammar, so extraction reports
/// `Failed` and the inventory records a `parse_failed` gap.
///
/// Broken *content* is not enough: tree-sitter recovers from a syntax error into
/// `Partial`, which is not a gap at all. The gap has to come from the language,
/// and `the_file_node_alone_is_never_reported_as_a_recovery` in
/// `devmap-extract` already pins this exact pairing.
const UNREADABLE_PATH: &str = "scripts/manifest.psd1";
const UNREADABLE: &str = "@{ ModuleVersion = '1.0' }\n";

/// Every gap the latest generation records, as `(label, path)`.
///
/// Labelled rather than a flat path list because one of the tests below turns a
/// `pattern_recovered` gap into a `parse_failed` one for the same path: a
/// reader that only answered "is this path a gap" would read both as a gap and
/// could not see the stale row at all.
fn gaps(store: &Store, db: &std::path::Path) -> Vec<(&'static str, String)> {
    let status = store.status(db.to_str().unwrap()).unwrap();
    let found = &status.coverage_gaps;
    [
        ("parse_failed", &found.parse_failed),
        ("pattern_recovered", &found.pattern_recovered),
        ("call_blind", &found.call_blind),
        ("import_blind", &found.import_blind),
        ("not_parsed", &found.not_parsed),
        ("discovery_refused", &found.discovery_refused),
    ]
    .into_iter()
    .flat_map(|(label, sample)| {
        sample
            .shown
            .iter()
            .map(move |row| (label, row.path.clone()))
    })
    .collect()
}

/// Whether `path` is recorded as a gap of any kind.
fn is_a_gap(store: &Store, db: &std::path::Path, path: &str) -> bool {
    gaps(store, db).iter().any(|(_, found)| found == path)
}

/// `db.rs:4355` -- `content_matches`, `==` mutated to `!=`.
///
/// The one mutation of the eleven that carries a *payload* rather than choosing
/// a code path. `content_matches` asks whether the hash this build computed for
/// a file equals the hash stored beside the payload it is about to reuse. A
/// content hash that moved while the path stayed out of `affected_paths` means
/// the caller's affected set is wrong, and the stored payload describes bytes
/// that are gone either way -- so the row is rebuilt, not carried.
///
/// Inverted, the test reads "carry it *because* the bytes moved": the generation
/// keeps the previous payload for a file whose content it was handed, and the
/// fresh extraction it was handed is skipped. `devmap build` cannot reach this
/// -- it derives `affected` from the same hashes -- which is exactly why the
/// guard is the store's and not the caller's.
#[test]
fn a_payload_is_not_carried_for_a_file_whose_bytes_moved() {
    let store = Store::open_in_memory().unwrap();

    let before = [
        ("lib.py", "def original():\n    return 1\n"),
        ("app.py", "def main():\n    return 2\n"),
    ];
    let (cold, resolution, analysis) = pipeline(&before);
    store
        .save_generation(&cold, &resolution, &analysis)
        .unwrap();

    let first = store.latest_symbol_names_by_file().unwrap();
    assert!(
        first
            .get("lib.py")
            .is_some_and(|names| names.contains("original")),
        "fixture precondition: the cold generation must hold lib.py::original, \
         or the carry-forward below has nothing stale to carry; got {first:#?}"
    );

    // `lib.py`'s bytes move and the caller declares only `app.py` affected.
    // The affected set is therefore wrong about lib.py, and the hashes the same
    // call hands over are what say so.
    let after = [
        ("lib.py", "def replacement():\n    return 1\n"),
        ("app.py", "def main():\n    return 3\n"),
    ];
    let (warm, resolution, analysis) = pipeline(&after);
    let moved = warm
        .iter()
        .find(|ext| ext.file_path == "lib.py")
        .expect("the write is handed lib.py's fresh extraction");
    assert_ne!(
        moved.content_hash,
        cold.iter()
            .find(|ext| ext.file_path == "lib.py")
            .unwrap()
            .content_hash,
        "fixture precondition: lib.py's content hash must move, or `content_matches` \
         is never the decision under test"
    );
    let fresh_hash = moved.content_hash;

    store
        .save_generation_with_opts(
            &warm,
            &resolution,
            &analysis,
            GenerationWriteOpts {
                affected_paths: vec!["app.py".into()],
                ..Default::default()
            },
        )
        .unwrap();

    let symbols = store.latest_symbol_names_by_file().unwrap();
    let lib = symbols
        .get("lib.py")
        .expect("lib.py is still in the tree and so must be in the generation");
    assert!(
        lib.contains("replacement"),
        "the generation lost lib.py's fresh symbol: the write carried the previous \
         payload forward for a file whose content hash had moved, and skipped the \
         extraction it was handed.\n{symbols:#?}"
    );
    assert!(
        !lib.contains("original"),
        "the generation still serves lib.py::original, a symbol the current bytes \
         do not declare: a stale payload was carried on a moved content hash.\n{symbols:#?}"
    );

    let hashes = store.latest_file_hashes().unwrap();
    assert_eq!(
        hashes.get("lib.py"),
        Some(&fresh_hash),
        "the generation records a content hash for lib.py that is not the one this \
         build measured, so the payload beside it describes different bytes"
    );
}

/// `db.rs:5018` -- the `!full_rewrite` guard on the coverage-gap carry-forward.
///
/// A differential write is not obliged to be handed the whole tree, and the
/// daemon is not: its incremental path passes `&fresh`, only the files it
/// re-read. So the gaps of every file the batch did not touch exist *only* in
/// the previous generation, and deriving the inventory from `extractions` alone
/// would drop them.
///
/// With the `!` deleted the carry-forward runs on full rewrites instead, which
/// is precisely when it is redundant -- and never on the differential writes
/// that need it. The whole suite stays green because every existing coverage-gap
/// test writes a full rewrite (`..Default::default()`, so an empty affected
/// set), and every existing differential test has a corpus that parses cleanly
/// and so has no gaps to lose.
///
/// A lost gap is not a cosmetic loss: coverage is a ratio, and a file silently
/// leaving the denominator reads as a repository better understood than it is.
#[test]
fn a_coverage_gap_in_an_untouched_file_survives_a_partial_extraction_write() {
    let dir = tmp_dir("gap-carry");
    let db = dir.join("index.sqlite");
    let store = Store::open(&db).unwrap();

    let cold_files = [
        (UNREADABLE_PATH, UNREADABLE),
        ("app.py", "def main():\n    return 1\n"),
    ];
    let (cold, resolution, analysis) = pipeline(&cold_files);
    assert!(
        cold.iter()
            .any(|ext| ext.file_path == UNREADABLE_PATH && ext.is_parse_failure()),
        "fixture precondition: {UNREADABLE_PATH} must fail to parse, or there is no \
         gap to carry; outcomes {:#?}",
        cold.iter()
            .map(|ext| (&ext.file_path, &ext.parse_outcome))
            .collect::<Vec<_>>()
    );
    store
        .save_generation(&cold, &resolution, &analysis)
        .unwrap();
    assert!(
        is_a_gap(&store, &db, UNREADABLE_PATH),
        "fixture precondition: the cold generation must record {UNREADABLE_PATH} as \
         a gap; got {:?}",
        gaps(&store, &db)
    );

    // The daemon's shape: only `app.py` was re-read, so only `app.py`'s
    // extraction is handed over. The unreadable file is neither affected nor
    // deleted and this write holds nothing at all about it -- the previous
    // generation is the only place its gap exists.
    let warm_files = [("app.py", "def main():\n    return 2\n")];
    let (warm, resolution, analysis) = pipeline(&warm_files);
    assert!(
        devmap_analyze::extraction_gaps(&warm).is_empty(),
        "fixture precondition: this write's own extractions must yield no gaps, or \
         the carried row would be indistinguishable from a re-derived one"
    );
    store
        .save_generation_with_opts(
            &warm,
            &resolution,
            &analysis,
            GenerationWriteOpts {
                affected_paths: vec!["app.py".into()],
                ..Default::default()
            },
        )
        .unwrap();

    assert!(
        is_a_gap(&store, &db, UNREADABLE_PATH),
        "the generation lost {UNREADABLE_PATH}'s parse-failure gap. The write was \
         handed no extraction for it, so the previous generation's inventory was the \
         only record of it, and the carry-forward did not run: coverage now claims a \
         file nothing ever read.\ngaps: {:?}",
        gaps(&store, &db)
    );
}

/// `db.rs:5035` -- the `deleted || affected` skip in the gap carry-forward,
/// `||` mutated to `&&`, in its `affected` direction.
///
/// Carrying a gap forward is right for a file this write knows nothing about and
/// wrong for one it just re-read: the fresh inventory below is the whole truth
/// about an affected file. Skipping `affected` is what lets a file that used to
/// fail to parse leave the list on the build that parses it.
///
/// Under `&&` a path must be *both* deleted and affected to be skipped, so an
/// affected file keeps the gap it no longer has, beside the gap it does -- an
/// inventory that only ever grows, describing a file in two contradictory ways
/// at once.
///
/// The edit here changes which *kind* of gap the file has rather than removing
/// it: `mod.proto` declaring a message is recovered by pattern
/// (`pattern_recovered`), and the same path holding no declarations is a
/// `parse_failed`. That is what makes the stale row visible -- a fresh gap of a
/// different label cannot mask it, where a re-derived identical gap would.
#[test]
fn a_gap_leaves_the_inventory_on_the_build_that_changed_the_file() {
    let dir = tmp_dir("gap-fixed");
    let db = dir.join("index.sqlite");
    let store = Store::open(&db).unwrap();

    let cold_files = [
        ("mod.proto", "message User {\n  string id = 1;\n}\n"),
        ("app.py", "def main():\n    return 1\n"),
    ];
    let (cold, resolution, analysis) = pipeline(&cold_files);
    assert!(
        devmap_analyze::extraction_gaps(&cold)
            .iter()
            .any(|entry| entry.path == "mod.proto" && entry.gap.label() == "pattern_recovered"),
        "fixture precondition: mod.proto must start as a `pattern_recovered` gap; \
         got {:?}",
        devmap_analyze::extraction_gaps(&cold)
            .iter()
            .map(|entry| (&entry.path, entry.gap.label()))
            .collect::<Vec<_>>()
    );
    store
        .save_generation(&cold, &resolution, &analysis)
        .unwrap();
    assert!(
        gaps(&store, &db).contains(&("pattern_recovered", "mod.proto".to_string())),
        "fixture precondition: the cold generation must record mod.proto as \
         `pattern_recovered`; got {:?}",
        gaps(&store, &db)
    );

    // The commit that empties it. The file is re-read and declared affected, so
    // this write's own extractions are the whole truth about it.
    let warm_files = [
        ("mod.proto", "// every message moved out of this file\n"),
        ("app.py", "def main():\n    return 1\n"),
    ];
    let (warm, resolution, analysis) = pipeline(&warm_files);
    assert!(
        devmap_analyze::extraction_gaps(&warm)
            .iter()
            .all(|entry| entry.path != "mod.proto" || entry.gap.label() != "pattern_recovered"),
        "fixture precondition: the edited mod.proto must no longer be a \
         `pattern_recovered` gap, or the carry-forward would not be the decision \
         under test; got {:?}",
        devmap_analyze::extraction_gaps(&warm)
            .iter()
            .map(|entry| (&entry.path, entry.gap.label()))
            .collect::<Vec<_>>()
    );
    store
        .save_generation_with_opts(
            &warm,
            &resolution,
            &analysis,
            GenerationWriteOpts {
                affected_paths: vec!["mod.proto".into()],
                ..Default::default()
            },
        )
        .unwrap();

    assert!(
        !gaps(&store, &db).contains(&("pattern_recovered", "mod.proto".to_string())),
        "mod.proto declares nothing now and was declared affected, but the \
         generation still records the `pattern_recovered` gap of its previous \
         contents: the carry-forward copied a stale gap over a file this write had \
         re-read.\ngaps: {:?}",
        gaps(&store, &db)
    );
}

/// `db.rs:5035` again -- the same `||` mutated to `&&`, in its `deleted`
/// direction.
///
/// Asserted separately from the `affected` case above because `&&` is refused by
/// each side alone, and a single test covering one side would leave the other
/// free to rot: a file can be deleted without being affected, and that is the
/// combination the watcher produces when a file is removed and nothing else
/// changes.
#[test]
fn a_deleted_file_takes_its_coverage_gap_with_it() {
    let dir = tmp_dir("gap-deleted");
    let db = dir.join("index.sqlite");
    let store = Store::open(&db).unwrap();

    let cold_files = [
        (UNREADABLE_PATH, UNREADABLE),
        ("app.py", "def main():\n    return 1\n"),
    ];
    let (cold, resolution, analysis) = pipeline(&cold_files);
    store
        .save_generation(&cold, &resolution, &analysis)
        .unwrap();
    assert!(
        is_a_gap(&store, &db, UNREADABLE_PATH),
        "fixture precondition: the cold generation must record {UNREADABLE_PATH} as \
         a gap; got {:?}",
        gaps(&store, &db)
    );

    // The unreadable file is removed from the tree. It is deleted and *not*
    // affected, and the write holds no extraction for it.
    let warm_files = [("app.py", "def main():\n    return 1\n")];
    let (warm, resolution, analysis) = pipeline(&warm_files);
    store
        .save_generation_with_opts(
            &warm,
            &resolution,
            &analysis,
            GenerationWriteOpts {
                deleted_paths: vec![UNREADABLE_PATH.into()],
                ..Default::default()
            },
        )
        .unwrap();

    assert!(
        !is_a_gap(&store, &db, UNREADABLE_PATH),
        "{UNREADABLE_PATH} is gone from the tree, and the generation still admits it \
         could not read it: the carry-forward copied the gap of a deleted file, and \
         coverage is now degraded by a path no consumer can open.\ngaps: {:?}",
        gaps(&store, &db)
    );
}

/// `db.rs:4730` -- the `deleted.contains(source) || deleted.contains(target)`
/// guard on the edge loop, `||` mutated to `&&`.
///
/// The guard exists for the caller the equality at `db.rs:4987` argues cannot
/// happen: one that passes a resolution computed *before* the deletion. Its own
/// comment says so. A resolution over the current tree has no edge touching a
/// deleted file, so both shipping writers stay clear of it -- which is why no
/// test reached it.
///
/// The two endings differ in the way that matters. With the guard, the skipped
/// edge leaves `edge_ord` one short of the analysis that counted it and the write
/// is refused, naming the disagreement. Under `&&` only an edge with *both*
/// endpoints deleted is skipped, so the edge is admitted, the counts agree, and
/// the generation quietly gains an edge whose source file this very call was told
/// is gone -- a phantom whose target resolves, which is how a dead symbol reads
/// as live.
#[test]
fn an_edge_from_a_deleted_file_is_refused_not_admitted() {
    let store = Store::open_in_memory().unwrap();

    let files = [
        ("gone.py", "def helper():\n    return 1\n"),
        ("app.py", "def main():\n    return 2\n"),
    ];
    let (extractions, _, _) = pipeline(&files);

    // One edge, out of `gone.py` and into a file that stays. Built by hand so
    // the endpoints are exactly the asymmetry under test: `&&` refuses only the
    // both-endpoints-deleted case, so an edge with one deleted endpoint is the
    // only shape that separates the two operators.
    let structural = Arc::new(Resolution::Structural {
        target_symbol: "main".to_string(),
        target_file: "app.py".to_string(),
    });
    let stale = ResolutionResult {
        edges: vec![ResolvedEdge::resolved(
            "gone.py".to_string(),
            "app.py".to_string(),
            "helper".to_string(),
            "main".to_string(),
            EdgeKind::Calls,
            Arc::clone(&structural),
            None,
        )],
        receiver_types: Default::default(),
        reexport_chains: Default::default(),
        unresolved: Vec::new(),
    };
    let analysis = analyze(&extractions, &stale);
    let cold = store
        .save_generation(&extractions, &stale, &analysis)
        .unwrap();
    assert_eq!(
        analysis.total_edges, 1,
        "fixture precondition: the analysis must count the edge, so that dropping \
         it is a disagreement this write can see"
    );

    // The same pre-deletion resolution, handed to a write that is told `gone.py`
    // has been removed.
    let survivors = [("app.py", "def main():\n    return 2\n")];
    let (warm, _, _) = pipeline(&survivors);
    let outcome = store.save_generation_with_opts(
        &warm,
        &stale,
        &analysis,
        GenerationWriteOpts {
            deleted_paths: vec!["gone.py".into()],
            ..Default::default()
        },
    );

    let error = match outcome {
        Err(error) => error.to_string(),
        Ok(generation) => {
            let edges = store.latest_edges_for_test().unwrap();
            panic!(
                "generation {generation} was committed from a resolution holding an \
                 edge out of gone.py, a file this write was told is deleted. The \
                 guard admitted it instead of dropping it, so the edge count matched \
                 the analysis and nothing refused the write.\nedges: {edges:#?}"
            );
        }
    };
    assert!(
        error.contains("edges") && error.contains("analysis"),
        "the write was refused, but not for the edge it dropped; a different \
         refusal would leave this test passing for the wrong reason: {error}"
    );
    assert_eq!(
        store.latest_generation_id().unwrap(),
        Some(cold),
        "the refused write must leave the previous generation as the latest"
    );
}
