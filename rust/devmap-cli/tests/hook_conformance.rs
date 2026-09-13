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
    // The exit code was the only thing checked here, and stderr appeared solely
    // inside that message — so a hook that failed in total silence passed. It
    // did: `diagnostic` hands the line to an asynchronous writer and the
    // failing path ends in `std::process::exit`, which waits for nothing, so
    // every non-zero hook exit reached the host with empty stderr.
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not-a-real-event"),
        "a failing hook must say why, naming the event it rejected; got {stderr:?}"
    );
}

#[test]
fn cursor_payload_selects_file_path_root() {
    // Warm the hook path before timing it.
    //
    // Measured 2026-09-13: the *first* `post-tool-use` in a fresh test process
    // costs ~950 ms and every one after it ~45 ms — same binary, same payload,
    // a fresh root each time. `--version` on the same binary is 8 ms, so it is
    // not process startup; it is demand-paging the hook path's working set out
    // of a 110 MB debug binary on a machine under memory pressure. Interleaved
    // against the installed 0.2.1 release the warm cost is identical
    // (25-63 ms vs 34-83 ms), so there is nothing here to catch.
    //
    // The 500 ms bound below is unchanged. What changes is that it now measures
    // the hook rather than the loader — which is what its message claims. The
    // sibling `post_tool_use_returns_under_200ms_while_detached` survives a
    // *tighter* 200 ms bound only because it runs a full `devmap build` first
    // and warms the same pages by accident; this does it on purpose and for the
    // cost of one no-op hook.
    // An empty payload: no root resolves, nothing is detached, and the hook
    // still walks the code path being timed below.
    let _ = run_hook("post-tool-use", b"{}", &[]);

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

/// The hook's detached build must index the repository it names, from anywhere.
///
/// `detach_build` spawns `devmap --root <project> build` from whatever
/// directory the agent happens to be in, with stdout and stderr on /dev/null.
/// `build` read the repository from its *positional* path while `cli.db()`
/// resolved the store from `--root`, so the child aborted with "DevMap store
/// belongs to worktree X, not Y" and the index silently stopped following the
/// tree — every hook still exiting 0.
///
/// The cwd here is deliberately NOT the repository: that is the only
/// configuration in which the bug appears, and it is the configuration hooks
/// always run in.
#[test]
fn a_root_pinned_build_indexes_that_root_from_another_directory() {
    let root = scratch("rootbuild");
    std::fs::write(root.join("a.py"), "def a():\n    return 1\n").unwrap();
    assert!(Command::new(DEVMAP)
        .args(["--json", "build", "."])
        .current_dir(&root)
        .status()
        .unwrap()
        .success());
    let before = generation_id(&root).expect("a built store reports a generation");
    std::fs::write(root.join("a.py"), "def a():\n    return 2\n").unwrap();

    let elsewhere = scratch("rootbuild-cwd");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let out = Command::new(DEVMAP)
        .arg("--root")
        .arg(&root)
        .arg("build")
        .current_dir(&elsewhere)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "--root must name the repository to build: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let after = generation_id(&root).expect("status after build");
    assert!(
        after > before,
        "the build reported success without advancing the index ({before} -> {after})"
    );
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&elsewhere);
}

/// The generation the store is currently on, or `None` before the first build.
///
/// Generations are the ledger a build cannot avoid writing, which is what makes
/// "how many builds actually ran" observable from outside those processes.
fn generation_id(root: &Path) -> Option<u64> {
    let out = Command::new(DEVMAP)
        .args(["--root"])
        .arg(root)
        .args(["--json", "status"])
        .output()
        .expect("status");
    let text = String::from_utf8_lossy(&out.stdout);
    let value: serde_json::Value = serde_json::from_str(text.trim()).ok()?;
    value.get("generation_id")?.as_u64()
}

/// Wait until no further generations appear, so the count below is final.
///
/// Builds are detached: the hooks return long before their children do. Bounded
/// so a wedged child fails this test rather than hanging the suite.
fn wait_for_quiescence(root: &Path) -> u64 {
    let overall = Instant::now();
    let mut last = generation_id(root).unwrap_or(0);
    let mut stable_since = Instant::now();
    while overall.elapsed() < Duration::from_secs(90) {
        thread::sleep(Duration::from_millis(200));
        let now = generation_id(root).unwrap_or(last);
        if now != last {
            last = now;
            stable_since = Instant::now();
        } else if stable_since.elapsed() >= Duration::from_secs(2) {
            break;
        }
    }
    last
}

