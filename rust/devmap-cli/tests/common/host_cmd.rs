//! One set of helpers for the host-command tests (`a_host_command_never_resolves…`,
//! `attack_host_command_resolution*`): drive the real binary with HOME, XDG, CODEX_HOME
//! pointed at scratch directories and CLAUDE_CONFIG_DIR removed, so nothing reaches a
//! real host config. `DEVMAP_UNDER_TEST` names another binary to run the same cases
//! against (to show they fail on an older one).

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

pub fn bin() -> PathBuf {
    std::env::var_os("DEVMAP_UNDER_TEST")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_devmap")))
}

/// A fresh canonical scratch directory.
pub fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "devmap-hostcmd-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

pub struct Run {
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Run {
    pub fn text(&self) -> String {
        format!("{}\n{}", self.stdout, self.stderr)
    }
}

pub fn command_for(
    exe: &Path,
    args: &[&str],
    cwd: &Path,
    home: &Path,
    path: Option<&str>,
) -> Command {
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

pub fn run_exe(exe: &Path, args: &[&str], cwd: &Path, home: &Path, path: Option<&str>) -> Run {
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

pub fn run(args: &[&str], cwd: &Path, home: &Path, path: Option<&str>) -> Run {
    run_exe(&bin(), args, cwd, home, path)
}

/// A hard link of the binary at dest, else a copy: the binary reached through
/// a different path, so current_exe is that path.
pub fn place_binary(dest: &Path) {
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    if std::fs::hard_link(bin().canonicalize().unwrap(), dest).is_err() {
        std::fs::copy(bin(), dest).unwrap();
    }
}

/// Every handler `command` of a `{"hooks": {...}}` block.
pub fn hook_commands_of(block: &Value) -> Vec<String> {
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

pub fn hook_commands(stdout: &str) -> Vec<String> {
    hook_commands_of(&serde_json::from_str(stdout).expect("hooks json"))
}

/// Waits for `child` for at most `limit`; a hang is a failure, not a stuck suite.
pub fn finished_within(mut child: Child, limit: Duration) -> Option<std::process::ExitStatus> {
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

pub fn put(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

pub fn executable(path: &Path, text: &str) {
    use std::os::unix::fs::PermissionsExt;
    put(path, text);
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// `doctor --json`'s `host_command_defect_warning` after `place(repo, home)`
/// has planted host documents; `repo` is a git repository (it holds `.git`).
pub fn doctor_warning(place: impl Fn(&Path, &Path)) -> Option<String> {
    let repo = scratch("doctor-repo");
    let home = scratch("doctor-home");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    place(&repo, &home);
    doctor_warning_in(&repo, &home)
}

pub fn doctor_warning_in(repo: &Path, home: &Path) -> Option<String> {
    let out = run(&["--json", "doctor"], repo, home, Some("/usr/bin:/bin"));
    let report: Value = serde_json::from_str(&out.stdout)
        .unwrap_or_else(|e| panic!("doctor is not JSON ({e}): {}\n{}", out.stdout, out.stderr));
    report["host_command_defect_warning"]
        .as_str()
        .map(str::to_string)
}

pub fn mcp_json(command: &str, args: &[&str]) -> String {
    serde_json::json!({"mcpServers": {"devmap": {"command": command, "args": args}}}).to_string()
}
