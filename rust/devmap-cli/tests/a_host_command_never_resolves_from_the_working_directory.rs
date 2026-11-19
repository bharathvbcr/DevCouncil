//! A host entry never names a `devmap` the working directory or the repository
//! can supply.
//!
//! `devmap integrate`, `devmap claude hooks` and the plugin writer used to emit
//! the bare name `devmap` whenever the first `devmap` on `PATH` was the running
//! binary, and the writers then resolved that name against the *working
//! directory*. A file named `devmap` at a repository root therefore became the
//! command Claude, Cursor and Codex hooks ran in every session (and, for Codex,
//! the command in the user-global `~/.codex/config.toml`). The Go host runs
//! `devmap integrate` with the repository root as its working directory, so that
//! was the default path.
//!
//! Driven through the binary. Every HOME is a scratch directory; nothing here
//! reaches the user's own host configs. `DEVMAP_UNDER_TEST` names another
//! binary to run the same assertions against (to show they fail on an older one).

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

fn bin() -> PathBuf {
    std::env::var_os("DEVMAP_UNDER_TEST")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_devmap")))
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "devmap-hostcmd-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    // The canonical spelling: macOS temp dirs sit behind a symlink.
    dir.canonicalize().unwrap()
}

struct Run {
    status: Option<i32>,
    stdout: String,
    stderr: String,
}

fn command(args: &[&str], cwd: &Path, home: &Path, path: Option<&Path>) -> Command {
    let mut cmd = Command::new(bin());
    cmd.args(args)
        .current_dir(cwd)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("CODEX_HOME", home.join(".codex"))
        .env_remove("CLAUDE_CONFIG_DIR");
    if let Some(path) = path {
        cmd.env("PATH", path);
    }
    cmd
}

