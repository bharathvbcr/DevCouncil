//! Attack round on host-command resolution (ft-258f2688 lane D).
//!
//! Every case drives the real binary with HOME, XDG_CONFIG_HOME, CODEX_HOME and
//! CLAUDE_CONFIG_DIR pointed at scratch directories; nothing here reaches a real
//! host config. A case that needs the binary to be reached through another path
//! (a copy or hard link in a repository, a symlink) builds that path itself.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::common::host_cmd::*;

use serde_json::Value;

// (1) current_exe reached through links ---------------------------------------

#[test]
fn a_symlink_in_the_repository_to_an_outside_binary_is_never_what_gets_registered() {
    use std::os::unix::fs::symlink;
    let repo = scratch("link-in-repo");
    let home = scratch("link-in-repo-home");
    let link = repo.join("devmap");
    symlink(bin().canonicalize().unwrap(), &link).unwrap();
    let out = run_exe(&link, &["--json", "claude", "hooks"], &repo, &home, None);
    if out.status == Some(0) {
        let canonical = bin().canonicalize().unwrap();
        for command in hook_commands(&out.stdout) {
            assert!(
                command.starts_with(&format!("'{}' hook ", canonical.display())),
                "registered {command:?}, not the canonical outside binary"
            );
        }
    } else {
        assert!(out.text().contains("inside repository"), "{}", out.text());
    }
}

