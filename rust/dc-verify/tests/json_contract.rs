//! The `--json` stdout contract, driven against the real DevCouncil CLI.
//!
//! `json_stdout`'s unit tests prove the scanner is right about strings. This is
//! the gate that matters: it runs every invocation in
//! `json_stdout_manifest.tsv` as a real process against a freshly-initialised
//! throwaway project, and requires each one's stdout to hold exactly one JSON
//! value.
//!
//! It exists because Python's `CliRunner` cannot prove this. Under Click 8.4
//! `result.output` is stdout and stderr merged, so a banner that leaked onto
//! stdout is indistinguishable there from one that went to stderr — which is
//! how `dev cost budget --json --set 5.00` shipped emitting unparseable stdout
//! with a green test suite. Two real file descriptors settle it.
//!
//! If DevCouncil's Python is not importable the test skips loudly rather than
//! passing quietly. `DC_JSON_REQUIRE_CLI=1` turns that skip into a failure, so
//! CI can demand the evidence: a check that could not run must never report
//! what a check that ran and passed reports.

use std::path::{Path, PathBuf};
use std::process::Command;

use dc_verify::json_stdout::{holds_exactly_one_value, holds_json_lines};

/// The DevCouncil checkout this crate is part of: `<repo>/rust/dc-verify`, so
/// the repository root is two levels up. Resolved by depth rather than by name
/// so a worktree or a differently-named clone still finds it.
fn devcouncil_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate is at <repo>/rust/dc-verify, so it has two ancestors")
}

/// Locates an interpreter with DevCouncil importable, or None.
fn devcouncil_python() -> Option<PathBuf> {
    let python = devcouncil_root().join(".venv/bin/python");
    if python.is_file() { Some(python) } else { None }
}

fn skip_or_fail(reason: &str) {
    let message = format!(
        "the --json stdout contract is UNVERIFIED: {reason}. No DevCouncil command was \
         actually run, so nothing here is evidence that stdout parses."
    );
    assert!(
        std::env::var_os("DC_JSON_REQUIRE_CLI").is_none(),
        "{message} (DC_JSON_REQUIRE_CLI is set, so this is a failure)"
    );
    eprintln!("SKIP: {message}");
}

#[derive(Debug)]
struct Entry {
    line_number: usize,
    jsonl: bool,
    argv: Vec<String>,
}

/// Reads the manifest, substituting the interpreter and project root.
///
/// A malformed line is a panic, not a skipped entry: silently dropping one
/// would shrink coverage invisibly, which is the failure mode this whole file
/// exists to prevent.
fn read_manifest(python: &Path, root: &Path) -> Vec<Entry> {
    let text = include_str!("json_stdout_manifest.tsv");
    let mut entries = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line_number = index + 1;
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let mut fields = line.split('\t');
        let jsonl = match fields.next() {
            Some("mode=json") => false,
            Some("mode=jsonl") => true,
            other => panic!("manifest line {line_number}: bad mode field {other:?}"),
        };
        let argv: Vec<String> = fields
            .map(|field| {
                field
                    .replace("{PY}", &python.display().to_string())
                    .replace("{ROOT}", &root.display().to_string())
            })
            .collect();
        assert!(
            !argv.is_empty(),
            "manifest line {line_number}: no command after the mode field"
        );
        entries.push(Entry {
            line_number,
            jsonl,
            argv,
        });
    }
    entries
}

/// Scaffolds a throwaway DevCouncil project for the manifest to run against.
fn init_project(python: &Path, dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|err| format!("could not create {dir:?}: {err}"))?;
    std::fs::write(dir.join("README.md"), "fixture\n")
        .map_err(|err| format!("could not seed {dir:?}: {err}"))?;
    let git = Command::new("git")
        .args(["init", "-q", "."])
        .current_dir(dir)
        .output()
        .map_err(|err| format!("could not run git: {err}"))?;
    if !git.status.success() {
        return Err(format!(
            "git init failed: {}",
            String::from_utf8_lossy(&git.stderr)
        ));
    }
    let init = Command::new(python)
        .args([
            "-m",
            "devcouncil.cli.main",
            "init",
            "--skip-map",
            "--skip-skills",
        ])
        .current_dir(dir)
        .output()
        .map_err(|err| format!("could not run dev init: {err}"))?;
    if !init.status.success() {
        return Err(format!(
            "dev init failed: {}",
            String::from_utf8_lossy(&init.stderr)
        ));
    }
    Ok(())
}

#[test]
fn every_json_invocation_puts_exactly_one_object_on_stdout() {
    let Some(python) = devcouncil_python() else {
        skip_or_fail("no .venv/bin/python in this checkout (run `uv sync`)");
        return;
    };

    // A directory beside the build output rather than /tmp, so a failure leaves
    // the scaffolded project where it can be inspected.
    let project = Path::new(env!("CARGO_TARGET_TMPDIR")).join("json-contract-fixture");
    let _ = std::fs::remove_dir_all(&project);
    if let Err(reason) = init_project(&python, &project) {
        skip_or_fail(&format!("could not scaffold a project: {reason}"));
        return;
    }

    let entries = read_manifest(&python, &project);
    assert!(
        entries.len() >= 30,
        "the manifest shrank to {} entries; coverage was removed rather than the CLI fixed",
        entries.len()
    );

    let mut failures = Vec::new();
    for entry in &entries {
        let output = Command::new(&entry.argv[0])
            .args(&entry.argv[1..])
            // Closed, not inherited: `dev apply-patch` reads stdin when
            // `--unified-diff` is blank and would otherwise hang the suite.
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap_or_else(|err| {
                panic!(
                    "manifest line {}: could not run {:?}: {err}",
                    entry.line_number, entry.argv[0]
                )
            });
        let stdout = String::from_utf8_lossy(&output.stdout);
        let verdict = if entry.jsonl {
            holds_json_lines(&stdout)
        } else {
            holds_exactly_one_value(&stdout)
        };
        if let Err(err) = verdict {
            failures.push(format!(
                "  manifest line {}: `{}`\n    exit {:?}: {err}",
                entry.line_number,
                entry.argv[1..].join(" "),
                output.status.code(),
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "{} of {} invocations broke the --json stdout contract:\n{}",
        failures.len(),
        entries.len(),
        failures.join("\n"),
    );
}
