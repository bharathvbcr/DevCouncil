#![cfg(feature = "parse")]

use devmap_extract::extract_file;
use devmap_extract::model::{ExtractedCall, Extraction, Span};
use devmap_resolve::model::{LimitPolicy, Resolution};
use devmap_resolve::Resolver;

fn fixture(declarations: usize, calls: usize) -> Vec<Extraction> {
    let mut inputs: Vec<_> = (0..declarations)
        .map(|i| extract_file(&format!("p{i}.py"), "def leaf(): pass\n"))
        .collect();
    for file in ["caller_a.py", "caller_b.py"] {
        let mut caller = extract_file(file, "def run(): pass\n");
        caller.calls = (0..calls)
            .map(|i| ExtractedCall {
                caller_symbol: Some(format!("{file}::run")),
                callee_name: "leaf".into(),
                receiver_expr: None,
                span: Span {
                    start_byte: i,
                    end_byte: i + 1,
                },
            })
            .collect();
        inputs.push(caller);
    }
    inputs
}

#[test]
fn repeated_ambiguity_shares_one_candidate_allocation() {
    let inputs = fixture(40, 800);
    let mut resolver = Resolver::new();
    resolver.index_extractions(&inputs);
    let result = resolver.resolve_all(&inputs).unwrap();
    let lists: Vec<_> = result
        .unresolved
        .iter()
        .filter_map(|row| match &row.resolution {
            Resolution::AmbiguousGlobal { candidates, .. } => Some(candidates),
            _ => None,
        })
        .collect();
    assert_eq!(lists.len(), 1600);
    assert!(lists.iter().all(|list| list.len() == 40));
    let unique: std::collections::BTreeSet<_> = lists.iter().map(|list| list.as_ptr()).collect();
    assert_eq!(
        unique.len(),
        1,
        "repeated sites must share the same immutable candidate set across files"
    );
}

#[test]
fn under_the_refuse_policy_aggregate_limits_refuse_the_whole_result() {
    let inputs = fixture(40, 80);
    let mut resolver = Resolver::new();
    resolver.index_extractions(&inputs);
    for (resource, limits) in [
        (
            "candidate visits",
            devmap_resolve::ResolutionLimits {
                candidate_visits: 39,
                on_exceed: LimitPolicy::Refuse,
                ..Default::default()
            },
        ),
        (
            "retained candidate bytes",
            devmap_resolve::ResolutionLimits {
                retained_candidate_bytes: 128,
                on_exceed: LimitPolicy::Refuse,
                ..Default::default()
            },
        ),
        (
            "ambiguity evidence bytes",
            devmap_resolve::ResolutionLimits {
                ambiguity_evidence_bytes: 1024,
                on_exceed: LimitPolicy::Refuse,
                ..Default::default()
            },
        ),
    ] {
        let error = resolver
            .resolve_all_with_limits(&inputs, limits)
            .unwrap_err();
        assert_eq!(error.resource, resource);
        assert!(error.attempted > error.limit);
        assert!(error.to_string().contains("no partial graph"));
        // The figure is whatever the charging thread had counted, which races
        // the others; it is only ever offered as a floor.
        assert!(
            error
                .to_string()
                .contains(&format!("requires at least {} units", error.attempted)),
            "a racy count must not read as the exact total: {error}"
        );
    }
    assert!(
        resolver.resolve_all(&inputs).is_ok(),
        "a refused attempt cannot poison the next resolution"
    );
}

/// The kernel-scale shape: 20,000 ambiguous sites naming one 500-candidate
/// list. Charged per site that is ten million candidate tuples and refused the
/// whole build (`ambiguity evidence bytes requires 134218520 units`); charged
/// per distinct list it is one list, which is what memory and the store hold.
#[test]
fn many_sites_naming_one_list_are_charged_for_one_list() {
    let inputs = fixture(500, 10_000);
    let mut resolver = Resolver::new();
    resolver.index_extractions(&inputs);
    let result = resolver
        .resolve_all(&inputs)
        .expect("the default budget holds one 500-candidate list however many sites name it");
    let rows: Vec<_> = result
        .unresolved
        .iter()
        .filter_map(|row| match &row.resolution {
            Resolution::AmbiguousGlobal { candidates, .. } => Some(candidates),
            _ => None,
        })
        .collect();
    assert_eq!(rows.len(), 20_000);
    assert!(rows.iter().all(|list| list.len() == 500));
    assert!(
        result.budget_gaps().is_empty(),
        "nothing was capped, so no gap is claimed"
    );
}

fn capped_rows(result: &devmap_resolve::model::ResolutionResult) -> Vec<&Resolution> {
    result
        .unresolved
        .iter()
        .map(|row| &row.resolution)
        .filter(|resolution| matches!(resolution, Resolution::CappedGlobal { .. }))
        .collect()
}