fn run(args: &[&str], cwd: &Path, home: &Path, path: Option<&Path>) -> Run {
    let out = command(args, cwd, home, path)
        .output()
        .expect("devmap runs");
    Run {
        status: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

/// A repository root holding an executable file named `devmap`, and a `PATH`
/// directory whose first `devmap` is the binary under test.
struct Planted {
    repo: PathBuf,
    home: PathBuf,
    bindir: PathBuf,
    canonical: PathBuf,
}

fn planted(tag: &str) -> Planted {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let repo = scratch(&format!("{tag}-repo"));
    let home = scratch(&format!("{tag}-home"));
    let bindir = scratch(&format!("{tag}-bin"));
    let fake = repo.join("devmap");
    std::fs::write(&fake, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let canonical = bin().canonicalize().unwrap();
    symlink(&canonical, bindir.join("devmap")).unwrap();
    Planted {
        repo,
        home,
        bindir,
        canonical,
    }
}

fn hook_commands(block: &Value) -> Vec<String> {
    let mut out = Vec::new();
    for groups in block["hooks"].as_object().expect("hooks object").values() {
        for group in groups.as_array().unwrap() {
            for handler in group["hooks"].as_array().unwrap() {
                out.push(handler["command"].as_str().unwrap().to_string());
            }
        }
    }
    assert!(!out.is_empty(), "no hook commands emitted");
    out
}

fn assert_all_name(commands: &[String], canonical: &Path) {
    let expected = format!("'{}' hook ", canonical.display());
    for command in commands {
        assert!(
            command.starts_with(&expected),
            "hook command {command:?} does not start with the binary under test {expected:?}"
        );
    }
}

#[test]
fn claude_hooks_name_the_binary_under_test_not_a_devmap_in_the_working_directory() {
    let p = planted("hooks");
    // PATH puts the binary under test first: the case that used to emit the bare name.
    let first = run(
        &["--json", "claude", "hooks"],
        &p.repo,
        &p.home,
        Some(&p.bindir),
    );
    assert_eq!(first.status, Some(0), "{}", first.stderr);
    assert_all_name(
        &hook_commands(&serde_json::from_str(&first.stdout).unwrap()),
        &p.canonical,
    );

    // No `devmap` on PATH at all: absolute too.
    let empty = scratch("hooks-empty-path");
    let none = run(
        &["--json", "claude", "hooks"],
        &p.repo,
        &p.home,
        Some(&empty),
    );
    assert_eq!(none.status, Some(0), "{}", none.stderr);
    assert_all_name(
        &hook_commands(&serde_json::from_str(&none.stdout).unwrap()),
        &p.canonical,
    );
}

#[test]
fn integrate_registers_the_outside_binary_in_every_host_document() {
    let p = planted("integrate");
    let root = p.repo.to_str().unwrap();
    let codex = run(
        &["--json", "integrate", "codex", "--project-root", root],
        &p.repo,
        &p.home,
        Some(&p.bindir),
    );
    assert_eq!(codex.status, Some(0), "{}\n{}", codex.stdout, codex.stderr);
    let toml: toml::Table = std::fs::read_to_string(p.home.join(".codex/config.toml"))
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(
        toml["mcp_servers"]["devmap"]["command"].as_str().unwrap(),
        p.canonical.to_str().unwrap(),
        "the user-global Codex command is not the binary under test"
    );
    let plugin_hooks = p.repo.join(".codex-plugin/hooks/hooks.json");
    if plugin_hooks.is_file() {
        let block: Value = serde_json::from_slice(&std::fs::read(&plugin_hooks).unwrap()).unwrap();
        assert_all_name(&hook_commands(&block), &p.canonical);
    }

    let cursor = run(
        &["--json", "integrate", "cursor", "--project-root", root],
        &p.repo,
        &p.home,
        Some(&p.bindir),
    );
    assert_eq!(
        cursor.status,
        Some(0),
        "{}\n{}",
        cursor.stdout,
        cursor.stderr
    );
    let mcp: Value =
        serde_json::from_slice(&std::fs::read(p.home.join(".cursor/mcp.json")).unwrap()).unwrap();
    assert_eq!(
        mcp["mcpServers"]["devmap"]["command"].as_str().unwrap(),
        p.canonical.to_str().unwrap()
    );
    let hooks: Value =
        serde_json::from_slice(&std::fs::read(p.repo.join(".cursor/hooks.json")).unwrap()).unwrap();
    for entries in hooks["hooks"].as_object().unwrap().values() {
        for entry in entries.as_array().unwrap() {
            let command = entry["command"].as_str().unwrap();
            assert!(
                command.starts_with(&format!("'{}' hook ", p.canonical.display())),
                "cursor hook {command:?} is not the binary under test"
            );
        }
    }
}

#[test]
fn an_explicit_binary_inside_the_repository_is_refused_before_anything_is_written() {
    let p = planted("inside");
    let fake = p.repo.join("devmap");
    let before = [std::fs::read_dir(&p.repo).unwrap().count()];
    let refused = run(
        &[
            "integrate",
            "codex",
            "--project-root",
            p.repo.to_str().unwrap(),
            "--binary",
            fake.to_str().unwrap(),
        ],
        &p.repo,
        &p.home,
        Some(&p.bindir),
    );
    assert_ne!(refused.status, Some(0), "{}", refused.stdout);
    assert!(
        refused.stderr.contains("inside repository")
            || refused.stdout.contains("inside repository"),
        "the refusal must say why:\n{}\n{}",
        refused.stdout,
        refused.stderr
    );
    assert!(
        !p.home.join(".codex/config.toml").exists(),
        "the global Codex config was written"
    );
    assert_eq!(
        std::fs::read_dir(&p.repo).unwrap().count(),
        before[0],
        "the repository gained files"
    );
}

/// Existing entries written by earlier binaries are found.
#[test]
fn doctor_reports_a_relative_or_in_repository_host_command_with_the_rewrite() {
    let p = planted("doctor");
    std::fs::create_dir_all(p.home.join(".cursor")).unwrap();
    std::fs::write(
        p.home.join(".cursor/mcp.json"),
        r#"{"mcpServers":{"devmap":{"type":"stdio","command":"devmap","args":["mcp"]}}}"#,
    )
    .unwrap();
    std::fs::create_dir_all(p.home.join(".codex")).unwrap();
    std::fs::write(
        p.home.join(".codex/config.toml"),
        format!(
            "[mcp_servers.devmap]\ncommand = \"{}\"\nargs = [\"mcp\"]\n",
            p.repo.join("devmap").display()
        ),
    )
    .unwrap();
    let out = run(&["--json", "doctor"], &p.repo, &p.home, Some(&p.bindir));
    let report: Value = serde_json::from_str(&out.stdout)
        .unwrap_or_else(|e| panic!("doctor is not JSON ({e}): {}\n{}", out.stdout, out.stderr));
    let warning = report["host_command_defect_warning"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "no defect reported: {}",
                report["host_command_defect_warning"]
            )
        });
    assert!(warning.contains("mcp.json"), "{warning}");
    assert!(warning.contains("relative"), "{warning}");
    assert!(warning.contains("devmap integrate cursor"), "{warning}");
    assert!(warning.contains("config.toml"), "{warning}");
    assert!(warning.contains("inside the repository"), "{warning}");
    assert!(warning.contains("devmap integrate codex"), "{warning}");
}

#[test]
fn doctor_is_quiet_about_an_absolute_outside_command() {
    let p = planted("doctor-clean");
    std::fs::create_dir_all(p.home.join(".cursor")).unwrap();
    std::fs::write(
        p.home.join(".cursor/mcp.json"),
        format!(
            r#"{{"mcpServers":{{"devmap":{{"command":"{}","args":["mcp"]}}}}}}"#,
            p.canonical.display()
        ),
    )
    .unwrap();
    let out = run(&["--json", "doctor"], &p.repo, &p.home, Some(&p.bindir));
    let report: Value = serde_json::from_str(&out.stdout).unwrap();
    assert!(report["host_command_defect_warning"].is_null(), "{report}");
}

#[test]
fn integrate_opencode_refuses_a_linked_integrations_directory_in_the_plan_pass() {
    use std::os::unix::fs::symlink;
    let p = planted("opencode");
    std::fs::remove_file(p.repo.join("devmap")).unwrap();
    let outside = scratch("opencode-outside");
    let state = p.repo.join(".devcouncil");
    std::fs::create_dir_all(&state).unwrap();
    symlink(&outside, state.join("integrations")).unwrap();

    let refused = run(
        &[
            "integrate",
            "opencode",
            "--project-root",
            p.repo.to_str().unwrap(),
        ],
        &p.repo,
        &p.home,
        Some(&p.bindir),
    );
    assert_ne!(
        refused.status,
        Some(0),
        "{}\n{}",
        refused.stdout,
        refused.stderr
    );
    assert_eq!(
        std::fs::read_dir(&outside).unwrap().count(),
        0,
        "the plugin was written through the link"
    );
    let names: Vec<_> = std::fs::read_dir(&p.repo)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(
        names,
        [".devcouncil"],
        "a refused plan left files behind: {names:?}"
    );
}

/// Waits for `child` for at most `limit`; a hang is a failure, not a stuck suite.
fn finished_within(mut child: Child, limit: Duration) -> Option<std::process::ExitStatus> {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return Some(status);
        }
        if start.elapsed() > limit {
            child.kill().ok();
            child.wait().ok();
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn paths_and_doctor_return_promptly_on_a_fifo_or_oversized_mcp_json() {
    let home = scratch("bounded-home");
    for (shape, args) in [
        ("fifo", ["--json", "paths"]),
        ("fifo", ["--json", "doctor"]),
        ("huge", ["--json", "paths"]),
        ("huge", ["--json", "doctor"]),
    ] {
        let cwd = scratch(&format!("bounded-{shape}"));
        let target = cwd.join(".mcp.json");
        if shape == "fifo" {
            let made = Command::new("mkfifo").arg(&target).status().unwrap();
            assert!(made.success(), "mkfifo failed");
        } else {
            let file = std::fs::File::create(&target).unwrap();
            file.set_len(1 << 30).unwrap();
        }
        let child = command(&args, &cwd, &home, None)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .stdin(Stdio::null())
            .spawn()
            .unwrap();
        let status = finished_within(child, Duration::from_secs(120));
        assert!(
            status.is_some(),
            "{args:?} did not return with a {shape} .mcp.json: the read is unbounded"
        );
    }
}

/// A packager names the install location before anything is installed there.
#[test]
fn a_binary_flag_naming_a_path_not_installed_yet_is_still_written_when_outside() {
    let p = planted("packager");
    let future = scratch("packager-prefix").join("bin/devmap");
    let ok = run(
        &[
            "--json",
            "claude",
            "hooks",
            "--binary",
            future.to_str().unwrap(),
        ],
        &p.repo,
        &p.home,
        Some(&p.bindir),
    );
    assert_eq!(ok.status, Some(0), "{}", ok.stderr);
    assert!(
        ok.stdout.contains(future.to_str().unwrap()),
        "{}",
        ok.stdout
    );

    let inside = p.repo.join("bin/devmap");
    let refused = run(
        &[
            "--json",
            "claude",
            "hooks",
            "--binary",
            inside.to_str().unwrap(),
        ],
        &p.repo,
        &p.home,
        Some(&p.bindir),
    );
    assert_ne!(refused.status, Some(0), "{}", refused.stdout);
}
