#![cfg(unix)]

use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn diagnostics_never_execute_discovered_commands() {
    let root = std::env::temp_dir().join(format!(
        "devmap-passive-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(root.join(".cursor")).unwrap();
    fs::create_dir_all(root.join("home")).unwrap();
    fs::create_dir_all(root.join("bin")).unwrap();
    let candidate = root.join("repository-command");
    fs::write(
        &candidate,
        "#!/bin/sh\necho ran >> \"$DEVMAP_PROBE_MARKER\"\necho 'devmap 99.0 (build untrusted)'\n",
    )
    .unwrap();
    fs::set_permissions(&candidate, fs::Permissions::from_mode(0o755)).unwrap();
    symlink(&candidate, root.join("bin/devmap")).unwrap();
    let marker = root.join("executed");
    for registration in [candidate.to_str().unwrap(), "devmap"] {
        fs::write(
            root.join(".cursor/mcp.json"),
            serde_json::to_vec(&serde_json::json!({
                "mcpServers": { "devmap": { "command": registration } }
            }))
            .unwrap(),
        )
        .unwrap();
        for command in ["paths", "doctor"] {
            let mut process = Command::new(env!("CARGO_BIN_EXE_devmap"));
            process
                .args(["--json", command])
                .current_dir(&root)
                .env("HOME", root.join("home"))
                .env(
                    "PATH",
                    root.join(if registration == "devmap" {
                        "bin"
                    } else {
                        "home"
                    }),
                )
                .env("DEVMAP_PROBE_MARKER", &marker)
                .env_remove("DEVMAP_HOME");
            let output = devmap_extract::subprocess::run_bounded(
                &mut process,
                devmap_extract::subprocess::Bounds {
                    deadline: std::time::Duration::from_secs(20),
                    stdout_cap: 1024 * 1024,
                    stderr_cap: 16384,
                },
            )
            .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                !marker.exists(),
                "{command} executed discovered command {registration}"
            );
            let payload: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            let rows = payload["binaries"].as_array().unwrap();
            let external = rows
                .iter()
                .find(|row| row["path"] == candidate.canonicalize().unwrap().to_str().unwrap())
                .unwrap();
            assert!(external["version"].is_null());
            assert!(external["build_id"].is_null());
            assert_eq!(external["probe_status"], "skipped");
            assert!(external["exists"].as_bool().unwrap());
            assert!(rows
                .iter()
                .any(|row| row["version"] == env!("CARGO_PKG_VERSION")
                    && row["probe_status"] == "current_process"));
        }
    }
    fs::remove_dir_all(root).unwrap();
}

/// A health command that can run for an unbounded time is worse than one that
/// reports "unknown", and `devmap paths` is the command the generated agent
/// guide tells every agent to run first.
///
/// It *was* unbounded, and was fixed in two steps that answer different halves.
/// The hash itself became `sha2` read in 64 KiB chunks and memoised on
/// `(path, size, mtime, ctime)`, which removed the slurp-copy-realloc that made
/// this process's own unoptimised executable cost hundreds of megabytes resident
/// and tens of seconds. That makes a *repeated* inventory free; it does not make
/// the first one finite, which is what `sha256::Budget` is for. Both `paths` and
/// `doctor` reach the inventory, which is why both are exercised here.
///
/// The bound below is deliberately far looser than the diagnostic's own budget
/// and far tighter than the hang it replaces: it must not go red because a
/// loaded machine took an extra second, and it must go red if the bound is ever
/// removed again.
#[test]
fn diagnostics_stay_inside_their_own_budget() {
    let root = std::env::temp_dir().join(format!(
        "devmap-budget-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(root.join("home")).unwrap();
    fs::create_dir_all(root.join("bin")).unwrap();
    // A small discovered binary, so that "hashing still works" is proved by
    // something whose cost cannot depend on how busy the machine is. The large
    // one in this fixture is the test binary itself, and whether *it* fits the
    // budget is a property of the machine, not of the contract.
    let small = root.join("bin/devmap");
    fs::write(&small, b"#!/bin/sh\nexit 0\n").unwrap();
    let small_key = small.canonicalize().unwrap().display().to_string();
    for command in ["paths", "doctor"] {
        let mut process = Command::new(env!("CARGO_BIN_EXE_devmap"));
        process
            .args(["--json", command])
            .current_dir(&root)
            .env("HOME", root.join("home"))
            .env("PATH", root.join("bin"))
            .env_remove("DEVMAP_HOME");
        let output = devmap_extract::subprocess::run_bounded(
            &mut process,
            devmap_extract::subprocess::Bounds {
                deadline: std::time::Duration::from_secs(15),
                stdout_cap: 1024 * 1024,
                stderr_cap: 16384,
            },
        )
        .unwrap_or_else(|failure| panic!("{command} must finish inside its budget: {failure}"));
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let payload: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let rows = payload["binaries"].as_array().unwrap();
        assert!(!rows.is_empty(), "{command} found no binaries to report on");
        let small_row = rows
            .iter()
            .find(|row| row["path"] == serde_json::json!(small_key))
            .unwrap_or_else(|| panic!("{command} did not list the discovered binary: {payload}"));
        assert_eq!(
            small_row["sha256_status"], "hashed",
            "a small discovered binary must still get a real digest: {small_row}"
        );
        assert_eq!(small_row["sha256"].as_str().map(str::len), Some(64));
        for row in rows {
            // The reading that could not be taken must not arrive looking like
            // the reading that was. Every row says which of the three it is,
            // and a hash that did not happen carries its reason.
            let status = row["sha256_status"]
                .as_str()
                .unwrap_or_else(|| panic!("{command} row has no sha256_status: {row}"));
            match status {
                "hashed" => {
                    assert_eq!(row["sha256"].as_str().map(str::len), Some(64), "{row}");
                    assert!(row["sha256_error"].is_null(), "{row}");
                }
                "unavailable" => {
                    assert!(row["sha256"].is_null(), "{row}");
                    assert!(
                        !row["sha256_error"]
                            .as_str()
                            .unwrap_or_default()
                            .trim()
                            .is_empty(),
                        "an unavailable hash must say why: {row}"
                    );
                }
                // Deliberately not coupled to `exists`: PATH rows report
                // `exists: true` for anything that canonicalised, so a
                // directory named `devmap` on PATH is `absent` for hashing and
                // `exists` for the inventory. That predates this bound and is
                // not what this test is about — what must hold is that nothing
                // is offered as a digest.
                "absent" => {
                    assert!(row["sha256"].is_null(), "{row}");
                    assert!(row["sha256_error"].is_null(), "{row}");
                }
                other => panic!("{command} reported unknown sha256_status {other:?}: {row}"),
            }
        }
    }
    fs::remove_dir_all(root).unwrap();
}

/// A stale-server check that could not run must not answer like one that ran
/// and found nothing.
///
/// Both halves of this were reported as `None` — the same value a completed
/// clean scan returns — so anything looking for a warning string read "no stale
/// servers" out of a check that never produced a process list. `ps` is shimmed
/// because a failing process table is not something a test may arrange on the
/// real one.
///
/// There is no fan-out left to cap here: `etime=` is elapsed time parsed in
/// process, where this once asked for a local-time `lstart=` and spawned a
/// `date` per matching line. A long list is now string parsing, so this fixture
/// prints three hundred rows to show that length alone is not a problem.
#[test]
fn a_stale_server_check_that_could_not_run_says_so() {
    for (label, script, expected) in [
        (
            "nonzero exit",
            "#!/bin/sh\necho 'ps: unsupported option' >&2\nexit 1\n",
            "exited with 1",
        ),
        ("not on PATH", "", "could not be read"),
    ] {
        let root = std::env::temp_dir().join(format!(
            "devmap-psfail-{}-{}",
            std::process::id(),
            NEXT_SCRATCH.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::create_dir_all(root.join("home")).unwrap();
        if !script.is_empty() {
            let ps = root.join("bin/ps");
            fs::write(&ps, script).unwrap();
            fs::set_permissions(&ps, fs::Permissions::from_mode(0o755)).unwrap();
        }

        let warning = paths_field(&root, "stale_server_warning");
        let warning = warning
            .as_str()
            .unwrap_or_else(|| panic!("{label}: expected a reading, got {warning}"));
        assert!(
            warning.contains("unavailable"),
            "{label}: a check that did not run must say so: {warning}"
        );
        assert!(
            warning.contains(expected),
            "{label}: the reason must name what happened: {warning}"
        );
        fs::remove_dir_all(root).unwrap();
    }
}

/// A process list long enough to have been a fan-out problem is now just lines.
#[test]
fn a_long_process_list_is_examined_whole() {
    let root = std::env::temp_dir().join(format!(
        "devmap-pslong-{}-{}",
        std::process::id(),
        NEXT_SCRATCH.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    fs::create_dir_all(root.join("bin")).unwrap();
    fs::create_dir_all(root.join("home")).unwrap();
    let ps = root.join("bin/ps");
    // `etime` of 99 days, so every row predates any binary mtime and all three
    // hundred must appear in the answer.
    fs::write(
        &ps,
        "#!/bin/sh\ni=0\nwhile [ $i -lt 300 ]; do\n  echo \"$((1000+i)) 99-00:00:01 /usr/local/bin/devmap mcp\"\n  i=$((i+1))\ndone\n",
    )
    .unwrap();
    fs::set_permissions(&ps, fs::Permissions::from_mode(0o755)).unwrap();

    let warning = paths_field(&root, "stale_server_warning");
    let warning = warning
        .as_str()
        .unwrap_or_else(|| panic!("expected a stale-server reading, got {warning}"));
    assert!(
        warning.contains("started before the installed binary's mtime"),
        "{warning}"
    );
    // Every line reached, so no coverage caveat belongs in the answer.
    assert!(
        !warning.contains("capped") && !warning.contains("incomplete"),
        "nothing caps this scan any more: {warning}"
    );
    // The shim's pids run 1000..1299, so the answer must name them rather than
    // only counting them.
    assert!(
        warning.contains("1000"),
        "the pids must be listed: {warning}"
    );
    assert!(
        warning.contains("1299"),
        "the last pid must be listed: {warning}"
    );
    fs::remove_dir_all(root).unwrap();
}

/// Distinct scratch names without a clock: nanosecond stamps collide when cargo
/// runs these in parallel on macOS, because the spawn is faster than the clock
/// is fine.
static NEXT_SCRATCH: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Run `devmap --json paths` in `root` with a shimmed PATH and return one field.
fn paths_field(root: &std::path::Path, field: &str) -> serde_json::Value {
    let mut process = Command::new(env!("CARGO_BIN_EXE_devmap"));
    process
        .args(["--json", "paths"])
        .current_dir(root)
        .env("HOME", root.join("home"))
        .env("PATH", root.join("bin"))
        .env_remove("DEVMAP_HOME");
    let output = devmap_extract::subprocess::run_bounded(
        &mut process,
        devmap_extract::subprocess::Bounds {
            deadline: std::time::Duration::from_secs(15),
            stdout_cap: 1024 * 1024,
            stderr_cap: 16384,
        },
    )
    .unwrap_or_else(|failure| panic!("paths must finish inside its budget: {failure}"));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    payload[field].clone()
}
