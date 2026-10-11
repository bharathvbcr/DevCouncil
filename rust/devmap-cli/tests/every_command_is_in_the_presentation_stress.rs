//! `scripts/cli-presentation-stress.py` is a hand-kept list of command forms,
//! and it had drifted: `ask`, `skeleton`, `suspects`, `blast`, `literals`,
//! `gap-record` and `version` were never driven through the progress and
//! terminal-presentation checks, and nothing noticed. The list of what must be
//! covered is the CLI's own command table, read from `devmap --help`; a command
//! the stress script does not name, and that is not exempt with a reason, fails
//! here.

use std::collections::BTreeSet;
use std::process::Command;

/// Commands the script cannot drive as a one-shot JSON query, and why.
const EXEMPT: &[(&str, &str)] = &[
    (
        "build",
        "driven first and separately, with --manifest, to create the store",
    ),
    (
        "serve",
        "a long-running IPC daemon, not a one-shot presentation",
    ),
    (
        "mcp",
        "speaks MCP on stdio; the script covers `mcp --print-config`",
    ),
    (
        "hook",
        "host hook entry point: stdin protocol, no presentation",
    ),
    (
        "claude",
        "driven as `claude events` and `claude hooks` in the terminal runs",
    ),
    ("help", "clap's own help"),
];

fn commands() -> BTreeSet<String> {
    let output = Command::new(env!("CARGO_BIN_EXE_devmap"))
        .arg("--help")
        .output()
        .expect("run devmap --help");
    assert!(output.status.success(), "devmap --help failed");
    let text = String::from_utf8(output.stdout).expect("help is UTF-8");
    let table = text
        .split("Commands:")
        .nth(1)
        .expect("devmap --help has a Commands: section");
    table
        .lines()
        .skip(1)
        .take_while(|line| !line.trim().is_empty() && line.starts_with("  "))
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_string)
        .collect()
}

fn script() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/cli-presentation-stress.py");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("{} must exist: {err}", path.display()))
}

/// Whether `source` names `command` as the first element of a command form.
fn names(source: &str, command: &str) -> bool {
    source.contains(&format!("[\"{command}\"")) || source.contains(&format!("\"{command}\","))
}

#[test]
fn the_command_table_is_read() {
    let all = commands();
    for expected in ["search", "suspects", "blast", "skeleton", "gap-record"] {
        assert!(all.contains(expected), "{expected} missing from {all:?}");
    }
    assert!(all.len() > 30, "the table looks truncated: {all:?}");
}

#[test]
fn every_command_is_driven_by_the_presentation_stress_or_exempt_with_a_reason() {
    let source = script();
    let exempt: BTreeSet<&str> = EXEMPT.iter().map(|(name, _)| *name).collect();
    let all = commands();
    let missing: Vec<&String> = all
        .iter()
        .filter(|name| !exempt.contains(name.as_str()) && !names(&source, name))
        .collect();
    assert!(
        missing.is_empty(),
        "scripts/cli-presentation-stress.py does not drive {missing:?}; add a form or an EXEMPT entry with a reason"
    );
    for (name, reason) in EXEMPT {
        assert!(
            all.contains(*name),
            "EXEMPT names {name}, which is no longer a command ({reason})"
        );
    }
}

#[test]
fn the_matcher_distinguishes_a_command_from_a_prefix_of_one() {
    assert!(names("[\"blast\", \"--since\"]", "blast"));
    assert!(!names("[\"blasts\"]", "blast"));
    assert!(!names("[\"api-impact\"]", "impact"));
}
