//! Every integration test is classified against the `parse` feature.
//!
//! `devmap-extract`'s `parse` feature is off in the shape an embedder links —
//! GitPulse links `devmap-query` to answer impact queries and indexes nothing —
//! and `.github/workflows/rust-port.yml` builds and tests that shape per crate.
//!
//! It has now silently broken three times. The mechanism is always the same: a
//! test file lands that uses a `#[cfg(feature = "parse")]` item, nobody adds a
//! `[[test]] required-features` entry for it, and **one unresolved import fails
//! the whole target**. A build error reads as "this target is not meant to be
//! tested" rather than as a failure, so it survives review — and the CI job
//! that would catch it is on a branch that has never been pushed, so it has
//! never actually run.
//!
//! This test closes that loop locally, in the default configuration, where it
//! runs on every `cargo test`. It does not decide whether a target needs a
//! grammar — a compiler does that, and this cannot — it asserts that **somebody
//! decided**: every file under `tests/` is either declared `required-features =
//! ["parse"]` or named below as feature-off-safe. A new test file is a failure
//! until it is classified, which is the only version of this check that cannot
//! rot.
//!
//! Both directions fail closed. A stale name in either list — a target renamed
//! or deleted — is a failure too, because a list that no longer describes the
//! tree is how the previous two regressions became invisible.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Targets that build and run with `parse` off.
///
/// Each touches only the model types, the walker, the store or the artifacts —
/// never a grammar. Adding a name here is a claim that
/// `cargo test -p <crate> --no-default-features` compiles and runs it, and the
/// CI job in `.github/workflows/rust-port.yml` is what proves it.
const FEATURE_OFF_SAFE: &[(&str, &[&str])] = &[
    (
        "devmap-extract",
        &[
            "a_cache_verdict_answers_for_the_root_it_was_asked",
            "a_deadline_reaches_the_children_the_child_started",
            "a_subprocess_is_bounded_in_time_and_bytes",
            "an_unreadable_subtree_is_a_hole_not_a_dead_build",
            "discovery_stays_inside_the_root",
            "ignore_rule_tolerance",
            "reliability_source_reads",
            "the_bounded_runner_under_hostile_children",
        ],
    ),
    ("devmap-resolve", &["resolution_kind_is_one_owner"]),
    (
        "devmap-analyze",
        &["adversarial_analyze", "pdg_bounds", "traversal_allocation"],
    ),
    (
        "devmap-query",
        &[
            "artifacts_sidecar_adversarial",
            "documentation_visibility",
            "freshness_parity",
            "git_is_bounded_on_the_artifact_path",
            "host_artifact_provider",
            "subsystem_handoff_paths_are_computed",
            "subsystem_roles_and_file_kinds_are_computed",
            "the_repository_inventory_bounds_are_honest",
            "the_freshness_inventory_counts_only_what_discovery_can_index",
            "viz_projection",
            "workspace_registry_concurrency",
        ],
    ),
    (
        // No `[[test]]` entry in this crate: nothing under `tests/` reaches a
        // grammar, because the store's whole job is reading back what was
        // already extracted.
        "devmap-store",
        &[
            "a_corrupt_analysis_is_not_an_absent_one",
            "a_refusal_names_its_reason_not_a_parameter",
            "adversarial_store",
            "coverage_gap_inventory",
            "digest_scoped_delta",
            "embedded_reader",
            "kernel_defects",
            "migration_ladder",
            "one_symlink_rule",
            "read_only_store",
            "store_hardening",
            "test_fault_injection",
            "validity_ranges",
            "write_breakdown",
        ],
    ),
];

fn workspace_root() -> PathBuf {
    // `CARGO_MANIFEST_DIR` is `<workspace>/crates/devmap-cli`.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate lives two levels below the workspace root")
        .to_path_buf()
}

