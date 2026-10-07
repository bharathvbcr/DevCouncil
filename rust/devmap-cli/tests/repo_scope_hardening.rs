//! Doctor / paths / build refusals for shared MCP registration and unsafe roots.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

/// Whether `text` names `path` in a spelling the platform gives it: as
/// written, or canonical. Windows canonicalises to a `\\?\` verbatim path
/// with long names where the temp dir used 8.3 short ones (`RUNNER~1`), and
/// macOS to `/private/var` for `/var`; every one is the same directory.
fn names(text: &str, path: &std::path::Path) -> bool {
    if text.contains(&path.display().to_string()) {
        return true;
    }
    let Ok(canonical) = path.canonicalize() else {
        return false;
    };
    let shown = canonical.display().to_string();
    text.contains(shown.strip_prefix(r"\\?\").unwrap_or(&shown))
}

fn scratch(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "devmap-cli-scope-{name}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn devmap() -> PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap")
}

fn run_in(root: &Path, args: &[&str], home: Option<&Path>) -> Output {
    let mut cmd = Command::new(devmap());
    cmd.args(args).current_dir(root).env_remove("DEVMAP_HOME");
    if let Some(home) = home {
        cmd.env("HOME", home);
    }
    cmd.output()
        .unwrap_or_else(|e| panic!("devmap {args:?}: {e}"))
}

fn one_json(output: &Output, what: &str) -> Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.lines().next().unwrap_or_else(|| {
        panic!(
            "{what}: empty stdout\nstderr={}",
            String::from_utf8_lossy(&output.stderr)
        )
    });
    serde_json::from_str(line).unwrap_or_else(|e| panic!("{what}: {e}: {line}"))
}

fn write_mcp(path: &Path, command: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(
        path,
        // Built, not formatted: a Windows path spliced into a JSON literal is
        // `"C:\Users\…"`, an invalid escape, and the host config it writes
        // fails to parse — doctor then correctly reports nothing registered.
        serde_json::json!({"mcpServers": {"devmap": {"type": "stdio", "command": command, "args": ["mcp"]}}})
            .to_string(),
    )
    .unwrap();
}

/// Lay down a Dev Map plugin install the way Claude Code records one: the
/// version directory under the cache, `installed_plugins.json` naming it, and
/// `enabledPlugins` in `settings.json`.
fn install_plugin(home: &Path, version: &str, enabled: bool) -> PathBuf {
    let plugins = home.join(".claude").join("plugins");
    let dir = plugins
        .join("cache")
        .join("devmap-local")
        .join("devmap")
        .join(version);
    write_mcp(&dir.join(".mcp.json"), "devmap");
    std::fs::write(
        plugins.join("installed_plugins.json"),
        serde_json::json!({"version": 2, "plugins": {"devmap@devmap-local": [
            {"scope": "user", "installPath": dir, "version": version}
        ]}})
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        home.join(".claude").join("settings.json"),
        serde_json::json!({"enabledPlugins": {"devmap@devmap-local": enabled}}).to_string(),
    )
    .unwrap();
    dir
}

fn doctor_payload(home: &Path, cwd: &Path) -> Value {
    let out = run_in(cwd, &["--json", "doctor"], Some(home));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    one_json(&out, "doctor")
}

/// The genuine duplicate: one host — Claude Code — loading a user-scope
/// `~/.claude.json` entry and an enabled plugin's `.mcp.json`.
#[test]
fn claude_json_beside_an_enabled_plugin_is_a_duplicate() {
    let home = scratch("doc-home");
    let cwd = scratch("doc-cwd");
    write_mcp(&home.join(".claude.json"), "devmap");
    let plugin = install_plugin(&home, "0.1.1", true);
    let payload = doctor_payload(&home, &cwd);
    let warning = payload
        .get("duplicate_mcp_registration_warning")
        .and_then(Value::as_str)
        .unwrap_or("");
    assert!(
        warning.contains("claude-code") && warning.contains(".claude.json"),
        "one host loading two registrations must be reported: {payload}"
    );
    assert!(
        names(warning, &plugin.join(".mcp.json")),
        "must name the plugin's .mcp.json: {warning}"
    );
}

