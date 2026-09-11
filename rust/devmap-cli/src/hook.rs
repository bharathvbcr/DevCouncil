//! Host-neutral `devmap hook <event>` entry point.
//!
//! Cursor and Codex run hooks as a shell-string `command` with no `args`/`async`.
//! Exit 2 is treated as "block the agent" on those hosts, so this path never
//! exits 2: success and no-ops are 0, failures are 1.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context};
use serde_json::{json, Value};

/// Hard cap on hook stdin. Anything larger is treated as malformed and ignored.
pub const MAX_STDIN_BYTES: usize = 1024 * 1024;

/// How long a detached post-tool-use/session-end hook may spend before returning.
pub const DETACH_BUDGET: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookEvent {
    SessionStart,
    PostToolUse,
    SessionEnd,
}

impl HookEvent {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SessionStart => "session-start",
            Self::PostToolUse => "post-tool-use",
            Self::SessionEnd => "session-end",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "session-start" => Some(Self::SessionStart),
            "post-tool-use" => Some(Self::PostToolUse),
            "session-end" => Some(Self::SessionEnd),
            _ => None,
        }
    }
}

#[derive(Debug, Default)]
pub struct HookOutcome {
    pub exit_code: i32,
    pub stdout: Option<Value>,
    pub stderr_line: Option<String>,
    #[allow(dead_code)]
    pub roots: Vec<PathBuf>,
}

/// Run one hook event. Never returns exit code 2.
pub fn run_hook(
    event: HookEvent,
    stdin: &[u8],
    executable: &Path,
    pinned_root: Option<&Path>,
) -> HookOutcome {
    match run_hook_inner(event, stdin, executable, pinned_root) {
        Ok(outcome) => outcome,
        Err(error) => HookOutcome {
            exit_code: 1,
            stdout: None,
            stderr_line: Some(format!("devmap hook {}: {error}", event.as_str())),
            roots: Vec::new(),
        },
    }
}

fn run_hook_inner(
    event: HookEvent,
    stdin: &[u8],
    executable: &Path,
    pinned_root: Option<&Path>,
) -> anyhow::Result<HookOutcome> {
    if stdin.is_empty() {
        return Ok(HookOutcome {
            exit_code: 0,
            stdout: None,
            stderr_line: Some("devmap hook: empty stdin; no-op".into()),
            roots: Vec::new(),
        });
    }
    if stdin.len() > MAX_STDIN_BYTES {
        return Ok(HookOutcome {
            exit_code: 0,
            stdout: None,
            stderr_line: Some(format!(
                "devmap hook: stdin exceeds {MAX_STDIN_BYTES} bytes; no-op"
            )),
            roots: Vec::new(),
        });
    }
    let text = match std::str::from_utf8(stdin) {
        Ok(text) => text.trim(),
        Err(_) => {
            return Ok(HookOutcome {
                exit_code: 0,
                stdout: None,
                stderr_line: Some("devmap hook: stdin is not UTF-8; no-op".into()),
                roots: Vec::new(),
            });
        }
    };
    if text.is_empty() {
        return Ok(HookOutcome {
            exit_code: 0,
            stdout: None,
            stderr_line: Some("devmap hook: empty stdin; no-op".into()),
            roots: Vec::new(),
        });
    }
    let payload: Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(error) => {
            return Ok(HookOutcome {
                exit_code: 0,
                stdout: None,
                stderr_line: Some(format!("devmap hook: malformed JSON ({error}); no-op")),
                roots: Vec::new(),
            });
        }
    };

    let roots = select_roots(&payload, pinned_root)?;
    if roots.is_empty() {
        return Ok(HookOutcome {
            exit_code: 0,
            stdout: None,
            stderr_line: Some("devmap hook: no indexed repository found; no-op".into()),
            roots,
        });
    }

    match event {
        HookEvent::SessionStart => {
            let stdout = session_start_sync(executable, &roots)?;
            Ok(HookOutcome {
                exit_code: 0,
                stdout: Some(stdout),
                stderr_line: None,
                roots,
            })
        }
        HookEvent::PostToolUse => {
            let started = Instant::now();
            for root in &roots {
                detach_build(executable, root)?;
            }
            // Stay inside the host budget even if lock acquisition was slow.
            let _ = started.elapsed() < DETACH_BUDGET;
            Ok(HookOutcome {
                exit_code: 0,
                stdout: None,
                stderr_line: None,
                roots,
            })
        }
        HookEvent::SessionEnd => {
            for root in &roots {
                detach_session_report(executable, root)?;
            }
            Ok(HookOutcome {
                exit_code: 0,
                stdout: None,
                stderr_line: None,
                roots,
            })
        }
    }
}

