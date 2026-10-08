use std::path::PathBuf;

use crate::cli::{default_root_hint, Cli};
use crate::commands::routes::{repo_root_for, report_scan, scan_budget};
use crate::output::{emit_json, open_for_read};

#[derive(clap::Args)]
#[group(id = "ShapeCheck")]
pub(crate) struct Args {
    #[arg(default_value_os_t = default_root_hint())]
    pub(crate) path: PathBuf,
    /// Only routes matching this path or id.
    #[arg(long)]
    pub(crate) filter: Option<String>,
    #[arg(long, default_value_t = 5_000)]
    pub(crate) max_files: usize,
    #[arg(long, default_value_t = 1 << 20)]
    pub(crate) max_file_bytes: u64,
}

pub(crate) fn run(cli: &Cli, args: &Args) -> anyhow::Result<()> {
    let Args {
        path,
        filter,
        max_files,
        max_file_bytes,
    } = args;
    let store = open_for_read(cli)?;
    let graph = devmap_query::graph_core_for_store(&store)?;
    let budget = scan_budget(*max_files, *max_file_bytes);
    let root = repo_root_for(&store, path)?;
    let checked = devmap_query::api_routes::shape_check(&root, &graph, &budget, filter.as_deref());
    if cli.json {
        emit_json(cli, &checked)?;
    } else {
        report_shape_check(&checked);
    }
    Ok(())
}

fn report_shape_check(checked: &serde_json::Value) {
    let checks = checked["checks"].as_array().cloned().unwrap_or_default();
    for check in &checks {
        let verdict = check["verdict"].as_str().unwrap_or("");
        outln!(
            "{:<7} {}  {}",
            check["verb"].as_str().unwrap_or("ANY"),
            check["route"].as_str().unwrap_or(""),
            verdict
        );
        if verdict == "mismatch" {
            let missing: Vec<&str> = check["missing_in_handler"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|k| k.as_str())
                .collect();
            outln!(
                "        consumers read, handler never returns: {}",
                missing.join(", ")
            );
        }
    }
    outln!(
        "{} of {} route(s) mismatch",
        checked["mismatch_count"].as_u64().unwrap_or(0),
        checks.len()
    );
    report_scan(checked);
}