/// Cursor and Claude Code never share a process: one registration in each is
/// two hosts served once apiece, not a duplicate.
#[test]
fn cursor_beside_a_claude_plugin_is_not_a_duplicate() {
    let home = scratch("doc-cross-home");
    let cwd = scratch("doc-cross-cwd");
    write_mcp(&home.join(".cursor").join("mcp.json"), "devmap");
    install_plugin(&home, "0.1.1", true);
    let payload = doctor_payload(&home, &cwd);
    assert!(
        payload["duplicate_mcp_registration_warning"].is_null(),
        "registrations in different hosts must not warn: {payload}"
    );
    let global = payload["mcp_registrations"]["global"]
        .as_array()
        .expect("global registrations");
    let hosts: Vec<&str> = global.iter().filter_map(|r| r["host"].as_str()).collect();
    assert_eq!(
        hosts.len(),
        2,
        "both registrations are inventoried: {payload}"
    );
    assert!(
        hosts.contains(&"cursor") && hosts.contains(&"claude-code"),
        "{payload}"
    );
}

/// Only an enabled plugin's `.mcp.json` is loaded, so only it counts.
#[test]
fn a_disabled_plugin_is_not_counted_as_a_registration() {
    let home = scratch("doc-disabled-home");
    let cwd = scratch("doc-disabled-cwd");
    write_mcp(&home.join(".claude.json"), "devmap");
    install_plugin(&home, "0.1.1", false);
    let payload = doctor_payload(&home, &cwd);
    assert!(
        payload["duplicate_mcp_registration_warning"].is_null(),
        "a disabled plugin loads nothing: {payload}"
    );
    let not_loaded = payload["mcp_registrations"]["not_loaded"]
        .as_array()
        .expect("not_loaded registrations");
    assert_eq!(
        not_loaded.len(),
        1,
        "the disabled plugin is still inventoried: {payload}"
    );
}

/// A cache directory from an earlier install is loaded by nothing. It used to
/// count as a second Claude registration and as a version mismatch.
#[test]
fn a_leftover_cache_version_is_neither_a_registration_nor_a_warning() {
    let home = scratch("doc-leftover-home");
    let cwd = scratch("doc-leftover-cwd");
    let leftover = home
        .join(".claude")
        .join("plugins")
        .join("cache")
        .join("devmap-local")
        .join("devmap")
        .join("0.0.1");
    write_mcp(&leftover.join(".mcp.json"), "devmap");
    install_plugin(&home, "0.1.1", true);
    let payload = doctor_payload(&home, &cwd);
    assert!(
        payload["duplicate_mcp_registration_warning"].is_null(),
        "a leftover cache version is not a registration: {payload}"
    );
    let rows = serde_json::to_string(&payload["mcp_registrations"]).unwrap();
    assert!(
        !rows.contains("0.0.1"),
        "a leftover must not be inventoried as a registration: {rows}"
    );
}

#[test]
fn doctor_warns_when_home_devcouncil_exists_without_a_store() {
    let home = scratch("doc-empty-home");
    std::fs::create_dir_all(home.join(".devcouncil")).unwrap();
    let cwd = scratch("doc-empty-cwd");
    let out = run_in(&cwd, &["--json", "doctor"], Some(&home));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let payload = one_json(&out, "doctor");
    let warning = payload
        .get("stray_state_warning")
        .and_then(Value::as_str)
        .unwrap_or("");
    assert!(
        warning.contains(".devcouncil"),
        "must warn about ~/.devcouncil without a store: {payload}"
    );
}

#[test]
fn doctor_json_embeds_a_build_identifier() {
    let cwd = scratch("doc-build");
    let out = run_in(&cwd, &["--json", "doctor"], None);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let payload = one_json(&out, "doctor");
    let build = payload.get("build").expect("build object");
    assert!(
        build.get("git").and_then(Value::as_str).is_some()
            || build.get("id").and_then(Value::as_str).is_some(),
        "doctor --json must embed a build identifier: {payload}"
    );
}

#[test]
fn version_line_embeds_a_build_identifier() {
    let cwd = scratch("ver-build");
    let out = run_in(&cwd, &["--version"], None);
    assert!(out.status.success());
    let text = output_stdout(&out);
    assert!(
        text.contains("build "),
        "--version must embed a build identifier: {text:?}"
    );
}

