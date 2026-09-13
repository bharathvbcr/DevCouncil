#![cfg(feature = "parse")]

use devmap_extract::extract_file;
use devmap_extract::model::{ExtractedCall, Extraction, Span};
use devmap_resolve::model::Resolution;
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
fn aggregate_limits_refuse_the_whole_result_without_sampling_candidates() {
    let inputs = fixture(40, 80);
    let mut resolver = Resolver::new();
    resolver.index_extractions(&inputs);
    for (resource, limits) in [
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
        (
            "ambiguity evidence bytes",
            devmap_resolve::ResolutionLimits {
                ambiguity_evidence_bytes: 1024,
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
    }
    assert!(
        resolver.resolve_all(&inputs).is_ok(),
        "a refused attempt cannot poison the next resolution"
    );
}

#[test]
fn repeated_evidence_is_bounded_by_default() {
    // Ten million repeated candidate tuples exceed the default evidence cap
    // even when ordinary ASCII is charged at its actual one-byte width.
    let inputs = fixture(500, 10_000);
    let mut resolver = Resolver::new();
    resolver.index_extractions(&inputs);
    assert_eq!(
        resolver.resolve_all(&inputs).unwrap_err().resource,
        "ambiguity evidence bytes"
    );
}

#[test]
fn ordinary_candidate_names_do_not_pay_for_escape_sequences() {
    // 8000 sites with 16 ordinary names are charged before edge deduplication.
    // Even that upper bound fits the default budget; charging six bytes for
    // every unescaped ASCII byte incorrectly refuses this workload.
    let inputs = fixture(16, 4000);
    let mut resolver = Resolver::new();
    resolver.index_extractions(&inputs);
    let result = resolver.resolve_all(&inputs).unwrap();
    let ambiguous: Vec<_> = result
        .edges
        .iter()
        .filter_map(|edge| match edge.resolution.as_deref() {
            Some(Resolution::AmbiguousGlobal { candidates, .. }) => Some(candidates),
            _ => None,
        })
        .collect();
    // Each of the two callers still reaches all 16 candidates after repeated
    // calls from the same symbol are merged into one edge per target.
    assert_eq!(ambiguous.len(), 32);
    assert!(ambiguous.iter().all(|candidates| candidates.len() == 16));
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
    let edges: Vec<_> = result
        .edges
        .iter()
        .filter(|edge| {
            matches!(
                edge.resolution.as_deref(),
                Some(Resolution::AmbiguousGlobal { .. })
            )
        })
        .collect();
    assert_eq!(edges.len(), 4);
    for edge in edges {
        let package = edge.source_file.split('/').next().unwrap();
        let Some(Resolution::AmbiguousGlobal { candidates, .. }) = edge.resolution.as_deref()
        else {
            unreachable!()
        };
        assert_eq!(candidates.len(), 2);
        assert!(candidates
            .iter()
            .all(|(path, _)| path.starts_with(&format!("{package}/"))));
    }
}
