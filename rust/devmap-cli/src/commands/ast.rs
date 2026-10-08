use crate::cli::Cli;
use crate::output::{emit_json, open_for_read};

#[derive(clap::Args)]
#[group(id = "Ast")]
pub(crate) struct Args {
    /// Case-insensitive substring of the name or qualified name.
    #[arg(default_value = "")]
    pub(crate) query: String,
    /// Only this symbol kind. `--facets` lists the ones this index holds.
    #[arg(long)]
    pub(crate) kind: Option<String>,
    /// Only this language.
    #[arg(long)]
    pub(crate) language: Option<String>,
    /// Rows to return. The total is reported whatever this is.
    #[arg(short, long, default_value_t = 100)]
    pub(crate) limit: usize,
    /// List the kinds and languages this generation holds, and stop.
    #[arg(long)]
    pub(crate) facets: bool,
}

pub(crate) fn run(cli: &Cli, args: &Args) -> anyhow::Result<()> {
    let Args {
        query,
        kind,
        language,
        limit,
        facets,
    } = args;
    let store = open_for_read(cli)?;
    if *facets {
        let facets = devmap_query::ast::ast_facets(&store)?;
        if cli.json {
            emit_json(cli, &facets)?;
        } else {
            outln!("kinds:");
            for (name, count) in facets["kinds"].as_object().into_iter().flatten() {
                outln!("  {name:<16} {count}");
            }
            outln!("languages:");
            for (name, count) in facets["languages"].as_object().into_iter().flatten() {
                outln!("  {name:<16} {count}");
            }
        }
        return Ok(());
    }
    let filter = devmap_query::ast::AstFilter {
        query: query.clone(),
        kind: kind.clone(),
        language: language.clone(),
        limit: *limit,
    };
    let answer = devmap_query::ast::ast_query(&store, &filter)?;
    if cli.json {
        emit_json(cli, &answer)?;
    } else {
        report_ast(&answer);
    }
    Ok(())
}

/// One line per symbol, then the counts — never a page length alone.
fn report_ast(answer: &serde_json::Value) {
    for hit in answer["matches"].as_array().into_iter().flatten() {
        outln!(
            "{:<10} {:<12} {}  {}",
            hit["kind"].as_str().unwrap_or(""),
            hit["language"].as_str().unwrap_or(""),
            hit["qualified_name"].as_str().unwrap_or(""),
            hit["path"].as_str().unwrap_or(""),
        );
    }
    let shown = answer["shown"].as_u64().unwrap_or(0);
    let total = answer["total"].as_u64().unwrap_or(0);
    if answer["truncated"].as_bool().unwrap_or(false) {
        outln!("{shown} of {total} match(es); raise --limit to see the rest");
    } else {
        outln!("{total} match(es)");
    }
    // An empty answer because the filter names something the index does not
    // hold is a different problem from an empty answer because nothing matched.
    for unmatched in answer["unmatched_filters"].as_array().into_iter().flatten() {
        outln!(
            "  --{} {:?}: {}",
            unmatched["filter"].as_str().unwrap_or(""),
            unmatched["value"].as_str().unwrap_or(""),
            unmatched["detail"].as_str().unwrap_or(""),
        );
    }
}
