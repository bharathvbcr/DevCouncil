//! Attack round on host-command resolution: cases that BREAK the implementation
//! under test as of 0b5232c5 (ft-258f2688 lane D); fixed in the fix-round commit.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_devmap"))
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "devmap-attack-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

struct Run {
    status: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Run {
    fn text(&self) -> String {
        format!("{}\n{}", self.stdout, self.stderr)
    }
}

fn command_for(exe: &Path, args: &[&str], cwd: &Path, home: &Path, path: Option<&str>) -> Command {
    let mut cmd = Command::new(exe);
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

fn run_exe(exe: &Path, args: &[&str], cwd: &Path, home: &Path, path: Option<&str>) -> Run {
    let out = command_for(exe, args, cwd, home, path)
        .stdin(Stdio::null())
        .output()
        .expect("devmap runs");
    Run {
        status: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

fn run(args: &[&str], cwd: &Path, home: &Path, path: Option<&str>) -> Run {
    run_exe(&bin(), args, cwd, home, path)
}

/// A hard link of the binary at dest, else a copy: the binary reached through
/// a different path, so current_exe is that path.
fn place_binary(dest: &Path) {
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    if std::fs::hard_link(bin().canonicalize().unwrap(), dest).is_err() {
        std::fs::copy(bin(), dest).unwrap();
    }
}

fn hook_commands(stdout: &str) -> Vec<String> {
    let block: Value = serde_json::from_str(stdout).expect("hooks json");
    let mut out = Vec::new();
    for groups in block["hooks"].as_object().expect("hooks object").values() {
        for group in groups.as_array().unwrap() {
            for handler in group["hooks"].as_array().unwrap() {
                out.push(handler["command"].as_str().unwrap().to_string());
            }
        }
    }
    assert!(!out.is_empty());
    out
}

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

fn put(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn executable(path: &Path, text: &str) {
    use std::os::unix::fs::PermissionsExt;
    put(path, text);
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn doctor_warning(place: impl Fn(&Path, &Path)) -> Option<String> {
    let repo = scratch("doctor-repo");
    let home = scratch("doctor-home");
    place(&repo, &home);
    let out = run(&["--json", "doctor"], &repo, &home, Some("/usr/bin:/bin"));
    let report: Value = serde_json::from_str(&out.stdout)
        .unwrap_or_else(|e| panic!("doctor is not JSON ({e}): {}\n{}", out.stdout, out.stderr));
    report["host_command_defect_warning"]
        .as_str()
        .map(str::to_string)
}

fn mcp_json(command: &str, args: &[&str]) -> String {
    serde_json::json!({"mcpServers": {"devmap": {"command": command, "args": args}}}).to_string()
}

#[test]
fn a_missing_binary_path_behind_an_outside_link_into_the_repository_is_not_registered() {
    use std::os::unix::fs::symlink;
    let base = scratch("missing-behind-link");
    let home = scratch("missing-behind-link-home");
    let repo = base.join("repo");
    let outside = base.join("out");
    std::fs::create_dir_all(repo.join("bin")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    symlink(repo.join("bin"), outside.join("link")).unwrap();
    // The file does not exist yet, so the check is lexical: out/link/devmap
    // looks outside, and the first file placed at repo/bin/devmap is what runs.
    let spelled = outside.join("link/devmap");
    let out = run(
        &["--json", "claude", "hooks", "--binary", spelled.to_str().unwrap()],
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
    assert!(silent.is_empty(), "doctor was silent on unreadable configs: {silent:#?}");
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
    assert!(missed.is_empty(), "doctor reported nothing for: {missed:#?}");
}
