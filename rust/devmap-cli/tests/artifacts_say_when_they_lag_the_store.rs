//! `devmap status --json` says whether `repo_map.json` and `code_graph.json`
//! were written from the store's current generation.
//!
//! A plain `devmap build` advances the store and writes neither file, so a
//! consumer that reads the files directly (GitPulse checks only that they
//! exist) used to be handed an older generation as though it were current.
//! The answer is read from the sidecar next to the store, never from the files.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn devmap() -> String {
    env!("CARGO_BIN_EXE_devmap").to_string()
}

fn run(root: &Path, args: &[&str]) -> Output {
    let output = Command::new(devmap())
        .args(args)
        .current_dir(root)
        .output()
        .unwrap_or_else(|error| panic!("devmap {args:?}: {error}"));
    assert!(
        output.status.success(),
        "devmap {args:?} failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn json(root: &Path, args: &[&str]) -> serde_json::Value {
    serde_json::from_slice(&run(root, args).stdout)
        .unwrap_or_else(|error| panic!("devmap {args:?} printed no JSON: {error}"))
}

fn git(root: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn fixture(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "devmap-artifact-currency-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    for file in 0..4 {
        std::fs::write(
            root.join(format!("src/m{file}.py")),
            format!("def f{file}(value):\n    return value + {file}\n"),
        )
        .unwrap();
    }
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "t@example.invalid"]);
    git(&root, &["config", "user.name", "t"]);
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "fixture"]);
    root
}

fn artifacts(status: &serde_json::Value) -> &serde_json::Value {
    status
        .get("artifacts")
        .unwrap_or_else(|| panic!("status carries no `artifacts` object: {status}"))
}

