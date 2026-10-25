//! A hand-written guide keeps its text while DevMap keeps one block of it
//! current: `--guides`, `--adopt-guides`, a malformed block, and what
//! `integrate --check` reports for it, all driven through the binary because
//! the flags, the JSON report and the exit code are the contract.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use devmap_query::guides::{AGENT_GUIDE_BLOCK_BEGIN, AGENT_GUIDE_BLOCK_END, AGENT_GUIDE_MARKER};
use serde_json::Value;

const MINE: &str = "# My notes\nKeep this line exactly.\n";

fn devmap(root: &Path, home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_devmap"))
        .current_dir(root)
        .env("DEVMAP_AUTOSPAWN", "0")
        .env("HOME", home)
        .env_remove("CLAUDE_CONFIG_DIR")
        .args(["--progress", "never"])
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("devmap {args:?}: {error}"))
}

#[track_caller]
fn json(root: &Path, home: &Path, args: &[&str]) -> Value {
    let mut full = vec!["--json"];
    full.extend_from_slice(args);
    let output = devmap(root, home, &full);
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success(),
        "devmap {full:?} failed:\n{stdout}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_str(&stdout).unwrap_or_else(|error| panic!("{full:?}: {error}: {stdout}"))
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "devmap-guide-blocks-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn fixture() -> PathBuf {
    let root = scratch("repo");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn answer() -> u32 { 42 }\n").unwrap();
    std::fs::write(root.join("CLAUDE.md"), MINE).unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.email", "t@example.invalid"],
        vec!["config", "user.name", "t"],
        vec!["add", "-A"],
        vec!["commit", "-qm", "fixture"],
    ] {
        let out = Command::new("git")
            .args(&args)
            .current_dir(&root)
            .output()
            .expect("git runs");
        assert!(out.status.success(), "git {args:?}");
    }
    root
}

/// The report entry for `name`, from a manifest/build payload's `guides`.
#[track_caller]
fn guide<'a>(report: &'a Value, name: &str) -> &'a Value {
    let guides = report["guides"]
        .as_array()
        .or_else(|| report["manifest"]["guides"].as_array())
        .unwrap_or_else(|| panic!("no guides in {report}"));
    guides
        .iter()
        .find(|g| g["path"].as_str().is_some_and(|p| p.ends_with(name)))
        .unwrap_or_else(|| panic!("{name} not reported: {report}"))
}

fn read(root: &Path) -> String {
    std::fs::read_to_string(root.join("CLAUDE.md")).unwrap()
}

#[test]
fn a_hand_written_guide_is_adopted_refreshed_in_place_and_refused_when_malformed() {
    let root = fixture();
    let home = scratch("home");

    // Without the flag a hand-written guide is reported and left alone.
    let built = json(&root, &home, &["build", "--manifest", "--guides"]);
    assert_eq!(guide(&built, "CLAUDE.md")["disposition"], "not_ours");
    assert_eq!(guide(&built, "AGENTS.md")["disposition"], "created");
    assert_eq!(read(&root), MINE);

    // `--adopt-guides` appends one block and keeps every byte before it.
    let adopted = json(&root, &home, &["manifest", "--guides", "--adopt-guides"]);
    let entry = guide(&adopted, "CLAUDE.md");
    assert_eq!(entry["disposition"], "adopted", "{adopted}");
    assert_eq!(entry["block"], true);
    assert_eq!(entry["changed"], true);
    let text = read(&root);
    assert!(
        text.starts_with(&format!("{MINE}\n{AGENT_GUIDE_BLOCK_BEGIN}\n")),
        "{text}"
    );
    assert!(
        text.ends_with(&format!("{AGENT_GUIDE_BLOCK_END}\n")),
        "{text}"
    );
    assert!(!text.contains(AGENT_GUIDE_MARKER), "{text}");
    // The whole-file guide DevMap created is still wholly its own.
    assert_eq!(guide(&adopted, "AGENTS.md")["block"], false);

    // A person edits around the block; a later run needs no flag and leaves
    // their edits alone.
    let edited = text.replace("Keep this line exactly.", "Edited by a person.")
        + "\n## Added after the block\n";
    std::fs::write(root.join("CLAUDE.md"), &edited).unwrap();
    let refreshed = json(&root, &home, &["manifest", "--guides"]);
    assert_eq!(guide(&refreshed, "CLAUDE.md")["disposition"], "unchanged");
    assert_eq!(read(&root), edited);

    // A stale block is rewritten, and only the block.
    let stale = edited.replace("# Agent Workspace Guide", "# An old heading");
    std::fs::write(root.join("CLAUDE.md"), &stale).unwrap();
    let updated = json(&root, &home, &["manifest", "--guides"]);
    assert_eq!(guide(&updated, "CLAUDE.md")["disposition"], "updated");
    assert_eq!(read(&root), edited);

    // A second begin line: nothing is guessed, nothing is written, and the
    // report says why and that a person must act.
    let malformed = edited.replacen(
        AGENT_GUIDE_BLOCK_END,
        &format!("{AGENT_GUIDE_BLOCK_BEGIN}\n{AGENT_GUIDE_BLOCK_END}"),
        1,
    );
    std::fs::write(root.join("CLAUDE.md"), &malformed).unwrap();
    let refused = json(&root, &home, &["manifest", "--guides", "--adopt-guides"]);
    let entry = guide(&refused, "CLAUDE.md");
    assert_eq!(entry["disposition"], "malformed_block", "{refused}");
    assert_eq!(entry["needs_attention"], true);
    assert_eq!(entry["changed"], false);
    assert!(
        entry["reason"]
            .as_str()
            .is_some_and(|r| r.contains("2 devmap:begin and 1 devmap:end")),
        "{entry}"
    );
    assert_eq!(read(&root), malformed);

    // `integrate --check` plans through the same code and names the file
    // instead of reading it as clean.
    let check = devmap(
        &root,
        &home,
        &[
            "--json",
            "integrate",
            "warp",
            "--project-root",
            root.to_str().unwrap(),
            "--check",
        ],
    );
    assert_ne!(
        check.status.code(),
        Some(0),
        "check passed a malformed block"
    );
    let stdout = String::from_utf8_lossy(&check.stdout).into_owned();
    let report: Value = serde_json::Deserializer::from_str(&stdout)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("no report: {stdout}"))
        .unwrap();
    assert_eq!(report["check_ok"], false);
    assert_eq!(
        guide(&report, "CLAUDE.md")["disposition"],
        "malformed_block",
        "{report}"
    );
    assert_eq!(read(&root), malformed, "a check wrote");

    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&home).ok();
}

#[test]
fn adopt_guides_without_guides_is_refused() {
    let root = fixture();
    let home = scratch("home-refused");
    let output = devmap(&root, &home, &["manifest", "--adopt-guides"]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("--guides"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(read(&root), MINE);
    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&home).ok();
}
