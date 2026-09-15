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

/// A store `status` will call `query_ready`. Required for PreToolUse, which
/// stays silent unless the index can actually answer.
fn seed_queryable(root: &Path) {
    std::fs::write(root.join("a.py"), "def a():\n    return 1\n").unwrap();
    let build = Command::new(DEVMAP)
        .args(["--json", "build", "."])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "tiny build must succeed: {}",
        String::from_utf8_lossy(&build.stderr)
    );
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

/// Cursor native sessionStart is fire-and-forget JSON. Plaintext briefing is
/// logged as "Failed to parse hook stdout as JSON" and never reaches the agent.
///
/// The live Cursor 3.20 payload has `cursor_version` + `workspace_roots` and
/// no `cwd`. The unwrap in `run_hook_command` that pastes Claude's
/// `additionalContext` as raw text is the defect this names.
#[test]
fn cursor_session_start_stdout_is_json_additional_context() {
    let root = scratch("cursor-start");
    seed_store(&root);
    let payload = json!({
        "conversation_id": "conv-cursor-1",
        "generation_id": "gen-1",
        "session_id": "conv-cursor-1",
        "hook_event_name": "sessionStart",
        "cursor_version": "3.20.7",
        "composer_mode": "agent",
        "workspace_roots": [root.to_string_lossy()],
    });
    let run = run_hook(
        "session-start",
        serde_json::to_string(&payload).unwrap().as_bytes(),
        &[],
    );
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    let value: serde_json::Value = serde_json::from_str(run.stdout.trim()).unwrap_or_else(|err| {
        panic!(
            "Cursor sessionStart stdout must be JSON, not plaintext ({err}):\n{}",
            run.stdout
        )
    });
    let ctx = value["additional_context"]
        .as_str()
        .unwrap_or_else(|| panic!("missing additional_context: {value}"));
    assert!(
        ctx.contains("Ask DevMap") || ctx.contains("devmap_search"),
        "the briefing the agent reads is missing: {ctx}"
    );
    assert!(
        value.get("permission").is_none(),
        "sessionStart must not emit a permission decision: {value}"
    );
    assert!(
        value.get("hookSpecificOutput").is_none(),
        "Cursor native schema is snake_case additional_context, not Claude nested JSON: {value}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Claude pastes exit-0 stdout into the model. A JSON blob there is the
/// briefing the agent never reads as prose.
#[test]
fn claude_session_start_stdout_is_plaintext_briefing() {
    let root = scratch("claude-start");
    seed_store(&root);
    let payload = json!({
        "session_id": "claude-1",
        "hook_event_name": "SessionStart",
        "cwd": root.to_string_lossy(),
        "source": "startup",
    });
    let run = run_hook(
        "session-start",
        serde_json::to_string(&payload).unwrap().as_bytes(),
        &[],
    );
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    let text = run.stdout.trim();
    assert!(
        !text.starts_with('{'),
        "Claude SessionStart must stay plaintext, got {text}"
    );
    assert!(
        text.contains("Ask DevMap") || text.contains("devmap_search"),
        "Claude must still receive the briefing: {text}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Quotes, newlines, and non-ASCII in the briefing must still be a document
/// Cursor's parser accepts. A hand-built string would break on the first
/// repository whose status line contained a quote.
#[test]
fn cursor_session_start_stdout_survives_hostile_briefing_characters() {
    let root = scratch("cursor-quotes");
    seed_store(&root);
    // The repository *name* is interpolated into the briefing.
    let named = root.join("repo \"quotes\" and 日本語");
    std::fs::create_dir_all(&named).unwrap();
    seed_store(&named);
    let payload = json!({
        "cursor_version": "3.20.7",
        "hook_event_name": "sessionStart",
        "session_id": "s-quotes",
        "workspace_roots": [named.to_string_lossy()],
    });
    let run = run_hook(
        "session-start",
        serde_json::to_string(&payload).unwrap().as_bytes(),
        &[],
    );
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    let value: serde_json::Value =
        serde_json::from_str(run.stdout.trim()).expect("hostile briefing must still be JSON");
    let ctx = value["additional_context"].as_str().expect("context");
    assert!(!ctx.is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

/// Cursor `preToolUse` is a permission hook: invalid JSON or a schema mismatch
/// **blocks the tool**. Dev Map must never sit there, and if a user wires the
/// binary there anyway, stdout must be empty so the call fails open.
#[test]
fn cursor_pre_tool_use_on_a_permission_event_emits_no_stdout() {
    let root = scratch("cursor-perm");
    seed_queryable(&root);
    let payload = json!({
        "cursor_version": "3.20.7",
        "hook_event_name": "preToolUse",
        "session_id": "perm-1",
        "workspace_roots": [root.to_string_lossy()],
        "tool_name": "Read",
        "tool_input": {"file_path": root.join("a.rs").to_string_lossy()},
    });
    let run = run_hook(
        "pre-tool-use",
        serde_json::to_string(&payload).unwrap().as_bytes(),
        &[],
    );
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert!(
        run.stdout.trim().is_empty(),
        "permission-hook stdout must be empty (fail open), got {:?}",
        run.stdout
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// The first-nav nudge on Cursor is `postToolUse` (additional_context is
/// documented there). Matcher `Read|Grep` is what the installer emits; this
/// is the binary contract that handler must satisfy.
#[test]
fn cursor_post_tool_use_navigation_emits_additional_context_json() {
    let root = scratch("cursor-post-nav");
    seed_queryable(&root);
    let payload = json!({
        "cursor_version": "3.20.7",
        "hook_event_name": "postToolUse",
        "session_id": "post-nav-1",
        "workspace_roots": [root.to_string_lossy()],
        "tool_name": "Read",
        "tool_input": {"file_path": root.join("a.rs").to_string_lossy()},
    });
    let run = run_hook(
        "pre-tool-use",
        serde_json::to_string(&payload).unwrap().as_bytes(),
        &[],
    );
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    let value: serde_json::Value = serde_json::from_str(run.stdout.trim()).unwrap_or_else(|err| {
        panic!(
            "Cursor postToolUse stdout must be JSON ({err}):\n{}",
            run.stdout
        )
    });
    let ctx = value["additional_context"]
        .as_str()
        .unwrap_or_else(|| panic!("missing additional_context: {value}"));
    assert!(
        ctx.contains("Ask DevMap") || ctx.contains("devmap"),
        "first-nav directive missing: {ctx}"
    );
    assert!(value.get("permission").is_none(), "{value}");
    let _ = std::fs::remove_dir_all(&root);
}

/// `--json` is the debug/CI shape: Claude nested document, even when stdin
/// looks like Cursor. Hosts never pass `--json`.
#[test]
fn json_flag_keeps_claude_nested_shape_for_a_cursor_payload() {
    let root = scratch("cursor-json-flag");
    seed_store(&root);
    let payload = json!({
        "cursor_version": "3.20.7",
        "hook_event_name": "sessionStart",
        "workspace_roots": [root.to_string_lossy()],
    });
    let run = run_hook(
        "session-start",
        serde_json::to_string(&payload).unwrap().as_bytes(),
        &["--json"],
    );
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    let value: serde_json::Value = serde_json::from_str(run.stdout.trim()).expect("JSON flag");
    assert!(
        value
            .pointer("/hookSpecificOutput/additionalContext")
            .is_some(),
        " --json must keep the internal Claude document: {value}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// A Cursor payload naming more roots than the cap still has to be a document
/// the host can parse, not a panic or a truncated fragment.
#[test]
fn cursor_session_start_caps_workspace_roots_and_stays_json() {
    let base = scratch("cursor-cap");
    let mut roots = Vec::new();
    for i in 0..10 {
        let root = base.join(format!("r{i}"));
        seed_store(&root);
        roots.push(root.to_string_lossy().into_owned());
    }
    let payload = json!({
        "cursor_version": "3.20.7",
        "hook_event_name": "sessionStart",
        "session_id": "cap-1",
        "workspace_roots": roots,
    });
    let run = run_hook(
        "session-start",
        serde_json::to_string(&payload).unwrap().as_bytes(),
        &[],
    );
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    let value: serde_json::Value =
        serde_json::from_str(run.stdout.trim()).expect("capped sessionStart must still be JSON");
    assert!(value["additional_context"].as_str().is_some());
    let _ = std::fs::remove_dir_all(&base);
}

/// Every Cursor permission event name must fail open at the process boundary,
/// not only in the unit that wraps stdout. One queryable store, because
/// PreToolUse stays silent when the index cannot answer — that silence is
/// not the fail-open this names.
#[test]
fn cursor_permission_events_emit_no_stdout() {
    let root = scratch("cursor-perm-all");
    seed_queryable(&root);
    for event_name in [
        "preToolUse",
        "beforeReadFile",
        "beforeShellExecution",
        "beforeMCPExecution",
        "beforeTabFileRead",
        "subagentStart",
    ] {
        let payload = json!({
            "cursor_version": "3.20.7",
            "hook_event_name": event_name,
            "session_id": format!("perm-{event_name}"),
            "workspace_roots": [root.to_string_lossy()],
            "tool_name": "Read",
            "tool_input": {"file_path": root.join("a.py").to_string_lossy()},
        });
        let run = run_hook(
            "pre-tool-use",
            serde_json::to_string(&payload).unwrap().as_bytes(),
            &[],
        );
        assert_eq!(run.code, Some(0), "{event_name}: {}", run.stderr);
        assert!(
            run.stdout.trim().is_empty(),
            "{event_name} must fail open, got {:?}",
            run.stdout
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// Concurrent Cursor sessionStarts must each be one JSON document, not a
/// torn or interleaved write.
#[test]
fn concurrent_cursor_session_start_stdout_is_each_one_json_document() {
    let root = Arc::new(scratch("cursor-fanout"));
    seed_store(root.as_path());
    let payload = serde_json::to_string(&json!({
        "cursor_version": "3.20.7",
        "hook_event_name": "sessionStart",
        "workspace_roots": [root.to_string_lossy()],
    }))
    .unwrap();
    let payload = Arc::new(payload);
    let mut handles = Vec::new();
    for i in 0..8 {
        let payload = Arc::clone(&payload);
        handles.push(thread::spawn(move || {
            let run = run_hook("session-start", payload.as_bytes(), &[]);
            (i, run)
        }));
    }
    for handle in handles {
        let (i, run) = handle.join().unwrap();
        assert_eq!(run.code, Some(0), "hook {i}: {}", run.stderr);
        let value: serde_json::Value =
            serde_json::from_str(run.stdout.trim()).unwrap_or_else(|err| {
                panic!(
                    "hook {i} stdout must be one JSON document ({err}): {:?}",
                    run.stdout
                )
            });
        assert!(
            value["additional_context"].as_str().is_some(),
            "hook {i}: {value}"
        );
        assert!(value.get("permission").is_none(), "hook {i}: {value}");
    }
    let _ = std::fs::remove_dir_all(root.as_path());
}

// ---- pre-tool-use under load -------------------------------------------
//
// The properties below were first established with throwaway shell scripts
// driving the release binary. That proved them once, on one machine, and then
// the evidence was deleted. They belong here, against `DEVMAP`, where a
// regression fails the build instead of going unnoticed.

/// Where a repository keeps its per-session hook markers.
///
/// Resolved, never assumed: a repository built by `devmap build` may use either
/// layout, and a test that hard-codes `.devcouncil` looks in an empty directory
/// and reports "0 markers" as a pass. That exact mistake made the first version
/// of this check pass while proving nothing.
fn hook_marker_dir(root: &Path) -> PathBuf {
    let state = if root.join(".devmap").is_dir() {
        root.join(".devmap")
    } else {
        root.join(".devcouncil")
    };
    state.join("codeintel").join("hooks")
}

fn claude_nav_payload(root: &Path, session: &str) -> String {
    serde_json::to_string(&json!({
        "session_id": session,
        "cwd": root.to_string_lossy(),
        "hook_event_name": "PreToolUse",
        "tool_name": "Read",
        "tool_input": {"file_path": root.join("a.py").to_string_lossy()},
    }))
    .unwrap()
}

/// A parallel fan-out of first reads must produce one directive, not N.
///
/// The claim is a single `create_new`, which is atomic, so exactly one member of
/// the burst wins. This is the test that catches a future rewrite to
/// read-then-write: that races, and under load would paste the directive into
/// the session several times over.
#[test]
fn pre_tool_use_first_navigation_emits_exactly_once_under_concurrency() {
    let root = Arc::new(scratch("pretool-burst"));
    seed_queryable(root.as_path());
    let payload = Arc::new(claude_nav_payload(root.as_path(), "burst-session"));

    let mut handles = Vec::new();
    for _ in 0..32 {
        let payload = Arc::clone(&payload);
        handles.push(thread::spawn(move || {
            run_hook("pre-tool-use", payload.as_bytes(), &[])
        }));
    }
    let mut spoke = 0;
    for handle in handles {
        let run = handle.join().unwrap();
        assert_eq!(run.code, Some(0), "{}", run.stderr);
        if !run.stdout.trim().is_empty() {
            spoke += 1;
        }
    }
    assert_eq!(
        spoke, 1,
        "a 32-way burst emitted the session's directive {spoke} times"
    );
    let _ = std::fs::remove_dir_all(root.as_path());
}

/// Markers are bounded, and the check that says so actually ran.
///
/// Asserting only `<= cap` passes when nothing was written at all, which is the
/// same answer a broken hook gives. Both bounds are required.
#[test]
fn pre_tool_use_markers_stay_bounded_across_many_sessions() {
    let root = scratch("pretool-bound");
    seed_queryable(&root);
    let markers = hook_marker_dir(&root);
    let _ = std::fs::remove_dir_all(&markers);

    for session in 0..150 {
        let run = run_hook(
            "pre-tool-use",
            claude_nav_payload(&root, &format!("sess-{session}")).as_bytes(),
            &[],
        );
        assert_eq!(run.code, Some(0), "session {session}: {}", run.stderr);
    }

    let count = std::fs::read_dir(&markers)
        .map(|entries| entries.flatten().count())
        .unwrap_or(0);
    assert!(
        count > 0,
        "no marker was written at all, so this check proved nothing about the bound"
    );
    assert!(
        count <= devmap_cli_max_markers(),
        "{count} markers survived 150 sessions; the cap is {}",
        devmap_cli_max_markers()
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// The cap the hook enforces. Kept beside the test that asserts it rather than
/// imported: this suite drives the binary as a subprocess and has no access to
/// the crate's internals, so the number is part of the observable contract.
fn devmap_cli_max_markers() -> usize {
    64
}

/// Exit 2 blocks the agent's tool call. Nothing this hook can be fed may reach
/// it — not malformed input, not a hostile session id, not stdin past the 1 MiB
/// bound.
#[test]
fn pre_tool_use_never_blocks_a_tool_call() {
    let root = scratch("pretool-hostile");
    seed_queryable(&root);

    let oversized = {
        let mut payload = String::from("{\"tool_name\":\"Read\",\"pad\":\"");
        payload.push_str(&"x".repeat(1024 * 1024 + 4096));
        payload.push_str("\"}");
        payload
    };

    let cases: Vec<(&str, String)> = vec![
        ("empty", String::new()),
        ("truncated json", "{".into()),
        ("json null", "null".into()),
        ("json array", "[]".into()),
        ("null tool name", json!({"tool_name": null}).to_string()),
        (
            "path-escaping session id",
            json!({
                "session_id": "../../etc/passwd",
                "cwd": root.to_string_lossy(),
                "tool_name": "Read",
            })
            .to_string(),
        ),
        (
            "nonexistent cwd",
            json!({"tool_name": "Read", "cwd": "/nonexistent/devmap/probe"}).to_string(),
        ),
        (
            "shell search verb",
            json!({
                "session_id": "shell-1",
                "cwd": root.to_string_lossy(),
                "tool_name": "Bash",
                "tool_input": {"command": "rg TODO src/"},
            })
            .to_string(),
        ),
        ("oversized stdin", oversized),
    ];

    for (label, payload) in cases {
        let run = run_hook("pre-tool-use", payload.as_bytes(), &[]);
        assert_ne!(
            run.code,
            Some(2),
            "{label}: exit 2 blocks the tool call; stderr: {}",
            run.stderr
        );
        assert_eq!(
            run.code,
            Some(0),
            "{label}: expected a clean no-op; stderr: {}",
            run.stderr
        );
    }

    // Every case above takes a no-op path, so none of them reach the error
    // branch — which is how a mutation turning that branch's exit code into 2
    // survived this test. `--root` at a path that does not exist is the one
    // input that makes root selection genuinely fail, so this is the case that
    // proves a *failure* still refuses to block.
    let failing = run_hook(
        "pre-tool-use",
        claude_nav_payload(&root, "forced-failure").as_bytes(),
        &["--root", "/nonexistent/devmap/pinned"],
    );
    assert_ne!(
        failing.code,
        Some(2),
        "a hook failure must never exit 2: it would block the tool call. stderr: {}",
        failing.stderr
    );
    assert_eq!(
        failing.code,
        Some(1),
        "a genuine failure is exit 1; stderr: {}",
        failing.stderr
    );
    assert!(
        failing.stdout.trim().is_empty(),
        "a failed hook must emit nothing, got {:?}",
        failing.stdout
    );

    // A hostile session id must not have escaped the marker directory.
    let markers = hook_marker_dir(&root);
    if let Ok(entries) = std::fs::read_dir(&markers) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            assert!(
                !name.contains("..") && !name.contains('/'),
                "marker name escaped its directory: {name}"
            );
        }
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// A tool that is not navigation must cost nothing and say nothing.
///
/// The root here is real and queryable **on purpose**. An earlier version of
/// this test pointed at a nonexistent directory, reasoning that a silent no-op
/// proved the short circuit had run first — but a payload naming no indexed
/// repository is silent anyway, so deleting the short circuit left the test
/// green. Against a root the hook *would* happily advertise, silence can only
/// mean the tool name was rejected before any of that.
#[test]
fn pre_tool_use_skips_non_navigation_tools() {
    let root = scratch("pretool-skip");
    seed_queryable(&root);

    // Control: this root does speak, so the assertions below mean something.
    let control = run_hook(
        "pre-tool-use",
        claude_nav_payload(&root, "skip-control").as_bytes(),
        &[],
    );
    assert_eq!(control.code, Some(0), "{}", control.stderr);
    assert!(
        !control.stdout.trim().is_empty(),
        "the control read must emit, or this test cannot detect a regression"
    );

    for tool in ["Write", "Edit", "NotebookEdit", "WebFetch", "TodoWrite"] {
        let payload = json!({
            "session_id": format!("skip-{tool}"),
            "cwd": root.to_string_lossy(),
            "hook_event_name": "PreToolUse",
            "tool_name": tool,
        });
        let run = run_hook(
            "pre-tool-use",
            serde_json::to_string(&payload).unwrap().as_bytes(),
            &[],
        );
        assert_eq!(run.code, Some(0), "{tool}: {}", run.stderr);
        assert!(
            run.stdout.trim().is_empty(),
            "{tool} produced stdout: {:?}",
            run.stdout
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// Two sessions are two directives; the marker is scoped, not global.
#[test]
fn pre_tool_use_speaks_once_for_each_distinct_session() {
    let root = scratch("pretool-two-sessions");
    seed_queryable(&root);

    let mut spoke = 0;
    for session in ["alpha", "beta"] {
        for attempt in 0..2 {
            let run = run_hook(
                "pre-tool-use",
                claude_nav_payload(&root, session).as_bytes(),
                &[],
            );
            assert_eq!(run.code, Some(0), "{session}/{attempt}: {}", run.stderr);
            if !run.stdout.trim().is_empty() {
                spoke += 1;
            }
        }
    }
    assert_eq!(
        spoke, 2,
        "two sessions reading twice each must yield exactly two directives"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// An index that is queryable but holds nothing must produce silence.
///
/// The store here is **built**, not a stub: this repository indexes cleanly to
/// `query_ready: true, node_count: 0`, and would answer every question
/// "absent" — the single most expensive wrong answer it can give. An earlier
/// version seeded a fake sqlite file instead, so the probe failed outright and
/// the emptiness guard was never the thing under test; deleting that guard left
/// it green.
///
/// The fixture is a `.txt` file specifically. A `README.md` does *not* work:
/// markdown headings are indexed, so it yields one symbol and the hook
/// correctly advertises it — measured, after that fixture failed this test.
#[test]
fn pre_tool_use_is_silent_when_the_index_is_empty() {
    let root = scratch("pretool-empty-index");
    std::fs::write(root.join("notes.txt"), "prose, no code and no headings\n").unwrap();
    let build = Command::new(DEVMAP)
        .args(["--json", "build", "."])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "building an empty repository must still succeed: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let run = run_hook(
        "pre-tool-use",
        claude_nav_payload(&root, "empty-1").as_bytes(),
        &[],
    );
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert!(
        run.stdout.trim().is_empty(),
        "an index holding no symbols must not be advertised, got {:?}",
        run.stdout
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// A store file that is not a usable index must also stay silent.
#[test]
fn pre_tool_use_is_silent_when_the_store_cannot_be_read() {
    let root = scratch("pretool-unreadable");
    seed_store(&root);
    let run = run_hook(
        "pre-tool-use",
        claude_nav_payload(&root, "unreadable-1").as_bytes(),
        &[],
    );
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert!(
        run.stdout.trim().is_empty(),
        "an unreadable store must not be advertised, got {:?}",
        run.stdout
    );
    let _ = std::fs::remove_dir_all(&root);
}
