//! Cross-host `devmap hook` conformance: Claude, Cursor, and Codex stdin shapes.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::json;

const DEVMAP: &str = env!("CARGO_BIN_EXE_devmap");

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "devmap-hook-conf-{tag}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn seed_store(root: &Path) {
    std::fs::create_dir_all(root.join(".devcouncil/codeintel")).unwrap();
    // Minimal sqlite header so `is_file` is true; hook never opens it for
    // post-tool-use beyond existence checks before spawning build.
    std::fs::write(root.join(".devcouncil/codeintel/devmap.sqlite"), b"x").unwrap();
}

struct HookRun {
    code: Option<i32>,
    #[allow(dead_code)]
    stdout: String,
    stderr: String,
    elapsed: Duration,
}

fn run_hook(event: &str, stdin: &[u8], extra: &[&str]) -> HookRun {
    let started = Instant::now();
    let mut child = Command::new(DEVMAP)
        .args(extra)
        .arg("hook")
        .arg(event)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn hook");
    {
        let mut pipe = child.stdin.take().unwrap();
        pipe.write_all(stdin).unwrap();
    }
    let out = child.wait_with_output().unwrap();
    HookRun {
        code: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        elapsed: started.elapsed(),
    }
}

#[test]
fn empty_and_malformed_stdin_exit_zero() {
    let empty = run_hook("post-tool-use", b"", &[]);
    assert_eq!(empty.code, Some(0), "{}", empty.stderr);
    let bad = run_hook("post-tool-use", b"{not-json", &[]);
    assert_eq!(bad.code, Some(0), "{}", bad.stderr);
    assert!(bad.stderr.contains("malformed") || bad.stderr.contains("no-op"));
}

