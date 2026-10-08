use crate::cli::Cli;
use crate::output::{emit_json, open_for_read};

#[derive(clap::Args)]
#[group(id = "Cypher")]
pub(crate) struct Args {
    /// The query.
    pub(crate) query: String,
    /// Rows to return when the query states no `LIMIT`. A query's own
    /// `LIMIT` is a request; the server's ceiling still applies, and both
    /// numbers are reported.
    #[arg(short, long, default_value_t = 50)]
    pub(crate) limit: usize,
}

pub(crate) fn run(cli: &Cli, args: &Args) -> anyhow::Result<()> {
    let Args { query, limit } = args;
    let store = open_for_read(cli)?;
    let graph = devmap_query::graph_core_for_store(&store)?;
    let result = devmap_query::cypher::run(&graph, query, *limit);
    if cli.json {
        emit_json(cli, &result)?;
    } else if result["ok"].as_bool() == Some(true) {
        for row in result["rows"].as_array().into_iter().flatten() {
            match row.get("rel").and_then(serde_json::Value::as_str) {
                Some(rel) => outln!(
                    "{}  -[{rel}]->  {}",
                    row["a_id"].as_str().unwrap_or(""),
                    row["b_id"].as_str().unwrap_or("")
                ),
                None => outln!("{}", row["a_id"].as_str().unwrap_or("")),
            }
        }
        // Both numbers, always: a page reported as a count reads as a
        // total, and this surface exists to answer "how many".
        outln!(
            "  {} of {} row(s){}",
            result["shown"].as_u64().unwrap_or(0),
            result["total"].as_u64().unwrap_or(0),
            if result["limit_capped"].as_bool() == Some(true) {
                format!(
                    " (LIMIT {} capped to {})",
                    result["limit_requested"].as_u64().unwrap_or(0),
                    result["limit_applied"].as_u64().unwrap_or(0)
                )
            } else {
                String::new()
            }
        );
    } else {
        // A refusal is an error exit, not a zero-row success: a caller
        // that scripts this must be able to tell "your query was not
        // run" from "your query matched nothing".
        anyhow::bail!("{}", result["error"].as_str().unwrap_or("query refused"));
    }
    Ok(())
}
