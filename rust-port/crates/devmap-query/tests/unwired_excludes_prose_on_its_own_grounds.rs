//! A README is not "a language with no import extractor".
//!
//! Two numbers describe the same fact and disagree. Measured on this
//! repository: `devmap status --json` reports `coverage_gaps.import_blind.total
//! = 71`, and the manifest beside it reports
//! `liveness_meta.unwired.excluded_import_blind = 355`. Five times as many, for
//! a question that has one answer.
//!
//! The gap is prose. `extraction_gaps` charges `ImportBlind` only for files a
//! grammar actually read — `a_grammar_read_this_file` draws that line, and its
//! own documentation explains why: charging every `.md`, `.json` and `.yaml`
//! would put 294 of this repository's files into a coverage counter and make
//! the degraded flag permanent, which is a flag carrying no information.
//! `unwired_candidates` never asked. It runs the parse-failure branch first —
//! which `Extraction::is_parse_failure` deliberately answers `false` for prose
//! — and then charges everything left with no `Imports` capability, Markdown
//! included.
//!
//! So the counter documented as "files dropped because **no importer could ever
//! have been seen** — their language has no import extractor in this build"
//! is mostly files that have no imports because they are prose. A reader
//! comparing the two published figures sees a contradiction, and the one that
//! is five times larger is the one on the agent-facing artifact.
//!
//! The exclusion itself must stay. A `README.md` has no inbound `Imports` edge
//! and would otherwise be reported as an unwired candidate — which it was,
//! before the W0.3 gate landed and swept it up for the wrong reason. It is
//! excluded here on its own grounds: nothing read it, it declares nothing to
//! strand, and it was never a candidate.

use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
use devmap_query::code_graph::generate_code_graph_json;
use devmap_query::model::FreshnessInfo;
use devmap_resolve::Resolver;
use serde_json::Value;

