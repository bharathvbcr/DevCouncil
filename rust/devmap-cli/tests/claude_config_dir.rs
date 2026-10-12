//! `CLAUDE_CONFIG_DIR` pins a Claude Code profile. Doctor must read that
//! profile, not `~/.claude`, and say when it registers the Dev Map MCP server
//! but loads none of its hooks.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

fn scratch(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "devmap-config-dir-{name}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    root
}

/// `devmap doctor` with `CLAUDE_CONFIG_DIR` set to `config_dir` (or removed).
fn doctor(home: &Path, cwd: &Path, config_dir: Option<&str>) -> Value {
    let mut command = Command::new(env!("CARGO_BIN_EXE_devmap"));
    command
        .args(["--json", "doctor"])
        .current_dir(cwd)
        .env("HOME", home)
        .env_remove("DEVMAP_HOME");
    match config_dir {
        Some(dir) => command.env("CLAUDE_CONFIG_DIR", dir),
        None => command.env_remove("CLAUDE_CONFIG_DIR"),
    };
    let out = command.output().expect("doctor runs");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(stdout.lines().next().unwrap()).expect("doctor json")
}

fn register_mcp(config: &Path) {
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(
        config,
        json!({"mcpServers": {"devmap": {"command": "/abs/devmap", "args": ["mcp"]}}}).to_string(),
    )
    .unwrap();
}

fn warning(payload: &Value) -> Option<String> {
    payload["profile_hooks_warning"]
        .as_str()
        .map(str::to_string)
}

#[test]
fn a_seat_profile_with_the_mcp_server_and_no_hooks_is_warned_about() {
    let home = scratch("seat-home");
    let cwd = scratch("seat-cwd");
    let profile = home.join(".claude-seat");
    register_mcp(&profile.join(".claude.json"));
    fs::write(profile.join("settings.json"), r#"{"theme":"dark"}"#).unwrap();
    // The default profile has hooks; the active one does not.
    let decoy = home.join(".claude");
    fs::create_dir_all(&decoy).unwrap();
    fs::write(
        decoy.join("settings.json"),
        r#"{"hooks":{"SessionEnd":[{"hooks":[{"type":"command","command":"/abs/devmap hook session-end"}]}]}}"#,
    )
    .unwrap();
    register_mcp(&home.join(".claude.json"));

    let payload = doctor(&home, &cwd, Some(profile.to_str().unwrap()));
    let text = warning(&payload).expect("the seat profile has the server and no hooks");
    assert!(text.contains(profile.to_str().unwrap()), "{text}");
    assert!(
        payload["mcp_registrations"]
            .to_string()
            .contains(profile.join(".claude.json").to_str().unwrap()),
        "the registration listed is the profile's own: {}",
        payload["mcp_registrations"]
    );
    // Unset, the same machine reads `~/.claude` and its hooks.
    assert_eq!(warning(&doctor(&home, &cwd, None)), None);
}

#[test]
fn hooks_in_any_scope_the_profile_loads_clear_the_warning() {
    let cwd = scratch("scopes-cwd");
    let hooks = r#"{"hooks":{"SessionEnd":[{"hooks":[{"type":"command","command":"/abs/devmap hook session-end"}]}]}}"#;
    for place in ["profile", "project", "project-local"] {
        let home = scratch("scopes-home");
        let profile = home.join("profile");
        register_mcp(&profile.join(".claude.json"));
        let target = match place {
            "profile" => profile.join("settings.json"),
            "project" => cwd.join(".claude/settings.json"),
            _ => cwd.join(".claude/settings.local.json"),
        };
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, hooks).unwrap();
        let payload = doctor(&home, &cwd, Some(profile.to_str().unwrap()));
        assert_eq!(warning(&payload), None, "{place}");
        let _ = fs::remove_file(&target);
    }
}

