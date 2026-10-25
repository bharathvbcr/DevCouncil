//! Every skill DevCouncil ships names only DevMap commands and MCP tools that
//! exist, and carries no link that breaks once it is installed.
//!
//! The generated guide has had this check for a while
//! (`standalone_namespace::every_command_the_guide_names_is_a_real_subcommand`);
//! the skills did not, though they are read the same way — literally, by an
//! agent with no other source. That covers the five DevMap skills and the
//! eighteen in DevCouncil's library, which `devcouncil skills scaffold`
//! installs. The library's `devcouncil` commands and `devcouncil_*` tools are
//! held to the host's dispatch table on the Go side
//! (`cmd/devcouncil/liveness_test.go`); this is the DevMap half.
//!
//! Skills are copied into `.claude/skills/<name>/SKILL.md` and the like, so a
//! relative link that resolves in this repository resolves to nothing there.
//! None is allowed.

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

/// (label, text) for every shipped skill, with how many of each kind were
/// found so an empty glob cannot pass.
fn shipped_skills() -> Vec<(String, String)> {
    let root = repo_root();
    let mut out = Vec::new();
    let devmap_skills = root.join("rust/devmap-cli/skills");
    for entry in std::fs::read_dir(&devmap_skills).expect("devmap skills dir") {
        let path = entry.expect("entry").path().join("SKILL.md");
        if path.is_file() {
            out.push((
                path.strip_prefix(&root).unwrap().display().to_string(),
                std::fs::read_to_string(&path).expect("read skill"),
            ));
        }
    }
    let devmap_count = out.len();
    assert_eq!(
        devmap_count, 5,
        "expected the five DevMap skills, found {devmap_count}"
    );
    let library = root.join("backend/go_orchestrator/devcouncil/skills/library");
    for entry in std::fs::read_dir(&library).expect("library dir") {
        let path = entry.expect("entry").path();
        if path.extension().is_some_and(|e| e == "md") {
            out.push((
                path.strip_prefix(&root).unwrap().display().to_string(),
                std::fs::read_to_string(&path).expect("read skill"),
            ));
        }
    }
    assert_eq!(
        out.len() - devmap_count,
        18,
        "expected the eighteen library skills (skills.TestEmbeddedLibraryHasEighteenSkills)"
    );
    out
}

/// The contents of every inline code span, outside fenced code too: a fenced
/// `devmap foo` is as much an instruction as an inline one.
fn code_spans(text: &str) -> Vec<String> {
    let mut spans = Vec::new();
    let mut fenced = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            spans.push(trimmed.trim_start_matches("$ ").to_string());
            continue;
        }
        let mut parts = line.split('`');
        parts.next();
        while let (Some(inside), Some(_)) = (parts.next(), parts.next()) {
            spans.push(inside.to_string());
        }
    }
    spans
}

fn known_subcommands() -> Vec<String> {
    let output = Command::new(env!("CARGO_BIN_EXE_devmap"))
        .arg("--help")
        .env("DEVMAP_AUTOSPAWN", "0")
        .output()
        .expect("devmap --help");
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    let mut names: Vec<String> = help
        .lines()
        .skip_while(|l| !l.starts_with("Commands:"))
        .skip(1)
        .take_while(|l| l.starts_with("  "))
        .filter_map(|l| l.split_whitespace().next().map(str::to_string))
        .collect();
    // Visible aliases are subcommands too: `Commands:` lists them on the
    // line of the command they alias, as `[aliases: a, b]`.
    for line in help.lines() {
        if let Some(rest) = line.split("[aliases: ").nth(1) {
            for alias in rest.trim_end_matches(']').split(", ") {
                names.push(alias.trim().to_string());
            }
        }
    }
    assert!(
        names.len() >= 10,
        "could not read the subcommands from --help:\n{help}"
    );
    names
}

#[test]
fn every_devmap_command_a_shipped_skill_names_is_a_real_subcommand() {
    let known = known_subcommands();
    let mut seen = 0usize;
    for (label, text) in shipped_skills() {
        for span in code_spans(&text) {
            for segment in span.split(['|', ';', '&']) {
                let mut words = segment.split_whitespace();
                if words.next() != Some("devmap") {
                    continue;
                }
                let Some(word) = words.next() else { continue };
                if word.starts_with('-') || word.starts_with('<') {
                    continue;
                }
                seen += 1;
                assert!(
                    known.iter().any(|k| k == word),
                    "{label} tells an agent to run `devmap {word}`, which is not a subcommand \
                     (known: {known:?})"
                );
            }
        }
    }
    assert!(
        seen >= 20,
        "expected the skills to name many devmap commands, found {seen}"
    );
}

#[test]
fn every_devmap_tool_a_shipped_skill_names_is_served() {
    let mut seen = 0usize;
    for (label, text) in shipped_skills() {
        for span in code_spans(&text) {
            let span = span.trim();
            if !span.starts_with("devmap_")
                || !span.chars().all(|c| c.is_ascii_lowercase() || c == '_')
            {
                continue;
            }
            seen += 1;
            assert!(
                devmap_serve::mcp::TOOL_NAMES.contains(&span),
                "{label} names the MCP tool `{span}`, which the server does not serve \
                 (served: {:?})",
                devmap_serve::mcp::TOOL_NAMES
            );
        }
    }
    assert!(
        seen >= 8,
        "expected the skills to name the MCP tools, found {seen}"
    );
}

#[test]
fn no_shipped_skill_carries_a_link_that_breaks_once_installed() {
    for (label, text) in shipped_skills() {
        for (index, _) in text.match_indices("](") {
            let target = text[index + 2..].split(')').next().unwrap_or("");
            let absolute = target.starts_with("https://")
                || target.starts_with("http://")
                || target.starts_with("mailto:")
                || target.starts_with('#');
            assert!(
                absolute,
                "{label} links to `{target}`, which resolves against this repository and \
                 to nothing in the directory the skill is installed into"
            );
        }
    }
}

#[test]
fn the_span_reader_sees_what_an_agent_sees() {
    let spans =
        code_spans("run `devmap status --json` then\n```sh\n$ devmap paths --json\n```\n`x`");
    assert_eq!(spans, ["devmap status --json", "devmap paths --json", "x"]);
}
