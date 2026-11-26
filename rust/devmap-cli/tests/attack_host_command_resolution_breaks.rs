//! Attack round on host-command resolution: cases that BREAK the implementation
//! under test as of 0b5232c5 (ft-258f2688 lane D); fixed in the fix-round commit.

use crate::common::host_cmd::*;

#[test]
fn a_missing_binary_path_behind_an_outside_link_into_the_repository_is_not_registered() {
    use std::os::unix::fs::symlink;
    let base = scratch("missing-behind-link");
    let home = scratch("missing-behind-link-home");
    let repo = base.join("repo");
    let outside = base.join("out");
    std::fs::create_dir_all(repo.join("bin")).unwrap();
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    symlink(repo.join("bin"), outside.join("link")).unwrap();
    // The file does not exist yet, so the check is lexical: out/link/devmap
    // looks outside, and the first file placed at repo/bin/devmap is what runs.
    let spelled = outside.join("link/devmap");
    let out = run(
        &[
            "--json",
            "claude",
            "hooks",
            "--binary",
            spelled.to_str().unwrap(),
        ],
        &repo,
        &home,
        None,
    );
    assert_ne!(
        out.status,
        Some(0),
        "registered {spelled:?}, which resolves into the repository once a file is placed: {}",
        out.stdout
    );
}

#[test]
fn doctor_does_not_call_an_unreadable_config_clean() {
    // A BOM or a comment makes serde_json refuse the document. The entry cannot
    // be judged, and the report must not read the same as "all absolute".
    let body = mcp_json("devmap", &["mcp"]);
    let forms = [
        format!("\u{feff}{body}"),
        format!("// note\n{body}"),
        "{\"mcpServers\": {\"devmap\": {\"command\": \"devmap\", /* c */ \"args\": [\"mcp\"]}}}"
            .to_string(),
    ];
    let mut silent = Vec::new();
    for form in &forms {
        let warning = doctor_warning(|_repo, home| put(&home.join(".cursor/mcp.json"), form));
        if warning.is_none() {
            silent.push(form.clone());
        }
    }
    assert!(
        silent.is_empty(),
        "doctor was silent on unreadable configs: {silent:#?}"
    );
}

#[test]
fn doctor_flags_a_relative_script_after_an_absolute_launcher() {
    let cases: [(&str, &[&str]); 2] = [
        ("/usr/bin/env", &["./devmap", "mcp"]),
        ("/bin/sh", &["-c", "./devmap mcp"]),
    ];
    let mut missed = Vec::new();
    for (command, args) in cases {
        let warning = doctor_warning(|repo, home| {
            executable(&repo.join("devmap"), "#!/bin/sh\nexit 0\n");
            put(&home.join(".cursor/mcp.json"), &mcp_json(command, args));
        });
        if warning.is_none() {
            missed.push((command, args));
        }
    }
    assert!(missed.is_empty(), "doctor reported nothing for: {missed:?}");
}

#[test]
fn doctor_flags_a_hook_command_that_runs_devmap_through_a_launcher_or_quote() {
    let commands = [
        "devmap.exe hook session-start",
        "\"$CLAUDE_PROJECT_DIR\"/devmap hook session-start",
        "env devmap hook session-start",
        "sh -c 'devmap hook session-start'",
        "FOO=1 devmap hook session-start",
        "cd . && devmap hook session-start",
    ];
    let mut missed = Vec::new();
    for command in commands {
        let warning = doctor_warning(|repo, _home| {
            put(
                &repo.join(".claude/settings.json"),
                &serde_json::json!({"hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": command}]}]}}).to_string(),
            );
        });
        if warning.is_none() {
            missed.push(command);
        }
    }
    assert!(
        missed.is_empty(),
        "doctor reported nothing for: {missed:#?}"
    );
}

// ---- fix round 2 ------------------------------------------------------------

/// A working directory in no repository (here, `$HOME` itself) is not a
/// repository: a binary under `~/.cargo/bin` is registered, not refused.
#[test]
fn a_cwd_in_no_repository_does_not_make_the_home_binaries_inside_it() {
    let home = scratch("norepo-home");
    let exe = home.join(".cargo/bin/devmap");
    place_binary(&exe);
    for args in [
        vec!["--json", "claude", "hooks"],
        vec!["--json", "claude", "plugin", "--dry-run"],
        vec!["--json", "mcp", "--print-config"],
    ] {
        let out = run_exe(&exe, &args, &home, &home, None);
        assert_eq!(
            out.status,
            Some(0),
            "{args:?} refused outside a repository:\n{}",
            out.text()
        );
    }
}

/// `mcp --print-config` is refused for an in-repository build like every writer.
#[test]
fn print_config_refuses_a_binary_inside_the_repository() {
    let repo = scratch("printcfg-repo");
    let home = scratch("printcfg-home");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    let exe = repo.join("target/debug/devmap");
    place_binary(&exe);
    let out = run_exe(
        &exe,
        &["--json", "mcp", "--print-config"],
        &repo,
        &home,
        None,
    );
    assert_ne!(
        out.status,
        Some(0),
        "printed a config naming an in-repository binary: {}",
        out.stdout
    );
    assert!(out.text().contains("inside repository"), "{}", out.text());
}

fn warns(place: impl Fn(&std::path::Path, &std::path::Path)) -> String {
    doctor_warning(place).unwrap_or_default()
}