#[test]
fn an_enabled_plugin_that_ships_hooks_clears_the_warning() {
    let home = scratch("plugin-home");
    let cwd = scratch("plugin-cwd");
    let profile = home.join("profile");
    register_mcp(&profile.join(".claude.json"));
    let bundle = profile.join("plugins/cache/devmap-local/devmap/0.0.1");
    fs::create_dir_all(bundle.join("hooks")).unwrap();
    fs::write(
        bundle.join("hooks/hooks.json"),
        r#"{"hooks":{"SessionEnd":[{"hooks":[{"type":"command","command":"devmap hook session-end"}]}]}}"#,
    )
    .unwrap();
    fs::write(bundle.join(".mcp.json"), r#"{"mcpServers":{}}"#).unwrap();
    fs::write(
        profile.join("plugins/installed_plugins.json"),
        json!({"version": 2, "plugins": {"devmap@devmap-local": [
            {"scope": "user", "installPath": bundle}
        ]}})
        .to_string(),
    )
    .unwrap();
    fs::write(
        profile.join("settings.json"),
        r#"{"enabledPlugins":{"devmap@devmap-local":true}}"#,
    )
    .unwrap();
    let payload = doctor(&home, &cwd, Some(profile.to_str().unwrap()));
    assert_eq!(warning(&payload), None);
}

#[test]
fn an_empty_value_falls_back_to_the_default_profile() {
    let home = scratch("empty-home");
    let cwd = scratch("empty-cwd");
    register_mcp(&home.join(".claude.json"));
    let payload = doctor(&home, &cwd, Some(""));
    let text = warning(&payload).expect("an empty CLAUDE_CONFIG_DIR is unset");
    assert!(
        text.contains(home.join(".claude").to_str().unwrap()),
        "{text}"
    );
}

#[test]
fn a_relative_value_is_read_from_the_working_directory() {
    let home = scratch("relative-home");
    let cwd = scratch("relative-cwd");
    register_mcp(&cwd.join("seat/.claude.json"));
    let payload = doctor(&home, &cwd, Some("seat"));
    let text = warning(&payload).expect("the relative profile registers the server");
    assert!(text.contains(cwd.join("seat").to_str().unwrap()), "{text}");
}

#[test]
fn a_missing_profile_directory_is_not_a_warning_or_a_failure() {
    let home = scratch("missing-home");
    let cwd = scratch("missing-cwd");
    let payload = doctor(&home, &cwd, Some(home.join("absent").to_str().unwrap()));
    assert_eq!(warning(&payload), None);
}

#[cfg(unix)]
#[test]
fn a_symlinked_profile_is_followed() {
    let home = scratch("link-home");
    let cwd = scratch("link-cwd");
    let real = home.join("real");
    register_mcp(&real.join(".claude.json"));
    let link = home.join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let payload = doctor(&home, &cwd, Some(link.to_str().unwrap()));
    assert!(warning(&payload).is_some());
}

#[test]
fn a_profile_that_cannot_be_read_is_reported_unchecked_not_clean() {
    let home = scratch("broken-home");
    let cwd = scratch("broken-cwd");
    let profile = home.join("profile");
    register_mcp(&profile.join(".claude.json"));
    fs::write(profile.join("settings.json"), "{not json").unwrap();
    let payload = doctor(&home, &cwd, Some(profile.to_str().unwrap()));
    let text = warning(&payload).expect("an unreadable settings.json is not a pass");
    assert!(text.contains("unchecked"), "{text}");
}

/// Only a SessionEnd hook writes the report. A profile that runs just
/// `devmap hook pre-tool-use` is as record-less as one with no hooks.
#[test]
fn a_profile_with_only_a_pre_tool_use_hook_is_still_warned_about() {
    let home = scratch("pretool-home");
    let cwd = scratch("pretool-cwd");
    let profile = home.join("profile");
    register_mcp(&profile.join(".claude.json"));
    fs::write(
        profile.join("settings.json"),
        r#"{"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"/abs/devmap hook pre-tool-use"}]}]}}"#,
    )
    .unwrap();
    let text = warning(&doctor(&home, &cwd, Some(profile.to_str().unwrap())))
        .expect("no SessionEnd hook, no records");
    assert!(text.contains("SessionEnd"), "{text}");
}

/// An enabled plugin whose hooks.json lacks SessionEnd records nothing either;
/// one whose hooks.json cannot be parsed is unchecked, never clean.
#[test]
fn a_plugin_without_a_session_end_hook_is_warned_about_and_a_bad_one_is_unchecked() {
    for (body, expect) in [
        (r#"{"hooks":{"PreToolUse":[]}}"#, "SessionEnd hook"),
        ("{not json", "unchecked"),
    ] {
        let home = scratch("plugin-end-home");
        let cwd = scratch("plugin-end-cwd");
        let profile = home.join("profile");
        register_mcp(&profile.join(".claude.json"));
        let bundle = profile.join("plugins/cache/devmap-local/devmap/0.0.1");
        fs::create_dir_all(bundle.join("hooks")).unwrap();
        fs::write(bundle.join("hooks/hooks.json"), body).unwrap();
        fs::write(bundle.join(".mcp.json"), r#"{"mcpServers":{}}"#).unwrap();
        fs::write(
            profile.join("plugins/installed_plugins.json"),
            json!({"version": 2, "plugins": {"devmap@devmap-local": [
                {"scope": "user", "installPath": bundle}
            ]}})
            .to_string(),
        )
        .unwrap();
        fs::write(
            profile.join("settings.json"),
            r#"{"enabledPlugins":{"devmap@devmap-local":true}}"#,
        )
        .unwrap();
        let text = warning(&doctor(&home, &cwd, Some(profile.to_str().unwrap())))
            .unwrap_or_else(|| panic!("{body}: expected a warning"));
        assert!(text.contains(expect), "{body}: {text}");
    }
}