#[test]
fn an_exceeded_ambiguity_budget_degrades_the_list_and_records_a_gap() {
    let inputs = fixture(40, 80);
    let mut resolver = Resolver::new();
    resolver.index_extractions(&inputs);
    let result = resolver
        .resolve_all_with_limits(
            &inputs,
            devmap_resolve::ResolutionLimits {
                ambiguity_evidence_bytes: 1024,
                ..Default::default()
            },
        )
        .expect("a budget degrades; it does not abort the build");

    // Every one of the 160 sites keeps its ledger row, with the sorted prefix
    // and the true count.
    let capped = capped_rows(&result);
    assert_eq!(capped.len(), 160);
    for resolution in &capped {
        let Resolution::CappedGlobal {
            candidates,
            total,
            limit,
            ..
        } = resolution
        else {
            unreachable!()
        };
        assert_eq!(candidates.len(), 8, "the retained prefix");
        assert_eq!(*total, Some(40), "the count the name really has");
        assert_eq!(&**limit, "ambiguity evidence bytes");
        assert!(
            candidates.windows(2).all(|pair| pair[0] <= pair[1]),
            "a sorted prefix"
        );
    }
    assert!(
        result.edges.iter().all(|edge| !matches!(
            edge.resolution.as_deref(),
            Some(Resolution::CappedGlobal { .. })
        )),
        "a capped list asserts no edge"
    );

    // The gap says what was cut and by how much, with totals not a sample.
    let gaps = result.budget_gaps();
    assert_eq!(gaps.len(), 1);
    let gap = &gaps[0];
    assert_eq!(gap.limit, "ambiguity evidence bytes");
    assert_eq!(gap.names_truncated, 1);
    assert_eq!(gap.candidates_dropped, 32);
    assert_eq!(gap.names_skipped, 0);
    assert_eq!(gap.sites, 160);
    assert_eq!(gap.sample_names, vec!["leaf".to_string()]);
    let sentence = gap.describe();
    assert!(sentence.contains("32 candidate(s) dropped"), "{sentence}");
    assert!(sentence.contains("160 ledger row(s)"), "{sentence}");
    assert!(!sentence.contains("  "), "no run of spaces: {sentence}");
}

#[test]
fn a_skipped_lookup_is_a_ledger_row_that_claims_nothing() {
    let inputs = fixture(40, 80);
    let mut resolver = Resolver::new();
    resolver.index_extractions(&inputs);
    for (limit_name, limits) in [
        (
            "candidate visits",
            devmap_resolve::ResolutionLimits {
                candidate_visits: 39,
                ..Default::default()
            },
        ),
        (
            "retained candidate bytes",
            devmap_resolve::ResolutionLimits {
                retained_candidate_bytes: 128,
                ..Default::default()
            },
        ),
    ] {
        let result = resolver.resolve_all_with_limits(&inputs, limits).unwrap();
        let capped = capped_rows(&result);
        assert_eq!(capped.len(), 160, "{limit_name}: every site keeps its row");
        for resolution in capped {
            let Resolution::CappedGlobal {
                candidates,
                total,
                limit,
                ..
            } = resolution
            else {
                unreachable!()
            };
            assert!(candidates.is_empty(), "{limit_name}");
            assert_eq!(*total, None, "{limit_name}: the count is unknown, not zero");
            assert_eq!(&**limit, limit_name);
        }
        let gap = &result.budget_gaps()[0];
        assert_eq!(
            (gap.names_skipped, gap.names_truncated),
            (1, 0),
            "{limit_name}"
        );
        assert!(
            result
                .edges
                .iter()
                .all(|edge| edge.edge_kind != devmap_extract::model::EdgeKind::Calls),
            "{limit_name}: a skipped lookup binds nothing"
        );
    }
}

/// Which lists a shared budget degrades is decided by the order they are
/// charged in, so the degrade pass must not depend on thread scheduling.
#[test]
fn a_degraded_resolution_is_identical_on_every_run_and_thread_count() {
    // Twelve names with 30 declarations each: the budget holds a few lists whole
    // and cuts the rest.
    let mut inputs = Vec::new();
    for name in 0..12 {
        for file in 0..30 {
            inputs.push(extract_file(
                &format!("d{file}/n{name}.py"),
                &format!("def leaf{name}(): pass\n"),
            ));
        }
    }
    let mut caller = extract_file("zz/caller.py", "def run(): pass\n");
    caller.calls = (0..12)
        .map(|name| ExtractedCall {
            caller_symbol: Some("zz/caller.py::run".into()),
            callee_name: format!("leaf{name}"),
            receiver_expr: None,
            span: Span {
                start_byte: name,
                end_byte: name + 1,
            },
        })
        .collect();
    inputs.push(caller);
    let limits = devmap_resolve::ResolutionLimits {
        ambiguity_evidence_bytes: 8 * 1024,
        ..Default::default()
    };
    let render = |threads: usize| {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        pool.install(|| {
            let mut resolver = Resolver::new();
            resolver.index_extractions(&inputs);
            let result = resolver.resolve_all_with_limits(&inputs, limits).unwrap();
            format!("{:?}", result.unresolved)
        })
    };
    let reference = render(1);
    assert!(reference.contains("CappedGlobal"), "the budget must trip");
    assert!(
        reference.contains("AmbiguousGlobal"),
        "and spare some lists"
    );
    for threads in [2, 8, 8] {
        assert_eq!(reference, render(threads), "{threads} threads");
    }
}

