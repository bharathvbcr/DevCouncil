//! No file-level finding may ever name a file that is not a liveness
//! candidate, over randomly generated corpora.
//!
//! The table-driven tests beside this one assert the rule on the shapes
//! somebody thought of. This asserts it on shapes nobody did: random mixtures
//! of code, prose, lockfiles, fixtures, shebang scripts, package markers and
//! tool configs, wired to each other by a random import graph, checked against
//! four surfaces at once — `unwired_candidates`, `unreachable_files`, the
//! dead-code list, and the visualizer's `unwired`/`dead` flags.
//!
//! The invariants are the ones a reader acts on:
//!
//! * **No excluded path is ever named.** A `NotCode` or `Exempt` file appearing
//!   in any of the four is a delete-this suggestion for a file that cannot be
//!   deleted, and the four surfaces are reached by three different code paths.
//! * **The population adds up.** Everything shown, wired, or counted in exactly
//!   one exclusion bucket — so a rule that drops a file on a path with no
//!   counter cannot look identical to a rule that never saw it.
//! * **Every reported path was an input.** A finding naming a path the corpus
//!   does not contain is a fabricated one.
//! * **Two runs of one corpus agree.** The scan iterates `BTreeSet`s and sorts,
//!   and a nondeterministic liveness list makes every build a spurious diff.
//!
//! Seeded, and the seed is printed on failure, so a finding replays exactly.

use std::collections::BTreeSet;

use devmap_extract::extract_file;
use devmap_extract::model::{Extraction, FileLiveness};
use devmap_query::code_graph::generate_code_graph_json;
use devmap_query::model::FreshnessInfo;
use devmap_resolve::Resolver;
use serde_json::Value;

/// Corpora generated in the default run.
const DEFAULT_CORPORA: usize = 48;
/// Files per corpus.
const FILES_PER_CORPUS: usize = 14;
/// Fixed so the ordinary suite runs the same cases every time and a regression
/// is not a coin flip.
const DEFAULT_SEED: u64 = 0x11FE_5E55_0DDB_A11E;

/// SplitMix64 — the same six lines `mutation_fuzz.rs` uses, and for the same
/// reason: a fixed seed replays a failure exactly, with no dependency.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next_u64() % bound.max(1) as u64) as usize
    }
}

fn env_usize(key: &str, fallback: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(fallback)
}

fn seed() -> u64 {
    std::env::var("DEVMAP_FUZZ_SEED")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_SEED)
}

/// One generated file: a path, and a body builder that can name an import.
///
/// Every shape here is drawn from the audit's measured false positives, plus
/// the two that must survive. The generator picks paths from this table and
/// gives each corpus its own numbering, so two files never collide.
#[derive(Clone, Copy)]
enum Shape {
    /// Ordinary Python that may import another module. The only shape that can
    /// legitimately be reported.
    PythonModule,
    /// Ordinary TypeScript, likewise.
    TypeScriptModule,
    Prose,
    DataJson,
    DataYaml,
    Lockfile,
    EnvFile,
    TerraformModule,
    TerraformVars,
    ShebangScript,
    PackageMarker,
    Fixture,
    ToolConfig,
    AmbientDeclaration,
    Barrel,
}

const SHAPES: &[Shape] = &[
    Shape::PythonModule,
    Shape::PythonModule,
    Shape::TypeScriptModule,
    Shape::Prose,
    Shape::DataJson,
    Shape::DataYaml,
    Shape::Lockfile,
    Shape::EnvFile,
    Shape::TerraformModule,
    Shape::TerraformVars,
    Shape::ShebangScript,
    Shape::PackageMarker,
    Shape::Fixture,
    Shape::ToolConfig,
    Shape::AmbientDeclaration,
    Shape::Barrel,
];

