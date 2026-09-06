//! Where a standalone Dev Map keeps its state, proved through the real binary.
//!
//! The unit tests in `devmap_extract::paths` pin the resolution *function*.
//! These pin the thing that actually matters: that a build invoked with no
//! `--db` puts the store, the map and the graph in the directory the resolver
//! names, and that a DevCouncil repository is not migrated out from under the
//! 38 Python modules that read `.devcouncil/repo_map.json` by name.
//!
//! Run against the built binary rather than the library, because the defaults
//! under test are clap's and a library test cannot see them.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

fn devmap() -> PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap")
}

fn fixture(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "devmap-ns-{label}-{}-{stamp}-{seq}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.py"), "def a():\n    return helper()\n").unwrap();
    std::fs::write(root.join("src/b.py"), "def helper():\n    return 1\n").unwrap();
    root
}

/// Build `root` with no `--db`, in a clean environment.
///
/// `env_clear` is the point of the helper: an inherited `DEVMAP_HOME` from the
/// developer's shell would silently redirect the store and make every assertion
/// below pass for the wrong reason.
fn build(root: &Path, home: Option<&str>) -> std::process::Output {
    let mut command = Command::new(devmap());
    command
        .args(["--progress", "never", "build", "."])
        .current_dir(root)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default());
    if let Some(home) = home {
        command.env("DEVMAP_HOME", home);
    }
    command.output().expect("devmap invocation")
}

