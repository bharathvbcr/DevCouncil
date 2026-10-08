use std::path::{Path, PathBuf};

use devmap_query::CODE_GRAPH_SCHEMA_VERSION;
use devmap_store::Store;

use crate::agents;
use crate::cli::Cli;
use crate::installation::{
    binaries_skew_warning, build_identity_json, duplicate_mcp_registration_warning,
    inventory_devmap_binaries, mcp_registration_inventory, missing_binary_warning,
    plugin_cleanup_note, plugin_warning, stale_server_warning, stray_state_warning,
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
        "plugin_warning": plugin_warning(),
        "plugin_cleanup_note": plugin_cleanup_note(),
        "stale_server_warning": stale_server_warning(),
        "mcp_registrations": mcp_registration_inventory(),
    });
    extend_agent_tools(&mut report, root);
    Ok(report)
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
