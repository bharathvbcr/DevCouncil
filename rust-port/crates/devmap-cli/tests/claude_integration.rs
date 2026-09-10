//! `devmap claude …` end to end: what it emits, what it refuses, and whether
//! the artifacts are the ones Claude Code actually accepts.
//!
//! Driven through the binary rather than the module, because the parts most
//! worth guarding are the ones a unit test cannot see: the subcommand list the
//! hooks name is the parser's own, the exit code is what a CI step reads, and
//! the bundle is a directory on disk that another program has to parse.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

const DEVMAP: &str = env!("CARGO_BIN_EXE_devmap");

fn repo_root() -> PathBuf {
    // <repo>/rust-port/crates/devmap-cli
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("crate is three levels below the repository root")
        .to_path_buf()
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "devmap-claude-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

struct Run {
    status: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Run {
    fn json(&self) -> Value {
        serde_json::from_str(&self.stdout).unwrap_or_else(|err| {
            panic!(
                "stdout is not JSON ({err}):\nstdout: {}\nstderr: {}",
                self.stdout, self.stderr
            )
        })
    }
    fn ok(&self) -> &Self {
        assert_eq!(
            self.status,
            Some(0),
            "expected success\nstdout: {}\nstderr: {}",
            self.stdout,
            self.stderr
        );
        self
    }
}

fn devmap(args: &[&str]) -> Run {
    let out = Command::new(DEVMAP)
        .args(args)
        .output()
        .expect("devmap binary runs");
    Run {
        status: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

// ---------------------------------------------------------------------------
// What is emitted
// ---------------------------------------------------------------------------

/// Every emitted handler must be exec form, anchored to the project root, and
/// naming a subcommand this binary registers.
///
/// Shell form would re-tokenize `${CLAUDE_PROJECT_DIR}` through `sh -c`, so a
/// checkout under a path with a space becomes two arguments; a relative `--db`
/// resolves against Claude's current directory, which after a `cd` or a
/// worktree entry is not the repository the index describes.
#[test]
fn emitted_hooks_are_exec_form_anchored_to_the_project_root() {
    let block = devmap(&["--json", "claude", "hooks"]).ok().json();
    let hooks = block["hooks"].as_object().expect("hooks object");
    assert!(!hooks.is_empty(), "no hooks emitted at all");

    let help = devmap(&["--help"]);
    for (event, groups) in hooks {
        for group in groups.as_array().expect("groups array") {
            for handler in group["hooks"].as_array().expect("handlers array") {
                assert_eq!(handler["type"], "command", "{event}");
                let command = handler["command"].as_str().expect("command string");
                assert!(
                    Path::new(command).is_absolute(),
                    "{event}: `command` must be this binary's absolute path, not a bare \
                     name a host may not have on PATH: {command}"
                );
                let args: Vec<&str> = handler["args"]
                    .as_array()
                    .unwrap_or_else(|| panic!("{event}: exec form requires `args`"))
                    .iter()
                    .map(|a| a.as_str().expect("string arg"))
                    .collect();
                assert_eq!(args[0], "--db", "{event}: {args:?}");
                assert!(
                    args[1].starts_with("${CLAUDE_PROJECT_DIR}/"),
                    "{event}: a relative store path must be anchored to the project root, \
                     got {:?}",
                    args[1]
                );
                // The subcommand is the parser's, checked against the parser.
                assert!(
                    help.stdout.contains(args[2]),
                    "{event}: `devmap {}` is not in this binary's help output, so the hook \
                     would fail on every fire",
                    args[2]
                );
            }
        }
    }
}

/// Dev Map installs nothing on an event that decides an authorization outcome.
///
/// This is the guard, not a comment. `PreToolUse`, `PermissionRequest` and
/// `PermissionDenied` are the three events whose output can allow, deny, alter
/// a tool's input, or write permission rules. A code index has nothing to say
/// there, and an entry added to the table later would widen what Dev Map is
/// able to approve without anyone deciding to.
#[test]
fn no_emitted_hook_sits_on_an_event_that_can_decide_a_permission() {
    // Read from the binary's own table rather than copied here: a second list
    // of "which events are the authorization surface" is one that can be
    // shortened on one side without the other noticing.
    let coverage = devmap(&["--json", "claude", "events"]).ok().json();
    let deciding: Vec<String> = coverage["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["decides_permission"] == true)
        .map(|row| row["event"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        deciding,
        ["PreToolUse", "PermissionRequest", "PermissionDenied"],
        "the documented set of permission-deciding events changed; a hook added to a \
         newly-deciding event would slip past this guard"
    );

    let block = devmap(&["--json", "claude", "hooks"]).ok().json();
    for event in &deciding {
        assert!(
            block["hooks"].get(event).is_none(),
            "{event} decides an authorization outcome; Dev Map must not install there"
        );
    }
    // And nothing emitted may carry a decision field even on a non-deciding
    // event, which is the other half of the same rule.
    let text = block.to_string();
    for field in [
        "permissionDecision",
        "updatedPermissions",
        "updatedInput",
        "bypassPermissions",
        "setMode",
    ] {
        assert!(
            !text.contains(field),
            "emitted config must contain no permission-decision field, found {field}"
        );
    }
}

/// The async refresh hook carries no `timeout`, because the field is documented
/// as unenforced there — writing one would state a bound that never applies.
#[test]
fn the_async_refresh_hook_states_no_timeout_it_cannot_hold() {
    let block = devmap(&["--json", "claude", "hooks"]).ok().json();
    let mut saw_async = false;
    for groups in block["hooks"].as_object().unwrap().values() {
        for group in groups.as_array().unwrap() {
            for handler in group["hooks"].as_array().unwrap() {
                if handler.get("async").and_then(Value::as_bool) == Some(true) {
                    saw_async = true;
                    assert!(
                        handler.get("timeout").is_none(),
                        "an async hook's timeout is never enforced: {handler}"
                    );
                }
            }
        }
    }
    assert!(
        saw_async,
        "the index refresh must run detached, or it holds the agent's tool loop"
    );
}

/// The SessionStart hook must succeed on a repository that has never been
/// indexed, because that is exactly when a user first configures the host.
///
/// `SessionStart` is one of the events whose exit-0 stdout becomes context
/// Claude can see; a non-zero exit instead renders a "hook error" notice in the
/// transcript that Claude never reads. So "no index yet" has to come back as a
/// successful answer that names the fix, not as a failure.
#[test]
fn the_session_start_hook_succeeds_and_says_so_on_an_unindexed_repository() {
    let dir = scratch("fresh");
    let db = devmap_extract::paths::store_path(&dir);
    let out = Command::new(DEVMAP)
        .arg("--db")
        .arg(&db)
        .arg("status")
        .output()
        .expect("devmap runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(0),
        "a fresh clone must not turn every session's first hook into an error notice: \
         {stdout}"
    );
    let report: Value = serde_json::from_str(&stdout).expect("status prints JSON");
    assert_eq!(report["is_fresh"], false);
    assert!(
        report["degraded_reason"]
            .as_str()
            .unwrap_or_default()
            .contains("devmap build"),
        "the context handed to Claude must name the fix: {stdout}"
    );
    assert!(
        !db.exists(),
        "reading status must not create the store it reports as missing"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// SessionEnd's report must succeed without an index: the hook cannot turn
/// teardown into an error notice, and it must not create a store just to say
/// nothing was queried.
#[test]
fn session_report_succeeds_on_an_unindexed_repository() {
    let dir = scratch("session-fresh");
    let db = devmap_extract::paths::store_path(&dir);
    let out = Command::new(DEVMAP)
        .arg("--db")
        .arg(&db)
        .arg("session-report")
        .output()
        .expect("devmap runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(0),
        "session-report on a fresh clone must exit 0\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.contains("no queries") || stdout.contains("DevMap session"),
        "the hook must say what happened: {stdout}"
    );
    assert!(
        !db.exists(),
        "session-report must not create the store it did not need"
    );
    let last = Command::new(DEVMAP)
        .arg("--db")
        .arg(&db)
        .arg("session-report")
        .arg("--last")
        .output()
        .expect("devmap runs");
    assert_eq!(last.status.code(), Some(0), "{}", String::from_utf8_lossy(&last.stderr));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn emitted_hooks_include_session_end_report() {
    let block = devmap(&["--json", "claude", "hooks"]).ok().json();
    let end = block["hooks"]["SessionEnd"]
        .as_array()
        .expect("SessionEnd group");
    let args = end[0]["hooks"][0]["args"]
        .as_array()
        .expect("args")
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(
        args.iter().any(|a| *a == "session-report"),
        "SessionEnd must run session-report: {args:?}"
    );
    let start = block["hooks"]["SessionStart"]
        .as_array()
        .expect("SessionStart groups");
    assert_eq!(start.len(), 1, "one matcher group, two handlers: {start:?}");
    let start_handlers = start[0]["hooks"].as_array().expect("handlers");
    assert_eq!(
        start_handlers.len(),
        2,
        "SessionStart must inject status and last-session insights: {start_handlers:?}"
    );
}

#[test]
fn plugin_manifest_points_at_bundled_skills() {
    let dir = scratch("skills-manifest");
    let out = dir.join("claude-plugin");
    devmap(&["--json", "claude", "plugin", "--out", out.to_str().unwrap()]).ok();
    let manifest: Value = serde_json::from_str(
        &std::fs::read_to_string(out.join("devmap/.claude-plugin/plugin.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["skills"], "./skills");
    let skill = out.join("devmap/skills/devmap/SKILL.md");
    let body = std::fs::read_to_string(&skill).expect("preference skill");
    assert!(body.contains("Do not use GitNexus"), "{body}");
    std::fs::remove_dir_all(&dir).ok();
}

/// Coverage is reported as both numbers, over every documented event.
#[test]
fn coverage_names_every_event_and_never_reports_a_subset_as_the_whole() {
    let report = devmap(&["--json", "claude", "events"]).ok().json();
    let total = report["events_total"].as_u64().expect("events_total");
    let handled = report["events_handled"].as_u64().expect("events_handled");
    let events = report["events"].as_array().expect("events array");
    assert_eq!(total as usize, events.len());
    assert_eq!(
        total, 33,
        "the documented surface is 33 events; a different count means this build's \
         table has drifted from the reference"
    );
    assert!(handled >= 1 && handled < total, "{handled} of {total}");
    for row in events {
        assert!(
            !row["reason"].as_str().unwrap_or("").is_empty(),
            "every event needs a stated decision, handled or not: {row}"
        );
    }
}

// ---------------------------------------------------------------------------
// Adversarial: the filesystem
// ---------------------------------------------------------------------------

#[test]
fn the_bundle_is_written_atomically_and_re_running_changes_nothing() {
    let dir = scratch("bundle");
    let out = dir.join("claude-plugin");
    let first = devmap(&["--json", "claude", "plugin", "--out", out.to_str().unwrap()])
        .ok()
        .json();
    let files = first["files"].as_array().expect("files");
    let json_files: Vec<&Value> = files
        .iter()
        .filter(|f| {
            f["path"]
                .as_str()
                .unwrap()
                .ends_with(".json")
        })
        .collect();
    let skill_files: Vec<&Value> = files
        .iter()
        .filter(|f| {
            f["path"]
                .as_str()
                .unwrap()
                .ends_with("SKILL.md")
        })
        .collect();
    assert_eq!(json_files.len(), 4, "json files: {first}");
    assert_eq!(
        skill_files.len(),
        5,
        "the plugin must ship the DevMap agent skills, not only hooks: {first}"
    );
    assert_eq!(
        first["changed"],
        files.len() as u64,
        "a fresh directory writes every file"
    );

    for file in files {
        let path = PathBuf::from(file["path"].as_str().unwrap());
        let text = std::fs::read_to_string(&path).expect("emitted file is readable");
        if path.extension().and_then(|e| e.to_str()) == Some("json") {
            serde_json::from_str::<Value>(&text)
                .unwrap_or_else(|e| panic!("{} is not valid JSON: {e}", path.display()));
        } else {
            assert!(
                text.contains("name:"),
                "{} is not a skill file: {text}",
                path.display()
            );
        }
        assert!(text.ends_with('\n'), "{}", path.display());
        // No temp file survived the write.
        assert!(
            !path.with_extension("tmp").exists(),
            "{} left a temp file behind",
            path.display()
        );
    }

    let second = devmap(&["--json", "claude", "plugin", "--out", out.to_str().unwrap()])
        .ok()
        .json();
    assert_eq!(
        second["changed"], 0,
        "re-emitting identical content must be a no-op, not a rewrite: {second}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// Concurrent emitters must not tear a file or lose to each other's rename.
#[test]
fn concurrent_emission_leaves_one_valid_bundle() {
    let dir = scratch("concurrent");
    let out = dir.join("claude-plugin");
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let out = out.clone();
            std::thread::spawn(move || {
                devmap(&["--json", "claude", "plugin", "--out", out.to_str().unwrap()])
            })
        })
        .collect();
    for handle in handles {
        let run = handle.join().expect("emitter thread");
        assert_eq!(
            run.status,
            Some(0),
            "a concurrent emitter failed\nstdout: {}\nstderr: {}",
            run.stdout,
            run.stderr
        );
    }
    let manifest = out.join("devmap/.claude-plugin/plugin.json");
    let text = std::fs::read_to_string(&manifest).expect("manifest exists");
    serde_json::from_str::<Value>(&text).expect("manifest is whole, not torn");
    // No process left a temp file stranded in the destination directory.
    let strays: Vec<_> = std::fs::read_dir(manifest.parent().unwrap())
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".tmp"))
        .collect();
    assert!(strays.is_empty(), "stranded temp files: {strays:?}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
#[cfg(unix)] // POSIX directory write bits; Windows directory read-only is not a write barrier.
fn an_unwritable_output_directory_fails_loudly_and_names_the_path() {
    let dir = scratch("readonly");
    let out = dir.join("locked");
    std::fs::create_dir_all(&out).unwrap();
    let mut perms = std::fs::metadata(&out).unwrap().permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o500); // r-x, no write
    }
    std::fs::set_permissions(&out, perms).unwrap();

    let run = devmap(&["claude", "plugin", "--out", out.to_str().unwrap()]);
    assert_ne!(run.status, Some(0), "stdout: {}", run.stdout);
    assert!(
        run.stderr.contains(out.to_str().unwrap()) || run.stderr.contains("could not write"),
        "the failure must name where it failed: {}",
        run.stderr
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&out).unwrap().permissions();
        perms.set_mode(0o700);
        std::fs::set_permissions(&out, perms).unwrap();
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
#[cfg(windows)]
fn a_read_only_output_file_is_refused_without_replacing_its_bytes() {
    let dir = scratch("readonly-file");
    let target = dir.join(".claude-plugin/marketplace.json");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, "preserve this file").unwrap();
    let original = std::fs::metadata(&target).unwrap().permissions();
    let mut perms = original.clone();
    perms.set_readonly(true);
    std::fs::set_permissions(&target, perms).unwrap();
    let run = devmap(&["claude", "plugin", "--out", dir.to_str().unwrap()]);
    assert_ne!(run.status, Some(0), "{}", run.stdout);
    assert!(run.stderr.contains("marketplace.json"), "{}", run.stderr);
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "preserve this file"
    );
    std::fs::set_permissions(&target, original).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

/// A non-UTF-8 store path is refused rather than written through
/// `Path::display()`, whose replacement characters would name no file on disk.
#[cfg(unix)]
#[test]
fn a_non_utf8_store_path_is_refused_rather_than_silently_mangled() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let raw = OsStr::from_bytes(b".devcouncil/\xff\xfe/devmap.sqlite");
    // Every command that turns a path into JSON, including `mcp --print-config`,
    // which is the entry an agent host is configured from: a mangled `args`
    // there points the host at a store that does not exist, and the host
    // reports only that the server would not start.
    for args in [
        vec!["claude", "hooks"],
        vec!["mcp", "--print-config"],
        vec!["claude", "plugin", "--dry-run"],
    ] {
        let out = Command::new(DEVMAP)
            .arg("--db")
            .arg(raw)
            .args(&args)
            .output()
            .expect("devmap runs");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert_ne!(
            out.status.code(),
            Some(0),
            "{args:?}: a path JSON cannot carry must fail, not be emitted lossily: {stdout}"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("not valid UTF-8"),
            "{args:?}: the refusal must say why: {stderr}"
        );
        // And the lossy form must not appear anywhere in what was produced.
        assert!(
            !stdout.contains('\u{FFFD}'),
            "{args:?}: no replacement character may reach the output"
        );
    }
}

#[test]
fn validating_a_missing_or_unparsable_file_fails_rather_than_passing() {
    let dir = scratch("validate");
    let missing = dir.join("nope.json");
    let run = devmap(&["claude", "validate", missing.to_str().unwrap()]);
    assert_ne!(run.status, Some(0));
    assert!(run.stderr.contains("could not read"), "{}", run.stderr);

    let garbage = dir.join("garbage.json");
    std::fs::write(&garbage, "{not json").unwrap();
    let run = devmap(&["claude", "validate", garbage.to_str().unwrap()]);
    assert_ne!(run.status, Some(0));
    assert!(run.stderr.contains("not valid JSON"), "{}", run.stderr);

    // A well-formed JSON document that is none of the three shapes must not
    // report a clean bill of health for a file nothing examined.
    let stranger = dir.join("stranger.json");
    std::fs::write(&stranger, r#"{"unrelated": true}"#).unwrap();
    let run = devmap(&["claude", "validate", stranger.to_str().unwrap()]);
    assert_ne!(
        run.status,
        Some(0),
        "an unrecognized document is not a passing one: {}",
        run.stdout
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// The validator's exit code is what a CI step reads, so it has to move.
#[test]
fn validate_exits_nonzero_on_a_bad_document_and_strict_promotes_warnings() {
    let dir = scratch("exit");
    let bad = dir.join("bad-hooks.json");
    std::fs::write(
        &bad,
        r#"{"hooks":{"PostToolUsage":[{"hooks":[{"type":"command","command":"x"}]}]}}"#,
    )
    .unwrap();
    let run = devmap(&["claude", "validate", bad.to_str().unwrap()]);
    assert_eq!(run.status, Some(1), "stdout: {}", run.stdout);
    assert!(run.stdout.contains("not a Claude Code hook event"));

    // Warning-only: clean by default, failing under --strict.
    let warn = dir.join("warn-manifest.json");
    std::fs::write(&warn, r#"{"name":"devmap","surprise":1}"#).unwrap();
    assert_eq!(
        devmap(&["claude", "validate", warn.to_str().unwrap()]).status,
        Some(0)
    );
    assert_eq!(
        devmap(&["claude", "validate", warn.to_str().unwrap(), "--strict"]).status,
        Some(1),
        "--strict must change the verdict, or it is decoration"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// What we emit is what we accept: every bundle file passes our own validator
/// in strict mode.
#[test]
fn every_emitted_file_passes_our_own_validator_in_strict_mode() {
    let dir = scratch("selfcheck");
    let out = dir.join("claude-plugin");
    let emitted = devmap(&["--json", "claude", "plugin", "--out", out.to_str().unwrap()])
        .ok()
        .json();
    for file in emitted["files"].as_array().unwrap() {
        let path = file["path"].as_str().unwrap();
        if path.ends_with(".mcp.json") || path.ends_with("SKILL.md") {
            continue; // MCP config and skills are not hook/manifest/marketplace JSON
        }
        let run = devmap(&["claude", "validate", path, "--strict"]);
        assert_eq!(
            run.status,
            Some(0),
            "{path}\n{}\n{}",
            run.stdout,
            run.stderr
        );
    }
    std::fs::remove_dir_all(&dir).ok();
}

// ---------------------------------------------------------------------------
// Differential: this table against the Python layer's
// ---------------------------------------------------------------------------

/// Extract a Python `frozenset({...})` / list literal's string members.
fn python_strings(source: &str, anchor: &str) -> Vec<String> {
    let start = source
        .find(anchor)
        .unwrap_or_else(|| panic!("{anchor} is gone from the Python source"));
    let tail = &source[start..];
    let open = tail.find('{').expect("literal opens");
    let close = tail[open..].find('}').expect("literal closes") + open;
    // `open + 1`: the slice must not include the opening brace, or the first
    // member arrives as `{"PreToolUse"` and is silently dropped — which reads
    // as the two tables disagreeing about an event that is in both.
    tail[open + 1..close]
        .split(',')
        .filter_map(|piece| {
            let piece = piece.trim();
            piece
                .strip_prefix('"')
                .and_then(|p| p.strip_suffix('"'))
                .map(str::to_string)
        })
        .collect()
}

/// The two implementations must agree on the event universe.
///
/// They emit different hooks — Dev Map's name `devmap` subcommands, DevCouncil's
/// name `devcouncil hook <slug>` — so the comparable artifact is the contract
/// itself: which event names exist, which use the narrow matcher set, and what
/// `SessionStart` accepts. A divergence here means one of the two is validating
/// against a surface that moved.
#[test]
fn the_event_contract_agrees_with_the_python_layer_field_by_field() {
    let python_path = repo_root().join("src/devcouncil/integrations/clients/hooks.py");
    // Not skipped when absent: a differential that quietly does not run reports
    // the same "passed" as one that ran and found agreement.
    let source = std::fs::read_to_string(&python_path).unwrap_or_else(|err| {
        panic!(
            "the differential needs the Python emitter at {}: {err}",
            python_path.display()
        )
    });

    let mut python_events = python_strings(&source, "CLAUDE_HOOK_EVENTS: frozenset[str]");
    python_events.sort();
    python_events.dedup();

    let ours = devmap(&["--json", "claude", "events"]).ok().json();
    let mut rust_events: Vec<String> = ours["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["event"].as_str().unwrap().to_string())
        .collect();
    rust_events.sort();

    assert_eq!(
        rust_events, python_events,
        "the Rust and Python event tables disagree; the documentation at \
         https://code.claude.com/docs/en/hooks#hook-events decides which is right"
    );

    let mut python_narrow = python_strings(&source, "_NARROW_MATCHER_EVENTS = frozenset");
    python_narrow.sort();
    assert_eq!(
        python_narrow,
        vec!["FileChanged".to_string(), "StopFailure".to_string()],
        "the narrow-matcher set moved on one side"
    );

    // SessionStart's alternation list is exact-matched, so a missing entry never
    // fires. Both sides must carry the same five sources.
    let python_session = source
        .lines()
        .find_map(|line| line.strip_prefix("SESSION_START_MATCHER = "))
        .expect("SESSION_START_MATCHER is gone")
        .trim()
        .trim_matches('"')
        .to_string();
    let block = devmap(&["--json", "claude", "hooks"]).ok().json();
    let rust_session = block["hooks"]["SessionStart"][0]["matcher"]
        .as_str()
        .expect("SessionStart matcher")
        .to_string();
    assert_eq!(
        rust_session, python_session,
        "the two emitters would register SessionStart for different session sources"
    );
}

/// Both emitters must agree that `timeout` is seconds.
///
/// The unit is the field's whole failure mode: the Python layer's plugin
/// generator once shipped 10000 and 150000 as if they were milliseconds, which
/// Claude Code read as a 2.8-hour and a 41.7-hour bound. Nothing in either
/// artifact says "seconds", so the only check is that every emitted number is
/// small enough to be one.
#[test]
fn every_emitted_timeout_is_plausible_as_seconds() {
    let block = devmap(&["--json", "claude", "hooks"]).ok().json();
    let mut seen = 0;
    for groups in block["hooks"].as_object().unwrap().values() {
        for group in groups.as_array().unwrap() {
            for handler in group["hooks"].as_array().unwrap() {
                if let Some(timeout) = handler.get("timeout") {
                    let secs = timeout.as_u64().expect("timeout is a whole number");
                    assert!(
                        (1..=600).contains(&secs),
                        "{secs} is outside the documented per-hook range for a command \
                         handler; a millisecond value would land here"
                    );
                    seen += 1;
                }
            }
        }
    }
    assert!(seen > 0, "no timeout was checked at all");
}

/// The plugin manifest must carry the same identity fields the Python bundle
/// does, so an installed Dev Map plugin can tell a user its terms and origin.
#[test]
fn the_plugin_manifest_carries_the_same_identity_fields_as_the_python_bundle() {
    let assets = repo_root().join("src/devcouncil/integrations/claude_assets.py");
    let source = std::fs::read_to_string(&assets)
        .unwrap_or_else(|err| panic!("the differential needs {}: {err}", assets.display()));
    let dir = scratch("identity");
    let out = dir.join("claude-plugin");
    devmap(&["--json", "claude", "plugin", "--out", out.to_str().unwrap()]).ok();
    let manifest: Value = serde_json::from_str(
        &std::fs::read_to_string(out.join("devmap/.claude-plugin/plugin.json")).unwrap(),
    )
    .unwrap();

    // Every key the Python manifest sets, minus the ones whose values are
    // DevCouncil's own text rather than a shared field.
    for key in [
        "name",
        "description",
        "version",
        "author",
        "homepage",
        "repository",
        "license",
        "keywords",
    ] {
        assert!(
            source.contains(&format!("\"{key}\"")),
            "{key} is no longer in the Python manifest; re-derive this list"
        );
        assert!(
            manifest.get(key).is_some(),
            "the Rust manifest omits {key}, which the Python bundle sets"
        );
    }
    assert_eq!(
        manifest["license"], "Apache-2.0",
        "both bundles ship the repository's own license"
    );
    std::fs::remove_dir_all(&dir).ok();
}

// ---------------------------------------------------------------------------
// Acceptance: the real thing
// ---------------------------------------------------------------------------

/// The bundle passes `claude plugin validate --strict`.
///
/// Ignored by default because it needs the `claude` binary, which a CI runner
/// does not have. `ignored` is reported by `cargo test` as its own count, so it
/// can never be mistaken for a pass — unlike a body that returns early. Run it
/// with `cargo test -p devmap-cli -- --ignored`.
#[test]
#[ignore = "requires the `claude` CLI on PATH; run with --ignored"]
fn the_emitted_bundle_passes_claude_plugin_validate_strict() {
    let dir = scratch("accept");
    let out = dir.join("claude-plugin");
    devmap(&["--json", "claude", "plugin", "--out", out.to_str().unwrap()]).ok();

    for target in [out.join("devmap"), out.clone()] {
        let result = Command::new("claude")
            .args(["plugin", "validate"])
            .arg(&target)
            .args(["--strict", "--json"])
            .output()
            .expect("`claude` is on PATH");
        let stdout = String::from_utf8_lossy(&result.stdout);
        assert_eq!(
            result.status.code(),
            Some(0),
            "claude plugin validate --strict rejected {}\nstdout: {stdout}\nstderr: {}",
            target.display(),
            String::from_utf8_lossy(&result.stderr)
        );
        let report: Value = serde_json::from_str(&stdout).expect("--json report");
        assert_eq!(report["success"], true, "{stdout}");
    }
    std::fs::remove_dir_all(&dir).ok();
}