fn output_stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn build_from_home_refuses_instead_of_indexing() {
    let home = scratch("build-home");
    std::fs::create_dir_all(home.join(".devcouncil")).unwrap();
    std::fs::write(home.join("notes.md"), "not a repository\n").unwrap();
    let out = run_in(&home, &["--json", "build"], Some(&home));
    assert!(
        !out.status.success(),
        "devmap build in $HOME must refuse: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        names(&combined, &home),
        "refusal must name the resolved root: {combined}"
    );
    let store = home
        .join(".devcouncil")
        .join("codeintel")
        .join("devmap.sqlite");
    assert!(
        !store.exists(),
        "must not create a store under $HOME: {}",
        store.display()
    );
}

#[test]
fn doctor_warns_when_a_host_config_names_a_missing_binary() {
    let home = scratch("doc-missing-bin");
    let cwd = scratch("doc-missing-cwd");
    let missing = home.join("no-such-devmap-binary");
    write_mcp(&home.join(".claude.json"), &missing.display().to_string());
    let out = run_in(&cwd, &["--json", "doctor"], Some(&home));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let payload = one_json(&out, "doctor");
    let warning = payload
        .get("missing_binary_warning")
        .and_then(Value::as_str)
        .unwrap_or("");
    assert!(
        !warning.is_empty(),
        "a host config pointing at a path that is not a file must not look like a clean doctor: {payload}"
    );
    assert!(
        names(warning, &missing) || warning.contains("no-such-devmap"),
        "warning must name the missing path: {warning}"
    );
    let skew = payload.get("binary_skew_warning");
    assert!(
        skew.is_some(),
        "binary_skew_warning remains a distinct field (null when versions match): {payload}"
    );
}

/// Given a fixture rather than the developer's own machine, for two reasons.
///
/// It used to run against the real `HOME` and assert that *some* listed binary
/// carried a hash — which held only because hashing was unbounded and would
/// therefore pay any price to finish. Now that the binary inventory has a
/// budget, "a hash is always present" is no longer a property of the diagnostic
/// but of how large the binaries on the machine are and how much CPU the
/// machine has to spare; on a loaded host an unoptimised 61 MiB binary does not
/// fit, and reporting that honestly is the fix, not a regression.
///
/// So the binary this test hashes is one it puts there itself, small enough
/// that it fits any budget, and the assertions are strengthened rather than
/// relaxed: *every* listed path must be absolute (it was *some*), and the bare
/// `devmap` named by the host config must resolve onto `PATH` and come back
/// with a real digest.
#[test]
fn doctor_resolves_bare_devmap_and_hashes_binaries() {
    let cwd = scratch("doc-hash");
    let home = scratch("doc-hash-home");
    let bin = home.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let discovered = bin.join("devmap");
    std::fs::write(&discovered, b"#!/bin/sh\nexit 0\n").unwrap();
    // A bare command name is the resolution this test is named for: doctor must
    // report where it lands, never echo back "devmap".
    write_mcp(&home.join(".cursor/mcp.json"), "devmap");

    let out = Command::new(devmap())
        .args(["--json", "doctor"])
        .current_dir(&cwd)
        .env("HOME", &home)
        .env("PATH", &bin)
        .env_remove("DEVMAP_HOME")
        .output()
        .expect("devmap --json doctor");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let payload = one_json(&out, "doctor");
    let binaries = payload["binaries"].as_array().expect("binaries");
    assert!(
        binaries.iter().all(|b| {
            b.get("path")
                .and_then(Value::as_str)
                .is_some_and(|p| Path::new(p).is_absolute())
        }),
        "every listed binary must be a resolved absolute path: {payload}"
    );
    let resolved = discovered.canonicalize().unwrap().display().to_string();
    let row = binaries
        .iter()
        .find(|b| b.get("path").and_then(Value::as_str) == Some(resolved.as_str()))
        .unwrap_or_else(|| panic!("bare `devmap` was not resolved onto PATH: {payload}"));
    assert_eq!(
        row.get("sha256").and_then(Value::as_str).map(str::len),
        Some(64),
        "a discovered binary that fits the budget must be hashed: {row}"
    );
    assert_eq!(row["sha256_status"], "hashed", "{row}");
}

