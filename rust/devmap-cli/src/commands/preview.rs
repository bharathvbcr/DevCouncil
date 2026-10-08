use devmap_query::StoreQueryEngine;

use crate::cli::Cli;
use crate::output::{emit_json, emit_truncation, open_for_read};

#[derive(clap::Args)]
#[group(id = "Preview")]
pub(crate) struct Args {
    /// Path the buffer would be written to. Resolved against the index as
    /// given, so it must match the indexed path.
    #[arg(long)]
    pub(crate) file: String,
    /// Where the candidate content comes from: a path, or `-` for stdin.
    #[arg(long, default_value = "-")]
    pub(crate) content: String,
    #[arg(short, long, default_value_t = 2000)]
    pub(crate) budget: u32,
    /// Confidence a call edge needs to be listed as affected. The default
    /// excludes the resolver's name-only tier, whose edges are counted
    /// separately rather than shown.
    #[arg(long, default_value_t = devmap_query::PREVIEW_CALLER_MIN_CONFIDENCE)]
    pub(crate) min_confidence: f32,
}

pub(crate) fn run(cli: &Cli, args: &Args) -> anyhow::Result<()> {
    let Args {
        file,
        content,
        budget,
        min_confidence,
    } = args;
    let source = if content == "-" {
        use std::io::Read;
        let mut buffer = Vec::new();
        std::io::stdin()
            .lock()
            .take(devmap_extract::MAX_SOURCE_BYTES + 1)
            .read_to_end(&mut buffer)?;
        if buffer.len() as u64 > devmap_extract::MAX_SOURCE_BYTES {
            anyhow::bail!("preview source exceeds the 1 MiB source ceiling");
        }
        String::from_utf8(buffer)?
    } else {
        devmap_extract::read_source(std::path::Path::new(content))
            .map_err(|e| anyhow::anyhow!("cannot read {content}: {e}"))?
    };
    let store = open_for_read(cli)?;
    let report = StoreQueryEngine::new(&store).preview(file, &source, *budget, *min_confidence)?;
    if cli.json {
        emit_json(cli, &serde_json::to_value(&report)?)?;
    } else {
        emit_preview(&report);
    }
    Ok(())
}

fn emit_preview(report: &devmap_query::PreviewReport) {
    outln!("{}  parse={}", report.file_path, report.parse_status);
    if report.compared_against == "nothing" {
        outln!("note: no file at this path; every symbol reads as added");
    }
    if !report.file_is_indexed {
        outln!("note: this file is not in the index; no caller graph is available for it");
    }
    if let Some(reason) = &report.degraded_reason {
        outln!("note: {reason}");
    }
    if !report.delta_available {
        // The reason above already says why. Printing an empty symbol list
        // underneath it would read as "no changes".
        return;
    }
    for symbol in &report.symbols {
        let change = match symbol.change {
            devmap_query::PreviewChange::Added => "added",
            devmap_query::PreviewChange::Removed => "removed",
            devmap_query::PreviewChange::SignatureChanged => "signature",
            devmap_query::PreviewChange::BodyChanged => "body",
            devmap_query::PreviewChange::Changed => "changed",
        };
        outln!("{change:<11} {} ({})", symbol.qualified_name, symbol.kind);
    }
    if report.symbols.is_empty() {
        outln!("no symbol-level change");
    }
    if report.bodies_not_compared > 0 {
        outln!(
            "{} symbol(s) declared identically but not body-compared \
             (below the signature size floor, or no grammar)",
            report.bodies_not_compared
        );
    }
    for caller in &report.broken_callers.items {
        // `caller_symbol` and `target_symbol` are already `path::Name`, so the
        // file is not printed again beside them.
        outln!(
            "  affects  {}  ->  {}  ({:.2})",
            caller.caller_symbol,
            caller.target_symbol,
            caller.confidence
        );
    }
    if report.broken_callers.total == 0 {
        outln!("no calls from other files are affected");
    }
    if report.ambiguous_callers > 0 {
        outln!(
            "{} further call edge(s) fell below the confidence floor and are not \
             listed (usually a bare method name matching many definitions); \
             pass --min-confidence 0 to see them",
            report.ambiguous_callers
        );
    }
    emit_truncation(
        report.broken_callers.shown,
        report.broken_callers.hidden,
        report.broken_callers.total,
        report.broken_callers.truncated,
    );
}
