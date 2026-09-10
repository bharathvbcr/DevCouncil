use serde_json::Value;
#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

const CONTRACT: &[u8] = include_bytes!("../../dc-evidence/fixtures/v1/contract.json");
const BUNDLE: &[u8] = include_bytes!("../../dc-evidence/fixtures/v1/bundle.json");
const NOTE: &[u8] = include_bytes!("../../dc-evidence/fixtures/v1/note.txt");

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "dc-evidence-cli-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("contract.json"), CONTRACT).unwrap();
        std::fs::write(root.join("bundle.json"), BUNDLE).unwrap();
        std::fs::write(root.join("note.txt"), NOTE).unwrap();
        Self(root)
    }

    fn command(&self) -> Command {
        let expected: Value = serde_json::from_slice(include_bytes!(
            "../../dc-evidence/fixtures/v1/expected.json"
        ))
        .unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_dcverify"));
        command
            .arg("evidence-check")
            .arg("--contract")
            .arg(self.0.join("contract.json"))
            .arg("--bundle")
            .arg(self.0.join("bundle.json"))
            .arg("--artifacts-root")
            .arg(&self.0)
            .arg("--expected-contract-sha256")
            .arg(expected["contract_sha256"].as_str().unwrap())
            .arg("--expected-capability-sha256")
            .arg(expected["capability_sha256"].as_str().unwrap())
            .args([
                "--expected-run-id",
                "run-1",
                "--expected-session-id",
                "desktop-1",
                "--expected-epoch",
                "1",
            ]);
        command
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn decoded(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| panic!("{e}: {:?}", output.stdout))
}

#[test]
fn evidence_capability_is_additive_to_legacy_health() {
    let output = Command::new(env!("CARGO_BIN_EXE_dcverify"))
        .arg("health")
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("\"schema_version\":1"), "{stdout}");
    assert!(
        stdout.contains("\"evidence_schema_versions\":[1]"),
        "{stdout}"
    );
}

#[test]
fn live_cli_reads_real_fixture_artifacts_and_emits_one_bound_report() {
    let fixture = Fixture::new();
    let output = fixture.command().output().unwrap();
    assert!(output.status.success(), "{:?}", output);
    let report = decoded(&output);
    assert_eq!(report["ok"], true);
    assert_eq!(report["verdict"], "passed");
    assert_eq!(report["bundle_sha256"], dc_evidence::sha256(BUNDLE));
    assert_eq!(report["criteria"][0]["verdict"], "passed");
}

#[test]
fn live_cli_accepts_expected_not_found_business_outcome() {
    let fixture = Fixture::new();
    let contract = include_bytes!("../../dc-evidence/fixtures/v1/contract-not-found.json");
    let bundle = include_bytes!("../../dc-evidence/fixtures/v1/bundle-not-found.json");
    std::fs::write(fixture.0.join("contract.json"), contract).unwrap();
    std::fs::write(fixture.0.join("bundle.json"), bundle).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_dcverify"));
    command
        .arg("evidence-check")
        .arg("--contract")
        .arg(fixture.0.join("contract.json"))
        .arg("--bundle")
        .arg(fixture.0.join("bundle.json"))
        .arg("--artifacts-root")
        .arg(&fixture.0)
        .arg("--expected-contract-sha256")
        .arg(dc_evidence::sha256(contract))
        .arg("--expected-capability-sha256")
        .arg("1a4bf7dac3b78318718a5047937473da4898acfb53cbbc9afdbe0c7fa9f3eec2")
        .args([
            "--expected-run-id",
            "run-1",
            "--expected-session-id",
            "desktop-1",
            "--expected-epoch",
            "1",
        ]);
    let output = command.output().unwrap();
    assert!(output.status.success(), "{:?}", output);
    let report = decoded(&output);
    assert_eq!(report["ok"], true);
    assert_eq!(report["verdict"], "passed");
    assert_eq!(report["criteria"][0]["id"], "outcome-id");
    assert_eq!(report["criteria"][0]["verdict"], "passed");
}