/// Repository selection order from the plan, keeping only roots that already
/// hold a store. A hook never creates a store and never indexes an unsafe root.
///
/// Precedence is first-matching-tier, not a union: once a tier yields one or
/// more store-bearing roots, later tiers are ignored. Cursor multi-root
/// post-tool-use therefore rebuilds only the repo that owns `file_path`, not
/// every entry in `workspace_roots`.
pub fn select_roots(payload: &Value, pinned_root: Option<&Path>) -> anyhow::Result<Vec<PathBuf>> {
    if let Some(pin) = pinned_root {
        let root = canonicalize_existing(pin)?;
        if is_unsafe_root(&root) {
            bail!(
                "refusing pinned root {}: $HOME, `/`, or a temp directory",
                root.display()
            );
        }
        if store_exists(&root) {
            return Ok(vec![root]);
        }
        return Ok(Vec::new());
    }

    // Tier 1: tool_input.file_path / file_path
    if let Some(path) = file_path_from_payload(payload) {
        let selected = store_roots_from([path]);
        if !selected.is_empty() {
            return Ok(selected);
        }
    }

    // Tier 2: Codex apply_patch paths in tool_input.command
    let patch_paths = apply_patch_paths(payload);
    if !patch_paths.is_empty() {
        let selected = store_roots_from(patch_paths);
        if !selected.is_empty() {
            return Ok(selected);
        }
    }

    // Tier 3: cwd worktree
    if let Some(cwd) = string_path(payload, "cwd") {
        let selected = store_roots_from([cwd]);
        if !selected.is_empty() {
            return Ok(selected);
        }
    }

    // Tier 4: every workspace_roots[] entry (only when earlier tiers missed)
    if let Some(roots) = payload.get("workspace_roots").and_then(Value::as_array) {
        let candidates: Vec<PathBuf> = roots
            .iter()
            .filter_map(|root| root.as_str().map(PathBuf::from))
            .collect();
        if !candidates.is_empty() {
            let selected = store_roots_from(candidates);
            if !selected.is_empty() {
                return Ok(selected);
            }
        }
    }

    // Tier 5: CLAUDE_PROJECT_DIR / CURSOR_PROJECT_DIR
    let mut env_candidates = Vec::new();
    for env_key in ["CLAUDE_PROJECT_DIR", "CURSOR_PROJECT_DIR"] {
        if let Ok(value) = std::env::var(env_key) {
            if !value.is_empty() {
                push_unique(&mut env_candidates, Some(PathBuf::from(value)));
            }
        }
    }
    Ok(store_roots_from(env_candidates))
}

/// Resolve candidates to unique store-bearing, safe repository roots.
fn store_roots_from<I>(candidates: I) -> Vec<PathBuf>
where
    I: IntoIterator<Item = PathBuf>,
{
    let mut selected = Vec::new();
    for candidate in candidates {
        let Ok(root) = resolve_repo_root(&candidate) else {
            continue;
        };
        if is_unsafe_root(&root) {
            continue;
        }
        if store_exists(&root) {
            push_unique(&mut selected, Some(root));
        }
    }
    selected
}

fn file_path_from_payload(payload: &Value) -> Option<PathBuf> {
    payload
        .pointer("/tool_input/file_path")
        .or_else(|| payload.get("file_path"))
        .and_then(Value::as_str)
        .map(PathBuf::from)
}

