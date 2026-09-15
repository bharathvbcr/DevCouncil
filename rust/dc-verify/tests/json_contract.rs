//! The `--json` stdout contract, driven against the real DevCouncil host CLI.
//!
//! `json_stdout`'s unit tests prove the scanner is right about strings. This is
//! the gate that matters: it runs every invocation in
//! `json_stdout_manifest.tsv` as a real process against a freshly-created
//! throwaway project, and requires each one's stdout to hold exactly one JSON
//! value.
//!
//! It exists because no in-process test can prove this. The Python `CliRunner`
//! this replaced merged the two streams, so a banner that leaked onto stdout
//! was indistinguishable there from one that went to stderr — which is how
//! `dev cost budget --json --set 5.00` shipped emitting unparseable stdout with
//! a green test suite. The Go host's own handler tests swap `os.Stdout`, which
//! is closer but still cannot see what `console.Session` renders around the
//! handler, nor what a child process writes. Two real file descriptors settle
//! it.
//!
//! Running the manifest is `dcjsoncheck`'s job, not this file's: that binary
//! already owns the bounded runner — a deadline, capped capture, both pipes
//! drained, and a kill that reaches the process group — and ships in this same
//! crate. This test resolves the host binary, scaffolds the project, and reads
//! the verdict.
//!
//! There is no skip. Until 2026-09-14 this file called `skip_or_fail` when it
//! could not scaffold a project, and turning that into a failure needed
//! `DC_JSON_REQUIRE_CLI=1`, which no workflow ever set. The manifest had named
//! a Python module deleted in 3286db5, so the gate had been running nothing and
//! reporting `ok`. A check that could not run must never report what a check
//! that ran and passed reports — so a missing binary, an unscaffoldable
//! project, or a harness that could not be started all fail here.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The DevCouncil checkout this crate is part of: `<repo>/rust/dc-verify`, so
/// the repository root is two levels up. Resolved by depth rather than by name
/// so a worktree or a differently-named clone still finds it.
fn devcouncil_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate is at <repo>/rust/dc-verify, so it has two ancestors")
}

/// Where a plain `go build -o bin/ ./cmd/devcouncil` leaves the host binary.
/// `backend/go_orchestrator/.gitignore` already ignores `bin/`.
fn built_host() -> PathBuf {
    devcouncil_root()
        .join("backend/go_orchestrator/bin")
        .join(format!("devcouncil{}", std::env::consts::EXE_SUFFIX))
}

/// The host binary to audit: `DEVCOUNCIL_BIN`, or this checkout's own build.
///
/// Deliberately *not* `PATH`. An installed `devcouncil` from a release is a
/// different program from the one in this working tree, and a contract gate
/// that silently audits someone else's binary reports a verdict about code that
/// was never changed. Absent means fail, with the command that fixes it.
fn host_binary() -> PathBuf {
    if let Some(override_path) = std::env::var_os("DEVCOUNCIL_BIN") {
        let path = PathBuf::from(override_path);
        assert!(
            path.is_file(),
            "DEVCOUNCIL_BIN is set to {path:?}, which is not a file"
        );
        return path;
    }
    let built = built_host();
    assert!(
        built.is_file(),
        "the --json stdout contract cannot be checked: no host binary at {built:?}.\n\
         Build it first — from the repository root:\n\
         \n    go -C backend/go_orchestrator build -o bin/ ./cmd/devcouncil\n\n\
         or point DEVCOUNCIL_BIN at one. This is a failure rather than a skip \
         on purpose: a gate that ran nothing must not report what a gate that \
         ran and passed reports."
    );
    built
}

/// Creates the throwaway project the manifest runs against.
///
/// A bare git repository with one file is the whole fixture: the host's
/// `--json` commands read `.devcouncil/` if it is there and report defaults if
/// it is not, and every entry in the manifest is read-only. There is no `init`
/// step to fail — the Python `dev init` this used to call is what left the gate
/// unrunnable for four days.
fn init_project(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).unwrap_or_else(|err| panic!("could not create {dir:?}: {err}"));
    std::fs::write(dir.join("README.md"), "fixture\n")
        .unwrap_or_else(|err| panic!("could not seed {dir:?}: {err}"));
    let git = Command::new("git")
        .args(["init", "-q", "."])
        .current_dir(dir)
        .output()
        .unwrap_or_else(|err| panic!("could not run git: {err}"));
    assert!(
        git.status.success(),
        "git init failed in {dir:?}: {}",
        String::from_utf8_lossy(&git.stderr)
    );
}

