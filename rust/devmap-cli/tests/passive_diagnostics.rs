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
/// It *was* unbounded. `inventory_devmap_binaries` hashed every discovered
/// binary by slurping the whole file, copying it again, and reallocating that
/// copy to twice its size before a scalar compression loop touched it — so this
/// process's own unoptimised 109 MB executable cost ~330 MB resident and more
/// than half a minute, with nothing to stop it. Both `paths` and `doctor` reach
/// it, which is why both are exercised here.
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

/// The stale-server check probes one child process per matching `ps` line, and
/// a child here is not cheap — `run_bounded` gives each one two drain threads,
/// a process group and a polled reap. Each probe was bounded at 100 ms; the
/// fan-out across them was not, and the fan-out is what scales with the
/// machine. Measured at ~13 ms per matching line before this cap: sixty MCP
/// servers open cost three seconds, and nothing stopped a longer list costing
/// minutes, on the command every agent is told to run first.
///
/// `ps` is shimmed rather than the real process table read, because the
/// property under test is how the scan behaves when the list is long — which
/// is not something a test may arrange by starting three hundred servers.
#[test]
fn stale_server_scan_is_capped_and_says_how_much_it_examined() {
    let root = std::env::temp_dir().join(format!(
        "devmap-psfanout-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(root.join("bin")).unwrap();
    fs::create_dir_all(root.join("home")).unwrap();
    let ps = root.join("bin/ps");
    // Start times far in the past, so every line is "older than the binary" and
    // the scan has real work to report rather than an empty result.
    fs::write(
        &ps,
        "#!/bin/sh\ni=0\nwhile [ $i -lt 300 ]; do\n  echo \"$((1000+i)) Mon Jan 01 00:00:01 2001 /usr/local/bin/devmap mcp\"\n  i=$((i+1))\ndone\n",
    )
    .unwrap();
    fs::set_permissions(&ps, fs::Permissions::from_mode(0o755)).unwrap();
    symlink("/bin/date", root.join("bin/date")).unwrap();

    let mut process = Command::new(env!("CARGO_BIN_EXE_devmap"));
    process
        .args(["--json", "paths"])
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
    .unwrap_or_else(|failure| panic!("a long process list must not run away: {failure}"));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let warning = payload["stale_server_warning"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a stale-server reading: {payload}"));
    assert!(
        warning.contains("scan capped"),
        "a capped scan must not be presented as complete coverage: {warning}"
    );
    assert!(
        warning.contains("of 300 devmap mcp processes"),
        "the answer must carry both numbers, not just the one it reached: {warning}"
    );
    fs::remove_dir_all(root).unwrap();
}