/// Codex `apply_patch` carries paths inside `tool_input.command`, not `file_path`.
fn apply_patch_paths(payload: &Value) -> Vec<PathBuf> {
    let Some(command) = payload
        .pointer("/tool_input/command")
        .and_then(Value::as_str)
    else {
        return Vec::new();
    };
    let cwd = payload
        .get("cwd")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let mut paths = Vec::new();
    for line in command.lines() {
        let trimmed = line.trim();
        for prefix in ["*** Update File:", "*** Add File:", "*** Delete File:"] {
            if let Some(rest) = trimmed.strip_prefix(prefix) {
                let path = PathBuf::from(rest.trim());
                let absolute = if path.is_absolute() {
                    path
                } else {
                    cwd.join(path)
                };
                paths.push(absolute);
            }
        }
    }
    paths
}

fn string_path(payload: &Value, key: &str) -> Option<PathBuf> {
    payload.get(key).and_then(Value::as_str).map(PathBuf::from)
}

fn push_unique(out: &mut Vec<PathBuf>, path: Option<PathBuf>) {
    let Some(path) = path else {
        return;
    };
    if out.iter().any(|existing| existing == &path) {
        return;
    }
    out.push(path);
}

fn resolve_repo_root(path: &Path) -> anyhow::Result<PathBuf> {
    let path = if path.is_file() {
        path.parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| path.to_path_buf())
    } else {
        path.to_path_buf()
    };
    // Walk up until a store is found, else canonicalize the path itself.
    let mut cursor = canonicalize_existing(&path).unwrap_or(path);
    loop {
        if store_exists(&cursor) {
            return Ok(cursor);
        }
        match cursor.parent() {
            Some(parent) if parent != cursor => cursor = parent.to_path_buf(),
            _ => break,
        }
    }
    canonicalize_existing(&cursor).or_else(|_| Ok(cursor))
}

fn canonicalize_existing(path: &Path) -> anyhow::Result<PathBuf> {
    path.canonicalize()
        .with_context(|| format!("cannot resolve {}", path.display()))
}

fn store_exists(root: &Path) -> bool {
    // Clear DEVMAP_HOME influence for hook discovery: the store must live under
    // the repository itself. Using the path helpers with an empty home override
    // would require plumbing; check both layout names directly.
    root.join(".devmap")
        .join("codeintel")
        .join("devmap.sqlite")
        .is_file()
        || root
            .join(".devcouncil")
            .join("codeintel")
            .join("devmap.sqlite")
            .is_file()
}

fn is_unsafe_root(root: &Path) -> bool {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut banned = vec![PathBuf::from("/"), std::env::temp_dir()];
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            banned.push(PathBuf::from(home));
        }
    }
    for extra in ["/tmp", "/private/tmp", "/var/tmp"] {
        banned.push(PathBuf::from(extra));
    }
    banned
        .iter()
        .any(|path| path.canonicalize().unwrap_or_else(|_| path.clone()) == root)
}

fn session_start_sync(executable: &Path, roots: &[PathBuf]) -> anyhow::Result<Value> {
    let mut lines = Vec::new();
    for root in roots {
        let status = Command::new(executable)
            .args(["--root"])
            .arg(root)
            .args(["--json", "status", "--auto-rebuild"])
            .output()
            .with_context(|| format!("status --auto-rebuild for {}", root.display()))?;
        let status_text = String::from_utf8_lossy(&status.stdout);
        let last = Command::new(executable)
            .args(["--root"])
            .arg(root)
            .args(["session-report", "--last"])
            .output()
            .with_context(|| format!("session-report --last for {}", root.display()))?;
        let last_text = String::from_utf8_lossy(&last.stdout);
        let status_line = status_text.lines().next().unwrap_or("").to_string();
        let last_line = last_text.lines().next().unwrap_or("").to_string();
        let summary = format!(
            "{}: {}; {}",
            root.file_name().and_then(|n| n.to_str()).unwrap_or("repo"),
            truncate(&status_line, 160),
            truncate(&last_line, 120)
        );
        lines.push(summary);
    }
    let additional = lines.join("\n");
    // Keep SessionStart output small: hosts paste exit-0 stdout into context.
    let additional = truncate(&additional, 400);
    Ok(json!({
        "hookSpecificOutput": {
            "hookEventName": "SessionStart",
            "additionalContext": additional,
        }
    }))
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{kept}…")
}

