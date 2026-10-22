use std::path::{Path, PathBuf};

use devmap_query::CODE_GRAPH_SCHEMA_VERSION;
use devmap_store::Store;

use crate::agents;
use crate::cli::Cli;
use crate::installation::{
    binaries_skew_warning, build_identity_json, duplicate_mcp_registration_warning,
    inventory_devmap_binaries, mcp_registration_inventory, missing_binary_warning,
    plugin_cleanup_note, plugin_warning, profile_hooks_warning, stale_server_warning,
    stray_state_warning,
};
use crate::output::emit_json;

pub(crate) fn run(cli: &Cli) -> anyhow::Result<()> {
    // Never creates a store. The probe is what a host runs *before* it
    // trusts this binary against a tree it may not own; creating one
    // here would turn "is this binary usable?" into a write.
    let payload = doctor_report(&cli.db(), &cli.root_hint())?;
    emit_json(cli, &payload)?;
    Ok(())
}

/// The structured answer `devmap doctor` emits.
///
/// One owner for the fields a host needs to verify a binary against a store
/// without scraping `--version` prose: the schema on disk (if any), the schema
/// this binary speaks, the code-graph artifact schema, how many grammars are
/// linked into this build, the resolved store path, and every `devmap` found on
/// `PATH` plus common host MCP configs (with version), so binary skew is a
/// structured fact rather than a silent wrong hook.
fn doctor_report(
    db: &std::path::Path,
    root: &std::path::Path,
) -> anyhow::Result<serde_json::Value> {
    let schema_version = Store::stored_schema_version(db)?;
    let state_dir = devmap_extract::paths::state_dir(root);
    // Read-only: this handler already refuses to create a store, on the
    // grounds that a usability probe must not write into a tree it may not
    // own. Leaving a memo behind would be the same write by another name.
    let mut digests = crate::digest_cache::BinaryDigests::open_read_only(Some(&state_dir));
    let binaries = inventory_devmap_binaries(&mut digests)?;
    let skew = binaries_skew_warning(&binaries);
    let mismatches = doctor_edge_confidence_mismatches(db, schema_version);
    let mut report = serde_json::json!({
        // `null` when nothing was measured; the warning then says why.
        "edge_confidence_mismatches": mismatches.as_ref().ok(),
        "edge_confidence_warning": edge_confidence_warning(&mismatches),
        "schema_version": schema_version,
        "expected_schema_version": devmap_store::CURRENT_SCHEMA_VERSION,
        "code_graph_schema_version": CODE_GRAPH_SCHEMA_VERSION,
        "linked_grammar_count": devmap_extract::linked_grammar_count(),
        "store_path": db.display().to_string(),
        "version": env!("CARGO_PKG_VERSION"),
        "build": build_identity_json(),
        "binaries": binaries,
        "binary_skew_warning": skew,
        "missing_binary_warning": missing_binary_warning(&binaries),
        "duplicate_mcp_registration_warning": duplicate_mcp_registration_warning(),
        "stray_state_warning": stray_state_warning(),
        "skills_receipt_warning": skills_receipt_warning(root),
        "plugin_warning": plugin_warning(),
        "plugin_cleanup_note": plugin_cleanup_note(),
        "profile_hooks_warning": profile_hooks_warning(&std::path::absolute(root).unwrap_or_else(|_| root.to_path_buf())),
        "stale_server_warning": stale_server_warning(root),
        "mcp_registrations": mcp_registration_inventory(),
        "git": git_report(root),
    });
    extend_agent_tools(&mut report, root);
    Ok(report)
}

/// Whether the skill files `devmap skills install` wrote under `root` still
/// match the five skills this binary embeds (`skills install --check`, read
/// only). `None` when the repository has no `.devcouncil-skills.json` receipt,
/// since nothing was installed to disagree; only layouts whose directory exists
/// are compared, so a repository that installed one host's layout is not told
/// the others are missing. A check that could not run is a warning, never
/// silence: the receipt is per-workspace state and is not version-controlled.
fn skills_receipt_warning(root: &Path) -> Option<String> {
    if !root.join(".devcouncil-skills.json").is_file() {
        return None;
    }
    let destinations: Vec<&str> = crate::skills::DEFAULT_DESTINATIONS
        .iter()
        .copied()
        .filter(|dest| root.join(dest).is_dir())
        .collect();
    match crate::skills::install_devmap_skills(root, &destinations, false, true) {
        Ok(report) if report.check_ok => None,
        Ok(report) => Some(format!(
            "{} installed skill file(s) are missing or differ from the skills this devmap \
             embeds; run `devmap skills install` to refresh them",
            report.differing.len()
        )),
        Err(error) => Some(format!(
            "the skills receipt could not be checked against the installed skills: {error}"
        )),
    }
}

