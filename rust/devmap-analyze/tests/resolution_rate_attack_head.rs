//! Head-only probes (read fields added by d747c124).

use devmap_analyze::*;
use devmap_extract::extract_file;
use devmap_extract::model::*;
use devmap_resolve::Resolver;

fn rate_of(extractions: &[Extraction]) -> ResolutionRate {
    let mut resolver = Resolver::new();
    resolver.index_extractions(extractions);
    let resolution = resolver.resolve_all(extractions).unwrap();
    resolution_rate(extractions, &resolution)
}

const PY: &str = "def helper():\n    return 1\n\n\ndef main():\n    return helper()\n";

/// Probe 6: a row stored before the new fields existed reads back.
#[test]
fn p6_old_stored_row_without_new_fields_reads_back() {
    let old = r#"{"resolved_sites":5,"unresolved_sites":2,"explained_sites":1,
        "gross_permille":714,"net_permille":833,"by_language":{}}"#;
    let rate: ResolutionRate = serde_json::from_str(old).expect("old row must deserialize");
    assert_eq!(rate.vendored_sites, 0);
    assert_eq!(rate.call_blind_sites, 0);
}

/// Probe 6: an explicit null (as the lane brief words it) must also read back.
#[test]
fn p6_explicit_null_new_fields_read_back() {
    let old = r#"{"resolved_sites":5,"unresolved_sites":2,"explained_sites":1,
        "gross_permille":714,"net_permille":833,"by_language":{},
        "vendored_sites":null,"vendored_files":null,"call_blind_sites":null}"#;
    let rate: ResolutionRate = serde_json::from_str(old).expect("null new fields must deserialize");
    assert_eq!(rate.vendored_sites, 0);
}

/// Probe 6: old per-language row (no extracts_calls/blind_to) reads back.
#[test]
fn p6_old_language_row_reads_back() {
    let old = r#"{"resolved_sites":5,"unresolved_sites":0,"explained_sites":0,
        "gross_permille":1000,"net_permille":1000,
        "by_language":{"html":{"resolved_sites":5,"unresolved_sites":0,"explained_sites":0,
        "gross_permille":1000,"net_permille":1000}}}"#;
    let rate: ResolutionRate = serde_json::from_str(old).expect("old language row");
    assert!(rate.by_language["html"].net_permille.is_some());
}

/// Probe 2: the headline's own denominators are recoverable from the top-level
/// fields (they are not: totals are raw).
#[test]
fn p2_top_level_fields_reproduce_the_headline() {
    let mut vendored = String::from("class K:\n");
    for i in 0..30 {
        vendored.push_str(&format!("    def m{i}(self):\n        return 1\n"));
    }
    for i in 0..30 {
        vendored.push_str(&format!("def v{i}():\n    return m{i}()\n"));
    }
    let ex = vec![
        extract_file("lib.py", PY),
        extract_file("vendor/d.py", &vendored),
    ];
    let rate = rate_of(&ex);
    eprintln!("{rate:#?}");
    // Repointed to measured_* by Opus ruling 2026-10-19: the raw totals are a public contract kept for attribution coverage; the headline's denominators are the additive measured_* fields.
    let derived = rate.measured_resolved_sites * 1000
        / (rate.measured_resolved_sites + rate.measured_unresolved_sites
            - rate.measured_explained_sites);
    assert_eq!(
        rate.net_permille,
        Some(derived as u32),
        "headline {:?} is not derivable from the measured_* fields (derived {derived}): {rate:?}",
        rate.net_permille
    );
    // Pinned invariant: the raw totals still count every attribution site.
    assert_eq!(
        rate.resolved_sites + rate.unresolved_sites,
        rate.measured_resolved_sites
            + rate.measured_unresolved_sites
            + rate.vendored_sites
            + rate.call_blind_sites,
        "raw totals must stay the attribution totals: {rate:?}"
    );
}

/// Probe 2: vendored_sites + call_blind_sites + measured == raw total.
#[test]
fn p2_exclusion_counters_partition_the_totals() {
    let ex = vec![
        extract_file("lib.py", PY),
        extract_file("vendor/d.py", "def a():\n    return nope()\n"),
        extract_file("index.html", "<a href=\"x.html\">x</a>\n"),
    ];
    let rate = rate_of(&ex);
    let measured: usize = rate
        .by_language
        .values()
        .filter(|r| r.extracts_calls)
        .map(|r| r.resolved_sites + r.unresolved_sites)
        .sum();
    assert_eq!(
        rate.vendored_sites + rate.call_blind_sites + measured,
        rate.resolved_sites + rate.unresolved_sites,
        "{rate:?}"
    );
    assert!(rate.vendored_files >= 1, "{rate:?}");
}
