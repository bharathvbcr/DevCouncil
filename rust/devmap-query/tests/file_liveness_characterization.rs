//! One rule decides which files can ever be called unwired, and this is the
//! table it is held to.
//!
//! Written first against the **unmodified** kernel, where it failed on
//! fourteen of the sixteen rows below. Every one of those was a delete-this
//! suggestion for a file that is reached by something the import graph cannot
//! see: a package marker every submodule import goes through, a shebang
//! executable, a tool config the toolchain names by convention, a fixture
//! tree, or a Terraform file whose unit of use is its directory.
//!
//! The two rows that must survive are the point of the exercise. A predicate
//! that cleared everything would pass a test asserting only "these are gone",
//! so the assertion is set equality against the whole corpus.
//!
//! The pre-fix output, for the record (`unwired_candidates`, 16 entries):
//!
//! ```text
//! Package.swift              build.gradle.kts   docs/conf.py
//! app/__init__.py            examples/demo.py   fixtures/sample.py
//! app/orphan.py              infra/main.tf      infra/vars.tfvars
//! infra/.terraform.lock.hcl  noxfile.py         src/lonely.ts
//! testdata/fixture.py        tools/release.mjs  tools/release.sh
//! vitest.setup.ts
//! ```

use devmap_extract::extract_file;
use devmap_extract::model::{Extraction, FileLiveness, WiringKind};
use devmap_query::code_graph::generate_code_graph_json;
use devmap_query::model::FreshnessInfo;
use devmap_resolve::Resolver;
use serde_json::Value;

/// What `Extraction::file_liveness()` must answer for one probe path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// A liveness candidate: absence of an importer is a real finding.
    Candidate,
    /// Not code in the sense liveness means. Never a candidate, never counted
    /// against a capability gap.
    NotCode,
    /// Code, but reached by something the import graph does not record.
    Exempt(WiringKind),
}

/// The probe corpus, and the verdict each row must get.
///
/// Deliberately mixes the rows that must change with two that must not, and
/// with the prose rows that were already excluded — by accident, through
/// `grammar_read_this_file()`, which answers "which engine ran" and not "is
/// this a liveness candidate". Those rows are here so the replacement rule is
/// shown to keep them out on its own grounds.
#[allow(clippy::type_complexity)]
fn probes() -> Vec<(&'static str, &'static str, Expect)> {
    vec![
        // --- data and prose: no grammar, nothing to strand -----------------
        ("README.md", "# Title\n", Expect::NotCode),
        ("data/config.json", "{\"a\": 1}\n", Expect::NotCode),
        (
            "ci/pipeline.yaml",
            "steps:\n  - run: make\n",
            Expect::NotCode,
        ),
        ("config/settings.toml", "a = 1\n", Expect::NotCode),
        ("web/index.html", "<html></html>\n", Expect::NotCode),
        ("web/app.css", "body { color: red; }\n", Expect::NotCode),
        // A lockfile indexed under a code grammar. `.terraform.lock.hcl` parses
        // as HCL and declares `provider` blocks, so every gate that asks the
        // engine says "a grammar read this" and reports it stranded.
        (
            "infra/.terraform.lock.hcl",
            "provider \"registry.terraform.io/hashicorp/aws\" {\n  version = \"5.0.0\"\n}\n",
            Expect::NotCode,
        ),
        // A lockfile under a data grammar: excluded today for the accidental
        // reason, and here on its own grounds.
        (
            "package-lock.json",
            "{\"lockfileVersion\": 3}\n",
            Expect::NotCode,
        ),
        // Terraform variables are data the directory's modules read.
        (
            "infra/vars.tfvars",
            "region = \"us-east-1\"\n",
            Expect::NotCode,
        ),
        // --- exempt: code reached by something the import graph cannot see --
        (
            "infra/main.tf",
            "resource \"aws_s3_bucket\" \"b\" {}\n",
            Expect::Exempt(WiringKind::DirectoryUnit),
        ),
        (
            "app/__init__.py",
            "\"\"\"Package.\"\"\"\n",
            Expect::Exempt(WiringKind::PackageMarker),
        ),
        (
            "tools/release.sh",
            "#!/usr/bin/env bash\necho hi\n",
            Expect::Exempt(WiringKind::ScriptEntry),
        ),
        (
            "tools/release.mjs",
            "#!/usr/bin/env node\nconsole.log(1);\n",
            Expect::Exempt(WiringKind::ScriptEntry),
        ),
        (
            "testdata/fixture.py",
            "def sample():\n    return 2\n",
            Expect::Exempt(WiringKind::Fixture),
        ),
        (
            "fixtures/sample.py",
            "def sample():\n    return 2\n",
            Expect::Exempt(WiringKind::Fixture),
        ),
        (
            "examples/demo.py",
            "def demo():\n    return 2\n",
            Expect::Exempt(WiringKind::Fixture),
        ),
        (
            "build.gradle.kts",
            "plugins { id(\"java\") }\n",
            Expect::Exempt(WiringKind::ToolConfig),
        ),
        (
            "Package.swift",
            "import PackageDescription\n",
            Expect::Exempt(WiringKind::ToolConfig),
        ),
        (
            "vitest.setup.ts",
            "export const setup = 1;\n",
            Expect::Exempt(WiringKind::ToolConfig),
        ),
        (
            "noxfile.py",
            "def lint(session):\n    pass\n",
            Expect::Exempt(WiringKind::ToolConfig),
        ),
        (
            "docs/conf.py",
            "project = 'x'\n",
            Expect::Exempt(WiringKind::ToolConfig),
        ),
        (
            "src/globals.d.ts",
            "declare const x: number;\n",
            Expect::Exempt(WiringKind::AmbientDeclaration),
        ),
        // Already exempt before this work, through `js_target_root_reason`.
        // Here so the new rules are shown not to have displaced them.
        (
            "vite.config.ts",
            "export default {};\n",
            Expect::Exempt(WiringKind::ToolConfig),
        ),
        // --- the findings that must survive --------------------------------
        (
            "app/orphan.py",
            "def helper():\n    return 1\n",
            Expect::Candidate,
        ),
        (
            "src/lonely.ts",
            "export const lonely = 1;\n",
            Expect::Candidate,
        ),
        // A source language with no import extractor is still a candidate the
        // scan *declines* on capability grounds, not a non-code file — it is
        // counted in `excluded_import_blind`, which this must not absorb.
        (
            "db/schema.sql",
            "SELECT helper(name) FROM widgets;\n",
            Expect::Candidate,
        ),
    ]
}

