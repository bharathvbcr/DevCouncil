use crate::cli::Cli;
use crate::output::{emit_json, open_for_read};

#[derive(clap::Args)]
#[group(id = "Suspects")]
pub(crate) struct Args {
    /// A symbol name, a `file::symbol` node id, or anything `search` can
    /// resolve to one.
    pub(crate) symptom: String,
    /// The last known-good revision. Commits after it are the candidates.
    #[arg(long)]
    pub(crate) since: String,
    /// How far to walk **outbound** call edges from the symptom — its
    /// transitive dependencies, which are what could have caused it.
    ///
    /// Outbound, not inbound. The symptom's callers sit downstream of the
    /// failure and cannot have caused it; walking towards them returns the
    /// symptom alone and reads as "no commit touched anything relevant".
    /// For the inbound direction — what a change *affects* — see `blast`.
    #[arg(long, default_value_t = dc_regress::DEFAULT_CONE_DEPTH)]
    pub(crate) depth: u32,
}

pub(crate) fn run(cli: &Cli, args: &Args) -> anyhow::Result<()> {
    let Args {
        symptom,
        since,
        depth,
    } = args;
    let store = open_for_read(cli)?;
    let root =
        std::path::absolute(cli.root_hint()).unwrap_or_else(|_| cli.root_hint().to_path_buf());
    let graph = dc_regress_store::StoreGraph::new(&store, &root)?;
    // The analysis runs against the commit the index was built at, not
    // against HEAD. Anything else compares spans taken from one
    // revision with lines from another — which the join refuses, so
    // the alternative is not a wrong answer but a report of nothing
    // but refusals.
    let until = graph.indexed_head().map(str::to_string).ok_or_else(|| {
        anyhow::anyhow!(
            "this store recorded no head commit, so there is no revision whose \
             content its spans are known to describe — run `devmap build` in a \
             git repository first"
        )
    })?;
    let report = dc_regress::suspects(&root, &graph, symptom, since, &until, *depth);
    if cli.json {
        emit_json(cli, &serde_json::to_value(&report)?)?;
    } else {
        emit_suspects(&report);
    }
    Ok(())
}

/// Render a suspect report.
///
/// The refusals print *before* the suspects and are never omitted, because the
/// question a reader brings to this output is "is this the whole story". A list
/// that was cut short and a list that is complete look identical otherwise, and
/// the one thing this report must not do is let the first read as the second.
pub(crate) fn emit_suspects(report: &dc_regress::SuspectReport) {
    if !report.unavailable.is_empty() {
        outln!("could not examine everything:");
        for reason in &report.unavailable {
            outln!("  - {}", reason.describe());
        }
        outln!("");
    }
    if report.suspects.is_empty() {
        if report.complete {
            outln!(
                "no commit in the window touched any of the {} symbol(s) that reach {}",
                report.cone_size,
                report.symptom
            );
        } else {
            outln!(
                "no suspects found — but the analysis was incomplete, so this is not \
                 evidence that none exist"
            );
        }
        return;
    }
    outln!(
        "{} suspect(s) over {} cone symbol(s), {} blamed:",
        report.suspects.len(),
        report.cone_size,
        report.blamed_symbols
    );
    for suspect in &report.suspects {
        let short: String = suspect.commit.chars().take(12).collect();
        outln!(
            "  {short}  {:?}  score {}  nearest {}  {}",
            suspect.evidence,
            suspect.score,
            suspect.nearest_distance,
            suspect.author
        );
        for touch in suspect.touched.iter().take(5) {
            outln!(
                "      d{} {} ({} line(s))",
                touch.distance,
                touch.qualified_name,
                touch.lines
            );
        }
        if suspect.touched.len() > 5 {
            outln!("      … {} more", suspect.touched.len() - 5);
        }
    }
}