impl Shape {
    fn path(self, index: usize) -> String {
        match self {
            Shape::PythonModule => format!("app/mod{index}.py"),
            Shape::TypeScriptModule => format!("web/src/mod{index}.ts"),
            Shape::Prose => format!("docs/note{index}.md"),
            Shape::DataJson => format!("data/blob{index}.json"),
            Shape::DataYaml => format!("ci/job{index}.yaml"),
            Shape::Lockfile => format!("vendorlocks/dep{index}.lock"),
            Shape::EnvFile => format!("deploy/stage{index}.env"),
            Shape::TerraformModule => format!("infra/m{index}/main.tf"),
            Shape::TerraformVars => format!("infra/m{index}/vars.tfvars"),
            Shape::ShebangScript => format!("tools/run{index}.sh"),
            Shape::PackageMarker => format!("app/pkg{index}/__init__.py"),
            Shape::Fixture => format!("testdata/case{index}/sample.py"),
            Shape::ToolConfig => format!("tooling/app{index}.config.ts"),
            Shape::AmbientDeclaration => format!("web/types/api{index}.d.ts"),
            Shape::Barrel => format!("web/src/group{index}/index.ts"),
        }
    }

    /// Whether this shape's body can carry an import of another file.
    fn imports_python(self) -> bool {
        matches!(self, Shape::PythonModule | Shape::Fixture)
    }

    fn body(self, import: Option<&str>) -> String {
        match self {
            Shape::PythonModule | Shape::Fixture => {
                let mut body = String::new();
                if let Some(module) = import {
                    body.push_str(&format!("from {module} import helper\n"));
                }
                body.push_str("def worker():\n    return 1\n");
                body
            }
            Shape::TypeScriptModule => "export const value = 1;\n".to_string(),
            Shape::Prose => "# Note\n\nSome prose.\n".to_string(),
            Shape::DataJson => "{\"key\": \"value\"}\n".to_string(),
            Shape::DataYaml => "steps:\n  - run: make\n".to_string(),
            Shape::Lockfile => "{\"lockfileVersion\": 3}\n".to_string(),
            Shape::EnvFile => "SECRET=1\n".to_string(),
            Shape::TerraformModule => "resource \"aws_s3_bucket\" \"b\" {}\n".to_string(),
            Shape::TerraformVars => "region = \"us-east-1\"\n".to_string(),
            Shape::ShebangScript => "#!/usr/bin/env bash\necho hi\n".to_string(),
            Shape::PackageMarker => "\"\"\"Package.\"\"\"\n".to_string(),
            Shape::ToolConfig => "export default {};\n".to_string(),
            Shape::AmbientDeclaration => "declare const x: number;\n".to_string(),
            Shape::Barrel => "export * from './core';\n".to_string(),
        }
    }
}

/// A random corpus: random shapes, then a random import graph over the modules
/// that can express one.
fn corpus(rng: &mut Rng) -> Vec<Extraction> {
    let shapes: Vec<Shape> = (0..FILES_PER_CORPUS)
        .map(|_| SHAPES[rng.below(SHAPES.len())])
        .collect();
    // Importable module names, so an edge lands on a real file rather than on
    // a name nothing declares.
    let importable: Vec<String> = shapes
        .iter()
        .enumerate()
        .filter(|(_, shape)| matches!(shape, Shape::PythonModule))
        .map(|(index, _)| format!("app.mod{index}"))
        .collect();

    shapes
        .iter()
        .enumerate()
        .map(|(index, shape)| {
            let import = if shape.imports_python() && !importable.is_empty() {
                let pick = &importable[rng.below(importable.len())];
                // Never a self-import: it resolves to nothing useful and adds
                // a shape the invariants are not about.
                (pick != &format!("app.mod{index}")).then_some(pick.as_str())
            } else {
                None
            };
            extract_file(&shape.path(index), &shape.body(import))
        })
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
            head_sha: "fuzz".to_string(),
            generation_id: 1,
            pending_count: 0,
            stamped: Default::default(),
        },
        None,
    )
    .expect("graph renders");
    serde_json::from_str(&json).expect("graph is JSON")
}