fn corpus() -> Vec<Extraction> {
    probes()
        .into_iter()
        .map(|(path, source, _)| extract_file(path, source))
        .collect()
}

fn graph(extractions: &[Extraction]) -> Value {
    let mut resolver = Resolver::new();
    resolver.index_extractions(extractions);
    let resolution = resolver.resolve_all(extractions);
    let analysis = devmap_analyze::analyze(extractions, &resolution);
    let json = generate_code_graph_json(
        extractions,
        &analysis,
        &resolution.edges,
        &FreshnessInfo {
            head_sha: "abc123".to_string(),
            generation_id: 7,
            pending_count: 0,
            stamped: Default::default(),
        },
        None,
    )
    .expect("graph renders");
    serde_json::from_str(&json).expect("graph is JSON")
}

/// Every probe path gets the verdict the table declares.
///
/// The unit-level half: one owner answers, so a consumer cannot get a
/// different verdict by asking a different way.
#[test]
fn every_probe_path_gets_its_declared_liveness_verdict() {
    for (path, source, expected) in probes() {
        let extraction = extract_file(path, source);
        let actual = extraction.file_liveness();
        match (expected, &actual) {
            (Expect::Candidate, FileLiveness::Candidate) => {}
            (Expect::NotCode, FileLiveness::NotCode { .. }) => {}
            (Expect::Exempt(kind), FileLiveness::Exempt { kind: got, .. }) => {
                assert_eq!(
                    kind, *got,
                    "{path} is exempt for the wrong reason: expected {kind:?}, got {got:?}"
                );
            }
            _ => panic!("{path}: expected {expected:?}, got {actual:?}"),
        }
    }
}

/// A `NotCode` verdict names its reason, and two different reasons are two
/// different strings.
///
/// The reason is what the manifest histogram is keyed on and what the HTML
/// detail panel shows, so a blank or constant reason turns the whole
/// disclosure into a count with no content.
#[test]
fn a_not_code_verdict_carries_a_reason_that_distinguishes_its_cause() {
    let prose = extract_file("README.md", "# Title\n").file_liveness();
    let lockfile = extract_file("package-lock.json", "{}\n").file_liveness();
    let (FileLiveness::NotCode { reason: prose }, FileLiveness::NotCode { reason: lockfile }) =
        (&prose, &lockfile)
    else {
        panic!("both must be NotCode: {prose:?} / {lockfile:?}");
    };
    assert!(!prose.is_empty(), "a reason must say something");
    assert_ne!(
        prose, lockfile,
        "prose and a lockfile are excluded for different reasons and must not \
         report the same one"
    );
}