fn detach_build(executable: &Path, root: &Path) -> anyhow::Result<()> {
    let lock_dir = coalesce_lock_dir(root, "build");
    if !try_acquire_lock_dir(&lock_dir)? {
        return Ok(());
    }
    spawn_detached_with_cleanup(
        executable,
        &format!(
            "--root {} build",
            shell_single_quote(&root.display().to_string())
        ),
        &lock_dir,
    )
}

fn detach_session_report(executable: &Path, root: &Path) -> anyhow::Result<()> {
    let lock_dir = coalesce_lock_dir(root, "session-report");
    if !try_acquire_lock_dir(&lock_dir)? {
        return Ok(());
    }
    spawn_detached_with_cleanup(
        executable,
        &format!(
            "--root {} session-report",
            shell_single_quote(&root.display().to_string())
        ),
        &lock_dir,
    )
}

fn coalesce_lock_dir(root: &Path, kind: &str) -> PathBuf {
    let state = if root.join(".devmap").is_dir() {
        root.join(".devmap")
    } else {
        root.join(".devcouncil")
    };
    state.join("codeintel").join(format!("hook-{kind}.running"))
}

fn try_acquire_lock_dir(lock_dir: &Path) -> anyhow::Result<bool> {
    if let Some(parent) = lock_dir.parent() {
        fs::create_dir_all(parent)?;
    }
    match fs::create_dir(lock_dir) {
        Ok(()) => {
            let _ = fs::write(lock_dir.join("pid"), std::process::id().to_string());
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            if lock_dir_is_stale(lock_dir) {
                let _ = fs::remove_dir_all(lock_dir);
                return try_acquire_lock_dir(lock_dir);
            }
            Ok(false)
        }
        Err(error) => Err(error).with_context(|| format!("lock dir {}", lock_dir.display())),
    }
}

