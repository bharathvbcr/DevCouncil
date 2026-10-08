use std::path::PathBuf;

use anyhow::Context;

use crate::cli::{default_root_hint, Cli};
use crate::commands::doctor::extend_agent_tools;
use crate::installation::{
    binaries_skew_warning, build_identity_json, duplicate_mcp_registration_warning,
    inventory_devmap_binaries, missing_binary_warning, plugin_cleanup_note, plugin_warning,
    stale_server_warning, stray_state_warning,
};
use crate::output::emit_json;

#[derive(clap::Args)]
#[group(id = "Paths")]
pub(crate) struct Args {
    #[arg(default_value_os_t = default_root_hint())]
    pub(crate) path: PathBuf,
}

pub(crate) fn run(cli: &Cli, args: &Args) -> anyhow::Result<()> {
    let Args { .. } = args;
    // Absolute, so a caller in another directory can use every field
    // as given; `validate_root` has already checked the directory exists.
    //
    // Resolved through `root_hint`, not the positional `path`, because
    // `--root` outranks it and `cli.db()` already honours that. Reading
    // the positional argument here made one answer describe two
    // repositories: `devmap --root A paths` reported A's `db_path`
    // beside the *working directory's* `root`, `state_dir` and
    // `repo_map`. Hooks are the case that breaks on — they pass
    // `--root <project>` precisely because the agent's working
    // directory is not the repository the index belongs to — and
    // `paths` is the first command the generated agent guide tells an
    // agent to run, so the mixed answer pointed it at another
    // checkout's map.
    let root = cli.root_hint();
    let root = root
        .canonicalize()
        .with_context(|| format!("repository root {}", root.display()))?;
    let state_dir = devmap_extract::paths::state_dir(&root);
    // Both explicit --db and relative roots are invocation-relative.
    // Joining this to root again duplicates the repository directory.
    let db_path = std::path::absolute(cli.db())?;
    let mut digests = crate::digest_cache::BinaryDigests::open(Some(&state_dir));
    let binaries = inventory_devmap_binaries(&mut digests)?;
    let skew = binaries_skew_warning(&binaries);
    let mut payload = serde_json::json!({
        "root": root,
        "state_dir": state_dir,
        "state_dir_exists": state_dir.is_dir(),
        "db_path": db_path,
        "store_exists": db_path.is_file(),
        "repo_map": devmap_extract::paths::repo_map_path(&root),
        "code_graph": devmap_extract::paths::code_graph_path(&root),
        "workspace": devmap_extract::paths::workspace_path(&root),
        "plugin_dir": devmap_extract::paths::plugin_dir(&root),
        "binaries": binaries,
        "binary_skew_warning": skew,
        "missing_binary_warning": missing_binary_warning(&binaries),
        "duplicate_mcp_registration_warning": duplicate_mcp_registration_warning(),
        "stray_state_warning": stray_state_warning(),
        "plugin_warning": plugin_warning(),
        "plugin_cleanup_note": plugin_cleanup_note(),
        "stale_server_warning": stale_server_warning(),
        "version": env!("CARGO_PKG_VERSION"),
        "build": build_identity_json(),
    });
    extend_agent_tools(&mut payload, &root);
    if cli.json {
        emit_json(cli, &payload)?;
    } else {
        for key in [
            "root",
            "state_dir",
            "db_path",
            "repo_map",
            "code_graph",
            "workspace",
            "plugin_dir",
        ] {
            outln!("{key:<12} {}", payload[key].as_str().unwrap_or(""));
        }
        if let Some(warning) = skew {
            outln!("warning: {warning}");
        }
        for row in payload["binaries"].as_array().into_iter().flatten() {
            outln!(
                "binary      {} ({}) version={}",
                row["path"].as_str().unwrap_or(""),
                row["source"].as_str().unwrap_or(""),
                row["version"].as_str().unwrap_or("unknown")
            );
        }
    }
    Ok(())
}