#[test]
fn doctor_flags_substitutions_flag_clusters_and_executable_extensions() {
    let commands = [
        "`echo ./devmap` hook session-start",
        "echo $(./devmap hook session-start)",
        "bash -lc './devmap hook session-start'",
        "sh -ic 'devmap hook session-start'",
        "bash -ec \"sh -c './devmap hook x'\"",
        "devmap.cmd hook session-start",
        "DEVMAP.PS1 hook session-start",
        "./devmap.bat hook session-start",
    ];
    let mut missed = Vec::new();
    for command in commands {
        let warning = warns(|repo, _| {
            put(
                &repo.join(".claude/settings.json"),
                &serde_json::json!({"hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": command}]}]}}).to_string(),
            );
        });
        if warning.is_empty() {
            missed.push(command);
        }
    }
    assert!(
        missed.is_empty(),
        "doctor reported nothing for: {missed:#?}"
    );
}

#[test]
fn the_validator_and_doctor_agree_on_home_and_other_variables() {
    let hook = |exe: &str| {
        serde_json::json!({"hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": format!("{exe} hook session-start")}]}]}}).to_string()
    };
    for (exe, bad) in [
        ("\"$CLAUDE_PROJECT_DIR\"/bin/devmap", true),
        ("$CLAUDE_PROJECT_DIR/bin/devmap", true),
        ("$HOME/.local/bin/devmap", false),
        ("\"${HOME}/.local/bin/devmap\"", false),
        ("~/.local/bin/devmap", false),
        ("/usr/bin/devmap", false),
    ] {
        let repo = scratch("agree-repo");
        let home = scratch("agree-home");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        put(&repo.join("hooks.json"), &hook(exe));
        let validated = run(
            &["claude", "validate", "hooks.json"],
            &repo,
            &home,
            Some("/usr/bin:/bin"),
        );
        put(&repo.join(".claude/settings.json"), &hook(exe));
        let flagged = doctor_warning_in(&repo, &home).is_some();
        assert_eq!(
            validated.status != Some(0),
            bad,
            "validator on {exe}: {}",
            validated.text()
        );
        assert_eq!(flagged, bad, "doctor on {exe}");
    }
}

/// The remedy doctor names must fix the flagged file, per host.
#[test]
fn the_suggested_remedy_clears_the_finding() {
    let fresh = |tag: &str| {
        let repo = scratch(&format!("remedy-{tag}-repo"));
        let home = scratch(&format!("remedy-{tag}-home"));
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        (repo, home)
    };
    let integrate = |host: &str, repo: &std::path::Path, home: &std::path::Path| {
        let out = run(
            &["integrate", host, "--project-root", repo.to_str().unwrap()],
            repo,
            home,
            Some("/usr/bin:/bin"),
        );
        assert_eq!(out.status, Some(0), "integrate {host}: {}", out.text());
    };

    let (repo, home) = fresh("cursor");
    put(
        &home.join(".cursor/mcp.json"),
        &mcp_json("devmap", &["mcp"]),
    );
    assert!(doctor_warning_in(&repo, &home)
        .unwrap()
        .contains("devmap integrate cursor"));
    integrate("cursor", &repo, &home);
    assert_eq!(
        doctor_warning_in(&repo, &home),
        None,
        "integrate cursor left a finding"
    );

    let (repo, home) = fresh("claude");
    put(&repo.join(".mcp.json"), &mcp_json("./devmap", &["mcp"]));
    assert!(doctor_warning_in(&repo, &home)
        .unwrap()
        .contains("devmap integrate claude"));
    integrate("claude", &repo, &home);
    assert_eq!(
        doctor_warning_in(&repo, &home),
        None,
        "integrate claude left a finding"
    );

    let (repo, home) = fresh("codex");
    put(
        &home.join(".codex/config.toml"),
        "[mcp_servers.devmap]\ncommand = \"devmap\"\nargs = [\"mcp\"]\n",
    );
    assert!(doctor_warning_in(&repo, &home)
        .unwrap()
        .contains("devmap integrate codex"));
    integrate("codex", &repo, &home);
    assert_eq!(
        doctor_warning_in(&repo, &home),
        None,
        "integrate codex left a finding"
    );

    // Hook handlers are not rewritten by integrate: the remedy is the edit itself,
    // naming the canonical path, and applying it clears the finding.
    for (tag, file, word) in [
        ("claude-hook", ".claude/settings.json", "devmap"),
        ("cursor-hook", ".cursor/hooks.json", "./devmap"),
    ] {
        let (repo, home) = fresh(tag);
        let body = serde_json::json!({"hooks": {"SessionStart": [{"command": format!("{word} hook session-start"), "hooks": [{"type": "command", "command": format!("{word} hook session-start")}]}]}}).to_string();
        put(&repo.join(file), &body);
        let warning = doctor_warning_in(&repo, &home).expect("flagged");
        let canonical = bin().canonicalize().unwrap();
        assert!(
            warning.contains(&format!(
                "replace {word:?} with \"{}\"",
                canonical.display()
            )),
            "{warning}"
        );
        let fixed = body.replace(word, canonical.to_str().unwrap());
        put(&repo.join(file), &fixed);
        assert_eq!(
            doctor_warning_in(&repo, &home),
            None,
            "{tag}: the named edit left a finding"
        );
    }
}