/// `unwired_candidates` is exactly the rows the table calls candidates.
///
/// Set equality, not a containment check. "None of these appear" passes for a
/// predicate that excluded the whole corpus, and clearing every finding is the
/// failure mode a liveness relaxation has.
#[test]
fn unwired_candidates_is_exactly_the_candidate_rows() {
    let extractions = corpus();
    let graph = graph(&extractions);
    let actual: Vec<&str> = graph["unwired_candidates"]
        .as_array()
        .expect("unwired_candidates array")
        .iter()
        .filter_map(Value::as_str)
        .collect();

    // `db/schema.sql` is a candidate the scan declines on capability grounds:
    // it never reaches the list, and it is counted in `excluded_import_blind`
    // rather than in either new counter. That distinction is the whole reason
    // the SQL row is in this fixture.
    let expected: Vec<&str> = vec!["app/orphan.py", "src/lonely.ts"];
    assert_eq!(
        actual, expected,
        "the unwired list must hold every real finding and nothing else"
    );
}

/// The exclusions are published, not silently subtracted.
///
/// A filtered list under a bare total is how "we did not look" comes to read
/// as "we looked and found nothing" — the rule this artifact already applies
/// to `excluded_coverage_loss` and `excluded_import_blind`.
#[test]
fn the_new_exclusions_are_counted_and_the_old_counters_are_unchanged() {
    let extractions = corpus();
    let graph = graph(&extractions);
    let meta = &graph["meta"]["devmap_rust"];

    let not_code = meta["unwired_excluded_not_code"]
        .as_u64()
        .expect("unwired_excluded_not_code must be published");
    let exempt = meta["unwired_excluded_exempt"]
        .as_u64()
        .expect("unwired_excluded_exempt must be published");
    let directory_unit = meta["unwired_excluded_directory_unit"]
        .as_u64()
        .expect("unwired_excluded_directory_unit must be published");

    // Nine data rows: six prose/data formats, two lockfiles, one `.tfvars`.
    assert_eq!(not_code, 9, "every data row is counted: {meta}");
    // Terraform's per-file exclusion is counted apart from the rest, because
    // its remedy is different: nothing will ever import one `.tf` file.
    assert_eq!(directory_unit, 1, "one `.tf` file: {meta}");
    // Fourteen exempt rows, of which the `.tf` file is one — it is counted in
    // both, and the sub-count above is what tells them apart.
    assert_eq!(exempt, 14, "every exempt row is counted: {meta}");

    let reasons = meta["unwired_excluded_not_code_reasons"]
        .as_object()
        .expect("the not-code exclusions carry a per-reason histogram");
    assert_eq!(
        reasons.values().filter_map(Value::as_u64).sum::<u64>(),
        not_code,
        "the histogram must account for every excluded file: {reasons:?}"
    );
    assert!(
        reasons.len() > 1,
        "a histogram with one bucket is a count wearing a costume: {reasons:?}"
    );

    // The counters that predate this work keep their meaning. `db/schema.sql`
    // is the one import-blind file in the fixture, and it must not have been
    // absorbed into either new bucket.
    assert_eq!(
        meta["unwired_excluded_import_blind"].as_u64(),
        Some(1),
        "SQL is a source language with no import extractor, and stays charged \
         to the capability counter: {meta}"
    );
    assert_eq!(
        meta["unwired_excluded_import_blind_files"]
            .as_array()
            .and_then(|files| files.first())
            .and_then(Value::as_str),
        Some("db/schema.sql")
    );
    assert_eq!(meta["unwired_excluded_coverage_loss"].as_u64(), Some(0));
}

/// The population adds up: everything is shown, wired, an entry root, or
/// counted in exactly one exclusion bucket.
///
/// Without this, a rule that dropped a file on a path with no counter would
/// look identical to one that never saw the file.
#[test]
fn every_file_is_shown_wired_an_entry_root_or_counted() {
    let extractions = corpus();
    let graph = graph(&extractions);
    let meta = &graph["meta"]["devmap_rust"];

    let shown = graph["unwired_candidates"].as_array().expect("array").len() as u64;
    let counted: u64 = [
        "unwired_excluded_not_code",
        "unwired_excluded_exempt",
        "unwired_excluded_coverage_loss",
        "unwired_excluded_import_blind",
    ]
    .iter()
    .map(|key| meta[*key].as_u64().unwrap_or_default())
    .sum();

    // `directory_unit` is a sub-count of `exempt`, so it is not added again.
    // Nothing in this fixture is wired or an entry root, so the two sides are
    // the whole corpus.
    assert_eq!(
        shown + counted,
        probes().len() as u64,
        "every probe file must be accounted for exactly once: {meta}"
    );
}