/// Lay down a local-path marketplace the way `/plugin marketplace add <dir>`
/// records one: `known_marketplaces.json` naming `root` as a `directory`
/// source, and `root/.claude-plugin/marketplace.json` listing the plugin at
/// the relative source `./devmap`. Claude Code loads such a plugin in place
/// from `root/devmap`, not from the cache copy `installed_plugins.json`
/// names. Returns the in-place plugin directory, holding a well-formed
/// bundle whose `plugin.json` says `version`.
fn local_marketplace(home: &Path, root: &Path, version: &str) -> PathBuf {
    let plugins = home.join(".claude").join("plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    std::fs::write(
        plugins.join("known_marketplaces.json"),
        serde_json::json!({"devmap-local": {
            "source": {"source": "directory", "path": root},
            "installLocation": root,
        }})
        .to_string(),
    )
    .unwrap();
    std::fs::create_dir_all(root.join(".claude-plugin")).unwrap();
    std::fs::write(
        root.join(".claude-plugin").join("marketplace.json"),
        serde_json::json!({"name": "devmap-local", "plugins": [
            {"name": "devmap", "source": "./devmap", "version": version}
        ]})
        .to_string(),
    )
    .unwrap();
    let dir = root.join("devmap");
    write_healthy_plugin(&dir, version);
    dir
}

/// A plugin directory that passes every `plugin_health` check at `version`.
fn write_healthy_plugin(dir: &Path, version: &str) {
    write_mcp(&dir.join(".mcp.json"), "devmap");
    std::fs::create_dir_all(dir.join("hooks")).unwrap();
    std::fs::write(
        dir.join("hooks").join("hooks.json"),
        r#"{"hooks":{"SessionEnd":[{"hooks":[{"type":"command","command":"/abs/devmap hook session-end","timeout":3}]}]}}"#,
    )
    .unwrap();
    std::fs::create_dir_all(dir.join(".claude-plugin")).unwrap();
    std::fs::write(
        dir.join(".claude-plugin").join("plugin.json"),
        serde_json::json!({"name": "devmap", "version": version}).to_string(),
    )
    .unwrap();
}

fn global_rows(payload: &Value) -> Vec<String> {
    payload["mcp_registrations"]["global"]
        .as_array()
        .expect("global registrations")
        .iter()
        .filter_map(|row| row["path"].as_str().map(str::to_string))
        .collect()
}

/// A plugin from a local-path marketplace loads in place. The cache copy the
/// install record names is not what a session runs, so a malformed, older
/// cache copy must not raise a warning, and the registration doctor
/// inventories is the in-place `.mcp.json`.
#[test]
fn an_in_place_marketplace_plugin_is_the_copy_doctor_checks() {
    let home = scratch("inplace-home");
    let cwd = scratch("inplace-cwd");
    let market = scratch("inplace-market");
    // The record names a cache copy that is both stale and malformed (no
    // hooks/hooks.json).
    let cache = install_plugin(&home, "0.0.1", true);
    let loaded = local_marketplace(&home, &market, env!("CARGO_PKG_VERSION"));
    let payload = doctor_payload(&home, &cwd);
    assert!(
        payload["plugin_warning"].is_null(),
        "the in-place copy is healthy; the cache copy is not loaded: {payload}"
    );
    let rows = global_rows(&payload);
    assert!(
        rows.iter().any(|row| names(row, &loaded.join(".mcp.json"))),
        "the in-place .mcp.json is the loaded registration: {payload}"
    );
    assert!(
        !rows.iter().any(|row| names(row, &cache.join(".mcp.json"))),
        "the cache copy is not a registration: {payload}"
    );
    let note = payload["plugin_cleanup_note"].as_str().unwrap_or("");
    assert!(
        !(note.contains("safe to delete") && names(note, &cache)),
        "the copy the install record names is not a deletable leftover: {payload}"
    );
}

/// The other direction: the cache copy is current and well-formed, but the
/// in-place copy a session loads has drifted. Checking the cache said nothing.
#[test]
fn a_drifted_in_place_plugin_is_reported_even_when_the_cache_is_current() {
    let home = scratch("drift-home");
    let cwd = scratch("drift-cwd");
    let market = scratch("drift-market");
    let cache = install_plugin(&home, env!("CARGO_PKG_VERSION"), true);
    write_healthy_plugin(&cache, env!("CARGO_PKG_VERSION"));
    let loaded = local_marketplace(&home, &market, "0.0.1");
    let payload = doctor_payload(&home, &cwd);
    let warning = payload["plugin_warning"].as_str().unwrap_or("");
    assert!(
        warning.contains("0.0.1") && names(warning, &loaded),
        "the loaded in-place copy is at 0.0.1 and must be named: {payload}"
    );
}

