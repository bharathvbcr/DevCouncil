use devmap_query::StoreQueryEngine;

use crate::cli::Cli;
use crate::output::{emit_json, open_for_read};

#[derive(clap::Args)]
#[group(id = "Savings")]
pub(crate) struct Args {
    /// Optional search to account for, in addition to the corpus figures.
    #[arg(long)]
    pub(crate) query: Option<String>,
    #[arg(short, long, default_value_t = 2000)]
    pub(crate) budget: u32,
}

pub(crate) fn run(cli: &Cli, args: &Args) -> anyhow::Result<()> {
    let Args { query, budget } = args;
    let store = open_for_read(cli)?;
    let report = StoreQueryEngine::new(&store).savings(query.as_deref(), *budget)?;
    if cli.json {
        emit_json(cli, &serde_json::to_value(&report)?)?;
    } else {
        emit_savings(&report);
    }
    Ok(())
}

fn emit_savings(report: &devmap_query::SavingsReport) {
    let tokens = |bytes: u64| bytes / u64::from(devmap_query::BYTES_PER_TOKEN);
    outln!("basis: {}", report.basis);
    outln!(
        "corpus:   {} files, {} bytes  (~{} tokens to read in full)",
        report.indexed_files,
        report.corpus_bytes,
        tokens(report.corpus_bytes)
    );
    if report.corpus_files_unreadable > 0 {
        // Named, not folded into the total as zero: an unread file makes the
        // corpus look smaller, which makes the map look better.
        outln!(
            "          {} indexed file(s) could not be read and are excluded from that total",
            report.corpus_files_unreadable
        );
    }
    match report.repo_map_bytes {
        Some(bytes) => outln!("repo_map: {} bytes  (~{} tokens)", bytes, tokens(bytes)),
        None => outln!("repo_map: not written yet"),
    }
    let Some(query) = &report.query else {
        outln!("(pass --query to account for one search)");
        return;
    };
    outln!(
        "query {:?}: {} hit(s) across {} file(s)",
        query.query,
        query.hits,
        query.files_named
    );
    outln!("  map answer cost:        {} tokens", query.answer_tokens);
    outln!(
        "  reading those files:    ~{} tokens ({} bytes)",
        tokens(query.files_bytes),
        query.files_bytes
    );
    if query.files_unreadable > 0 {
        outln!(
            "  {} named file(s) could not be read and are excluded",
            query.files_unreadable
        );
    }
    let alternative = tokens(query.files_bytes);
    if alternative > u64::from(query.answer_tokens) {
        outln!(
            "  floor on the saving:    ~{} tokens, and only for a reader who already \
             knew which files to open",
            alternative - u64::from(query.answer_tokens)
        );
    } else {
        // Said plainly rather than suppressed. On a tiny corpus, or a query
        // whose hits live in small files, the map is not cheaper — and a
        // savings report that can only ever report a saving is advertising.
        outln!("  no saving on this query: the files are smaller than the answer");
    }
}
