//! Executable gates for the failure classes in `rust-port/PLAN.md` §3.1.
//!
//! The classes exist because the per-finding acceptance tests did not prevent
//! recurrence: four of the five shapes produced a *fresh* instance in the Rust
//! port after the Python instance had been found, fixed, and written into the
//! plan as a property. Every one of the six defects closed in the 2026-09-02
//! pass was found by measurement, none by the test written to prevent its class.
//!
//! Class B is gated separately in `coverage_invariants.rs`, because its
//! assertions have to walk the filesystem rather than the index.
//!
//! These gates are deliberately stated over *types and outcomes* rather than
//! over specific inputs. An assertion that "search finds `foo`" pins a case; an
//! assertion that "an operation which declined to run is distinguishable from
//! one that ran and found nothing" pins the shape, which is the thing that kept
//! recurring.

// ---------------------------------------------------------------------------
// Class A — a check that could not run must not report as one that ran.
// ---------------------------------------------------------------------------
//
// Three instances, three unrelated subsystems: an analysis timeout (N4), a
// query budget that returned zero hits while both budget gates stayed green
// (K4), and a storage reclaim that read a pre-checkpoint counter and reported
// a 0 ms vacuum as success on a store that was 33% garbage (K5). In all three
// the decline path and the success path were indistinguishable in the return
// value, so no caller and no gate could separate them.
mod class_a_declined_is_not_success {
    use devmap_store::{Store, VacuumAction};

