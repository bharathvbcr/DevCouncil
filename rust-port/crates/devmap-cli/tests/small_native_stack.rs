//! Windows starts executable entry points with a 1 MiB stack. Exercise that
//! constraint on Unix too, so the portability regression is reproducible locally.
#![cfg(unix)]

#[test]
fn parser_introspection_survives_a_one_mib_entry_stack() {
    let root = std::env::temp_dir().join(format!("devmap-small-stack-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    for args in [
        ["status", "--json"].as_slice(),
        ["claude", "events", "--json"].as_slice(),
    ] {
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", "ulimit -s 1024; exec \"$@\"", "stack-probe"])
            .arg(env!("CARGO_BIN_EXE_devmap"))
            .args(args)
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let status: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(status.is_object());
    }
    std::fs::remove_dir_all(root).unwrap();
}
