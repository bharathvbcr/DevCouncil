//! One rule, three artifacts, one `devmap build` of a real tree.
//!
//! Everything else about file liveness is asserted in-process against
//! hand-built extractions. That is the right level for the rules themselves and
//! it cannot see the wiring: `repo_map.json`, `code_graph.json` and the HTML
//! payload are written by three different functions, and the reason this work
//! exists at all is that three surfaces were each deciding separately which
//! files could be called dead.
//!
//! So this walks a probe tree from disk exactly as an agent's `devmap build`
//! does — discovery, extraction, the cache key, the store, the artifact
//! writers — and asserts the same verdict arrives in all three. It is also the
//! only test here that exercises discovery, which is what proves a `.env` never
//! reaches the graph in the first place rather than being excluded once it has.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

fn temp_root(tag: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock must be after epoch")
        .as_nanos();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "devmap-file-liveness-{tag}-{}-{stamp}-{seq}",
        std::process::id()
    ))
}

/// The probe layout: one file of each measured false-positive class, two files
/// that must stay findings, and one wired pair so the graph is not degenerate.
const FIXTURE: &[(&str, &str)] = &[
    // Wired: `main.py` imports `core.py`, so neither is a finding.
    (
        "app/main.py",
        "from app.core import helper\n\n\nif __name__ == \"__main__\":\n    helper()\n",
    ),
    ("app/core.py", "def helper():\n    return 1\n"),
    // The two that must survive as findings.
    ("app/orphan.py", "def stranded():\n    return 1\n"),
    ("web/src/lonely.ts", "export const lonely = 1;\n"),
    // The classes that must stop being findings.
    ("app/__init__.py", "\"\"\"The app package.\"\"\"\n"),
    ("tools/release.sh", "#!/usr/bin/env bash\necho release\n"),
    ("testdata/case/sample.py", "def sample():\n    return 2\n"),
    (
        "noxfile.py",
        "def lint(session):\n    session.run(\"ruff\")\n",
    ),
    ("web/vite.config.ts", "export default {};\n"),
    ("web/types/api.d.ts", "declare const api: number;\n"),
    ("infra/main.tf", "resource \"aws_s3_bucket\" \"b\" {}\n"),
    ("infra/vars.tfvars", "region = \"us-east-1\"\n"),
    (
        "infra/.terraform.lock.hcl",
        "provider \"registry.terraform.io/hashicorp/aws\" {\n  version = \"5.0.0\"\n}\n",
    ),
    // Data that was already excluded, for the accidental reason.
    ("README.md", "# Probe\n\nProse.\n"),
    ("ci/pipeline.yaml", "steps:\n  - run: make\n"),
    ("package-lock.json", "{\"lockfileVersion\": 3}\n"),
    // Never indexed at all, which is the first of the two bounds.
    ("deploy/staging.env", "SECRET=1\n"),
];

/// Paths that must never appear in a file-level finding, with why.
const MUST_NOT_BE_A_FINDING: &[(&str, &str)] = &[
    ("app/__init__.py", "every submodule import runs it"),
    ("tools/release.sh", "the shebang says something executes it"),
    ("testdata/case/sample.py", "fixture data, not program text"),
    ("noxfile.py", "nox finds it by name"),
    ("web/vite.config.ts", "the bundler finds it by name"),
    (
        "web/types/api.d.ts",
        "an ambient declaration is never imported",
    ),
    (
        "infra/main.tf",
        "the Terraform unit of use is the directory",
    ),
    ("infra/vars.tfvars", "variable values are data"),
    ("infra/.terraform.lock.hcl", "a lockfile that parses as HCL"),
    ("README.md", "prose"),
    ("ci/pipeline.yaml", "data"),
    ("package-lock.json", "a lockfile"),
    ("deploy/staging.env", "never indexed at all"),
];

/// Paths a finding must still name, or the rule has swallowed the repository.
const MUST_STAY_A_FINDING: &[&str] = &["app/orphan.py", "web/src/lonely.ts"];

fn write_fixture(root: &Path) {
    for (path, source) in FIXTURE {
        let full = root.join(path);
        fs::create_dir_all(full.parent().expect("a fixture path has a parent"))
            .expect("create fixture directory");
        fs::write(&full, source).expect("write fixture file");
    }
}