#[test]
fn an_outside_symlink_to_a_binary_inside_the_repository_is_refused() {
    use std::os::unix::fs::symlink;
    let repo = scratch("link-to-repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    let home = scratch("link-to-repo-home");
    let outside = scratch("link-to-repo-out");
    let inside = repo.join("bin/devmap");
    place_binary(&inside);
    let link = outside.join("devmap");
    symlink(&inside, &link).unwrap();
    let out = run_exe(&link, &["--json", "claude", "hooks"], &repo, &home, None);
    assert_ne!(out.status, Some(0), "{}", out.stdout);
    assert!(out.text().contains("inside repository"), "{}", out.text());
    assert!(
        !out.stdout.contains("\"hooks\""),
        "a hooks document was emitted for a binary that resolves into the repository: {}",
        out.stdout
    );
}

// (3) containment geometry ----------------------------------------------------

#[test]
fn a_binary_run_from_inside_its_own_repository_is_refused_with_the_remedy() {
    let repo = scratch("dev-in-repo");
    let home = scratch("dev-in-repo-home");
    let exe = repo.join("rust/target/debug/devmap");
    place_binary(&exe);
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    for cwd in [repo.clone(), repo.join("rust/target")] {
        for args in [
            vec!["--json", "claude", "hooks"],
            vec![
                "--json",
                "integrate",
                "codex",
                "--project-root",
                repo.to_str().unwrap(),
            ],
        ] {
            let out = run_exe(&exe, &args, &cwd, &home, None);
            assert_ne!(out.status, Some(0), "{args:?} from {cwd:?}: {}", out.stdout);
            assert!(
                out.text().contains("inside repository"),
                "{args:?}: refusal does not say why: {}",
                out.text()
            );
            assert!(
                !home.join(".codex/config.toml").exists(),
                "the global codex config was written"
            );
        }
    }
}

#[test]
fn a_repository_nested_inside_the_binary_directory_is_not_inside_it() {
    let outer = scratch("nested-repo");
    let home = scratch("nested-repo-home");
    let exe = outer.join("devmap");
    place_binary(&exe);
    let repo = outer.join("project");
    std::fs::create_dir_all(&repo).unwrap();
    let out = run_exe(&exe, &["--json", "claude", "hooks"], &repo, &home, None);
    assert_eq!(out.status, Some(0), "{}", out.text());
    for command in hook_commands(&out.stdout) {
        assert!(command.starts_with(&format!("'{}' hook ", exe.display())));
    }
}

#[test]
fn a_case_variant_spelling_of_the_repository_does_not_open_a_missing_binary_path() {
    let repo = scratch("case");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    let home = scratch("case-home");
    let upper = PathBuf::from(repo.to_str().unwrap().to_uppercase());
    if !upper.exists() {
        eprintln!("SKIPPED (not run): this filesystem is case-sensitive, so a case-variant spelling names a different directory");
        return;
    }
    let variant = upper.join("new/devmap");
    let out = run(
        &[
            "--json",
            "claude",
            "hooks",
            "--binary",
            variant.to_str().unwrap(),
        ],
        &repo,
        &home,
        None,
    );
    assert_ne!(
        out.status,
        Some(0),
        "a not-yet-existing path spelled with other case inside the repository was registered: {}",
        out.stdout
    );
}

// (2) --binary forms ----------------------------------------------------------

#[test]
fn binary_flag_relative_forms_are_resolved_absolute_or_refused() {
    let base = scratch("binflag");
    let home = scratch("binflag-home");
    let repo = base.join("repo");
    let outside = base.join("tools");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    place_binary(&outside.join("devmap"));
    place_binary(&repo.join("devmap"));
    let canonical_outside = outside.join("devmap").canonicalize().unwrap();

    // relative, existing, outside: registered as the absolute canonical path.
    let ok = run(
        &["--json", "claude", "hooks", "--binary", "../tools/devmap"],
        &repo,
        &home,
        None,
    );
    assert_eq!(ok.status, Some(0), "{}", ok.text());
    for command in hook_commands(&ok.stdout) {
        assert!(
            command.starts_with(&format!("'{}' hook ", canonical_outside.display())),
            "{command:?}"
        );
    }

    // relative, existing, inside (directly and by a .. detour): refused.
    for rel in [
        "devmap",
        "./devmap",
        "../repo/devmap",
        "../tools/../repo/devmap",
    ] {
        let out = run(
            &["--json", "claude", "hooks", "--binary", rel],
            &repo,
            &home,
            None,
        );
        assert_ne!(out.status, Some(0), "{rel}: {}", out.stdout);
    }

    // relative and missing: never registered.
    for rel in ["nope/devmap", "../tools/missing", "devmap-missing"] {
        let out = run(
            &["--json", "claude", "hooks", "--binary", rel],
            &repo,
            &home,
            None,
        );
        assert_ne!(out.status, Some(0), "{rel}: {}", out.stdout);
    }

    // absolute and missing, spelled to reach the repository through "..".
    for detour in [
        format!("{}/../repo/not-yet/devmap", outside.display()),
        format!("{}/./../repo//not-yet/devmap", outside.display()),
        format!("{}/not-yet/devmap", repo.display()),
    ] {
        let out = run(
            &["--json", "claude", "hooks", "--binary", &detour],
            &repo,
            &home,
            None,
        );
        assert_ne!(out.status, Some(0), "{detour}: {}", out.stdout);
    }
}

/// A path the shell would otherwise expand, split or end the quote on.
fn awkward_dir(tag: &str) -> PathBuf {
    scratch(tag).join("a b'c $(touch PWNED) \x60touch PWNED2\x60 \"q\\ é ü;&|")
}

fn hook_command_runs_the_stub_or_is_refused(dir: &Path, tag: &str) {
    let repo = scratch(&format!("{tag}-repo"));
    let home = scratch(&format!("{tag}-home"));
    let stub = dir.join("devmap");
    let proof = scratch(&format!("{tag}-proof")).join("ran");
    executable(&stub, "#!/bin/sh\nprintf '%s ' \"$@\" > \"$STUBOUT\"\n");
    let out = run(
        &[
            "--json",
            "claude",
            "hooks",
            "--binary",
            stub.to_str().unwrap(),
        ],
        &repo,
        &home,
        None,
    );
    if out.status != Some(0) {
        assert!(
            !out.stdout.contains("\"hooks\""),
            "a refusal still emitted a hooks document: {}",
            out.stdout
        );
        return;
    }
    let cwd = scratch(&format!("{tag}-cwd"));
    for command in hook_commands(&out.stdout) {
        let _ = std::fs::remove_file(&proof);
        let status = Command::new("/bin/sh")
            .arg("-c")
            .arg(&command)
            .current_dir(&cwd)
            .env("STUBOUT", &proof)
            .status()
            .unwrap();
        assert!(status.success(), "{command:?} did not run the stub");
        let seen = std::fs::read_to_string(&proof).expect("stub did not run");
        assert!(seen.starts_with("hook "), "stub saw {seen:?}");
    }
    for stray in ["PWNED", "PWNED2"] {
        assert!(
            !cwd.join(stray).exists(),
            "{stray} was created by shell expansion"
        );
        assert!(
            !dir.join(stray).exists(),
            "{stray} was created by shell expansion"
        );
        assert!(
            !repo.join(stray).exists(),
            "{stray} was created by shell expansion"
        );
    }
}

#[test]
fn a_hook_command_survives_spaces_quotes_dollars_backticks_and_unicode_in_the_path() {
    hook_command_runs_the_stub_or_is_refused(&awkward_dir("quote"), "quote");
}

#[test]
fn a_hook_command_for_a_path_with_a_newline_is_refused_or_runs_correctly() {
    hook_command_runs_the_stub_or_is_refused(&scratch("nl").join("a\nb"), "nl");
}

// (4) PATH entries ------------------------------------------------------------

fn command_values(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                match (key.as_str(), child) {
                    ("command", Value::String(s)) => out.push(s.clone()),
                    ("command", Value::Array(items)) => {
                        if let Some(Value::String(first)) = items.first() {
                            out.push(first.clone());
                        }
                    }
                    _ => command_values(child, out),
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|i| command_values(i, out)),
        _ => {}
    }
}