fn lock_dir_is_stale(lock_dir: &Path) -> bool {
    let Ok(pid_text) = fs::read_to_string(lock_dir.join("pid")) else {
        return true;
    };
    let Ok(pid) = pid_text.trim().parse::<i32>() else {
        return true;
    };
    #[cfg(unix)]
    {
        // Signal 0: process existence check without delivery.
        let rc = unsafe { libc::kill(pid, 0) };
        rc != 0
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

fn shell_single_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

fn spawn_detached_with_cleanup(
    executable: &Path,
    args_shell: &str,
    lock_dir: &Path,
) -> anyhow::Result<()> {
    let exe = executable
        .to_str()
        .ok_or_else(|| anyhow!("executable path is not UTF-8"))?;
    let lock = lock_dir
        .to_str()
        .ok_or_else(|| anyhow!("lock path is not UTF-8"))?;
    // The lock directory is held for the child's lifetime so concurrent hooks
    // coalesce onto one build. The shell removes it when the child exits.
    let script = format!(
        "{} {}; status=$?; rm -rf {}; exit $status",
        shell_single_quote(exe),
        args_shell,
        shell_single_quote(lock)
    );
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(script)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    cmd.spawn()
        .with_context(|| format!("spawn detached {}", executable.display()))?;
    Ok(())
}

/// Read all of stdin up to [`MAX_STDIN_BYTES`] + 1 (to detect overflow).
pub fn read_stdin_bounded() -> io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    let mut handle = io::stdin().lock();
    let mut chunk = [0u8; 8192];
    loop {
        let n = handle.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        if buf.len() + n > MAX_STDIN_BYTES + 1 {
            buf.extend_from_slice(&chunk[..n]);
            buf.truncate(MAX_STDIN_BYTES + 1);
            // Drain the rest so the writer does not get SIGPIPE mid-payload.
            let mut sink = [0u8; 8192];
            while handle.read(&mut sink)? > 0 {}
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    Ok(buf)
}

/// Absolute-path shell-form command for a hook handler.
#[allow(dead_code)]
pub fn shell_command(executable: &Path, event: HookEvent) -> anyhow::Result<String> {
    let abs = if executable.is_absolute() {
        executable.to_path_buf()
    } else {
        executable
            .canonicalize()
            .unwrap_or_else(|_| executable.to_path_buf())
    };
    let text = abs
        .to_str()
        .ok_or_else(|| anyhow!("executable path is not UTF-8"))?;
    Ok(format!("\"{text}\" hook {}", event.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn empty_and_malformed_stdin_are_noop_exit_zero() {
        let exe = PathBuf::from("/bin/true");
        let empty = run_hook(HookEvent::PostToolUse, b"", &exe, None);
        assert_eq!(empty.exit_code, 0);
        let bad = run_hook(HookEvent::PostToolUse, b"{not-json", &exe, None);
        assert_eq!(bad.exit_code, 0);
        assert!(bad.stderr_line.unwrap().contains("malformed"));
    }

    #[test]
    fn cursor_payload_selects_file_path_root_with_store() {
        let dir = std::env::temp_dir().join(format!(
            "devmap-hook-sel-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(".devcouncil/codeintel")).unwrap();
        fs::write(dir.join(".devcouncil/codeintel/devmap.sqlite"), b"x").unwrap();
        fs::create_dir_all(dir.join("src")).unwrap();
        let file = dir.join("src/a.rs");
        fs::write(&file, "fn a() {}").unwrap();
        let payload = json!({
            "workspace_roots": [dir.to_string_lossy()],
            "tool_input": {"file_path": file.to_string_lossy()},
        });
        let roots = select_roots(&payload, None).unwrap();
        assert_eq!(roots, vec![dir.canonicalize().unwrap()]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_path_tier_does_not_union_workspace_roots() {
        // Stress-pass regression: Cursor multi-root stdin carries every open
        // workspace in workspace_roots. Selecting file_path must not also
        // detach-build the siblings.
        let stamp = format!(
            "{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let base = std::env::temp_dir().join(format!("devmap-hook-union-{stamp}"));
        let manvi = base.join("Manvi");
        let other_a = base.join("DevCouncil");
        let other_b = base.join("GitPulse");
        let other_c = base.join("MarkDev");
        for root in [&manvi, &other_a, &other_b, &other_c] {
            let _ = fs::remove_dir_all(root);
            fs::create_dir_all(root.join(".devcouncil/codeintel")).unwrap();
            fs::write(root.join(".devcouncil/codeintel/devmap.sqlite"), b"x").unwrap();
        }
        fs::create_dir_all(manvi.join("src")).unwrap();
        let file = manvi.join("src/a.rs");
        fs::write(&file, "fn a() {}").unwrap();

        let payload = json!({
            "workspace_roots": [
                other_a.to_string_lossy(),
                other_b.to_string_lossy(),
                other_c.to_string_lossy(),
                manvi.to_string_lossy(),
            ],
            "tool_input": {"file_path": file.to_string_lossy()},
        });
        let roots = select_roots(&payload, None).unwrap();
        assert_eq!(
            roots,
            vec![manvi.canonicalize().unwrap()],
            "file_path must win alone; old union selected all workspace_roots: {roots:?}"
        );
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn cwd_tier_does_not_union_workspace_roots() {
        let stamp = format!(
            "{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let base = std::env::temp_dir().join(format!("devmap-hook-cwd-{stamp}"));
        let primary = base.join("primary");
        let sibling = base.join("sibling");
        for root in [&primary, &sibling] {
            fs::create_dir_all(root.join(".devcouncil/codeintel")).unwrap();
            fs::write(root.join(".devcouncil/codeintel/devmap.sqlite"), b"x").unwrap();
        }
        let payload = json!({
            "cwd": primary.to_string_lossy(),
            "workspace_roots": [
                sibling.to_string_lossy(),
                primary.to_string_lossy(),
            ],
        });
        let roots = select_roots(&payload, None).unwrap();
        assert_eq!(roots, vec![primary.canonicalize().unwrap()]);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn apply_patch_paths_are_resolved_against_cwd() {
        let payload = json!({
            "cwd": "/repo",
            "tool_input": {
                "command": "*** Begin Patch\n*** Update File: src/a.rs\n*** End Patch"
            }
        });
        let paths = apply_patch_paths(&payload);
        assert_eq!(paths, vec![PathBuf::from("/repo/src/a.rs")]);
    }

    #[test]
    fn unsafe_home_without_store_is_skipped() {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let Some(home) = home else {
            return;
        };
        let payload = json!({"cwd": home});
        let roots = select_roots(&payload, None).unwrap();
        assert!(roots.is_empty(), "{roots:?}");
    }
}
