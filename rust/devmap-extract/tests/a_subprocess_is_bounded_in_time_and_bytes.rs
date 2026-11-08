//! The one subprocess runner keeps every promise its three callers used to
//! keep separately — or failed to.
//!
//! Each case stands a shell script in for the program, so the child's
//! behaviour is the test's to choose: stall, flood, close its pipes and hang,
//! fail with a message. Unix only, because the scripts are.

#![cfg(unix)]

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use devmap_extract::subprocess::{run_bounded, Bounds, Failure};

/// One directory per call, unique across the tests this binary runs in
/// parallel. A wall-clock stamp alone is not: macOS reports `SystemTime` at
/// microsecond resolution, and seven tests started by the harness in the same
/// instant collided on it — one test's script overwritten with another's
/// body, one test's directory removed under another's spawn.
fn script(body: &str) -> (PathBuf, PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "devmap-subprocess-{}-{sequence}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("child");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    (dir, path)
}

fn bounds(deadline_secs: u64, cap: usize) -> Bounds {
    Bounds {
        deadline: Duration::from_secs(deadline_secs),
        stdout_cap: cap,
        stderr_cap: 4096,
    }
}

#[test]
fn a_quiet_child_is_captured_whole() {
    let (dir, path) = script("echo out; echo err >&2; exit 3");
    let captured = run_bounded(&mut Command::new(&path), bounds(5, 1 << 20)).unwrap();
    assert_eq!(captured.stdout, b"out\n");
    assert_eq!(captured.stderr_trimmed(), "err");
    assert!(!captured.stdout_truncated && !captured.stderr_truncated);
    assert_eq!(
        captured.status.code(),
        Some(3),
        "a non-zero exit is an answer, not a failure: the caller decides"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_stalled_child_is_killed_at_the_deadline() {
    // `exec`, so the kill lands on the sleeper itself rather than on a shell
    // whose orphan would hold the pipes for the rest of its thirty seconds.
    let (dir, path) = script("exec sleep 30");
    let started = Instant::now();
    let failure = run_bounded(&mut Command::new(&path), bounds(1, 1 << 20))
        .expect_err("a child past its deadline must not come back as captured");
    let elapsed = started.elapsed();
    assert!(
        matches!(failure, Failure::Deadline { .. }),
        "expected the deadline failure, got {failure}"
    );
    assert!(
        failure.to_string().contains("killed"),
        "the message must say the child was killed: {failure}"
    );
    assert!(
        elapsed < Duration::from_secs(3),
        "the kill must land near the one-second deadline, not the sleeper's exit: {elapsed:?}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// Past the cap the pipe keeps draining, so the child finishes on its own
/// terms: its exit status still means what it says, and the caller pays for
/// the bytes, not for the deadline. (The churn reader this replaced closed the
/// pipe at its cap instead and let `SIGPIPE` end git — the same visible
/// result by a mechanism that turns every capped answer into a killed child.)
#[test]
fn output_past_the_cap_is_dropped_without_stalling_the_child() {
    // ~6 MiB of `a`, against a 1 MiB cap. The child must exit on its own well
    // inside the 10 s deadline, and exit *successfully*: a reader that stopped
    // at the cap would either leave it blocked on the pipe until the deadline
    // or, closing the pipe, kill it with `SIGPIPE`.
    let (dir, path) = script("head -c 6000000 /dev/zero | tr '\\0' a");
    let started = Instant::now();
    let captured = run_bounded(&mut Command::new(&path), bounds(10, 1 << 20)).unwrap();
    let elapsed = started.elapsed();
    assert_eq!(captured.stdout.len(), 1 << 20, "exactly the cap is kept");
    assert!(
        captured.stdout_truncated,
        "the caller must be told the answer is incomplete"
    );
    assert!(captured.stdout.iter().all(|byte| *byte == b'a'));
    assert!(
        captured.status.success(),
        "the child ran to completion: {:?}",
        captured.status
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "a flood must cost its bytes, not the deadline: {elapsed:?}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn stderr_is_capped_on_its_own() {
    let (dir, path) = script("head -c 100000 /dev/zero | tr '\\0' e >&2; echo fine");
    let captured = run_bounded(&mut Command::new(&path), bounds(5, 1 << 20)).unwrap();
    assert_eq!(captured.stdout, b"fine\n");
    assert!(!captured.stdout_truncated);
    assert_eq!(captured.stderr.len(), 4096);
    assert!(captured.stderr_truncated);
    let _ = std::fs::remove_dir_all(dir);
}

/// A child that closes both pipes and then hangs — a hook that daemonised —
/// must not turn EOF into an unbounded `wait`.
#[test]
fn a_child_that_closes_its_pipes_and_hangs_is_still_killed() {
    let (dir, path) = script("exec >/dev/null 2>&1; exec sleep 30");
    let started = Instant::now();
    let failure = run_bounded(&mut Command::new(&path), bounds(1, 1 << 20))
        .expect_err("EOF is not exit; the reap is bounded too");
    assert!(matches!(failure, Failure::Deadline { .. }), "{failure}");
    assert!(started.elapsed() < Duration::from_secs(3));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_missing_program_names_itself() {
    let failure = run_bounded(
        &mut Command::new("/nonexistent/devmap-no-such-program"),
        bounds(1, 1024),
    )
    .expect_err("spawning a missing program cannot succeed");
    assert!(matches!(failure, Failure::Spawn { .. }), "{failure}");
    assert!(
        failure.to_string().contains("devmap-no-such-program"),
        "the message names the program: {failure}"
    );
}

/// A `#[cfg(test)] mod name;` in the sibling `lib.rs` or `main.rs` is a test
/// module that lives under `src/` so it can see private items. The `tests/`
/// directory skip does not cover it, and its fixture `Command::new("git")` is
/// the same legitimate unbounded spawn the directory skip already allows.
fn cfg_test_module(path: &std::path::Path) -> bool {
    let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
        return false;
    };
    let Some(dir) = path.parent() else {
        return false;
    };
    let needle = format!("mod {stem};");
    for root_name in ["lib.rs", "main.rs"] {
        let Ok(text) = std::fs::read_to_string(dir.join(root_name)) else {
            continue;
        };
        let mut cfg_test = false;
        for line in text.lines() {
            let trim = line.trim();
            if trim.starts_with("//") {
                continue;
            }
            if trim == "#[cfg(test)]" {
                cfg_test = true;
                continue;
            }
            if cfg_test && trim == needle {
                return true;
            }
            if !trim.is_empty() {
                cfg_test = false;
            }
        }
    }
    false
}

/// `text` with comments and the insides of string and char literals blanked to
/// spaces, newlines and length kept, so a brace or a `Command::new(` that only
/// appears in prose or a literal neither opens a block nor counts as a spawn.
fn blank_comments_and_literals(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let blank = |c: char| if c == '\n' { '\n' } else { ' ' };
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c == '/' && next == Some('/') {
            while i < chars.len() && chars[i] != '\n' {
                out.push(' ');
                i += 1;
            }
        } else if c == '/' && next == Some('*') {
            let mut depth = 0usize;
            while i < chars.len() {
                if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
                    depth += 1;
                    out.push_str("  ");
                    i += 2;
                } else if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    out.push_str("  ");
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    out.push(blank(chars[i]));
                    i += 1;
                }
            }
        } else if c == 'r' && matches!(next, Some('"' | '#')) && {
            let mut j = i + 1;
            while chars.get(j) == Some(&'#') {
                j += 1;
            }
            chars.get(j) == Some(&'"')
        } {
            let mut hashes = 0usize;
            let mut j = i + 1;
            while chars[j] == '#' {
                hashes += 1;
                j += 1;
            }
            // Keep the opener, blank the body, keep the closer.
            out.extend(std::iter::repeat_n(' ', j + 1 - i));
            i = j + 1;
            while i < chars.len() {
                if chars[i] == '"' && (0..hashes).all(|k| chars.get(i + 1 + k) == Some(&'#')) {
                    out.extend(std::iter::repeat_n(' ', 1 + hashes));
                    i += 1 + hashes;
                    break;
                }
                out.push(blank(chars[i]));
                i += 1;
            }
        } else if c == '"' {
            out.push(' ');
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    out.push(' ');
                    i += 1;
                }
                out.push(blank(chars[i]));
                i += 1;
            }
            if i < chars.len() {
                out.push(' ');
                i += 1;
            }
        } else if c == '\'' {
            // A char literal closes within a few chars; a lifetime does not.
            let close = if next == Some('\\') {
                (i + 2..(i + 12).min(chars.len())).find(|&j| chars[j] == '\'')
            } else if chars.get(i + 2) == Some(&'\'') {
                Some(i + 2)
            } else {
                None
            };
            match close {
                Some(end) => {
                    out.extend(std::iter::repeat_n(' ', end + 1 - i));
                    i = end + 1;
                }
                None => {
                    out.push(c);
                    i += 1;
                }
            }
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// Zero-based indexes of the lines of `text` that spawn a child in production
/// code. An inline `#[cfg(test)] mod name { … }` block is test code however
/// large it is, so its lines are skipped, tracked by brace depth to the close
/// that matches its opener. Code before and after the block is still scanned.
fn production_spawn_lines(text: &str) -> Vec<usize> {
    let scrubbed = blank_comments_and_literals(text);
    let mut hits = Vec::new();
    let mut armed = false;
    let mut depth = 0usize;
    for (index, line) in scrubbed.lines().enumerate() {
        let trim = line.trim();
        if depth > 0 {
            depth += line.matches('{').count();
            depth -= line.matches('}').count().min(depth);
            continue;
        }
        let rest = trim.strip_prefix("#[cfg(test)]").map(str::trim);
        let candidate = match rest {
            Some("") => {
                armed = true;
                continue;
            }
            Some(rest) => rest,
            None if armed && trim.starts_with("#[") => continue,
            None if armed => trim,
            None => "",
        };
        armed = false;
        let item = candidate
            .strip_prefix("pub ")
            .or_else(|| {
                candidate
                    .strip_prefix("pub(")
                    .and_then(|r| r.split_once(") ").map(|(_, r)| r))
            })
            .unwrap_or(candidate);
        // `mod name;` has a `;` before any `{`; `mod name { … ; … }` does not.
        let opens_a_body = item
            .find('{')
            .is_some_and(|brace| !item[..brace].contains(';'));
        if item.starts_with("mod ") && opens_a_body {
            depth = line.matches('{').count().saturating_sub(line.matches('}').count());
            continue;
        }
        if line.contains("Command::new(") {
            hits.push(index);
        }
    }
    hits
}

#[test]
fn an_inline_cfg_test_module_is_exempt_and_production_spawns_are_not() {
    let source = r##"use std::process::Command;

pub fn production() {
    Command::new("git"); // flagged
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() {
        Command::new("git");
        let _ = "}"; // a brace in a literal must not close the block
        let _ = '{'; // nor open one
        let _ = r#"} } }"#;
        /* } */
        if true {
            Command::new("chflags");
        }
    }

    mod nested {
        fn deeper() {
            Command::new("devmap");
        }
    }
}

pub fn after() {
    Command::new("git"); // flagged again, the block has closed
}

#[cfg(test)]
#[allow(dead_code)]
pub(crate) mod odd { fn f() { Command::new("x"); } }

#[cfg(test)]
mod declared_elsewhere;

pub fn last<'a>(_: &'a str) {
    Command::new("git"); // flagged: a lifetime must not read as a char literal
}
"##;
    let hits: Vec<usize> = production_spawn_lines(source)
        .into_iter()
        .map(|index| index + 1)
        .collect();
    assert_eq!(
        hits,
        [4, 29, 40],
        "only production spawns are flagged; scrubbed text was:\n{}",
        blank_comments_and_literals(source)
    );
    assert_eq!(
        production_spawn_lines("mod tests {\n    fn f() { Command::new(\"x\"); }\n}\n"),
        [1],
        "a module without `#[cfg(test)]` is production code"
    );
}

/// Structural: no library crate's production code spawns a child on its own.
/// The three runners this module replaced each began as one reasonable
/// `Command::new`, and the way a fourth arrives is the same — so a new one
/// is a failure until it is either routed through `run_bounded` or listed
/// here with its reason.
///
/// The walk covers `devmap-{extract,resolve,analyze,store,query,serve}` only.
/// `dc-*` crates share this workspace but are not the kernel; `devmap-cli`
/// re-execs itself for SessionStart auto-rebuild and stamps git from
/// `build.rs`, which cannot link this runner.
#[test]
fn every_child_process_in_the_kernel_goes_through_the_runner() {
    // `CARGO_MANIFEST_DIR` is `<workspace>/devmap-extract`.
    let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crate lives in the rust workspace");
    let mut stack = Vec::new();
    for entry in std::fs::read_dir(workspace).expect("read workspace") {
        let path = entry.expect("dir entry").path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if path.is_dir() && name.starts_with("devmap-") && name != "devmap-cli" {
            stack.push(path);
        }
    }
    let mut scanned = 0usize;
    let mut offenders = Vec::new();
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read crates tree") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                // Only production sources: tests spawn the binary and git
                // for their fixtures, legitimately and unbounded.
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                if name == "tests" || name == "target" || name == "examples" || name == "benches" {
                    continue;
                }
                stack.push(path);
                continue;
            }
            if path.extension().is_none_or(|ext| ext != "rs")
                || path
                    .file_name()
                    .is_some_and(|name| name == "subprocess.rs" || name == "build.rs")
                || cfg_test_module(&path)
            {
                continue;
            }
            scanned += 1;
            let text = std::fs::read_to_string(&path).expect("read source");
            let lines: Vec<&str> = text.lines().collect();
            for index in production_spawn_lines(&text) {
                offenders.push(format!(
                    "{}:{}: {}",
                    path.display(),
                    index + 1,
                    lines[index].trim()
                ));
            }
        }
    }
    assert!(
        scanned > 50,
        "the scan must have found the kernel's sources: {scanned} files"
    );
    assert!(
        offenders.is_empty(),
        "a child process spawned outside `devmap_extract::subprocess` has no deadline and no \
         output cap; route it through `run_bounded`:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn the_git_constructor_refuses_prompts_and_optional_locks() {
    let command = devmap_extract::subprocess::git(std::path::Path::new("/tmp"), ["status"])
        .expect("a git outside /tmp resolves");
    let args: Vec<String> = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert_eq!(args[..2], ["-C", "/tmp"]);
    assert_eq!(
        args[2..],
        devmap_extract::subprocess::guarded_args(["status"])
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>()[..]
    );
    let envs: Vec<(String, String)> = command
        .get_envs()
        .filter_map(|(key, value)| {
            Some((
                key.to_string_lossy().into_owned(),
                value?.to_string_lossy().into_owned(),
            ))
        })
        .collect();
    assert!(envs.contains(&("GIT_TERMINAL_PROMPT".to_string(), "0".to_string())));
    assert!(envs.contains(&("GIT_OPTIONAL_LOCKS".to_string(), "0".to_string())));
}
