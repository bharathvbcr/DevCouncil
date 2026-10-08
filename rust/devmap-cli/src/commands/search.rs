use devmap_query::{Request, StoreQueryEngine};

use crate::cli::Cli;
use crate::output::{emit_json, emit_search, open_for_read};

#[derive(clap::Args)]
#[group(id = "Search")]
pub(crate) struct Args {
    pub(crate) query: String,
    #[arg(short, long, default_value_t = 2000)]
    pub(crate) budget: u32,
    /// Rank by TF-IDF similarity of symbol names instead of FTS5 prefix
    /// matching. Finds symbols whose names are *about* the query rather
    /// than ones that contain it.
    #[arg(long)]
    pub(crate) semantic: bool,
    /// Rank only files under this repository-relative path prefix
    /// (repeatable). Applied before the page is cut, for keyword and
    /// `--semantic` search. A prefix matching no indexed file is refused.
    #[arg(long = "path", value_name = "PREFIX")]
    pub(crate) paths: Vec<String>,
    /// Rank only files in this language, as the index labels it
    /// (repeatable; `typescript` and `tsx` are distinct).
    #[arg(long = "language", value_name = "LANGUAGE")]
    pub(crate) languages: Vec<String>,
    /// Keep only symbols of this kind (repeatable). `function` and
    /// `Function` are the same kind. An unknown kind is refused.
    #[arg(long = "kind", value_name = "KIND")]
    pub(crate) kinds: Vec<String>,
}

pub(crate) fn run(cli: &Cli, args: &Args) -> anyhow::Result<()> {
    let Args {
        query,
        budget,
        semantic,
        paths,
        languages,
        kinds,
    } = args;
    let filter = devmap_query::NameQueryFilter::new(paths, languages, kinds)?;
    let store = open_for_read(cli)?;
    let engine = StoreQueryEngine::new(&store);
    let resp = if *semantic {
        let scope = filter
            .as_ref()
            .map(|filter| filter.symbol_scope())
            .transpose()?
            .flatten();
        let kinds = filter.as_ref().map(|filter| filter.kinds()).unwrap_or(&[]);
        engine.search_semantic_filtered(query, *budget, scope.as_ref(), kinds)?
    } else {
        engine.search_filtered(
            Request {
                query: query.clone(),
                token_budget: *budget,
                min_confidence: 0.0,
                max_depth: 1,
            },
            filter.as_ref(),
        )?
    };
    if cli.json {
        emit_json(cli, &serde_json::to_value(&resp)?)?;
    } else {
        emit_search(&resp);
    }
    Ok(())
}