#[test]
fn ordinary_candidate_names_do_not_pay_for_escape_sequences() {
    // 8000 sites with 16 ordinary names are charged one ledger row each. Even
    // that upper bound fits the default budget; charging six bytes for every
    // unescaped ASCII byte incorrectly refuses this workload.
    let inputs = fixture(16, 4000);
    let mut resolver = Resolver::new();
    resolver.index_extractions(&inputs);
    let result = resolver.resolve_all(&inputs).unwrap();
    let ambiguous: Vec<_> = result
        .unresolved
        .iter()
        .filter_map(|row| match &row.resolution {
            Resolution::AmbiguousGlobal { candidates, .. } => Some(candidates),
            _ => None,
        })
        .collect();
    // Every site keeps its row, each naming all 16 candidates, and none
    // emits an edge.
    assert_eq!(ambiguous.len(), 8000);
    assert!(ambiguous.iter().all(|candidates| candidates.len() == 16));
    assert!(result.edges.iter().all(|edge| !matches!(
        edge.resolution.as_deref(),
        Some(Resolution::AmbiguousGlobal { .. })
    )));
}

#[test]
fn candidate_cache_preserves_go_private_visibility() {
    let mut inputs = Vec::new();
    for package in ["one", "two"] {
        for file in ["a", "b"] {
            inputs.push(extract_file(
                &format!("{package}/{file}.go"),
                "package p\nfunc hidden() {}\n",
            ));
        }
        inputs.push(extract_file(
            &format!("{package}/caller.go"),
            "package p\nfunc run() { hidden() }\n",
        ));
    }
    let mut resolver = Resolver::new();
    resolver.index_extractions(&inputs);
    let result = resolver.resolve_all(&inputs).unwrap();
    let rows: Vec<_> = result
        .unresolved
        .iter()
        .filter(|row| matches!(row.resolution, Resolution::AmbiguousGlobal { .. }))
        .collect();
    // Two packages, one bare call each: one site per package.
    assert_eq!(rows.len(), 2);
    for row in rows {
        let package = row.source_file.split('/').next().unwrap();
        let Resolution::AmbiguousGlobal { candidates, .. } = &row.resolution else {
            unreachable!()
        };
        assert_eq!(candidates.len(), 2);
        assert!(candidates
            .iter()
            .all(|(path, _)| path.starts_with(&format!("{package}/"))));
    }
}