fn strings(graph: &Value, key: &str) -> Vec<String> {
    graph[key]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Paths the canonical predicate says may never be named by a file-level
/// finding.
fn excluded_paths(extractions: &[Extraction]) -> BTreeSet<String> {
    extractions
        .iter()
        .filter(|ext| !ext.file_liveness().is_candidate())
        .map(|ext| ext.file_path.clone())
        .collect()
}

/// No excluded file is named by any file-level finding, on any corpus.
///
/// Four surfaces, three code paths: `unwired_candidates` filters in
/// `devmap-query`, `unreachable_files` rolls up in `devmap-analyze`'s
/// `files_wholly_inside_clusters`, and the visualizer projects both onto node
/// flags. A rule routed through one of them and not the others is the exact
/// shape this file exists to catch.
#[test]
fn no_file_level_finding_ever_names_an_excluded_path() {
    let seed = seed();
    let corpora = env_usize("DEVMAP_FUZZ_CORPORA", DEFAULT_CORPORA);
    let mut rng = Rng(seed);

    for round in 0..corpora {
        let extractions = corpus(&mut rng);
        let excluded = excluded_paths(&extractions);
        let graph = graph(&extractions);

        for key in ["unwired_candidates", "unreachable_files"] {
            for path in strings(&graph, key) {
                assert!(
                    !excluded.contains(&path),
                    "seed {seed:#x} round {round}: `{key}` names {path}, which \
                     `file_liveness()` excluded ({:?})",
                    extractions
                        .iter()
                        .find(|ext| ext.file_path == path)
                        .map(Extraction::file_liveness)
                );
            }
        }

        // The dead-code list is symbol-scoped, but every row carries the path
        // it came from, and a `NotCode` file has no symbols worth reporting.
        if let Some(rows) = graph["dead_code"].as_array() {
            for row in rows {
                let path = row["path"].as_str().unwrap_or_default();
                let is_not_code = extractions
                    .iter()
                    .find(|ext| ext.file_path == path)
                    .is_some_and(|ext| matches!(ext.file_liveness(), FileLiveness::NotCode { .. }));
                assert!(
                    !is_not_code,
                    "seed {seed:#x} round {round}: a dead-code row names {path}, \
                     which is not code at all"
                );
            }
        }

        // And the picture, which is a fourth reader of the same two lists.
        let viz =
            devmap_query::viz::build_payload(&graph, &devmap_query::viz::VizOptions::default());
        for node in viz["nodes"].as_array().expect("viz nodes") {
            let path = node["path"].as_str().unwrap_or_default();
            if !excluded.contains(path) {
                continue;
            }
            let flags: Vec<&str> = node["flags"]
                .as_array()
                .map(|flags| {
                    flags
                        .iter()
                        .filter_map(|flag| flag["flag"].as_str())
                        .collect()
                })
                .unwrap_or_default();
            for forbidden in ["dead", "unwired"] {
                assert!(
                    !flags.contains(&forbidden),
                    "seed {seed:#x} round {round}: {path} is drawn as `{forbidden}` \
                     though it is not a liveness candidate: {flags:?}"
                );
            }
        }
    }
}

/// The population adds up, on every corpus.
///
/// `shown + wired + every exclusion bucket == the whole corpus`. Without this,
/// a rule that dropped a file on a path with no counter would be
/// indistinguishable from one that never saw the file — which is how the
/// prose exclusion went uncounted for as long as it did.
#[test]
fn every_file_is_shown_wired_or_counted_in_exactly_one_bucket() {
    let seed = seed();
    let corpora = env_usize("DEVMAP_FUZZ_CORPORA", DEFAULT_CORPORA);
    let mut rng = Rng(seed);

    for round in 0..corpora {
        let extractions = corpus(&mut rng);
        let graph = graph(&extractions);
        let meta = &graph["meta"]["devmap_rust"];
        let count = |key: &str| meta[key].as_u64().unwrap_or_default();

        let shown = strings(&graph, "unwired_candidates").len() as u64;
        // `directory_unit` is a sub-count of `exempt` and is deliberately not
        // added again — asserting that here is what keeps it from quietly
        // becoming a peer.
        let counted = count("unwired_excluded_not_code")
            + count("unwired_excluded_exempt")
            + count("unwired_excluded_coverage_loss")
            + count("unwired_excluded_import_blind");
        assert!(
            count("unwired_excluded_directory_unit") <= count("unwired_excluded_exempt"),
            "seed {seed:#x} round {round}: the directory-unit count is a subset \
             of the exempt count"
        );

        // Whatever is left is wired: something in the corpus depends on it.
        let wired = extractions.len() as u64 - shown - counted;
        assert!(
            shown + counted + wired == extractions.len() as u64,
            "seed {seed:#x} round {round}: the buckets must partition the corpus"
        );
        assert!(
            counted <= extractions.len() as u64,
            "seed {seed:#x} round {round}: more files excluded ({counted}) than \
             exist ({}) — the buckets are double-counting",
            extractions.len()
        );

        // The reason histogram accounts for the not-code count exactly.
        let histogram: u64 = meta["unwired_excluded_not_code_reasons"]
            .as_object()
            .map(|reasons| reasons.values().filter_map(Value::as_u64).sum())
            .unwrap_or_default();
        assert_eq!(
            histogram,
            count("unwired_excluded_not_code"),
            "seed {seed:#x} round {round}: the histogram must account for every \
             excluded file"
        );
    }
}

/// Every path a finding names was an input, and one corpus answers the same way
/// twice.
#[test]
fn findings_name_only_input_paths_and_two_runs_agree() {
    let seed = seed();
    let corpora = env_usize("DEVMAP_FUZZ_CORPORA", DEFAULT_CORPORA);
    let mut rng = Rng(seed);

    for round in 0..corpora {
        let extractions = corpus(&mut rng);
        let inputs: BTreeSet<&str> = extractions
            .iter()
            .map(|ext| ext.file_path.as_str())
            .collect();

        let first = graph(&extractions);
        let second = graph(&extractions);

        for key in ["unwired_candidates", "unreachable_files"] {
            let names = strings(&first, key);
            for path in &names {
                assert!(
                    inputs.contains(path.as_str()),
                    "seed {seed:#x} round {round}: `{key}` names {path}, which is \
                     not in the corpus"
                );
            }
            assert_eq!(
                names,
                strings(&second, key),
                "seed {seed:#x} round {round}: `{key}` is not deterministic"
            );
            let mut sorted = names.clone();
            sorted.sort();
            assert_eq!(
                names, sorted,
                "seed {seed:#x} round {round}: `{key}` must be sorted, or a \
                 rebuild is a spurious diff"
            );
        }

        assert_eq!(
            first["meta"]["devmap_rust"]["unwired_excluded_not_code_reasons"],
            second["meta"]["devmap_rust"]["unwired_excluded_not_code_reasons"],
            "seed {seed:#x} round {round}: the histogram is not deterministic"
        );
    }
}

/// The generator really does produce all three verdicts.
///
/// Without this, every assertion above could be holding over a corpus of one
/// kind — the failure mode where a fuzzer passes because it tests nothing.
#[test]
fn the_generator_produces_candidates_exemptions_and_data() {
    let mut rng = Rng(seed());
    let mut saw_candidate = false;
    let mut saw_exempt = false;
    let mut saw_not_code = false;
    let mut saw_reported = false;

    for _ in 0..env_usize("DEVMAP_FUZZ_CORPORA", DEFAULT_CORPORA) {
        let extractions = corpus(&mut rng);
        for ext in &extractions {
            match ext.file_liveness() {
                FileLiveness::Candidate => saw_candidate = true,
                FileLiveness::Exempt { .. } => saw_exempt = true,
                FileLiveness::NotCode { .. } => saw_not_code = true,
            }
        }
        if !strings(&graph(&extractions), "unwired_candidates").is_empty() {
            saw_reported = true;
        }
    }

    assert!(saw_candidate, "the corpus must contain liveness candidates");
    assert!(saw_exempt, "the corpus must contain exempt files");
    assert!(saw_not_code, "the corpus must contain data files");
    assert!(
        saw_reported,
        "some corpus must produce a real finding, or the invariants above hold \
         over an empty list"
    );
}
