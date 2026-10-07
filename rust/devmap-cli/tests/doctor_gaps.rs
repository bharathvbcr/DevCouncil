//! Doctor classification and plugin/stray warnings (CLI-owned fields).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

fn scratch(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "devmap-doctor-{name}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
}

fn devmap_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_devmap"))
}

fn doctor_with_home(home: &Path, cwd: &Path) -> Value {
    let out = Command::new(devmap_bin())
        .args(["--json", "doctor"])
        .current_dir(cwd)
        .env("HOME", home)
        .env_remove("DEVMAP_HOME")
        .output()
        .expect("doctor runs");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(stdout.lines().next().unwrap()).expect("doctor json")
}

/// Record `dir` as Claude Code's active Dev Map plugin install. Only the
/// version `installed_plugins.json` names is the one Claude Code loads.
fn record_install(home: &Path, dir: &Path) {
    let plugins = home.join(".claude/plugins");
    fs::create_dir_all(&plugins).unwrap();
    fs::write(
        plugins.join("installed_plugins.json"),
        serde_json::json!({"version": 2, "plugins": {"devmap@devmap-local": [
            {"scope": "user", "installPath": dir}
        ]}})
        .to_string(),
    )
    .unwrap();
}

/// A well-formed bundle directory.
fn write_healthy_bundle(dir: &Path) {
    fs::create_dir_all(dir.join("hooks")).unwrap();
    fs::write(
        dir.join("hooks/hooks.json"),
        r#"{"hooks":{"SessionEnd":[{"hooks":[{"type":"command","command":"/abs/devmap hook session-end","timeout":3}]}]}}"#,
    )
    .unwrap();
    fs::write(dir.join(".mcp.json"), r#"{"mcpServers":{}}"#).unwrap();
}

/// Two versions in the cache, one of them active. The leftover used to read as
/// "installed plugin version 0.0.1 does not match binary" in every SessionStart
/// brief; it is housekeeping, not a fault, and only diagnostics mention it.
#[test]
fn a_leftover_cache_version_is_a_cleanup_note_not_a_plugin_warning() {
    let home = scratch("home-two-versions");
    let cwd = scratch("cwd-two-versions");
    let cache = home.join(".claude/plugins/cache/devmap-local/devmap");
    let active = cache.join(env!("CARGO_PKG_VERSION"));
    write_healthy_bundle(&active);
    write_healthy_bundle(&cache.join("0.0.1"));
    record_install(&home, &active);
    let payload = doctor_with_home(&home, &cwd);
    assert!(
        payload["plugin_warning"].is_null(),
        "only the active install is checked: {payload}"
    );
    let note = payload["plugin_cleanup_note"].as_str().unwrap_or("");
    assert!(
        note.contains("version(s) 0.0.1 under"),
        "the leftover, and only it, is named for cleanup: {payload}"
    );
    let _ = fs::remove_dir_all(&home);
    let _ = fs::remove_dir_all(&cwd);
}

/// An install record that cannot be read is not a record that names nothing:
/// the version check did not run, and the warning says so.
#[test]
fn an_unreadable_install_record_is_reported_not_skipped() {
    let home = scratch("home-bad-record");
    let cwd = scratch("cwd-bad-record");
    write_healthy_bundle(&home.join(".claude/plugins/cache/devmap-local/devmap/0.0.1"));
    fs::write(
        home.join(".claude/plugins/installed_plugins.json"),
        "{not json",
    )
    .unwrap();
    let payload = doctor_with_home(&home, &cwd);
    let warning = payload["plugin_warning"].as_str().unwrap_or("");
    assert!(
        warning.contains("unchecked") && warning.contains("installed_plugins.json"),
        "an unreadable record must be named: {payload}"
    );
    let _ = fs::remove_dir_all(&home);
    let _ = fs::remove_dir_all(&cwd);
}

#[test]
fn pinned_registrations_are_not_counted_as_global_duplicates() {
    let home = scratch("home-pin");
    let cwd = scratch("cwd-pin");
    fs::create_dir_all(home.join(".cursor")).unwrap();
    // One global (no --root) and one pinned project entry.
    fs::write(
        home.join(".cursor/mcp.json"),
        // Built, not formatted: a Windows path in a JSON literal is an
        // invalid escape, and an unparseable config registers nothing.
        serde_json::json!({"mcpServers": {"devmap": {
            "type": "stdio", "command": devmap_bin(), "args": ["mcp"]
        }}})
        .to_string(),
    )
    .unwrap();
    fs::create_dir_all(cwd.join(".cursor")).unwrap();
    fs::write(
        cwd.join(".cursor/mcp.json"),
        serde_json::json!({"mcpServers": {"devmap": {
            "type": "stdio",
            "command": devmap_bin(),
            "args": ["--root", cwd, "mcp"]
        }}})
        .to_string(),
    )
    .unwrap();
    let payload = doctor_with_home(&home, &cwd);
    let regs = &payload["mcp_registrations"];
    assert!(
        regs["pinned"].as_array().map(|a| a.len()).unwrap_or(0) >= 1,
        "{payload}"
    );
    // A single global must not warn.
    if regs["global"].as_array().map(|a| a.len()).unwrap_or(0) <= 1 {
        assert!(
            payload["duplicate_mcp_registration_warning"].is_null(),
            "one global + pinned must not warn: {payload}"
        );
    }
    let _ = fs::remove_dir_all(&home);
    let _ = fs::remove_dir_all(&cwd);
}

#[test]
fn stray_devmap_dir_is_reported() {
    let home = scratch("home-stray");
    let cwd = scratch("cwd-stray");
    fs::create_dir_all(home.join(".devmap")).unwrap();
    // No store under it.
    let payload = doctor_with_home(&home, &cwd);
    let warning = payload["stray_state_warning"].as_str().unwrap_or("");
    assert!(
        warning.contains(".devmap"),
        "expected .devmap stray warning, got {payload}"
    );
    let _ = fs::remove_dir_all(&home);
    let _ = fs::remove_dir_all(&cwd);
}

#[test]
fn plugin_dir_missing_hooks_is_reported() {
    let home = scratch("home-plugin");
    let cwd = scratch("cwd-plugin");
    let plugin = home.join(".claude/plugins/cache/devmap-local/devmap/0.2.0");
    fs::create_dir_all(&plugin).unwrap();
    record_install(&home, &plugin);
    // Marketplace-only layout: no hooks/hooks.json, no .mcp.json.
    fs::write(plugin.join("README.md"), "stale").unwrap();
    let payload = doctor_with_home(&home, &cwd);
    let warning = payload["plugin_warning"].as_str().unwrap_or("");
    assert!(
        warning.contains("missing hooks") || warning.contains("malformed"),
        "expected plugin warning, got {payload}"
    );
    let _ = fs::remove_dir_all(&home);
    let _ = fs::remove_dir_all(&cwd);
}

#[test]
fn plugin_version_mismatch_is_reported() {
    let home = scratch("home-ver");
    let cwd = scratch("cwd-ver");
    let plugin = home.join(".claude/plugins/cache/devmap-local/devmap/0.0.1");
    fs::create_dir_all(plugin.join("hooks")).unwrap();
    record_install(&home, &plugin);
    fs::write(
        plugin.join("hooks/hooks.json"),
        r#"{"hooks":{"SessionEnd":[{"hooks":[{"type":"command","command":"/abs/devmap hook session-end","timeout":3}]}]}}"#,
    )
    .unwrap();
    fs::write(plugin.join(".mcp.json"), r#"{"mcpServers":{}}"#).unwrap();
    let payload = doctor_with_home(&home, &cwd);
    let warning = payload["plugin_warning"].as_str().unwrap_or("");
    assert!(
        warning.contains("does not match binary") || warning.contains("0.0.1"),
        "expected version mismatch, got {payload}"
    );
    let _ = fs::remove_dir_all(&home);
    let _ = fs::remove_dir_all(&cwd);
}
