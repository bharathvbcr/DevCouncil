use devmap_query::{ResolutionAvailability, StoreQueryEngine};

use crate::cli::Cli;
use crate::output::{emit_json, emit_truncation, emit_unavailable, open_for_read};

#[derive(clap::Args)]
#[group(id = "Skeleton")]
pub(crate) struct Args {
    /// Repository-relative path, or an absolute path under the indexed root.
    pub(crate) file: String,
    #[arg(short, long, default_value_t = 2000)]
    pub(crate) budget: u32,
}

pub(crate) fn run(cli: &Cli, args: &Args) -> anyhow::Result<()> {
    let Args { file, budget } = args;
    let store = open_for_read(cli)?;
    let payload = StoreQueryEngine::new(&store).skeleton(file, *budget)?;
    if cli.json {
        emit_json(cli, &serde_json::to_value(&payload)?)?;
    } else {
        emit_skeleton(&payload);
    }
    Ok(())
}

/// File signatures without bodies.
fn emit_skeleton(report: &devmap_query::SkeletonReport) {
    use devmap_query::SkeletonPresence;
    if let ResolutionAvailability::Unavailable { reason } = &report.resolution {
        emit_unavailable(reason);
        return;
    }
    match report.presence {
        SkeletonPresence::NotInIndex => {
            outln!("{}: not in the index", report.file);
            return;
        }
        SkeletonPresence::Empty => {
            outln!("{}: indexed, no definitions", report.file);
            return;
        }
        SkeletonPresence::Indexed => {
            outln!("{}: {} definition(s)", report.file, report.total);
        }
    }
    for item in &report.items {
        let sig = item
            .signature
            .as_deref()
            .or(item.signature_note.as_deref())
            .unwrap_or("not extracted");
        // Collapse newlines in the signature so a multi-line declaration does
        // not break the one-row-per-symbol layout the budget counts against.
        let sig_one_line: String = sig
            .chars()
            .map(|ch| if ch == '\n' || ch == '\r' { ' ' } else { ch })
            .collect();
        outln!(
            "  L{}-{}  {}  ({})  {}",
            item.start_line,
            item.end_line,
            item.qualified_name,
            item.kind,
            sig_one_line
        );
    }
    emit_truncation(
        report.shown,
        report.total.saturating_sub(report.shown),
        report.total,
        report.truncated,
    );
}