/// Twenty concurrent hooks must produce ONE build, not twenty.
///
/// The predecessor of this test fired exactly this burst and asserted only that
/// each hook exited 0 — which twenty concurrent builds satisfy precisely as well
/// as one. Its name claimed coalescing and its body never looked, so the lock
/// could record the *hook's* pid — a process that exits within milliseconds of
/// spawning the build — and every following hook read the lock as abandoned,
/// deleted it, and started another build, all under a green suite.
///
/// The ceiling is deliberately loose. The point is not to pin an exact number,
/// which timing makes unstable, but to separate "the lock held" from "the lock
/// did nothing": measured on this tree the fixed path writes 1-2 generations and
/// the broken one writes one per hook.
#[test]
fn concurrent_post_tool_use_coalesces_to_one_build() {
    let root = Arc::new(scratch("coalesce"));
    std::fs::write(root.join("a.py"), "def a():\n    return 1\n").unwrap();
    assert!(Command::new(DEVMAP)
        .args(["--json", "build", "."])
        .current_dir(root.as_path())
        .status()
        .unwrap()
        .success());
    let before = generation_id(root.as_path()).expect("a built store reports a generation");
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
    for handle in handles {
        let run = handle.join().unwrap();
        assert_eq!(run.code, Some(0), "{}", run.stderr);
    }

    let after = wait_for_quiescence(root.as_path());
    let builds = after.saturating_sub(before);
    eprintln!("coalescing: {builds} generation(s) from a 20-hook burst");

    assert!(
        builds >= 1,
        "the burst produced no build at all, so this test would pass against a \
         hook that does nothing"
    );
    assert!(
        builds <= 6,
        "{builds} builds from a 20-hook burst: the lock is not coalescing them"
    );

    let lock = root.join(".devcouncil/codeintel/hook-build.running");
    assert!(
        !lock.exists(),
        "the build lock outlived every child that could remove it"
    );
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

/// Make a store-bearing root's lock directory unwritable.
///
/// `detach_build` creates `codeintel/hook-build.running`, so a read-only
/// `codeintel` turns "this root was acted on" into a non-zero exit. That is the
/// only externally visible difference between acting on a root and skipping it,
/// and this file already relies on it for the sibling-union regression.
fn make_undetachable(root: &Path) {
    let codeintel = root.join(".devcouncil/codeintel");
    let mut perms = std::fs::metadata(&codeintel).unwrap().permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(&codeintel, perms).unwrap();
}

fn make_detachable(root: &Path) {
    let codeintel = root.join(".devcouncil/codeintel");
    let mut perms = std::fs::metadata(&codeintel).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    std::fs::set_permissions(&codeintel, perms).unwrap();
}

/// `--root` must pin the repository, and the payload must not move it.
///
/// This asserted only that the hook exited 0, which a hook that honours the pin
/// and a hook that quietly indexes the payload's repository satisfy equally —
/// and so would a hook that did nothing at all. Both directions are checked
/// here instead, each by the root that fails when it is the one acted on.
#[test]
fn pinned_root_disables_discovery() {
    let pinned = scratch("pin");
    seed_store(&pinned);
    let other = scratch("pin-other");
    seed_store(&other);
    let payload = serde_json::to_string(&json!({
        "cwd": other.to_string_lossy(),
        "workspace_roots": [other.to_string_lossy()],
    }))
    .unwrap();

    // The payload's repository is the one that cannot be built. Discovery would
    // pick it and fail; honouring the pin never touches it.
    make_undetachable(&other);
    let run = run_hook(
        "post-tool-use",
        payload.as_bytes(),
        &["--root", pinned.to_str().unwrap()],
    );
    assert_eq!(
        run.code,
        Some(0),
        "the payload's repository was indexed despite the pin: {}",
        run.stderr
    );
    make_detachable(&other);

    // Inverted: now the PINNED repository is the one that cannot be built, so a
    // non-zero exit is positive evidence the pin — not the payload — was acted
    // on. Without this half, a hook that selected nothing would still pass.
    //
    // A FRESH pinned root, because the first half already took and released a
    // build lock under the old one. A hook that finds a lock still standing
    // returns `Ok(false)` and exits 0 — which would read here as "the pin was
    // ignored" when it actually means "the pin was honoured twice".
    let pinned = scratch("pin-locked");
    seed_store(&pinned);
    make_undetachable(&pinned);
    let run = run_hook(
        "post-tool-use",
        payload.as_bytes(),
        &["--root", pinned.to_str().unwrap()],
    );
    assert_ne!(
        run.code,
        Some(0),
        "the pinned repository was never acted on, so the pin proves nothing"
    );
    assert!(
        run.stderr.contains(pinned.to_str().unwrap()),
        "the failure must name the pinned root, not another repository: {}",
        run.stderr
    );
    make_detachable(&pinned);

    let _ = std::fs::remove_dir_all(&pinned);
    let _ = std::fs::remove_dir_all(&other);
}