#[test]
fn a_plain_build_leaves_the_artifacts_reported_as_older_than_the_store() {
    let root = fixture("lag");
    run(&root, &["build", ".", "--manifest", "--force"]);

    let current = json(&root, &["status", "--json"]);
    let generation = current["generation_id"].as_u64().unwrap();
    let report = artifacts(&current);
    assert_eq!(report["current"], true, "{report}");
    assert_eq!(report["written_from_generation"], generation, "{report}");
    assert_eq!(report["store_generation"], generation, "{report}");
    assert_eq!(report["reason"], serde_json::Value::Null, "{report}");

    // The sidecar's generation is the one the artifact itself declares.
    let map_path = devmap_extract::paths::repo_map_path(&root);
    let map: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&map_path).unwrap()).unwrap();
    assert_eq!(map["freshness"]["generation_id"], generation);

    // One file changes and the store advances without `--manifest`: the files on
    // disk still say the old generation, and status must say they lag.
    std::fs::write(
        root.join("src/m1.py"),
        "def f1(value):\n    return value * 2\n\n\ndef added(value):\n    return value\n",
    )
    .unwrap();
    run(&root, &["build", "."]);
    let lagging = json(&root, &["status", "--json"]);
    let advanced = lagging["generation_id"].as_u64().unwrap();
    assert!(
        advanced > generation,
        "the plain build must add a generation"
    );
    let report = artifacts(&lagging);
    assert_eq!(report["current"], false, "{report}");
    assert_eq!(report["written_from_generation"], generation, "{report}");
    assert_eq!(report["store_generation"], advanced, "{report}");
    let reason = report["reason"].as_str().expect("a lag names its reason");
    assert!(
        reason.contains(&generation.to_string()) && reason.contains(&advanced.to_string()),
        "{reason}"
    );
    // Reporting a lag must not rewrite what lags.
    let map_after: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&map_path).unwrap()).unwrap();
    assert_eq!(map_after["freshness"]["generation_id"], generation);

    // Asking for the artifacts catches them up.
    run(&root, &["build", ".", "--manifest", "--force"]);
    let caught_up = json(&root, &["status", "--json"]);
    assert_eq!(artifacts(&caught_up)["current"], true, "{caught_up}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn artifacts_with_no_stamp_are_never_reported_current() {
    let root = fixture("nostamp");
    run(&root, &["build", ".", "--manifest", "--force"]);
    let db = devmap_extract::paths::store_path(&root);
    let sidecar = PathBuf::from(format!("{}.artifacts.json", db.display()));
    assert!(sidecar.is_file(), "the build writes its stamp");
    std::fs::remove_file(&sidecar).unwrap();

    let status = json(&root, &["status", "--json"]);
    let report = artifacts(&status);
    assert_eq!(report["current"], false, "{report}");
    assert!(
        report["reason"]
            .as_str()
            .unwrap()
            .contains("no artifact stamp"),
        "{report}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_manifest_build_times_each_phase_of_the_artifact_write() {
    let root = fixture("spans");
    let build = json(&root, &["build", ".", "--manifest", "--force", "--json"]);
    let stages = build["timings"]["stages"].as_array().unwrap();
    let artifacts_stage = stages
        .iter()
        .find(|stage| stage["stage"] == "writing consumer artifacts")
        .expect("the artifact stage is timed");
    let labels: Vec<&str> = artifacts_stage["sub"]
        .as_array()
        .expect("the artifact stage breaks itself down")
        .iter()
        .map(|sub| sub["stage"].as_str().unwrap())
        .collect();
    for wanted in [
        "artifacts:status",
        "artifacts:freshness",
        "artifacts:load",
        "artifacts:render",
        "artifacts:write",
        "artifacts:stamp_write",
    ] {
        assert!(labels.contains(&wanted), "{wanted} missing from {labels:?}");
    }
    // The payload's own status is the one the write took, and says the same.
    assert_eq!(
        build["manifest"]["status"]["generation_id"],
        build["manifest"]["generation_id"]
    );
    assert_eq!(build["manifest"]["status"]["artifacts"]["current"], true);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn build_repo_map_refreshes_repo_map_without_rewriting_code_graph() {
    let root = fixture("repo-map-only");
    run(&root, &["build", ".", "--manifest", "--force"]);

    let initial = json(&root, &["status", "--json"]);
    let gen1 = initial["generation_id"].as_u64().unwrap();
    let map_path = devmap_extract::paths::repo_map_path(&root);
    let graph_path = devmap_extract::paths::code_graph_path(&root);
    let graph_bytes_before = std::fs::read(&graph_path).unwrap();
    let graph_meta_before = std::fs::metadata(&graph_path).unwrap();

    // Edit one file and rebuild with `--repo-map`: repo_map advances, code_graph stays untouched.
    std::fs::write(
        root.join("src/m1.py"),
        "def f1(value):\n    return value * 3\n\n\ndef extra(value):\n    return value + 10\n",
    )
    .unwrap();
    let repo_map_build = json(&root, &["build", ".", "--repo-map", "--json"]);
    assert_eq!(
        repo_map_build["manifest"]["artifacts_unchanged"], false,
        "{repo_map_build}"
    );
    assert!(
        repo_map_build["manifest"]["graph_output"].is_null(),
        "expected --repo-map to skip code_graph output: {repo_map_build}"
    );
    assert!(
        repo_map_build["manifest"]["compact_graph_output"].is_null(),
        "expected --repo-map to skip compact graph output: {repo_map_build}"
    );

    let after = json(&root, &["status", "--json"]);
    let gen2 = after["generation_id"].as_u64().unwrap();
    assert!(gen2 > gen1, "expected store to advance: {gen1} -> {gen2}");

    // repo_map.json updated to gen2; code_graph.json was NOT rewritten.
    let map_after: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&map_path).unwrap()).unwrap();
    assert_eq!(map_after["freshness"]["generation_id"], gen2);
    assert_eq!(std::fs::read(&graph_path).unwrap(), graph_bytes_before);
    let graph_meta_after = std::fs::metadata(&graph_path).unwrap();
    assert_eq!(
        graph_meta_after.modified().unwrap(),
        graph_meta_before.modified().unwrap(),
        "code_graph.json mtime must not change under --repo-map"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(
            graph_meta_after.ino(),
            graph_meta_before.ino(),
            "code_graph.json inode must not change under --repo-map"
        );
    }

    // Status fails closed on overall currency while reporting per-artifact generations.
    let report = artifacts(&after);
    assert_eq!(report["current"], false, "{report}");
    assert_eq!(report["written_from_generation"], gen1, "{report}");
    assert_eq!(report["store_generation"], gen2, "{report}");
    assert_eq!(report["lag_generations"], gen2 - gen1, "{report}");
    assert_eq!(report["output_generations"]["repo_map"], gen2, "{report}");
    assert_eq!(report["output_generations"]["code_graph"], gen1, "{report}");
    let reason = report["reason"].as_str().unwrap();
    assert!(reason.contains("code_graph"), "{reason}");
    assert!(reason.contains("repo_map is current"), "{reason}");

    // A second `--repo-map` run with no source change skips rewriting repo_map.json.
    let second = json(&root, &["build", ".", "--repo-map", "--json"]);
    assert_eq!(second["manifest"]["artifacts_unchanged"], true, "{second}");

    // Standalone `devmap manifest --repo-map-only` also skips when repo_map is already current.
    let standalone = json(&root, &["manifest", ".", "--repo-map-only", "--json"]);
    assert_eq!(standalone["artifacts_unchanged"], true, "{standalone}");
    assert!(standalone["graph_output"].is_null(), "{standalone}");

    // Full `--manifest` notices code_graph still lags and brings all artifacts current.
    let full = json(&root, &["build", ".", "--manifest", "--json"]);
    assert_eq!(full["manifest"]["artifacts_unchanged"], false, "{full}");
    assert!(full["manifest"]["graph_output"].is_string(), "{full}");
    let caught_up = json(&root, &["status", "--json"]);
    let report = artifacts(&caught_up);
    assert_eq!(report["current"], true, "{report}");
    assert_eq!(report["lag_generations"], 0, "{report}");
    assert_eq!(report["output_generations"]["repo_map"], gen2, "{report}");
    assert_eq!(report["output_generations"]["code_graph"], gen2, "{report}");

    let _ = std::fs::remove_dir_all(&root);
}