#[test]
fn corrupt_artifact_is_failed_but_missing_artifact_is_incomplete() {
    let fixture = Fixture::new();
    std::fs::write(fixture.0.join("note.txt"), b"Different content").unwrap();
    let output = fixture.command().output().unwrap();
    assert!(output.status.success());
    assert_eq!(decoded(&output)["verdict"], "failed");
    std::fs::remove_file(fixture.0.join("note.txt")).unwrap();
    let output = fixture.command().output().unwrap();
    assert!(output.status.success());
    assert_eq!(decoded(&output)["verdict"], "incomplete");
}

#[test]
fn contract_substitution_fails_even_with_identical_json_semantics() {
    let fixture = Fixture::new();
    let mut bytes = CONTRACT.to_vec();
    bytes.push(b' ');
    std::fs::write(fixture.0.join("contract.json"), bytes).unwrap();
    assert_eq!(
        decoded(&fixture.command().output().unwrap())["verdict"],
        "failed"
    );
}

#[test]
fn unsupported_missing_and_duplicate_inputs_are_json_errors() {
    for args in [
        vec!["evidence-check"],
        vec!["evidence-check", "--runner-passed", "true"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_dcverify"))
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert_eq!(decoded(&output)["ok"], false);
    }
    let fixture = Fixture::new();
    let output = fixture
        .command()
        .args(["--expected-epoch", "2"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(decoded(&output)["ok"], false);
}

#[test]
fn malformed_or_oversized_bundle_does_not_become_an_empty_success() {
    let fixture = Fixture::new();
    for bytes in [
        b"{}".to_vec(),
        vec![b' '; dc_evidence::MAX_BUNDLE_BYTES + 1],
    ] {
        std::fs::write(fixture.0.join("bundle.json"), bytes).unwrap();
        let output = fixture.command().output().unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert_eq!(decoded(&output)["ok"], false);
    }
}

#[test]
fn legacy_empty_diff_check_remains_compatible() {
    let output = Command::new(env!("CARGO_BIN_EXE_dcverify"))
        .arg("check")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(decoded(&output)["ok"], true);
    assert_eq!(decoded(&output)["files"], 0);
}

#[cfg(unix)]
#[test]
fn symlink_artifacts_and_directories_are_unavailable_without_following_them() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let external = Fixture::new();
    std::fs::remove_file(fixture.0.join("note.txt")).unwrap();
    symlink(external.0.join("note.txt"), fixture.0.join("note.txt")).unwrap();
    assert_eq!(
        decoded(&fixture.command().output().unwrap())["verdict"],
        "incomplete"
    );
    std::fs::remove_file(fixture.0.join("note.txt")).unwrap();
    symlink(&external.0, fixture.0.join("nested")).unwrap();
    rewrite_artifact_path(&fixture.0, "nested/note.txt");
    assert_eq!(
        decoded(&fixture.command().output().unwrap())["verdict"],
        "incomplete"
    );
}

#[cfg(unix)]
fn rewrite_artifact_path(root: &Path, path: &str) {
    let mut bundle: Value = serde_json::from_slice(BUNDLE).unwrap();
    bundle["artifacts"][0]["path"] = path.into();
    std::fs::write(
        root.join("bundle.json"),
        serde_json::to_vec(&bundle).unwrap(),
    )
    .unwrap();
}

#[cfg(unix)]
#[test]
fn fifo_is_refused_before_opening_or_waiting_for_a_writer() {
    let fixture = Fixture::new();
    std::fs::remove_file(fixture.0.join("note.txt")).unwrap();
    assert!(
        Command::new("mkfifo")
            .arg(fixture.0.join("note.txt"))
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(
        decoded(&fixture.command().output().unwrap())["verdict"],
        "incomplete"
    );
}
