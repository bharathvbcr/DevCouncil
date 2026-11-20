//! The edge set a build resolves must not depend on how loaded the machine was.
//!
//! `devmap-extract/tests/edge_inputs_under_cpu_contention.rs` pins equality at
//! the extraction seam, because that crate cannot link the resolver. This one
//! can: it runs extraction *and* resolution over a small multi-file, two
//! language tree and compares the resolved edges themselves.
//!
//! Contention is simulated deterministically. A zero first-pass budget refuses
//! every file in the parallel pass, which is what a starved machine does to the
//! large ones, and the serial retry must restore the idle result. A control run
//! with the retry starved too proves the comparison can fail: those edges
//! differ, and the refusal is named.

use devmap_extract::{extract_all_with_budgets, Extraction, FileRef, ParseBudgets};
use devmap_resolve::Resolver;
use std::time::Duration;

const FILES: &[(&str, &str)] = &[
    (
        "app/main.py",
        "from app.lib import helper\n\ndef run():\n    return helper() + local()\n\ndef local():\n    return 1\n",
    ),
    (
        "app/lib.py",
        "def helper():\n    return deep()\n\ndef deep():\n    return 2\n",
    ),
    (
        "svc/lib.go",
        "package svc\n\nfunc Used() int { return inner() }\n\nfunc inner() int { return 1 }\n",
    ),
    (
        "svc/app.go",
        "package svc\n\nfunc Main() int { return Used() + other() }\n\nfunc other() int { return 3 }\n",
    ),
];

fn extract(budgets: ParseBudgets) -> (Vec<Extraction>, devmap_extract::BudgetRetryReport) {
    let files: Vec<FileRef> = FILES
        .iter()
        .map(|(path, source)| FileRef { path, source })
        .collect();
    extract_all_with_budgets(&files, None, budgets, None)
}

fn edges_of(extractions: &[Extraction]) -> String {
    let mut resolver = Resolver::new();
    resolver.index_extractions(extractions);
    let result = resolver.resolve_all(extractions).unwrap();
    serde_json::to_string(&result.edges).unwrap()
}

fn idle() -> String {
    let (out, report) = extract(ParseBudgets {
        first_pass: Duration::from_secs(60),
        retry: Duration::from_secs(60),
        retry_total: Duration::from_secs(600),
    });
    assert_eq!(
        report.first_pass_refused, 0,
        "the idle baseline was refused"
    );
    let edges = edges_of(&out);
    assert_ne!(edges, "[]", "equality over no edges is vacuous");
    edges
}

#[test]
fn a_refused_first_pass_recovered_by_the_retry_resolves_the_idle_edge_set() {
    let baseline = idle();
    let (out, report) = extract(ParseBudgets {
        first_pass: Duration::ZERO,
        retry: Duration::from_secs(60),
        retry_total: Duration::from_secs(600),
    });
    assert_eq!(
        report.first_pass_refused,
        FILES.len(),
        "contention not forced"
    );
    assert_eq!(report.recovered, FILES.len());
    assert_eq!(report.still_refused_total, 0);
    assert_eq!(
        edges_of(&out),
        baseline,
        "resolved edges drifted under a refused first pass"
    );
}

#[test]
fn control_without_a_retry_the_resolved_edges_differ_and_the_refusal_is_named() {
    let baseline = idle();
    let (out, report) = extract(ParseBudgets {
        first_pass: Duration::ZERO,
        retry: Duration::ZERO,
        retry_total: Duration::from_secs(600),
    });
    assert_eq!(report.still_refused_total, FILES.len());
    assert!(out.iter().all(|e| e.is_budget_refusal()));
    assert_ne!(
        edges_of(&out),
        baseline,
        "the comparison could not have failed"
    );
}