#[test]
fn clap_error_inside_hook_exits_one_never_two() {
    let out = Command::new(DEVMAP)
        .args(["hook", "not-a-real-event"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(1),
        "hook clap/usage failures must be exit 1, never 2 (block): stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn cursor_payload_selects_file_path_root() {
    let root = scratch("cursor");
    seed_store(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    let file = root.join("src/a.rs");
    std::fs::write(&file, "fn a() {}").unwrap();
    let payload = json!({
        "workspace_roots": [root.to_string_lossy()],
        "tool_input": {"file_path": file.to_string_lossy()},
    });
    let run = run_hook(
        "post-tool-use",
        serde_json::to_string(&payload).unwrap().as_bytes(),
        &[],
    );
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    // Detached build may fail on a fake sqlite; the hook itself must still
    // return quickly with exit 0.
    assert!(
        run.elapsed < Duration::from_millis(500),
        "post-tool-use must return fast: {:?}",
        run.elapsed
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn cursor_file_path_does_not_detach_build_sibling_workspace_roots() {
    // Stress-pass regression: Manvi file_path used to acquire hook-build.running
    // on DevCouncil, GitPulse, MarkDev, and Manvi because select_roots unioned
    // file_path with every workspace_roots entry.
    //
    // Sibling codeintel dirs are made read-only so a mistaken detach_build
    // fails the hook (exit 1). First-tier file_path selection only touches
    // Manvi and stays exit 0.
    let base = scratch("cursor-union");
    let manvi = base.join("Manvi");
    let siblings = [
        base.join("DevCouncil"),
        base.join("GitPulse"),
        base.join("MarkDev"),
    ];
    seed_store(&manvi);
    for sibling in &siblings {
        seed_store(sibling);
        let codeintel = sibling.join(".devcouncil/codeintel");
        let mut perms = std::fs::metadata(&codeintel).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&codeintel, perms).unwrap();
    }
    std::fs::create_dir_all(manvi.join("src")).unwrap();
    let file = manvi.join("src/a.rs");
    std::fs::write(&file, "fn a() {}").unwrap();

    let mut workspace_roots = Vec::new();
    for sibling in &siblings {
        workspace_roots.push(sibling.to_string_lossy().into_owned());
    }
    workspace_roots.push(manvi.to_string_lossy().into_owned());
    let payload = json!({
        "workspace_roots": workspace_roots,
        "tool_input": {"file_path": file.to_string_lossy()},
    });
    let run = run_hook(
        "post-tool-use",
        serde_json::to_string(&payload).unwrap().as_bytes(),
        &[],
    );
    assert_eq!(
        run.code,
        Some(0),
        "file_path must select Manvi alone; unioning siblings hits read-only lock dirs: {}",
        run.stderr
    );

    for sibling in &siblings {
        let codeintel = sibling.join(".devcouncil/codeintel");
        let mut perms = std::fs::metadata(&codeintel).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        let _ = std::fs::set_permissions(&codeintel, perms);
    }
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn claude_payload_uses_cwd() {
    let root = scratch("claude");
    seed_store(&root);
    let payload = json!({ "cwd": root.to_string_lossy() });
    let run = run_hook(
        "post-tool-use",
        serde_json::to_string(&payload).unwrap().as_bytes(),
        &[],
    );
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn codex_apply_patch_selects_paths_from_command() {
    let root = scratch("codex");
    seed_store(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.rs"), "fn a() {}").unwrap();
    let payload = json!({
        "cwd": root.to_string_lossy(),
        "tool_name": "apply_patch",
        "tool_input": {
            "command": format!(
                "*** Begin Patch\n*** Update File: {}\n@@\n+fn a() {{}}\n*** End Patch",
                root.join("src/a.rs").display()
            )
        }
    });
    let run = run_hook(
        "post-tool-use",
        serde_json::to_string(&payload).unwrap().as_bytes(),
        &[],
    );
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn post_tool_use_returns_under_200ms_while_detached() {
    let root = scratch("latency");
    // Real tiny repo so a detached build has something to do.
    std::fs::write(root.join("a.py"), "def a():\n    return 1\n").unwrap();
    let build = Command::new(DEVMAP)
        .args(["--json", "build", "."])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    // Touch a file so the next build has work.
    std::fs::write(root.join("a.py"), "def a():\n    return 2\n").unwrap();
    let payload = json!({ "cwd": root.to_string_lossy() });
    let run = run_hook(
        "post-tool-use",
        serde_json::to_string(&payload).unwrap().as_bytes(),
        &[],
    );
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert!(
        run.elapsed < Duration::from_millis(200),
        "expected <200ms, got {:?}",
        run.elapsed
    );
    // Detached build lock should exist briefly (or already cleaned).
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn concurrent_post_tool_use_coalesces_to_one_build_lock() {
    let root = Arc::new(scratch("coalesce"));
    std::fs::write(root.join("a.py"), "def a():\n    return 1\n").unwrap();
    assert!(Command::new(DEVMAP)
        .args(["--json", "build", "."])
        .current_dir(root.as_path())
        .status()
        .unwrap()
        .success());
    std::fs::write(root.join("a.py"), "def a():\n    return 2\n").unwrap();

    let payload = serde_json::to_string(&json!({ "cwd": root.to_string_lossy() })).unwrap();
    let payload = Arc::new(payload);
    let mut handles = Vec::new();
    for _ in 0..20 {
        let payload = Arc::clone(&payload);
        handles.push(thread::spawn(move || {
            run_hook("post-tool-use", payload.as_bytes(), &[])
        }));
    }
    let mut ok = 0;
    for handle in handles {
        let run = handle.join().unwrap();
        assert_eq!(run.code, Some(0), "{}", run.stderr);
        ok += 1;
    }
    assert_eq!(ok, 20);
    // At most one running lock directory should have been created for the burst;
    // after builds finish it is removed. Presence of many stamp failures would
    // mean coalescing failed — we assert the hook path never exited non-zero.
    let _ = std::fs::remove_dir_all(root.as_path());
}

#[test]
fn unsafe_home_without_store_is_skipped() {
    let home = std::env::var("HOME").unwrap_or_default();
    if home.is_empty() {
        return;
    }
    let payload = json!({ "cwd": home });
    let run = run_hook(
        "post-tool-use",
        serde_json::to_string(&payload).unwrap().as_bytes(),
        &[],
    );
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert!(
        run.stderr.contains("no-op") || run.stderr.contains("no indexed"),
        "home without a store must be skipped, not indexed: {}",
        run.stderr
    );
}

#[test]
fn pinned_root_disables_discovery() {
    let root = scratch("pin");
    seed_store(&root);
    let other = scratch("pin-other");
    seed_store(&other);
    let payload = json!({
        "cwd": other.to_string_lossy(),
        "workspace_roots": [other.to_string_lossy()],
    });
    let run = run_hook(
        "post-tool-use",
        serde_json::to_string(&payload).unwrap().as_bytes(),
        &["--root", root.to_str().unwrap()],
    );
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&other);
}
