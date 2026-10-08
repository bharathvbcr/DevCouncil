use devmap_query::{ResolutionAvailability, StoreQueryEngine};

use crate::cli::Cli;
use crate::output::{emit_json, emit_truncation, emit_unavailable, open_for_read};

#[derive(clap::Args)]
#[group(id = "Clones")]
pub(crate) struct Args {
    #[arg(short, long, default_value_t = 2000)]
    pub(crate) budget: u32,
    /// Report only one kind. Both are reported by default.
    #[arg(long, value_parser = ["exact", "structural"])]
    pub(crate) kind: Option<String>,
    /// Drop groups whose smallest body is under this many parse nodes.
    /// Raises the floor for this query only; it cannot lower it below the
    /// one signatures were computed with.
    #[arg(long, default_value_t = 0)]
    pub(crate) min_nodes: u32,
}

pub(crate) fn run(cli: &Cli, args: &Args) -> anyhow::Result<()> {
    let Args {
        budget,
        kind,
        min_nodes,
    } = args;
    let store = open_for_read(cli)?;
    // `value_parser` has already rejected anything but the two names,
    // so a `None` here can only be "no filter requested".
    let wanted = kind.as_deref().and_then(devmap_query::parse_clone_kind);
    let report = StoreQueryEngine::new(&store).clones(*budget, wanted, *min_nodes)?;
    if cli.json {
        emit_json(cli, &serde_json::to_value(&report)?)?;
    } else {
        emit_clones(&report);
    }
    Ok(())
}

fn emit_clones(report: &devmap_query::CloneReport) {
    if let ResolutionAvailability::Unavailable { reason } = &report.groups.resolution {
        emit_unavailable(reason);
        return;
    }
    for group in &report.groups.items {
        let kind = match group.kind {
            devmap_analyze::CloneKind::Exact => "exact",
            devmap_analyze::CloneKind::Structural => "structural",
        };
        outln!(
            "{kind}  {} members  {} nodes  #{:016x}",
            group.members.len(),
            group.min_nodes,
            group.signature
        );
        for member in &group.members {
            outln!(
                "    {}:{}  {}",
                member.file_path,
                member.span_start,
                member.symbol_name
            );
        }
        if group.members_omitted > 0 {
            outln!("    ... {} more members not listed", group.members_omitted);
        }
    }
    // Always printed, including when nothing was found: "no duplicates" and
    // "nothing was examined" are different answers and must not print the same.
    outln!(
        "coverage: {} symbols signed, {} unsigned",
        report.signed_symbols,
        report.unsigned_symbols
    );
    emit_truncation(
        report.groups.shown,
        report.groups.hidden,
        report.groups.total,
        report.groups.truncated,
    );
}