fn run(root: &Path, db: &Path, args: &[&str]) -> std::process::Output {
    let output = Command::new(env!("CARGO_BIN_EXE_devmap"))
        .current_dir(root)
        .args(["--json", "--db"])
        .arg(db)
        .args(args)
        .output()
        .expect("run devmap");
    assert!(
        output.status.success(),
        "devmap {args:?} failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn strings(value: &Value, key: &str) -> Vec<String> {
    value[key]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

struct Built {
    map: Value,
    graph: Value,
    html: String,
}

fn build_probe(tag: &str) -> Built {
    let root = temp_root(tag);
    fs::create_dir_all(&root).expect("create probe root");
    write_fixture(&root);
    let db = root.join("index.sqlite");
    run(&root, &db, &["build", root.to_str().expect("utf-8 root")]);
    run(
        &root,
        &db,
        &["manifest", root.to_str().expect("utf-8 root")],
    );

    let html_path = root.join("graph.html");
    run(
        &root,
        &db,
        &[
            "html",
            root.to_str().expect("utf-8 root"),
            "--out",
            html_path.to_str().expect("utf-8 out"),
        ],
    );

    Built {
        map: serde_json::from_str(
            &fs::read_to_string(devmap_extract::paths::repo_map_path(&root))
                .expect("repo_map.json is written"),
        )
        .expect("repo_map.json parses"),
        graph: serde_json::from_str(
            &fs::read_to_string(devmap_extract::paths::code_graph_path(&root))
                .expect("code_graph.json is written"),
        )
        .expect("code_graph.json parses"),
        html: fs::read_to_string(&html_path).expect("graph.html is written"),
    }
}

/// The two artifacts agree, and neither names a file that is not a candidate.
#[test]
fn neither_artifact_names_a_file_that_is_not_a_liveness_candidate() {
    let built = build_probe("artifacts");

    for (artifact, value, key) in [
        ("repo_map.json", &built.map, "unwired_candidates"),
        ("code_graph.json", &built.graph, "unwired_candidates"),
        ("repo_map.json", &built.map, "unreachable_files"),
        ("code_graph.json", &built.graph, "unreachable_files"),
    ] {
        let reported = strings(value, key);
        for (path, why) in MUST_NOT_BE_A_FINDING {
            assert!(
                !reported.contains(&(*path).to_string()),
                "{artifact} `{key}` names {path}, and {why}: {reported:?}"
            );
        }
    }

    // And the findings that must survive really do, in both artifacts. Without
    // this the assertions above are satisfied by a rule that reported nothing.
    for artifact in [&built.map, &built.graph] {
        let reported = strings(artifact, "unwired_candidates");
        for path in MUST_STAY_A_FINDING {
            assert!(
                reported.contains(&(*path).to_string()),
                "{path} is a stranded module and must still be reported: {reported:?}"
            );
        }
    }
    assert_eq!(
        strings(&built.map, "unwired_candidates"),
        strings(&built.graph, "unwired_candidates"),
        "the two artifacts of one build must not disagree about one repository"
    );
}

/// A `.env` is never indexed, so the question is never asked of it.
///
/// The first of the two bounds: discovery declines it before extraction, and
/// `non_code_path_reason` is what catches the one that arrives by some other
/// route. A test that only asserted the second would pass on a build that had
/// put credentials into the graph.
#[test]
fn an_environment_file_never_enters_the_graph_at_all() {
    let built = build_probe("env");
    let indexed: Vec<&str> = built.graph["nodes"]
        .as_array()
        .expect("nodes array")
        .iter()
        .filter_map(|node| node["path"].as_str())
        .collect();
    assert!(
        !indexed.contains(&"deploy/staging.env"),
        "an environment file must not be a node in the graph at all"
    );
    assert!(
        indexed.contains(&"app/core.py"),
        "the fixture must otherwise have been indexed: {indexed:?}"
    );
}

/// The exclusions are published in both artifacts, with a reason histogram.
#[test]
fn both_artifacts_publish_the_new_exclusion_counters() {
    let built = build_probe("counters");

    let map_unwired = &built.map["liveness_meta"]["unwired"];
    let graph_meta = &built.graph["meta"]["devmap_rust"];

    for (label, not_code, exempt, directory_unit) in [
        (
            "repo_map.json",
            &map_unwired["excluded_not_code"],
            &map_unwired["excluded_exempt"],
            &map_unwired["excluded_directory_unit"],
        ),
        (
            "code_graph.json",
            &graph_meta["unwired_excluded_not_code"],
            &graph_meta["unwired_excluded_exempt"],
            &graph_meta["unwired_excluded_directory_unit"],
        ),
    ] {
        assert!(
            not_code.as_u64().is_some_and(|count| count > 0),
            "{label} must count the data files it excluded: {not_code}"
        );
        assert!(
            exempt.as_u64().is_some_and(|count| count > 0),
            "{label} must count the exempt files it excluded: {exempt}"
        );
        assert_eq!(
            directory_unit.as_u64(),
            Some(1),
            "{label}: one `.tf` file in the probe tree"
        );
    }

    // The counters agree across the two artifacts, which is the property the
    // whole "one owner" change exists to produce.
    assert_eq!(
        map_unwired["excluded_not_code"],
        graph_meta["unwired_excluded_not_code"]
    );
    assert_eq!(
        map_unwired["excluded_exempt"],
        graph_meta["unwired_excluded_exempt"]
    );

    let reasons = map_unwired["excluded_not_code_reasons"]
        .as_object()
        .expect("the not-code exclusions carry a per-reason histogram");
    assert_eq!(
        reasons.values().filter_map(Value::as_u64).sum::<u64>(),
        map_unwired["excluded_not_code"]
            .as_u64()
            .unwrap_or_default(),
        "the histogram must account for every excluded file: {reasons:?}"
    );
    // A lockfile and prose are different exclusions and must not share a
    // bucket, or the histogram is a count wearing a costume.
    assert!(
        reasons.len() >= 2,
        "the probe tree holds prose, a lockfile and Terraform variables: {reasons:?}"
    );

    // The counters that predate this work keep their keys and their meaning.
    for key in ["excluded_coverage_loss", "excluded_import_blind"] {
        assert!(
            map_unwired[key].as_u64().is_some(),
            "{key} must still be published for GitPulse's RepoMapUnwiredMeta"
        );
    }
}

/// The file nodes carry a liveness verdict, and the picture can read it.
#[test]
fn file_nodes_carry_a_liveness_verdict_the_page_can_render() {
    let built = build_probe("viz");

    let mut seen = std::collections::BTreeMap::new();
    for node in built.graph["nodes"].as_array().expect("nodes array") {
        if node["kind"] != "file" {
            continue;
        }
        let path = node["path"].as_str().unwrap_or_default().to_string();
        let liveness = node["extras"]["liveness"]
            .as_str()
            .unwrap_or_else(|| panic!("every file node carries a liveness verdict: {node}"));
        assert!(
            ["not_applicable", "exempt", "candidate"].contains(&liveness),
            "{path}: unknown liveness value {liveness:?}"
        );
        seen.insert(path, liveness.to_string());
    }

    assert_eq!(
        seen.get("README.md").map(String::as_str),
        Some("not_applicable")
    );
    assert_eq!(
        seen.get("app/__init__.py").map(String::as_str),
        Some("exempt")
    );
    assert_eq!(
        seen.get("app/orphan.py").map(String::as_str),
        Some("candidate")
    );
    assert_eq!(
        seen.get("infra/main.tf").map(String::as_str),
        Some("exempt")
    );

    // An exempt or excluded node explains itself. A three-valued label with no
    // reason is a category a reader cannot check.
    for node in built.graph["nodes"].as_array().expect("nodes array") {
        if node["kind"] != "file" || node["extras"]["liveness"] == "candidate" {
            continue;
        }
        let reason = node["extras"]["liveness_reason"]
            .as_str()
            .unwrap_or_default();
        assert!(
            !reason.is_empty(),
            "{}: an exclusion must carry its reason",
            node["path"]
        );
    }

    // And the page renders the category rather than leaving a lone data dot
    // unexplained beside a legend that names "dead" and "unwired".
    assert!(
        built.html.contains("not a liveness candidate"),
        "the legend must name the category"
    );
    assert!(
        built.html.contains("not_applicable"),
        "the payload must carry the verdict the legend refers to"
    );
}