/// An in-place plugin needs no install record; `enabledPlugins` alone decides
/// whether it loads. Without a record it must still count as a registration.
#[test]
fn an_in_place_plugin_without_an_install_record_still_counts() {
    let home = scratch("norecord-home");
    let cwd = scratch("norecord-cwd");
    let market = scratch("norecord-market");
    let loaded = local_marketplace(&home, &market, env!("CARGO_PKG_VERSION"));
    std::fs::write(
        home.join(".claude").join("settings.json"),
        serde_json::json!({"enabledPlugins": {"devmap@devmap-local": true}}).to_string(),
    )
    .unwrap();
    write_mcp(&home.join(".claude.json"), "devmap");
    let payload = doctor_payload(&home, &cwd);
    let warning = payload["duplicate_mcp_registration_warning"]
        .as_str()
        .unwrap_or("");
    assert!(
        warning.contains(".claude.json") && names(warning, &loaded.join(".mcp.json")),
        "the user-scope entry duplicates the in-place plugin: {payload}"
    );
}

/// A `file` marketplace source is not resolved to an in-place copy. That is an
/// unanswered question, and it must read as one rather than as a clean check
/// of the cache copy.
#[test]
fn an_unresolved_marketplace_source_is_reported_not_assumed() {
    let home = scratch("filesrc-home");
    let cwd = scratch("filesrc-cwd");
    install_plugin(&home, env!("CARGO_PKG_VERSION"), true);
    std::fs::write(
        home.join(".claude/plugins/known_marketplaces.json"),
        serde_json::json!({"devmap-local": {
            "source": {"source": "file", "path": "/m/.claude-plugin/marketplace.json"},
        }})
        .to_string(),
    )
    .unwrap();
    let payload = doctor_payload(&home, &cwd);
    let warning = payload["plugin_warning"].as_str().unwrap_or("");
    assert!(
        warning.contains("`file` source") && warning.contains("may not be the one"),
        "{payload}"
    );
}

fn integrate_claude(home: &Path, project: &Path, extra: &[&str]) -> Value {
    let project_arg = project.display().to_string();
    let mut args = vec![
        "--json",
        "integrate",
        "claude",
        "--project-root",
        &project_arg,
    ];
    args.extend_from_slice(extra);
    let out = run_in(project, &args, Some(home));
    assert!(
        out.status.success(),
        "integrate claude {extra:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    one_json(&out, "integrate claude")
}

fn claude_json_outcome(report: &Value, home: &Path) -> Value {
    report["global_mcp"]
        .as_array()
        .expect("global_mcp outcomes")
        .iter()
        .find(|row| {
            row["path"]
                .as_str()
                .is_some_and(|path| names(path, &home.join(".claude.json")))
        })
        .cloned()
        .unwrap_or_else(|| panic!("no ~/.claude.json outcome: {report}"))
}

/// `~/.claude.json` with an owned unpinned `devmap` entry beside another
/// server and an unrelated top-level key.
fn claude_json_with_devmap(home: &Path) -> PathBuf {
    let path = home.join(".claude.json");
    std::fs::write(
        &path,
        serde_json::json!({
            "numStartups": 7,
            "mcpServers": {
                "devmap": {"type": "stdio", "command": "devmap", "args": ["mcp"]},
                "other": {"type": "stdio", "command": "other", "args": []},
            },
        })
        .to_string(),
    )
    .unwrap();
    path
}

/// The duplicate `devmap doctor` reports came back after every manual removal:
/// `integrate claude` wrote the user-scope entry whether or not the enabled
/// plugin already registered the same server. It now removes our own entry
/// and leaves everything else in the file alone.
#[test]
fn integrate_claude_removes_the_user_scope_duplicate_of_an_enabled_plugin() {
    let home = scratch("int-dup-home");
    let project = scratch("int-dup-project");
    let plugin = install_plugin(&home, env!("CARGO_PKG_VERSION"), true);
    let path = claude_json_with_devmap(&home);
    let report = integrate_claude(&home, &project, &[]);
    let after: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(
        after["mcpServers"].get("devmap").is_none(),
        "the duplicate user-scope entry is removed: {after}"
    );
    assert_eq!(after["mcpServers"]["other"]["command"], "other", "{after}");
    assert_eq!(after["numStartups"], 7, "{after}");
    let outcome = claude_json_outcome(&report, &home);
    assert_eq!(outcome["changed"], true, "{report}");
    let note = outcome["note"].as_str().unwrap_or("");
    assert!(
        names(note, &plugin.join(".mcp.json")),
        "the note names the plugin registration that made it a duplicate: {report}"
    );
    // A second run has nothing left to do.
    let again = integrate_claude(&home, &project, &[]);
    assert_eq!(
        claude_json_outcome(&again, &home)["changed"],
        false,
        "{again}"
    );
}

#[test]
fn integrate_claude_dry_run_reports_the_removal_without_writing() {
    let home = scratch("int-dry-home");
    let project = scratch("int-dry-project");
    let plugin = install_plugin(&home, env!("CARGO_PKG_VERSION"), true);
    let path = claude_json_with_devmap(&home);
    let before = std::fs::read(&path).unwrap();
    let report = integrate_claude(&home, &project, &["--dry-run"]);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "dry-run writes nothing"
    );
    let outcome = claude_json_outcome(&report, &home);
    assert_eq!(outcome["changed"], true, "{report}");
    // A pending removal, not a pending rewrite of the entry.
    assert!(
        names(
            outcome["note"].as_str().unwrap_or(""),
            &plugin.join(".mcp.json")
        ),
        "{report}"
    );
}