fn assert_built(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_fresh_repository_gets_the_standalone_directory() {
    let root = fixture("fresh");
    assert_built(&build(&root, None));

    assert!(
        root.join(".devmap/codeintel/devmap.sqlite").is_file(),
        "a repository that has never heard of DevCouncil must not have an \
         orchestrator's directory created in it by a code index"
    );
    assert!(
        !root.join(".devcouncil").exists(),
        "nothing may create `.devcouncil/` in a standalone repository"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_devcouncil_repository_keeps_writing_where_its_python_consumers_read() {
    // The compatibility rule, and the one whose failure would be silent:
    // DevCouncil's Python modules open `.devcouncil/repo_map.json` by name. A
    // build that moved the artifact to `.devmap/` would leave every one of them
    // reading the last map written before the split and reporting it fresh.
    let root = fixture("devcouncil");
    std::fs::create_dir_all(root.join(".devcouncil")).unwrap();

    assert_built(&build(&root, None));

    assert!(
        root.join(".devcouncil/codeintel/devmap.sqlite").is_file(),
        "an existing `.devcouncil/` must keep being used"
    );
    assert!(
        !root.join(".devmap").exists(),
        "a DevCouncil repository must not end up with two state directories"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_migrated_repository_prefers_the_standalone_directory() {
    // Both present: `.devmap/` wins, so a repository that has migrated does not
    // fall back to the orchestrator's directory merely because the orchestrator
    // still keeps its own state there.
    let root = fixture("migrated");
    std::fs::create_dir_all(root.join(".devcouncil")).unwrap();
    std::fs::create_dir_all(root.join(".devmap")).unwrap();

    assert_built(&build(&root, None));

    assert!(root.join(".devmap/codeintel/devmap.sqlite").is_file());
    assert!(
        !root.join(".devcouncil/codeintel").exists(),
        "the migrated layout must win outright, not write to both"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn devmap_home_overrides_both_layouts() {
    let root = fixture("home-env");
    std::fs::create_dir_all(root.join(".devcouncil")).unwrap();
    let elsewhere = root.join("state");

    assert_built(&build(&root, Some(&elsewhere.to_string_lossy())));

    assert!(
        elsewhere.join("codeintel/devmap.sqlite").is_file(),
        "an explicit DEVMAP_HOME must win over an existing `.devcouncil/`"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_manifest_lands_in_the_same_directory_as_the_store() {
    // The property that makes the whole resolver worth having: a map written to
    // one directory while the store lives in another is a consumer reading a
    // generation nothing is building.
    for (label, precreate) in [("fresh-pair", None), ("legacy-pair", Some(".devcouncil"))] {
        let root = fixture(label);
        if let Some(dir) = precreate {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        let output = Command::new(devmap())
            .args(["--progress", "never", "build", ".", "--manifest"])
            .current_dir(&root)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .output()
            .expect("devmap invocation");
        assert_built(&output);

        let state = devmap_extract::paths::state_dir(&root);
        assert!(state.join("codeintel/devmap.sqlite").is_file(), "{label}");
        assert!(state.join("repo_map.json").is_file(), "{label}");
        assert!(state.join("graph/code_graph.json").is_file(), "{label}");
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[test]
fn the_emitted_plugin_does_not_share_devcouncils_marketplace_file() {
    // Both emitters wrote a single-repo marketplace to
    // `<state>/claude-plugin/.claude-plugin/marketplace.json`, under different
    // names — `devcouncil-local` and `devmap-local` — so whichever ran last
    // silently replaced the other's registration.
    let root = fixture("plugin");
    assert_built(&build(&root, None));

    let devcouncils = root.join(".devmap/claude-plugin");
    std::fs::create_dir_all(devcouncils.join(".claude-plugin")).unwrap();
    std::fs::write(
        devcouncils.join(".claude-plugin/marketplace.json"),
        r#"{"name":"devcouncil-local"}"#,
    )
    .unwrap();

    let output = Command::new(devmap())
        .args(["claude", "plugin"])
        .current_dir(&root)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .output()
        .expect("devmap invocation");
    assert!(
        output.status.success(),
        "plugin emit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert_eq!(
        std::fs::read_to_string(devcouncils.join(".claude-plugin/marketplace.json")).unwrap(),
        r#"{"name":"devcouncil-local"}"#,
        "the emitter must not have written over DevCouncil's marketplace"
    );
    assert!(
        root.join(".devmap/devmap-plugin/.claude-plugin/marketplace.json")
            .is_file(),
        "the bundle must land in its own directory"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_emitted_hook_does_not_name_a_path_that_exists_on_one_machine() {
    // `current_exe()` is an absolute path into whatever build tree the binary
    // came from. Committed in a bundle, it names a path only the author has;
    // emitted from a build tree, it keeps pointing at a stale binary after a
    // current one is installed on PATH.
    let root = fixture("binary-flag");
    assert_built(&build(&root, None));

    let output = Command::new(devmap())
        .args([
            "--json",
            "claude",
            "hooks",
            "--binary",
            "/usr/local/bin/devmap",
        ])
        .current_dir(&root)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .output()
        .expect("devmap invocation");
    assert!(output.status.success());

    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.contains("/usr/local/bin/devmap"),
        "--binary must be what the emitted handlers name: {text}"
    );
    assert!(
        !text.contains("target/debug") && !text.contains("target-split"),
        "the build tree path must not survive an explicit --binary: {text}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// WAS A DEFECT, NOW THE GATE: the guide must not name a command that does not
/// exist.
///
/// The generated guide told every agent to run `devmap dead --confidence
/// extracted`. That flag was DevCouncil's — `dev map dead` had it, `devmap dead`
/// never did — so an agent following step 7 got a clap parse error instead of a
/// dead-code list, and the guide is the one file whose whole job is to be
/// followed literally.
///
/// Checked against the parser's own subcommand list rather than a list typed
/// here, so a command renamed in `Commands` fails this test rather than silently
/// invalidating the guide.
#[test]
fn every_command_the_guide_names_is_a_real_subcommand() {
    let map = serde_json::json!({
        "meta": {"devmap_rust": {"role_files_computed": true, "handoff_paths_computed": true}},
        "subsystems": [{"area": "src", "summary": "the code"}],
        "important_files": ["src/main.rs"]
    });
    let guide = devmap_query::guides::agent_guide_text(
        &map,
        ".devmap/repo_map.json",
        ".devmap/graph/code_graph.json",
        ".devmap/codeintel/devmap.sqlite",
    );

    let known = known_subcommand_names();
    let mut seen = 0usize;
    for (index, _) in guide.match_indices("`devmap ") {
        let rest = &guide[index + "`devmap ".len()..];
        let Some(word) = rest.split([' ', '`', '\n']).next() else {
            continue;
        };
        if word.is_empty() {
            continue;
        }
        seen += 1;
        assert!(
            known.contains(&word.to_string()),
            "the guide tells an agent to run `devmap {word}`, which is not a subcommand. \
Known: {known:?}"
        );
    }
    // A guide that named no commands at all would pass the loop above without
    // checking anything, which is the failure mode this whole file guards
    // against: a check that could not run reporting as one that passed.
    assert!(
        seen >= 5,
        "expected the guide to name several commands, found {seen}"
    );
}

/// Every flag the guide names must parse, too.
///
/// The `--confidence` defect was a *flag*, not a command, so checking command
/// names alone would not have caught it. This runs each documented invocation
/// through the real parser with `--help`, which exits non-zero on an unknown
/// flag and touches no store.
#[test]
fn every_flag_the_guide_names_is_accepted_by_the_parser() {
    let map = serde_json::json!({"subsystems": [], "important_files": []});
    let guide = devmap_query::guides::agent_guide_text(&map, "m.json", "g.json", "s.sqlite");

    let mut checked = 0usize;
    for (index, _) in guide.match_indices("`devmap ") {
        let rest = &guide[index + "`devmap ".len()..];
        let Some(invocation) = rest.split('`').next() else {
            continue;
        };
        // Only the flags: a placeholder like `<name>` is prose, not an argument
        // the parser could resolve.
        let args: Vec<&str> = invocation
            .split_whitespace()
            .filter(|token| !token.starts_with('<'))
            .collect();
        if args.len() < 2 || !args[1..].iter().any(|t| t.starts_with("--")) {
            continue;
        }
        checked += 1;
        let output = Command::new(devmap())
            .args(&args)
            .arg("--help")
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .output()
            .expect("devmap invocation");
        assert!(
            output.status.success(),
            "`devmap {}` does not parse: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert!(checked >= 1, "no flagged invocation was checked");
}

/// The parser's subcommand names, read from the binary rather than restated.
fn known_subcommand_names() -> Vec<String> {
    let output = Command::new(devmap())
        .arg("--help")
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .output()
        .expect("devmap --help");
    let text = String::from_utf8_lossy(&output.stdout);
    let mut names = Vec::new();
    let mut in_commands = false;
    for line in text.lines() {
        if line.starts_with("Commands:") {
            in_commands = true;
            continue;
        }
        if in_commands {
            if line.trim().is_empty() || line.starts_with("Options:") {
                break;
            }
            if let Some(name) = line.split_whitespace().next() {
                names.push(name.to_string());
            }
        }
    }
    assert!(
        !names.is_empty(),
        "could not read the subcommand list: {text}"
    );
    names
}
