use std::path::PathBuf;

use crate::cli::{default_root_hint, Cli};
use crate::commands::routes::{repo_root_for, report_scan, scan_budget};
use crate::output::{emit_json, open_for_read};

#[derive(clap::Args)]
#[group(id = "ApiImpact")]
pub(crate) struct Args {
    /// The route path or `"VERB /path"` id.
    pub(crate) route: String,
    #[arg(default_value_os_t = default_root_hint())]
    pub(crate) path: PathBuf,
    #[arg(long, default_value_t = 5_000)]
    pub(crate) max_files: usize,
    #[arg(long, default_value_t = 1 << 20)]
    pub(crate) max_file_bytes: u64,
}

pub(crate) fn run(cli: &Cli, args: &Args) -> anyhow::Result<()> {
    let Args {
        route,
        path,
        max_files,
        max_file_bytes,
    } = args;
    let store = open_for_read(cli)?;
    let graph = devmap_query::graph_core_for_store(&store)?;
    let budget = scan_budget(*max_files, *max_file_bytes);
    let root = repo_root_for(&store, path)?;
    let impact = devmap_query::api_routes::api_impact(&root, &graph, &budget, route);
    if cli.json {
        emit_json(cli, &impact)?;
    } else {
        report_api_impact(&impact);
    }
    Ok(())
}

fn report_api_impact(impact: &serde_json::Value) {
    if impact["found"] != serde_json::json!(true) {
        outln!(
            "No route matched {:?}.",
            impact["route"].as_str().unwrap_or("")
        );
        report_scan(impact);
        return;
    }
    outln!(
        "{} {}",
        impact["verb"].as_str().unwrap_or("ANY"),
        impact["route"].as_str().unwrap_or("")
    );
    outln!(
        "  risk: {} — {}",
        impact["risk"].as_str().unwrap_or("unknown"),
        impact["risk_reason"].as_str().unwrap_or("")
    );
    for consumer in impact["consumers"].as_array().into_iter().flatten() {
        outln!(
            "  called from {}:{}",
            consumer["path"].as_str().unwrap_or(""),
            consumer["line"].as_u64().unwrap_or(0)
        );
    }
    for mismatch in impact["shape_mismatches"].as_array().into_iter().flatten() {
        let missing: Vec<&str> = mismatch["missing_in_handler"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|k| k.as_str())
            .collect();
        outln!("  shape: consumers read {}", missing.join(", "));
    }
    report_scan(impact);
}