/// Two import-blind source files and three files no grammar reads.
///
/// C# and Swift, not the Java and Terraform this test was written with. Both of
/// those gained import extraction in W0.3 move 2 and are no longer import-blind
/// at all, which would have made the assertions below pass for the wrong reason
/// — zero equals zero. C# and Swift are two of the four languages whose
/// exclusion is a documented decision rather than a gap waiting to close, so
/// the fixture cannot rot the same way.
fn corpus() -> Vec<Extraction> {
    vec![
        extract_file(
            "src/Helper.cs",
            "using System;\n\npublic class Helper {\n    public void Run() {}\n}\n",
        ),
        extract_file(
            "Sources/App/Helper.swift",
            "import Foundation\n\nfunc helper() -> Int { return 1 }\n",
        ),
        extract_file("README.md", "# Title\n\nProse, and no imports.\n"),
        extract_file("data/config.json", "{\"a\": 1}\n"),
        extract_file("ci/pipeline.yaml", "steps:\n  - run: make\n"),
    ]
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

fn counter(graph: &Value, key: &str) -> u64 {
    graph["meta"]["devmap_rust"][key]
        .as_u64()
        .unwrap_or_else(|| panic!("{key} must be published: {}", graph["meta"]["devmap_rust"]))
}

/// The premise: prose really does reach this filter, and really does have no
/// `Imports` capability. Stated apart so a change to either would fail here
/// first rather than making the tests below hold for a different reason.
#[test]
fn the_prose_fixtures_are_not_parse_failures_and_declare_no_capability() {
    for path in ["README.md", "data/config.json", "ci/pipeline.yaml"] {
        let extraction = extract_file(path, "x\n");
        assert!(
            !extraction.is_parse_failure(),
            "{path} must not read as a parse failure, or the branch under test \
             is never reached: {:?}",
            extraction.parse_outcome
        );
        assert!(
            !devmap_extract::languages::capabilities_for_language(&extraction.language)
                .contains(devmap_extract::languages::Capability::Imports),
            "{path} must declare no import capability, or it would not be \
             charged"
        );
    }
}

/// The filter's count must be a **subset** of the population's.
///
/// The two are not the same number and are not meant to be.
/// `coverage_gaps.import_blind` is every file whose language has no import
/// extractor; `unwired_excluded_import_blind` is only those the filter actually
/// dropped, which excludes any that returned earlier for being an entry root, a
/// test, vendored, or already imported. Measured on this repository after the
/// fix: 57 against 71, and the 14 are exactly that.
///
/// So `<=` is the law, and it is the law the defect broke in the one direction
/// that cannot be explained away: **355 against 71**. A filter cannot drop more
/// files than exist for it to drop, and that impossibility is what an equality
/// assertion here would have hidden behind a fixture where the two happen to
/// coincide.
#[test]
fn the_import_blind_counter_is_a_subset_of_the_coverage_gap_inventory() {
    let extractions = corpus();
    let coverage = devmap_analyze::extraction_coverage(&extractions);
    let excluded = counter(&graph(&extractions), "unwired_excluded_import_blind");

    assert!(
        excluded <= coverage.import_blind_files as u64,
        "the filter dropped {excluded} files for having no import extractor, \
         but only {} exist — a subset cannot outnumber its population, which \
         is what counting prose here did",
        coverage.import_blind_files
    );

    // On *this* fixture nothing is an entry root, a test or imported, so the
    // two do coincide — pinned separately so the subset law above is not the
    // only thing holding, and so a filter that started dropping nothing at all
    // would still fail.
    assert_eq!(
        excluded, coverage.import_blind_files as u64,
        "every import-blind file in this fixture reaches the filter, so here \
         the subset is the whole population"
    );
}

/// And the population is the source languages, **named** — not merely counted.
///
/// **M5.** This asserted a bare `excluded == 2` on a five-file fixture, which
/// verifies a number and not that the two files are the ones its own message
/// names. Any two of the five reaching the counter would satisfy it, including
/// the README and the JSON that the neighbouring test exists to keep *out* of
/// it: swap the population entirely and the count is unchanged.
///
/// So the identities are checked, by re-deriving which files the filter must
/// have charged from the capability registry rather than by restating the two
/// paths a second time.
#[test]
fn only_source_files_are_charged_as_import_blind() {
    use devmap_extract::languages::Capability;

    let extractions = corpus();
    let excluded = counter(&graph(&extractions), "unwired_excluded_import_blind");

    // Derived: a file is charged exactly when a grammar read it and this build
    // has no import extractor for its language. Both halves matter — the README
    // fails the first and the C# file fails the second.
    let expected: Vec<&str> = extractions
        .iter()
        .filter(|ext| {
            ext.grammar_read_this_file() && !ext.capabilities().contains(Capability::Imports)
        })
        .map(|ext| ext.file_path.as_str())
        .collect();
    assert_eq!(
        expected,
        vec!["src/Helper.cs", "Sources/App/Helper.swift"],
        "fixture assumption: these are the two import-blind source files, and \
         the count below is about them and not about any other two"
    );
    assert_eq!(
        excluded,
        expected.len() as u64,
        "the C# file and the Swift file are import-blind; the README, the JSON \
         and the YAML are not source and were never candidates"
    );

    // The other three are excluded on their own grounds and charged to nothing,
    // which is the claim the bare count could not make.
    for path in ["README.md", "data/config.json", "ci/pipeline.yaml"] {
        let ext = extractions
            .iter()
            .find(|ext| ext.file_path == path)
            .expect("fixture file");
        assert!(
            !ext.grammar_read_this_file(),
            "{path} must be excluded because nothing read it, not because its \
             language lacks an import extractor"
        );
    }
}

/// The exclusion itself must survive: prose stays out of the list.
///
/// Before the W0.3 gate landed, every Markdown, JSON and YAML file in every
/// repository was an unwired candidate, and the capability check swept them up
/// by accident. Removing the accident must not restore the flood.
#[test]
fn prose_is_still_not_an_unwired_candidate() {
    let graph = graph(&corpus());
    let unwired: Vec<&str> = graph["unwired_candidates"]
        .as_array()
        .expect("unwired_candidates array")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    for path in ["README.md", "data/config.json", "ci/pipeline.yaml"] {
        assert!(
            !unwired.contains(&path),
            "{path} is prose and cannot be wired to anything: {unwired:?}"
        );
    }
}

/// A source file whose imports *are* read is still reported when nothing
/// imports it — the filter must not have widened into "exclude everything".
#[test]
fn a_readable_source_file_nothing_imports_is_still_a_candidate() {
    let mut extractions = corpus();
    extractions.push(extract_file(
        "app/orphan.py",
        "def helper():\n    return 1\n",
    ));
    let graph = graph(&extractions);
    let unwired: Vec<&str> = graph["unwired_candidates"]
        .as_array()
        .expect("unwired_candidates array")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(
        unwired.contains(&"app/orphan.py"),
        "Python imports are extracted, so this file's absence from the import \
         graph is a real finding: {unwired:?}"
    );
}