/// The git the kernels would run against `root`: where it was found, its
/// version, and whether it honours `GIT_NO_LAZY_FETCH`. A git that does not
/// honour it still reads ordinary clones, and refuses to read a partial
/// clone (ft-ff2153d9, D9), so the version is what explains such a refusal.
/// Asking `git version` runs nothing the repository names.
fn git_report(root: &Path) -> serde_json::Value {
    use devmap_extract::subprocess::{git_version, resolve_outside};
    let path_var = std::env::var_os("PATH");
    let unavailable = |path: Option<&Path>, error: String| {
        serde_json::json!({
            "path": path.map(|p| p.display().to_string()),
            "version": null,
            "raw_version": null,
            "honors_no_lazy_fetch": null,
            "warning": null,
            "error": error,
        })
    };
    let git = match resolve_outside(std::ffi::OsStr::new("git"), path_var.as_deref(), root) {
        Ok(git) => git,
        Err(failure) => return unavailable(None, failure.to_string()),
    };
    match git_version(&git) {
        Ok(Some(version)) => {
            let honours = version.honors_no_lazy_fetch();
            let warning = (!honours).then(|| {
                format!(
                    "git {version} does not honour GIT_NO_LAZY_FETCH: reads of a partial clone \
                     are refused rather than risk a lazy fetch. Upgrade to 2.45.0 or later, or to \
                     a maintenance release with the CVE-2024-32465 fix (2.39.4, 2.40.2, 2.41.1, \
                     2.42.2, 2.43.4, 2.44.1)"
                )
            });
            serde_json::json!({
                "path": git.display().to_string(),
                "version": version.to_string(),
                "raw_version": version.raw,
                "honors_no_lazy_fetch": honours,
                "warning": warning,
                "error": null,
            })
        }
        Ok(None) => unavailable(
            Some(&git),
            "git ran but reported no version this can read; reads of a partial clone are refused"
                .to_string(),
        ),
        Err(failure) => unavailable(Some(&git), failure.to_string()),
    }
}

/// Add `agent_tools_warning` and `agent_tool_gaps`: the Claude Code agent
/// definitions a session rooted at `root` loads whose tool grant leaves DevMap
/// out, so subagents of that type cannot call `devmap_*`. See [`agents`].
pub(crate) fn extend_agent_tools(value: &mut serde_json::Value, root: &Path) {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    // A relative root is invocation-relative; the warning names files by
    // absolute path so it points at them wherever it is read.
    let root = std::path::absolute(root).unwrap_or_else(|_| root.to_path_buf());
    let scan = agents::scan(&agents::agent_dirs(home.as_deref(), &root));
    let gaps: Vec<serde_json::Value> = scan
        .gaps
        .iter()
        .map(|gap| serde_json::json!({"path": gap.path, "reason": gap.reason}))
        .collect();
    if let Some(fields) = value.as_object_mut() {
        fields.insert(
            "agent_tools_warning".into(),
            serde_json::json!(agents::warning(&scan)),
        );
        fields.insert("agent_tool_gaps".into(), serde_json::json!(gaps));
    }
}

/// The edges check `doctor` lost when the Go `dcmap doctor` was retired:
/// stored edges whose confidence contradicts their recorded resolution kind.
///
/// Read through the same SQL owner `status` uses, on a read-only connection,
/// and only when the stored schema is the one this binary speaks — `Store::open`
/// migrates, and a probe must not rewrite a store it was only asked to judge.
/// `Err` carries why nothing was measured — no store, a schema this binary does
/// not read, no generation, or a read that failed — so the report can say so
/// instead of failing the whole diagnosis over one check.
fn doctor_edge_confidence_mismatches(
    db: &std::path::Path,
    schema_version: Option<i32>,
) -> Result<usize, String> {
    match schema_version {
        None => return Err("no devmap store at this path".to_string()),
        Some(version) if version != devmap_store::CURRENT_SCHEMA_VERSION => {
            return Err(format!(
                "store schema is {version}, this binary reads {}",
                devmap_store::CURRENT_SCHEMA_VERSION
            ))
        }
        Some(_) => {}
    }
    let store = Store::open_read_only(db).map_err(|error| format!("store unreadable: {error}"))?;
    store
        .edge_confidence_mismatches()
        .map_err(|error| format!("edge read failed: {error}"))?
        .ok_or_else(|| "the store holds no generation".to_string())
}

/// Zero is the only passing reading. A count above zero is a store whose
/// edges no longer agree with their own evidence, and an unmeasured count is
/// reported as unknown rather than allowed to pass as a clean one.
fn edge_confidence_warning(mismatches: &Result<usize, String>) -> Option<String> {
    match mismatches {
        Ok(0) => None,
        Ok(count) => Some(format!(
            "{count} stored edge(s) carry a confidence that contradicts their recorded \
             resolution kind; rebuild with `devmap build --full`"
        )),
        Err(reason) => Some(format!("edge confidence is unknown, not passed: {reason}")),
    }
}