/// Attack: every budget zero and no candidates kept. The pass must still
/// finish, keep every site's ledger row, and claim no edge it cannot support.
#[test]
fn zero_budgets_degrade_everything_without_panicking_or_inventing_edges() {
    let inputs = fixture(40, 80);
    let mut resolver = Resolver::new();
    resolver.index_extractions(&inputs);
    let result = resolver
        .resolve_all_with_limits(
            &inputs,
            devmap_resolve::ResolutionLimits {
                candidate_visits: 0,
                retained_candidate_bytes: 0,
                ambiguity_evidence_bytes: 0,
                degraded_candidates: 0,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(capped_rows(&result).len(), 160);
    assert!(result
        .edges
        .iter()
        .all(|edge| edge.edge_kind != devmap_extract::model::EdgeKind::Calls));
}

/// What a zero budget costs is pinned, not just survived: a skipped lookup
/// binds nothing, and that includes a name with exactly one declaration, whose
/// edge the default budgets would have asserted. The gap says which names were
/// skipped, so the lost edge is accounted for rather than silent. The
/// ambiguity budget is the exception that proves the rule: it only ever
/// charges lists of two or more, so a unique name keeps its edge under it.
#[test]
fn a_zero_budget_loses_the_unique_name_edge_and_the_gap_names_it() {
    use devmap_extract::model::EdgeKind;
    let mut inputs = fixture(40, 3);
    inputs.push(extract_file("solo.py", "def solo(): pass\n"));
    let caller = inputs
        .iter_mut()
        .find(|ext| ext.file_path == "caller_a.py")
        .unwrap();
    caller.calls.push(ExtractedCall {
        caller_symbol: Some("caller_a.py::run".into()),
        callee_name: "solo".into(),
        receiver_expr: None,
        span: Span {
            start_byte: 99,
            end_byte: 100,
        },
    });
    let mut resolver = Resolver::new();
    resolver.index_extractions(&inputs);
    let solo_edges = |result: &devmap_resolve::model::ResolutionResult| {
        result
            .edges
            .iter()
            .filter(|edge| {
                edge.edge_kind == EdgeKind::Calls && edge.target_symbol == "solo.py::solo"
            })
            .count()
    };

    let control = resolver.resolve_all(&inputs).unwrap();
    assert_eq!(
        solo_edges(&control),
        1,
        "inside its budgets the edge exists"
    );
    assert!(control.budget_gaps().is_empty());

    for (limit_name, limits) in [
        (
            "candidate visits",
            devmap_resolve::ResolutionLimits {
                candidate_visits: 0,
                ..Default::default()
            },
        ),
        (
            "retained candidate bytes",
            devmap_resolve::ResolutionLimits {
                retained_candidate_bytes: 0,
                ..Default::default()
            },
        ),
    ] {
        let result = resolver.resolve_all_with_limits(&inputs, limits).unwrap();
        assert_eq!(
            solo_edges(&result),
            0,
            "{limit_name}: the lookup was skipped"
        );
        let gaps = result.budget_gaps();
        assert_eq!(gaps.len(), 1, "{limit_name}");
        assert_eq!(gaps[0].limit, limit_name);
        assert_eq!(
            gaps[0].names_skipped, 2,
            "{limit_name}: `leaf` and `solo` were both skipped"
        );
        assert!(
            gaps[0].sample_names.contains(&"solo".to_string()),
            "{limit_name}: the lost edge's name is on the gap: {:?}",
            gaps[0].sample_names
        );
    }

    let evidence = resolver
        .resolve_all_with_limits(
            &inputs,
            devmap_resolve::ResolutionLimits {
                ambiguity_evidence_bytes: 0,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        solo_edges(&evidence),
        1,
        "the ambiguity budget never charges a one-candidate list"
    );
    assert_eq!(evidence.budget_gaps()[0].names_skipped, 0);
}

/// `CappedGlobal` means candidates were dropped. A list that already fits the
/// retained prefix loses nothing when the ambiguity budget trips, so it stays
/// the complete `AmbiguousGlobal` it was and records no gap. Tagging it capped
/// reported "50 candidate list(s) cut to a retained prefix, 0 candidate(s)
/// dropped" and, through `AnalysisStatus::Partial`, marked the graph degraded
/// for a loss that did not happen.
#[test]
fn a_truncation_that_drops_nothing_stays_ambiguous_and_records_no_gap() {
    let inputs = fixture(3, 5);
    let mut resolver = Resolver::new();
    resolver.index_extractions(&inputs);
    // The budget trips on the first list; the default prefix (8) holds all 3.
    let tight = devmap_resolve::ResolutionLimits {
        ambiguity_evidence_bytes: 1,
        ..Default::default()
    };
    let result = resolver.resolve_all_with_limits(&inputs, tight).unwrap();
    assert!(
        capped_rows(&result).is_empty(),
        "nothing was dropped, so no row is capped"
    );
    assert!(
        result.budget_gaps().is_empty(),
        "and no gap is recorded: {:?}",
        result.budget_gaps()
    );
    let ambiguous: Vec<_> = result
        .unresolved
        .iter()
        .filter_map(|row| match &row.resolution {
            Resolution::AmbiguousGlobal { candidates, .. } => Some(candidates.len()),
            _ => None,
        })
        .collect();
    assert_eq!(ambiguous.len(), 10, "every site keeps its row");
    assert!(
        ambiguous.iter().all(|len| *len == 3),
        "with the complete list: {ambiguous:?}"
    );

    // The boundary: a prefix of exactly the list's length drops nothing, one
    // shorter drops one and says so.
    for (keep, dropped) in [(3usize, None), (2, Some(1usize))] {
        let result = resolver
            .resolve_all_with_limits(
                &inputs,
                devmap_resolve::ResolutionLimits {
                    ambiguity_evidence_bytes: 1,
                    degraded_candidates: keep,
                    ..Default::default()
                },
            )
            .unwrap();
        match dropped {
            None => assert!(result.budget_gaps().is_empty(), "keep={keep}"),
            Some(dropped) => {
                let gaps = result.budget_gaps();
                assert_eq!(gaps.len(), 1, "keep={keep}");
                assert_eq!(gaps[0].candidates_dropped, dropped, "keep={keep}");
                assert_eq!(gaps[0].names_truncated, 1, "keep={keep}");
            }
        }
    }
}