/// Names declared `required-features = ["parse"]` in one crate's manifest.
///
/// Parsed from the text rather than through a TOML crate, so it carries its own
/// proof: a parser that matched nothing would make this whole test vacuous,
/// which is the failure mode it exists to prevent, so the caller asserts the
/// count it found against the tree.
fn gated_targets(manifest: &str) -> BTreeSet<String> {
    let mut gated = BTreeSet::new();
    let mut lines = manifest.lines().peekable();
    while let Some(line) = lines.next() {
        if line.trim() != "[[test]]" {
            continue;
        }
        let mut name = None;
        let mut required = false;
        // A `[[test]]` block runs to the next table header or a blank line.
        for entry in lines.by_ref() {
            let entry = entry.trim();
            if entry.is_empty() || entry.starts_with('[') {
                break;
            }
            if let Some(rest) = entry.strip_prefix("name = ") {
                name = Some(rest.trim_matches(['"', ' ']).to_string());
            }
            if entry.starts_with("required-features") && entry.contains("\"parse\"") {
                required = true;
            }
        }
        if let (Some(name), true) = (name, required) {
            gated.insert(name);
        }
    }
    gated
}

#[test]
fn every_integration_test_is_classified_against_the_parse_feature() {
    let root = workspace_root();
    let mut problems = Vec::new();
    let mut total_files = 0usize;

    for (crate_name, safe) in FEATURE_OFF_SAFE {
        let crate_dir = root.join("crates").join(crate_name);
        let manifest = std::fs::read_to_string(crate_dir.join("Cargo.toml"))
            .unwrap_or_else(|error| panic!("{crate_name}/Cargo.toml unreadable: {error}"));
        let gated = gated_targets(&manifest);
        let safe: BTreeSet<String> = safe.iter().map(|name| name.to_string()).collect();

        let mut files = BTreeSet::new();
        for entry in std::fs::read_dir(crate_dir.join("tests"))
            .unwrap_or_else(|error| panic!("{crate_name}/tests unreadable: {error}"))
        {
            let path = entry.expect("a readable directory entry").path();
            if path.extension().is_some_and(|ext| ext == "rs") {
                files.insert(
                    path.file_stem()
                        .expect("a .rs file has a stem")
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
        total_files += files.len();

        for unclassified in files.difference(&gated.union(&safe).cloned().collect()) {
            problems.push(format!(
                "{crate_name}: tests/{unclassified}.rs is neither declared \
                 `required-features = [\"parse\"]` in Cargo.toml nor listed in \
                 FEATURE_OFF_SAFE. Build it with \
                 `cargo check -p {crate_name} --no-default-features --all-targets`: \
                 if it compiles, add it to FEATURE_OFF_SAFE; if it does not, add a \
                 `[[test]]` entry with `required-features = [\"parse\"]`."
            ));
        }
        // A name in either list that no longer names a file is stale, and a
        // stale list is how the last two regressions stayed invisible.
        for stale in gated.difference(&files) {
            problems.push(format!(
                "{crate_name}: Cargo.toml declares a `[[test]]` named {stale:?} \
                 but tests/{stale}.rs does not exist"
            ));
        }
        for stale in safe.difference(&files) {
            problems.push(format!(
                "{crate_name}: FEATURE_OFF_SAFE names {stale:?} but \
                 tests/{stale}.rs does not exist"
            ));
        }
        // Both at once is a contradiction, not a belt-and-braces. A target
        // declared `required-features = ["parse"]` is skipped with the feature
        // off, so calling it feature-off-safe claims coverage that is not
        // there — and the union above would have hidden it.
        for both in gated.intersection(&safe) {
            problems.push(format!(
                "{crate_name}: {both:?} is declared `required-features = \
                 [\"parse\"]` *and* listed in FEATURE_OFF_SAFE. A gated target \
                 does not run with the feature off, so it is not feature-off-safe \
                 — drop it from one of the two."
            ));
        }
    }

    assert!(
        total_files > 90,
        "only {total_files} test files were found across five crates — the walk \
         or the manifest parse is broken, and a check that could not run must \
         not report the same green as one that ran"
    );
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