/// No user-scope entry and an enabled plugin: nothing is written, and the
/// report says why rather than staying silent.
#[test]
fn integrate_claude_does_not_write_beside_an_enabled_plugin() {
    let home = scratch("int-skip-home");
    let project = scratch("int-skip-project");
    let plugin = install_plugin(&home, env!("CARGO_PKG_VERSION"), true);
    let report = integrate_claude(&home, &project, &[]);
    let path = home.join(".claude.json");
    if path.exists() {
        let after: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(after["mcpServers"].get("devmap").is_none(), "{after}");
    }
    let outcome = claude_json_outcome(&report, &home);
    assert_eq!(outcome["changed"], false, "{report}");
    assert!(
        names(
            outcome["note"].as_str().unwrap_or(""),
            &plugin.join(".mcp.json")
        ),
        "{report}"
    );
}

/// A disabled plugin loads nothing, so the user-scope entry is the only
/// registration and is still written.
#[test]
fn integrate_claude_registers_when_the_plugin_is_disabled() {
    let home = scratch("int-off-home");
    let project = scratch("int-off-project");
    install_plugin(&home, env!("CARGO_PKG_VERSION"), false);
    let report = integrate_claude(&home, &project, &[]);
    let after: Value =
        serde_json::from_str(&std::fs::read_to_string(home.join(".claude.json")).unwrap()).unwrap();
    assert!(after["mcpServers"].get("devmap").is_some(), "{after}");
    assert_eq!(
        claude_json_outcome(&report, &home)["changed"],
        true,
        "{report}"
    );
}

/// A pinned user-scope entry is a deliberate choice, not the unpinned
/// duplicate; it is reported and left in place.
#[test]
fn integrate_claude_leaves_a_pinned_user_scope_entry() {
    let home = scratch("int-pin-home");
    let project = scratch("int-pin-project");
    install_plugin(&home, env!("CARGO_PKG_VERSION"), true);
    let path = home.join(".claude.json");
    let pinned = serde_json::json!({"mcpServers": {"devmap": {
        "type": "stdio", "command": "devmap", "args": ["--root", "/some/repo", "mcp"]
    }}})
    .to_string();
    std::fs::write(&path, &pinned).unwrap();
    let report = integrate_claude(&home, &project, &[]);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), pinned);
    let outcome = claude_json_outcome(&report, &home);
    assert_eq!(outcome["changed"], false, "{report}");
    assert!(
        outcome["note"].as_str().unwrap_or("").contains("pinned"),
        "{report}"
    );
}
