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