/// The manifest with its placeholders resolved.
fn manifest_for(host: &Path, root: &Path) -> String {
    include_str!("json_stdout_manifest.tsv")
        .replace("{DEVCOUNCIL}", &host.display().to_string())
        .replace("{ROOT}", &root.display().to_string())
}

/// Runs a manifest through `dcjsoncheck` and returns its exit code and report.
///
/// Writing the whole manifest before reading the report cannot deadlock:
/// `dcjsoncheck` reads stdin to EOF before it runs anything or writes a byte,
/// and it caps the manifest at 1 MiB — this one is a few kilobytes of
/// `include_str!`.
fn check(manifest: &str, cwd: &Path) -> (Option<i32>, serde_json::Value) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_dcjsoncheck"))
        // Per invocation, not for the run: these are commands that print a JSON
        // value and exit in milliseconds, so one still alive after a minute is
        // wedged. Without it a hung CLI would hang the suite instead of failing.
        .args(["--deadline-secs", "60", "--cwd"])
        .arg(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|err| panic!("could not start dcjsoncheck: {err}"));
    child
        .stdin
        .take()
        .expect("stdin was piped")
        .write_all(manifest.as_bytes())
        .unwrap_or_else(|err| panic!("could not send the manifest to dcjsoncheck: {err}"));
    let output = child
        .wait_with_output()
        .unwrap_or_else(|err| panic!("dcjsoncheck did not finish: {err}"));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let report = serde_json::from_str(&stdout).unwrap_or_else(|err| {
        panic!(
            "dcjsoncheck's own report is not JSON ({err}): {stdout}\n--- stderr ---\n{}",
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (output.status.code(), report)
}

/// One line per failed invocation, for a report a human can act on.
fn failures(report: &serde_json::Value) -> String {
    report["invocations"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter(|entry| entry["passed"] != serde_json::Value::Bool(true))
        .map(|entry| {
            format!(
                "  `{}`\n    exit {}: {}",
                entry["command"].as_str().unwrap_or("?"),
                entry["exit_code"],
                entry["error"].as_str().unwrap_or("?"),
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The smallest manifest that still counts as this gate.
///
/// Shrinking the list is how coverage gets removed instead of a CLI getting
/// fixed, and the count is taken from what `dcjsoncheck` actually ran rather
/// than from what parsed, so a dropped line cannot hide behind a green report.
const MINIMUM_INVOCATIONS: u64 = 30;

#[test]
fn every_json_invocation_puts_exactly_one_object_on_stdout() {
    let host = host_binary();
    // Beside the build output rather than in /tmp, so a failure leaves the
    // scaffolded project where it can be inspected.
    let project = Path::new(env!("CARGO_TARGET_TMPDIR")).join("json-contract-fixture");
    init_project(&project);

    let (code, report) = check(&manifest_for(&host, &project), &project);
    assert_ne!(
        code,
        Some(2),
        "dcjsoncheck could not carry out the run, so nothing here is evidence: {report}"
    );

    let checked = report["checked"].as_u64().unwrap_or_else(|| {
        panic!("dcjsoncheck reported no invocation count, so nothing ran: {report}")
    });
    assert!(
        checked >= MINIMUM_INVOCATIONS,
        "the manifest shrank to {checked} invocations, below the floor of \
         {MINIMUM_INVOCATIONS}; coverage was removed rather than the CLI fixed"
    );

    assert_eq!(
        code,
        Some(0),
        "{} of {checked} invocations broke the --json stdout contract:\n{}",
        report["failed"],
        failures(&report),
    );
}

/// The gate can still fail — proved by making it fail, every run.
///
/// `every_json_invocation_…` passing means one of two things: the host honours
/// the contract, or the harness stopped being able to see a breach. Only this
/// separates them. `devcouncil version` prints a plain line on stdout by
/// design, so it is a breach of the *JSON* contract that the host will never
/// "fix", which makes it a stable negative control.
#[test]
fn the_harness_fails_a_command_that_breaks_the_contract() {
    let host = host_binary();
    let project = Path::new(env!("CARGO_TARGET_TMPDIR")).join("json-contract-negative");
    init_project(&project);

    let manifest = format!("mode=json\t{}\tversion\n", host.display());
    let (code, report) = check(&manifest, &project);

    assert_eq!(
        code,
        Some(1),
        "a command that prints a plain line on stdout must fail the contract, \
         not pass it and not read as an operational error: {report}"
    );
    assert_eq!(report["checked"], 1, "{report}");
    assert_eq!(report["failed"], 1, "{report}");
    let reason = report["invocations"][0]["error"].as_str().unwrap_or("");
    assert!(
        reason.contains("does not begin with a JSON value"),
        "the failure must name what was wrong with stdout, got {reason:?}: {report}"
    );
}