fn files_under(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.is_dir() {
            files_under(&path, out);
        } else if meta.is_file() {
            out.push(path);
        }
    }
}

/// The executable word of a written command: the quoted path or the first word.
fn first_word(command: &str) -> &str {
    match command.strip_prefix('\'') {
        Some(rest) => rest.split('\'').next().unwrap_or(""),
        None => command.split_whitespace().next().unwrap_or(""),
    }
}

/// Every command a written document names must be absolute, and never the
/// planted repository file.
fn assert_written_commands_absolute(roots: &[&Path], planted: &Path, ctx: &str) {
    let mut files = Vec::new();
    for root in roots {
        files_under(root, &mut files);
    }
    for file in files {
        if file == planted {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        assert!(
            !text.contains(planted.to_str().unwrap()),
            "{ctx}: {} names the planted repository executable",
            file.display()
        );
        let mut commands = Vec::new();
        match file.extension().and_then(|e| e.to_str()) {
            Some("json") => {
                if let Ok(value) = serde_json::from_str::<Value>(&text) {
                    command_values(&value, &mut commands);
                }
            }
            Some("toml") => {
                if let Some(command) = text.parse::<toml::Table>().ok().and_then(|t| {
                    t.get("mcp_servers")?
                        .get("devmap")?
                        .get("command")?
                        .as_str()
                        .map(str::to_string)
                }) {
                    commands.push(command);
                }
            }
            Some("mjs") => {
                assert!(
                    !text.contains("\"devmap\"") && !text.contains("'devmap'"),
                    "{ctx}: {} spawns a bare devmap",
                    file.display()
                );
            }
            _ => {}
        }
        for command in commands {
            assert!(
                Path::new(first_word(&command)).is_absolute(),
                "{ctx}: {} registers the relative command {command:?}",
                file.display()
            );
        }
    }
}

#[test]
fn every_writer_registers_the_outside_binary_whatever_leads_path() {
    use std::os::unix::fs::symlink;
    let bindir = scratch("pathlead-bin");
    symlink(bin().canonicalize().unwrap(), bindir.join("devmap")).unwrap();
    for (n, lead) in [".", "", ".:", ":", "./"].iter().enumerate() {
        let path = format!("{lead}:{}:/usr/bin:/bin", bindir.display());
        for host in [
            "claude",
            "cursor",
            "codex",
            "opencode",
            "antigravity",
            "warp",
        ] {
            let repo = scratch(&format!("pathlead-repo-{n}-{host}"));
            let home = scratch(&format!("pathlead-home-{n}-{host}"));
            let planted = repo.join("devmap");
            executable(&planted, "#!/bin/sh\nexit 0\n");
            let out = run(
                &[
                    "--json",
                    "integrate",
                    host,
                    "--project-root",
                    repo.to_str().unwrap(),
                ],
                &repo,
                &home,
                Some(&path),
            );
            assert_eq!(out.status, Some(0), "{host} PATH={path}: {}", out.text());
            assert_written_commands_absolute(
                &[&repo, &home],
                &planted,
                &format!("integrate {host} PATH={path}"),
            );
        }
        let repo = scratch(&format!("pathlead-hooks-{n}"));
        let home = scratch(&format!("pathlead-hooks-home-{n}"));
        executable(&repo.join("devmap"), "#!/bin/sh\nexit 0\n");
        let out = run(&["--json", "claude", "hooks"], &repo, &home, Some(&path));
        assert_eq!(out.status, Some(0), "{}", out.text());
        for command in hook_commands(&out.stdout) {
            assert!(command.starts_with("'/"), "PATH={path}: {command:?}");
        }
    }
}

// (5) doctor ------------------------------------------------------------------

type McpForm = (&'static str, Box<dyn Fn(&Path) -> (String, Vec<String>)>);

#[test]
fn doctor_flags_each_relative_or_in_repository_mcp_command_form() {
    let fixed =
        |name: &'static str, command: &'static str, args: &'static [&'static str]| -> McpForm {
            (
                name,
                Box::new(move |_| {
                    (
                        command.to_string(),
                        args.iter().map(|a| a.to_string()).collect(),
                    )
                }),
            )
        };
    let cases: Vec<McpForm> = vec![
        fixed("./devmap", "./devmap", &["mcp"]),
        fixed("devmap.exe", "devmap.exe", &["mcp"]),
        fixed("bin/devmap", "bin/devmap", &["mcp"]),
        fixed("../devmap", "../devmap", &["mcp"]),
        fixed("env devmap", "env", &["devmap", "mcp"]),
        fixed("sh -c devmap", "sh", &["-c", "devmap mcp"]),
        fixed(
            "$CLAUDE_PROJECT_DIR/devmap",
            "$CLAUDE_PROJECT_DIR/devmap",
            &["mcp"],
        ),
        (
            "absolute in repository",
            Box::new(|repo| {
                (
                    repo.join("devmap").display().to_string(),
                    vec!["mcp".into()],
                )
            }),
        ),
        (
            "absolute via ..",
            Box::new(|repo| {
                (
                    format!(
                        "{}/../{}/devmap",
                        repo.display(),
                        repo.file_name().unwrap().to_str().unwrap()
                    ),
                    vec!["mcp".into()],
                )
            }),
        ),
    ];
    let mut missed = Vec::new();
    for (name, build) in &cases {
        let warning = doctor_warning(|repo, home| {
            let (command, args) = build(repo);
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            executable(&repo.join("devmap"), "#!/bin/sh\nexit 0\n");
            put(&home.join(".cursor/mcp.json"), &mcp_json(&command, &args));
        });
        if warning.is_none() {
            missed.push(*name);
        }
    }
    assert!(missed.is_empty(), "doctor reported nothing for: {missed:?}");
}

#[test]
fn doctor_flags_a_codex_command_in_every_toml_spelling() {
    let spellings = [
        "[mcp_servers.devmap]\ncommand = \"devmap\"\nargs = [\"mcp\"]\n",
        "[mcp_servers.devmap]\ncommand = 'devmap'\n",
        "[mcp_servers.\"devmap\"]\ncommand = \"./devmap\"\n",
        "mcp_servers.devmap.command = \"devmap\"\n",
        "[mcp_servers]\ndevmap = { command = \"devmap\", args = [\"mcp\"] }\n",
        "mcp_servers = { devmap = { command = \"./devmap\", args = [\"mcp\"] } }\n",
        "\u{feff}[mcp_servers.devmap]\ncommand = \"devmap\"\n",
    ];
    let mut missed = Vec::new();
    for text in spellings {
        let warning = doctor_warning(|_repo, home| put(&home.join(".codex/config.toml"), text));
        if warning.is_none() {
            missed.push(text);
        }
    }
    assert!(
        missed.is_empty(),
        "doctor reported nothing for: {missed:#?}"
    );
}

#[test]
fn doctor_flags_a_hook_command_led_by_a_relative_or_expanded_word() {
    let commands = [
        "./devmap hook session-start",
        "devmap hook session-start",
        "bin/devmap hook session-start",
        "  ./devmap hook session-start",
        "$CLAUDE_PROJECT_DIR/devmap hook session-start",
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

#[test]
fn doctor_and_paths_survive_a_deeply_nested_or_huge_host_config() {
    let home = scratch("deep-home");
    let depth = 400_000;
    let json_deep = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
    let toml_arrays = format!("a = {}{}\n", "[".repeat(depth), "]".repeat(depth));
    let toml_tables = format!(
        "a = {}1{}\n",
        "{a=".repeat(depth / 4),
        "}".repeat(depth / 4)
    );
    let wide = format!(
        "{{\"hooks\": {{\"x\": [{}{{}}]}}}}",
        "{\"command\":\"/bin/true\"},".repeat(30_000)
    );
    let cases: Vec<(&str, &str, bool, String)> = vec![
        ("json-deep", ".mcp.json", false, json_deep.clone()),
        (
            "json-deep-cursor",
            ".cursor/mcp.json",
            false,
            json_deep.clone(),
        ),
        (
            "json-deep-claude",
            ".claude/settings.json",
            false,
            json_deep,
        ),
        ("toml-arrays", ".codex/config.toml", true, toml_arrays),
        ("toml-tables", ".codex/config.toml", true, toml_tables),
        ("json-wide", ".claude/settings.json", false, wide),
    ];
    for (name, rel, in_home, body) in cases {
        let cwd = scratch(&format!("deep-{name}"));
        let target = if in_home {
            home.join(rel)
        } else {
            cwd.join(rel)
        };
        put(&target, &body);
        for args in [["--json", "doctor"], ["--json", "paths"]] {
            let child = command_for(&bin(), &args, &cwd, &home, None)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .stdin(Stdio::null())
                .spawn()
                .unwrap();
            let status = finished_within(child, Duration::from_secs(120));
            let status = status.unwrap_or_else(|| panic!("{name} {args:?}: did not return"));
            assert!(
                status.code().is_some(),
                "{name} {args:?}: killed by a signal ({status:?}), a stack overflow on a 1 MiB config"
            );
        }
        std::fs::remove_file(&target).ok();
    }
}

// (6) non-regular host documents ----------------------------------------------

#[test]
fn a_host_document_that_is_a_link_to_a_fifo_or_directory_does_not_block_doctor() {
    use std::os::unix::fs::symlink;
    let home = scratch("fifo-home");
    for kind in ["fifo", "dir"] {
        let cwd = scratch(&format!("fifo-{kind}"));
        let real = cwd.join("real");
        if kind == "fifo" {
            assert!(Command::new("mkfifo")
                .arg(&real)
                .status()
                .unwrap()
                .success());
        } else {
            std::fs::create_dir(&real).unwrap();
        }
        for name in [".mcp.json", ".cursor/mcp.json", ".claude/settings.json"] {
            let link = cwd.join(name);
            std::fs::create_dir_all(link.parent().unwrap()).unwrap();
            symlink(&real, &link).unwrap();
        }
        let child = command_for(&bin(), &["--json", "doctor"], &cwd, &home, None)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .stdin(Stdio::null())
            .spawn()
            .unwrap();
        assert!(
            finished_within(child, Duration::from_secs(120)).is_some(),
            "doctor blocked on a {kind} behind a link"
        );
    }
}

// (7) preflight_write ---------------------------------------------------------

fn opencode(repo: &Path, home: &Path) -> Run {
    run(
        &[
            "integrate",
            "opencode",
            "--project-root",
            repo.to_str().unwrap(),
        ],
        repo,
        home,
        Some("/usr/bin:/bin"),
    )
}

#[test]
fn a_linked_integrations_path_is_refused_and_writes_nothing_beyond_the_link() {
    use std::os::unix::fs::symlink;
    let home = scratch("pre-home");

    // integrations -> outside; .devcouncil -> outside (a link two levels up).
    for shape in ["integrations", "devcouncil"] {
        let repo = scratch(&format!("pre-{shape}"));
        let outside = scratch(&format!("pre-{shape}-out"));
        if shape == "integrations" {
            std::fs::create_dir_all(repo.join(".devcouncil")).unwrap();
            symlink(&outside, repo.join(".devcouncil/integrations")).unwrap();
        } else {
            symlink(&outside, repo.join(".devcouncil")).unwrap();
        }
        let out = opencode(&repo, &home);
        assert_ne!(out.status, Some(0), "{shape}: {}", out.text());
        let mut written = Vec::new();
        files_under(&outside, &mut written);
        assert!(
            written.is_empty(),
            "{shape}: wrote through the link: {written:?}"
        );
        assert!(
            !repo.join("opencode.json").exists(),
            "{shape}: half-applied"
        );
    }

    // A dangling link at the integrations directory and at the final file.
    for shape in ["dangling-dir", "dangling-file"] {
        let repo = scratch(&format!("pre-{shape}"));
        let ghost = scratch(&format!("pre-{shape}-out")).join("ghost");
        if shape == "dangling-dir" {
            std::fs::create_dir_all(repo.join(".devcouncil")).unwrap();
            symlink(&ghost, repo.join(".devcouncil/integrations")).unwrap();
        } else {
            symlink(&ghost, repo.join("opencode.json")).unwrap();
        }
        let out = opencode(&repo, &home);
        assert_ne!(out.status, Some(0), "{shape}: {}", out.text());
        assert!(!ghost.exists(), "{shape}: created the link's target");
    }
}

#[test]
fn an_integrations_link_to_a_directory_inside_the_repository_is_also_refused() {
    use std::os::unix::fs::symlink;
    let home = scratch("pre-in-home");
    let repo = scratch("pre-in");
    std::fs::create_dir_all(repo.join(".devcouncil")).unwrap();
    std::fs::create_dir_all(repo.join("elsewhere")).unwrap();
    symlink(
        repo.join("elsewhere"),
        repo.join(".devcouncil/integrations"),
    )
    .unwrap();
    let out = opencode(&repo, &home);
    assert_ne!(out.status, Some(0), "{}", out.text());
    let mut written = Vec::new();
    files_under(&repo.join("elsewhere"), &mut written);
    assert!(
        written.is_empty(),
        "wrote through an in-repository link: {written:?}"
    );
}