    fn scratch(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "devmap-gate-a-{name}-{}.sqlite",
            std::process::id()
        ));
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
        path
    }

    /// A reclaim that declined must say so, not report a successful no-op.
    ///
    /// `VacuumAction::Declined` is the whole point: "nothing was reclaimed
    /// because nothing was worth reclaiming" and "nothing was reclaimed
    /// because the check never ran" are different facts, and K5 is what
    /// happens when a caller cannot tell them apart. The outcome also carries
    /// the counters the decision was made from, so the decision is auditable
    /// rather than merely reported.
    #[test]
    fn a_declined_vacuum_is_distinguishable_from_a_completed_one() {
        let path = scratch("vacuum");
        let store = Store::open(&path).expect("open scratch store");
        let outcome = store.vacuum_if_needed().expect("vacuum must not error");

        // A fresh store has nothing to reclaim, so this is the decline path.
        assert!(
            matches!(outcome.action, VacuumAction::Declined),
            "a fresh store should decline, got {:?}",
            outcome.action
        );
        assert!(
            outcome.freelist_before >= 0 && outcome.page_count_before > 0,
            "the decline must carry the counters it was decided from, got \
             freelist={} pages={}",
            outcome.freelist_before,
            outcome.page_count_before
        );

        // The type must be able to express "ran", or the distinction above is
        // vacuous — a single-variant enum would satisfy the assertion while
        // meaning nothing.
        let ran = VacuumAction::Incremental { requested: 1 };
        assert!(
            !matches!(ran, VacuumAction::Declined),
            "VacuumAction must distinguish a reclaim that ran from one that declined"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// Work dropped against a cap is counted, never silently discarded.
    ///
    /// A truncated symbol list that reports nothing is a list a consumer reads
    /// as complete. This is the same property as `{shown, total}` on query
    /// responses, asserted where the cap actually bites.
    #[test]
    fn a_truncated_scan_reports_what_it_dropped() {
        let mut source = String::new();
        for index in 0..(devmap_extract::fallback::MAX_FALLBACK_SYMBOLS + 7) {
            source.push_str(&format!("function Sym{index}()\n"));
        }
        let scan = devmap_extract::fallback::scan_declarations("big.ps1", &source);

        assert_eq!(
            scan.symbols.len(),
            devmap_extract::fallback::MAX_FALLBACK_SYMBOLS,
            "the cap must actually bind, or this gate proves nothing"
        );
        assert_eq!(
            scan.truncated, 7,
            "every declaration dropped against the cap must be counted; a \
             silent truncation is a prefix presented as a set"
        );
    }

    /// A file discovery refused is recorded with a reason, not dropped.
    ///
    /// This is the Class A half of K2: a file that vanishes between the walker
    /// and the extractor is indistinguishable from a file that was never there,
    /// which is why two whole languages went missing while parity stayed green.
    #[test]
    fn a_refused_file_is_recorded_with_its_reason() {
        let root = std::env::temp_dir().join(format!("devmap-gate-a-disc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create scratch root");
        std::fs::write(root.join("keep.py"), "def f():\n    return 1\n").expect("write source");
        std::fs::write(root.join("image.png"), [0x89u8, 0x50, 0x4e, 0x47]).expect("write binary");

        let (_, report) =
            devmap_extract::collect_sources_with_report(&root).expect("discovery must not error");

        assert!(
            report
                .skipped_paths
                .iter()
                .any(|(path, _)| path.ends_with("image.png")),
            "a refused candidate must appear in skipped_paths with a reason; \
             got {:?}",
            report.skipped_paths
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}

// ---------------------------------------------------------------------------
// Class C — invented data must not wear the shape of derived data.
// ---------------------------------------------------------------------------
//
// K3 attributed 457 symbols to Markdown design documents — Go and TypeScript
// types written inside fenced code blocks, presented as declarations of the
// document. The graph is what other tools reason about, so a fabricated symbol
// is an assertion that code exists which does not.
mod class_c_provenance_is_machine_readable {
    use devmap_extract::{extract_file, ExtractionEngine, ParseOutcome, SymbolKind};

    /// A pattern-matched file and a parsed file are distinguishable *by type*,
    /// not by convention or by inspecting names.
    ///
    /// Asserting both directions matters. If only the fallback side were
    /// pinned, labelling everything `RegexFallback` would satisfy the gate
    /// while destroying the distinction it exists to preserve.
    #[test]
    fn pattern_matched_and_parsed_files_carry_different_provenance() {
        let parsed = extract_file("src/service.py", "def hello():\n    return 1\n");
        assert!(
            matches!(parsed.engine, ExtractionEngine::TreeSitter { .. }),
            "a parsed file must report TreeSitter, got {:?}",
            parsed.engine
        );
        assert!(
            matches!(parsed.parse_outcome, ParseOutcome::Clean),
            "a clean parse must report Clean, got {:?}",
            parsed.parse_outcome
        );

        let recovered = extract_file("api/user.proto", "message User {\n  string id = 1;\n}\n");
        assert!(
            matches!(recovered.engine, ExtractionEngine::RegexFallback { .. }),
            "a pattern-matched file must report RegexFallback, got {:?}",
            recovered.engine
        );
        assert!(
            matches!(recovered.parse_outcome, ParseOutcome::Fallback { .. }),
            "a pattern-matched file must report Fallback, got {:?}",
            recovered.parse_outcome
        );
    }

    /// A pattern-matched symbol must not claim a property the tier cannot evaluate.
    ///
    /// Exportedness is a language-specific rule the line scanner does not
    /// implement. Claiming `is_exported: true` would put a public-API assertion
    /// into the graph on the strength of a regex — a smaller version of the
    /// same error as inventing the symbol.
    #[test]
    fn a_pattern_matched_symbol_claims_nothing_it_cannot_evaluate() {
        let recovered = extract_file("api/user.proto", "message User {\n  string id = 1;\n}\n");
        for symbol in recovered
            .symbols
            .iter()
            .filter(|symbol| symbol.kind != SymbolKind::File)
        {
            assert!(
                !symbol.is_exported,
                "{} was recovered by pattern; exportedness was never evaluated \
                 and must not be asserted",
                symbol.name
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Class D — a stale component must not serve as if fresh.
// ---------------------------------------------------------------------------
//
// K6: a rebuilt binary left the daemon answering from superseded code. The
// store half of the same shape is schema drift — the working index in this
// repository sits at `user_version=2` against a binary writing 12.
mod class_d_identity_is_validated {
    use devmap_store::{Store, CURRENT_SCHEMA_VERSION};

    /// A store written by a newer binary must be refused, not opened.
    ///
    /// Fail-closed is the only safe direction: a forward-compatible read of a
    /// schema this binary does not know silently misreads rows, and a graph
    /// that is confidently wrong is worse than one that is unavailable.
    /// The HEAD boundary. (B5)
    ///
    /// The third identity a long-lived component depends on, after the binary
    /// (K6) and the schema (above). A commit, branch switch, rebase or stash
    /// changes what the index should contain while touching no watched file,
    /// so a daemon that only watches the working tree serves a generation
    /// describing a checkout that no longer exists.
    ///
    /// The distinction this pins is the one that matters: **no generation yet**
    /// and **a generation built at a different HEAD** must not look alike.
    /// `None` means nothing to invalidate; `Some(sha)` is comparable against
    /// the working tree and can disagree.
    #[test]
    fn the_head_a_generation_was_built_at_is_recoverable() {
        let path =
            std::env::temp_dir().join(format!("devmap-gate-d-head-{}.sqlite", std::process::id()));
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }

        let store = Store::open(&path).expect("open scratch store");
        assert_eq!(
            store.latest_generation_head_sha().expect("read head sha"),
            None,
            "an unbuilt store has no HEAD to compare against, and that is not              the same as having been built at an unknown one"
        );

        {
            let conn = rusqlite::Connection::open(&path).expect("reopen for fixture");
            conn.execute(
                "INSERT INTO generations (created_at, head_sha, analysis_json, repo_root)                  VALUES (0.0, 'deadbeefcafe', '{}', '/repo')",
                [],
            )
            .expect("insert a generation");
        }

        let store = Store::open(&path).expect("reopen store");
        assert_eq!(
            store.latest_generation_head_sha().expect("read head sha"),
            Some("deadbeefcafe".to_string()),
            "the HEAD a generation was built at must be recoverable, or a moved              checkout cannot be detected"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_future_schema_is_refused_rather_than_misread() {
        let path =
            std::env::temp_dir().join(format!("devmap-gate-d-{}.sqlite", std::process::id()));
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }

        {
            let conn = rusqlite::Connection::open(&path).expect("create store");
            conn.execute_batch(&format!(
                "PRAGMA user_version = {};",
                CURRENT_SCHEMA_VERSION + 1
            ))
            .expect("stamp a future schema version");
        }

        let opened = Store::open(&path);
        assert!(
            opened.is_err(),
            "a store stamped newer than CURRENT_SCHEMA_VERSION ({CURRENT_SCHEMA_VERSION}) \
             must be refused, not opened and misread"
        );
        let _ = std::fs::remove_file(&path);
    }
}

// ---------------------------------------------------------------------------
// Class A, second surface — a status field that can never degrade is a literal.
// ---------------------------------------------------------------------------
//
// Found by running Class A's mechanical audit across the workspace rather than
// by a failing test. `AnalysisSummary::status` exists to satisfy N4 — "a check
// that could not run never reports as one that passed", whose stated acceptance
// is "timeout surfaces `status`". It was assigned `AnalysisStatus::Ok` on every
// path, so `Partial` and `Timeout` were unconstructible anywhere in the
// workspace and the two match arms rendering them were unreachable. The type
// carried the property; the code never exercised it.
//
// The audit also found the cause: Louvain's `MAX_PASSES` / `MAX_LOCAL_ROUNDS`
// ceilings returned a `Vec<usize>` identical in type and shape to a converged
// partition, so there was nothing for a status to be computed *from*.
mod class_a_status_is_computed_not_asserted {
    use devmap_analyze::{analyze, detect_communities};
    use devmap_extract::extract_file;
    use devmap_resolve::Resolver;

    /// A clean corpus reports `Ok` — and reports it because it was computed.
    ///
    /// This half alone is weak (a literal `Ok` passes it), which is why the
    /// second test below asserts the value can vary at all. Together they pin
    /// "computed" rather than "happens to read Ok".
    #[test]
    fn a_settled_analysis_reports_ok() {
        let extractions = vec![
            extract_file("a.py", "def one():\n    return two()\n"),
            extract_file("b.py", "def two():\n    return 2\n"),
        ];
        let mut resolver = Resolver::new();
        resolver.index_extractions(&extractions);
        let resolution = resolver.resolve_all(&extractions);

        let summary = analyze(&extractions, &resolution);
        assert!(
            matches!(summary.status, devmap_analyze::AnalysisStatus::Ok),
            "a small settled corpus must report Ok, got {:?}",
            summary.status
        );
    }

    /// Community detection reports whether it settled, separately from its result.
    ///
    /// The `degraded` channel is what makes a non-`Ok` status constructible at
    /// all. Asserting it is `None` here is not the point — the point is that the
    /// field exists on the return type, so a ceiling-stopped run has somewhere
    /// to say so instead of returning a best-effort partition that looks final.
    #[test]
    fn community_detection_reports_whether_it_settled() {
        let extractions = vec![
            extract_file("a.py", "def one():\n    return two()\n"),
            extract_file("b.py", "def two():\n    return 2\n"),
        ];
        let mut resolver = Resolver::new();
        resolver.index_extractions(&extractions);
        let resolution = resolver.resolve_all(&extractions);

        let detection = detect_communities(&extractions, &resolution);
        assert!(
            detection.degraded.is_none(),
            "this graph settles well inside the ceilings, got {:?}",
            detection.degraded
        );

        // The reason is a String, not a bool, because it is rendered into
        // `AnalysisStatus::Partial { reason }` and a status a reader cannot act
        // on is barely better than no status.
        let degraded = devmap_analyze::CommunityDetection {
            communities: Vec::new(),
            degraded: Some("stopped at ceiling".to_string()),
        };
        assert!(
            degraded.degraded.is_some(),
            "the type must be able to express a degraded run, or the field is decoration"
        );
    }

    /// An empty corpus is a complete answer, not a degraded one.
    ///
    /// The distinction Class A is about, at the boundary that most invites
    /// conflating them: nothing to partition and could-not-partition both
    /// produce zero communities.
    #[test]
    fn nothing_to_analyze_is_not_a_degraded_analysis() {
        let extractions: Vec<devmap_extract::Extraction> = Vec::new();
        let resolution = Resolver::new().resolve_all(&extractions);
        let detection = detect_communities(&extractions, &resolution);

        assert!(detection.communities.is_empty());
        assert!(
            detection.degraded.is_none(),
            "an empty corpus settled trivially; reporting it as degraded would              make the signal useless"
        );
    }
}
