use std::path::{Path, PathBuf};

use crate::cli::{default_root_hint, Cli};
use crate::commands::export::graph_value_for_read;
use crate::output::{emit_json, open_for_read};

#[derive(clap::Args)]
#[group(id = "Html")]
pub(crate) struct Args {
    #[arg(default_value_os_t = default_root_hint())]
    pub(crate) path: PathBuf,
    /// Where to write. Defaults to `<state dir>/graph.html`.
    #[arg(short, long)]
    pub(crate) out: Option<PathBuf>,
    /// What a node is: files and their imports, or symbols and their calls.
    ///
    /// The subsystem view is `devmap map-html`, which reads `repo_map.json`
    /// and colours by language.
    #[arg(long, value_enum, default_value_t = HtmlLevel::Files)]
    pub(crate) level: HtmlLevel,
    /// Most nodes to draw, ranked by degree so the hubs survive.
    ///
    /// A force layout stops converging in a browser tab well before a real
    /// repository's node count, so this is a cap rather than a preference.
    /// Whatever it cuts is stated in the payload *and* in the page header:
    /// "1,000 of 12,103 nodes", never "1,000 nodes".
    #[arg(long, default_value_t = 1_500)]
    pub(crate) max_nodes: usize,
}

pub(crate) fn run(cli: &Cli, args: &Args) -> anyhow::Result<()> {
    let Args {
        path,
        out,
        level,
        max_nodes,
    } = args;
    let store = open_for_read(cli)?;
    let gen_id = store.latest_generation_id()?.unwrap_or(0);
    let repo_root = store.latest_repo_root()?;

    let title = repo_root
        .as_deref()
        .and_then(|root| Path::new(root).file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Dev Map".to_string());

    let graph = graph_value_for_read(&store, &cli.db())?;
    let options = devmap_query::viz::VizOptions {
        symbols: *level == HtmlLevel::Symbols,
        max_nodes: *max_nodes,
        title,
    };
    let payload = devmap_query::viz::build_payload(&graph, &options);
    let html = devmap_query::viz::render_html(&graph, &options);

    let destination = out
        .clone()
        .unwrap_or_else(|| devmap_extract::paths::state_dir(path).join("graph.html"));
    devmap_query::write_atomic(&destination, html.as_bytes())?;

    let counts = &payload["counts"];
    if cli.json {
        emit_json(
            cli,
            &serde_json::json!({
                "output": destination,
                "generation_id": gen_id,
                "level": payload["level"],
                // Both numbers travel with the answer, as they do in the
                // page: a capped view reported as a node count is a
                // capped view nobody knows is capped.
                "counts": counts,
                "bytes": html.len(),
            }),
        )?;
    } else {
        outln!("Wrote {}", destination.display());
        let shown = counts["nodes_shown"].as_u64().unwrap_or(0);
        let total = counts["nodes_total"].as_u64().unwrap_or(0);
        if counts["nodes_truncated"].as_bool().unwrap_or(false) {
            outln!(
                "  {shown} of {total} nodes drawn (most connected first); \
raise --max-nodes to widen"
            );
        } else {
            outln!("  {total} nodes drawn");
        }
    }
    Ok(())
}

/// Which graph the HTML view draws.
#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum HtmlLevel {
    /// Files and their imports.
    Files,
    /// Symbols and their calls, inheritance and named imports.
    Symbols,
}
