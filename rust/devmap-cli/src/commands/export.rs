use std::path::PathBuf;

use devmap_query::{resolved_edge_from_stored, FreshnessInfo, StampedFreshness};
use devmap_store::Store;

use crate::cli::{default_root_hint, Cli};
use crate::output::{emit_json, open_for_read};
use crate::write_stdout;

#[derive(clap::Args)]
#[group(id = "Export")]
pub(crate) struct Args {
    #[arg(default_value_os_t = default_root_hint())]
    pub(crate) path: PathBuf,
    /// Where to write. Defaults to `<state dir>/graph.graphml`; `-` is stdout.
    #[arg(short, long)]
    pub(crate) out: Option<PathBuf>,
}

pub(crate) fn run(cli: &Cli, args: &Args) -> anyhow::Result<()> {
    let Args { path, out } = args;
    if cli.json && out.as_deref() == Some(std::path::Path::new("-")) {
        anyhow::bail!("--json cannot be combined with --out -: choose a JSON receipt with an output file, or raw GraphML on stdout");
    }
    let store = open_for_read(cli)?;
    let graph = graph_value_for_read(&store, &cli.db())?;
    let gen_id = store.latest_generation_id()?.unwrap_or(0);
    let (xml, report) = devmap_query::export::export_graphml(&graph);

    let to_stdout = out.as_deref() == Some(std::path::Path::new("-"));
    let destination = out
        .clone()
        .filter(|_| !to_stdout)
        .unwrap_or_else(|| devmap_extract::paths::state_dir(path).join("graph.graphml"));
    if to_stdout {
        write_stdout(format_args!("{xml}"));
    } else {
        devmap_query::write_atomic(&destination, xml.as_bytes())?;
    }

    if cli.json {
        emit_json(
            cli,
            &serde_json::json!({
                "output": if to_stdout { serde_json::Value::Null }
                          else { serde_json::json!(destination) },
                "generation_id": gen_id,
                "nodes": report.nodes,
                "edges": report.edges,
                // GraphML cannot express an edge to an undeclared node,
                // and a symbol name can hold bytes XML forbids. Both are
                // repairs, and a repair nobody is told about is a
                // difference between the graph and its export.
                "edges_dangling": report.edges_dangling,
                "characters_replaced": report.characters_replaced,
                "bytes": xml.len(),
            }),
        )?;
    } else if !to_stdout {
        outln!("Wrote {}", destination.display());
        outln!("  {} nodes, {} edges", report.nodes, report.edges);
        if report.edges_dangling > 0 {
            outln!(
                "  {} edge(s) omitted: an endpoint is not a declared node \
(GraphML cannot express one)",
                report.edges_dangling
            );
        }
        if report.characters_replaced > 0 {
            outln!(
                "  {} character(s) replaced with U+FFFD: XML 1.0 cannot \
represent them",
                report.characters_replaced
            );
        }
    }
    Ok(())
}

/// The graph model, read from the store, for a surface that only wants to look.
///
/// One owner for `html` and `cypher`: both project the same value the artifact
/// writer builds, so a picture and a query cannot describe different
/// generations — and neither pays to parse a 20 MB `code_graph.json` back off
/// disk to answer.
///
/// The freshness stamps are the store's own and are deliberately not computed
/// here. These surfaces describe the generation, not the working tree, and
/// digesting the tree would make rendering a picture cost a full rehash.
pub(crate) fn graph_value_for_read(
    store: &Store,
    db: &std::path::Path,
) -> anyhow::Result<serde_json::Value> {
    let gen_id = store
        .latest_generation_id()?
        .ok_or_else(|| anyhow::anyhow!("no committed generation: run `devmap build` first"))?;
    let extractions = store.latest_extractions()?;
    let analysis = store
        .latest_analysis()?
        .ok_or_else(|| anyhow::anyhow!("no committed generation: run `devmap build` first"))?;
    let edges = store
        .latest_edges(0.0)?
        .into_iter()
        .map(resolved_edge_from_stored)
        .collect::<anyhow::Result<Vec<_>>>()?;
    let freshness = FreshnessInfo {
        head_sha: store
            .latest_generation_head()?
            .unwrap_or_else(|| "unavailable".to_string()),
        generation_id: gen_id,
        pending_count: store.status(&db.display().to_string())?.pending_count,
        stamped: StampedFreshness::default(),
    };
    let repo_root = store.latest_repo_root()?;
    devmap_query::build_code_graph_value(
        &extractions,
        &analysis,
        &edges,
        &freshness,
        repo_root.as_deref(),
    )
}
