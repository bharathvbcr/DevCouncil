use std::path::PathBuf;

use devmap_store::Store;

use crate::cli::{default_root_hint, Cli};
use crate::output::{emit_json, open_for_read};

#[derive(clap::Args)]
#[group(id = "Routes")]
pub(crate) struct Args {
    #[arg(default_value_os_t = default_root_hint())]
    pub(crate) path: PathBuf,
    /// Only routes matching this path or id.
    #[arg(long)]
    pub(crate) filter: Option<String>,
    /// Files the client scan may open before it stops.
    #[arg(long, default_value_t = 5_000)]
    pub(crate) max_files: usize,
    /// Largest file the client scan will read, in bytes.
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
    let mut mapped = devmap_query::api_routes::route_map(&root, &graph, &budget);
    if let Some(filter) = filter {
        devmap_query::api_routes::retain_matching_routes(&mut mapped, filter);
    }
    if cli.json {
        emit_json(cli, &mapped)?;
    } else {
        report_routes(&mapped);
    }
    Ok(())
}

/// The client scan's bounds, from the flags.
pub(crate) fn scan_budget(
    max_files: usize,
    max_file_bytes: u64,
) -> devmap_query::api_routes::ScanBudget {
    devmap_query::api_routes::ScanBudget {
        max_files,
        max_file_bytes,
        ..Default::default()
    }
}

/// The tree the scan reads, which is the one the store was built from.
///
/// The path argument names a repository; the store records the root it indexed.
/// Those disagree when `--db` points elsewhere, and the file paths in the graph
/// are relative to the *store's* root — resolving them against the argument
/// would read a different tree, or nothing.
pub(crate) fn repo_root_for(store: &Store, path: &std::path::Path) -> anyhow::Result<PathBuf> {
    Ok(store
        .latest_repo_root()?
        .map(PathBuf::from)
        .unwrap_or_else(|| path.to_path_buf()))
}

/// One line per route, then the scan's own limits.
fn report_routes(mapped: &serde_json::Value) {
    let routes = mapped["routes"].as_array().cloned().unwrap_or_default();
    if routes.is_empty() {
        outln!("No routes in this generation.");
    }
    for route in &routes {
        let handlers: Vec<String> = route["handlers"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|h| match h["resolution"].as_str() {
                Some("ambiguous") => format!(
                    "{} (ambiguous: {} candidates)",
                    h["id"].as_str().unwrap_or("?"),
                    h["candidates"].as_array().map(Vec::len).unwrap_or(0)
                ),
                Some("unresolved") => format!("{} (unresolved)", h["id"].as_str().unwrap_or("?")),
                _ => h["id"].as_str().unwrap_or("?").to_string(),
            })
            .collect();
        outln!(
            "{:<7} {}  -> {}",
            route["verb"].as_str().unwrap_or("ANY"),
            route["path"].as_str().unwrap_or(""),
            if handlers.is_empty() {
                "(no handler)".to_string()
            } else {
                handlers.join(", ")
            }
        );
        let consumers = route["consumers"].as_array().map(Vec::len).unwrap_or(0);
        if consumers > 0 {
            outln!("        {consumers} client call site(s)");
        }
    }
    report_scan(mapped);
}

/// What the scan read, and what it did not. Printed whenever it did not finish,
/// because every count above it is then a lower bound.
pub(crate) fn report_scan(payload: &serde_json::Value) {
    let scan = &payload["scan"];
    match scan["complete"].as_bool() {
        Some(true) => return,
        Some(false) => {}
        // Absent is not complete. The library reads this same field as
        // `unwrap_or(false)`; defaulting the *human* output to "complete" is
        // how an answer that never reported its coverage comes to read as one
        // that covered everything.
        None => {
            outln!(
                "  scan coverage not reported; the counts above are not a \
completeness claim."
            );
            return;
        }
    }
    outln!(
        "  scan incomplete: read {} of {} file(s); {} skipped for budget, \
{} over size, {} unreadable. Counts above are lower bounds.",
        scan["files_read"].as_u64().unwrap_or(0),
        scan["files_eligible"].as_u64().unwrap_or(0),
        scan["files_skipped_budget"].as_u64().unwrap_or(0),
        scan["files_over_size"].as_u64().unwrap_or(0),
        scan["files_unreadable"].as_u64().unwrap_or(0),
    );
}
