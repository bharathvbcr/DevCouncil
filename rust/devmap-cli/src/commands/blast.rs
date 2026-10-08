use clap::ValueEnum;

use crate::cli::Cli;
use crate::output::{emit_json, open_for_read};

#[derive(clap::Args)]
#[group(id = "Blast")]
pub(crate) struct Args {
    /// The revision the change is measured from. The change is everything
    /// between it and the indexed head.
    #[arg(long, conflicts_with = "at")]
    pub(crate) since: Option<String>,
    /// An explicit location instead of a diff: `path:start-end`, or
    /// `path:line` for one line. Read at the indexed head.
    ///
    /// For "what depends on the line I am looking at", which has no
    /// revision range to speak of.
    #[arg(long, conflicts_with = "since")]
    pub(crate) at: Option<String>,
    /// How far to walk inbound call edges from the changed symbols.
    #[arg(long, default_value_t = dc_regress::DEFAULT_BLAST_DEPTH)]
    pub(crate) depth: u32,
    /// How to render the report. `markdown` is for review notes; analysis
    /// still runs in `dc-regress`. `--json` wins when both are set.
    #[arg(long, value_enum, default_value_t = BlastFormat::Text)]
    pub(crate) format: BlastFormat,
}

pub(crate) fn run(cli: &Cli, args: &Args) -> anyhow::Result<()> {
    let Args {
        since,
        at,
        depth,
        format,
    } = args;
    let store = open_for_read(cli)?;
    let root =
        std::path::absolute(cli.root_hint()).unwrap_or_else(|_| cli.root_hint().to_path_buf());
    let graph = dc_regress_store::StoreGraph::new(&store, &root)?;
    // Pinned to the indexed head for the same reason `suspects` is:
    // the graph's byte offsets describe that revision's content and
    // no other, so any other post-image makes every file's basis check
    // refuse — a report of nothing but refusals rather than a wrong
    // answer, but still not an answer.
    let until = graph.indexed_head().map(str::to_string).ok_or_else(|| {
        anyhow::anyhow!(
            "this store recorded no head commit, so there is no revision whose \
             content its spans are known to describe — run `devmap build` in a \
             git repository first"
        )
    })?;
    let report = match (since.as_deref(), at.as_deref()) {
        (Some(since), _) => dc_regress::blast(&root, &graph, since, &until, *depth),
        (None, Some(at)) => {
            let (path, start, end) =
                dc_regress::change::parse_location(at).map_err(|error| anyhow::anyhow!(error))?;
            dc_regress::blast::blast_change_with_program(
                std::ffi::OsStr::new("git"),
                &root,
                &graph,
                &dc_regress::ChangeSet::at(&path, start, end),
                at,
                &until,
                *depth,
            )
        }
        // Refused in `validate` before the store was opened.
        (None, None) => unreachable!("blast requires --since or --at"),
    };
    if cli.json {
        emit_json(cli, &serde_json::to_value(&report)?)?;
    } else if *format == BlastFormat::Markdown {
        emit_blast_markdown(&report);
    } else {
        emit_blast(&report);
    }
    Ok(())
}

/// How `devmap blast` renders when `--json` is not set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub(crate) enum BlastFormat {
    #[default]
    Text,
    Markdown,
}

/// Render a blast report.
///
/// The refusals and the unattributed lines print *first*, for the same reason
/// they do in `emit_suspects`: a reader's question is "is this the whole
/// story", and an impact list computed from half the change looks exactly like
/// one computed from all of it.
fn emit_blast(report: &dc_regress::BlastReport) {
    if !report.unavailable.is_empty() {
        outln!("could not examine everything:");
        for reason in &report.unavailable {
            outln!("  - {}", reason.describe());
        }
        outln!("");
    }

    if report.changed_files.is_empty() {
        // The distinction this whole command is built on, applied to its own
        // summary line. An empty change set after a refusal is not a finding
        // that nothing changed — it is the absence of a finding, and printing
        // "changed no files" under a banner explaining that the diff could not
        // be read invites a reader to take the first sentence and leave.
        if report.complete {
            outln!("{} changed no files", report.change);
        } else {
            outln!(
                "{}: what changed could not be determined — see above. This is not a \
                 finding that nothing changed.",
                report.change
            );
        }
        return;
    }

    outln!(
        "{}: {} file(s) changed, {} symbol(s) touched",
        report.change,
        report.changed_files.len(),
        report.seeds.len()
    );
    for seed in report.seeds.iter().take(10) {
        outln!(
            "  changed  {} ({}-{}, {} line(s){})",
            seed.qualified_name,
            seed.start_line,
            seed.end_line,
            seed.changed_lines,
            if seed.deletion_only {
                ", deletions only"
            } else {
                ""
            }
        );
    }
    if report.seeds.len() > 10 {
        outln!("  … {} more changed symbol(s)", report.seeds.len() - 10);
    }

    // Printed even when empty is *not* the rule here — an empty list with
    // nothing in `unavailable` means the change really did land entirely in
    // symbols, which is worth not saying. A non-empty one is always shown in
    // full up to its own cap, because each entry is a place the walk could not
    // start.
    if !report.unattributed.is_empty() {
        outln!("");
        outln!(
            "{} range(s) landed in no symbol — the impact below cannot account for them:",
            report.unattributed.len()
        );
        for entry in report.unattributed.iter().take(10) {
            if entry.start_line == 0 {
                outln!("  {}: {}", entry.path, entry.reason.describe());
            } else {
                outln!(
                    "  {}:{}-{}: {}",
                    entry.path,
                    entry.start_line,
                    entry.end_line,
                    entry.reason.describe()
                );
            }
        }
        if report.unattributed.len() > 10 {
            outln!("  … {} more", report.unattributed.len() - 10);
        }
    }

    outln!("");
    if report.impacted.is_empty() {
        if report.complete {
            outln!("nothing outside the changed symbols depends on them");
        } else {
            outln!(
                "no dependents found — but the analysis was incomplete, so this is not \
                 evidence that none exist"
            );
        }
    } else {
        outln!("{} symbol(s) affected:", report.impacted.len());
        for symbol in report.impacted.iter().take(15) {
            outln!("  d{} {}", symbol.distance, symbol.qualified_name);
        }
        if report.impacted.len() > 15 {
            outln!("  … {} more", report.impacted.len() - 15);
        }
    }

    if !report.modules.is_empty() {
        outln!("");
        outln!("{} module(s) affected:", report.modules.len());
        for module in report.modules.iter().take(15) {
            outln!(
                "  d{} {} ({} file(s), {} symbol(s){}, tests: {})",
                module.nearest_distance,
                module.path,
                module.files,
                module.symbols,
                if module.changed { ", changed" } else { "" },
                test_signal_label(module.test_signal)
            );
        }
        if report.modules.len() > 15 {
            outln!("  … {} more", report.modules.len() - 15);
        }
    }

    if !report.tests.is_empty() {
        outln!("");
        outln!("{} test file(s) to run:", report.tests.len());
        for test in report.tests.iter().take(20) {
            outln!("  d{} {}", test.distance, test.path);
        }
        if report.tests.len() > 20 {
            outln!("  … {} more", report.tests.len() - 20);
        }
    }

    if !report.owners.is_empty() {
        outln!("");
        outln!("{} owner(s):", report.owners.len());
        for owner in report.owners.iter().take(15) {
            outln!("  {} <{}>", owner.name, owner.email);
        }
        if report.owners.len() > 15 {
            outln!("  … {} more", report.owners.len() - 15);
        }
    }
}

/// Markdown rendering of a blast report for review notes.
///
/// Analysis stays in `dc-regress`; this is only presentation. Refusals and
/// unattributed ranges print first for the same reason as [`emit_blast`].
fn emit_blast_markdown(report: &dc_regress::BlastReport) {
    outln!("# Blast: {}", report.change);
    outln!("");
    outln!(
        "Complete: **{}**",
        if report.complete { "yes" } else { "no" }
    );

    if !report.unavailable.is_empty() {
        outln!("");
        outln!("## Could not examine everything");
        for reason in &report.unavailable {
            outln!("- {}", reason.describe());
        }
    }

    if report.changed_files.is_empty() {
        outln!("");
        if report.complete {
            outln!("Changed no files.");
        } else {
            outln!(
                "What changed could not be determined — see above. This is not a finding \
                 that nothing changed."
            );
        }
        return;
    }

    outln!("");
    outln!("## Change");
    outln!(
        "{} file(s) changed, {} symbol(s) touched.",
        report.changed_files.len(),
        report.seeds.len()
    );
    for seed in report.seeds.iter().take(20) {
        outln!(
            "- `{}` ({}–{}, {} line(s){})",
            seed.qualified_name,
            seed.start_line,
            seed.end_line,
            seed.changed_lines,
            if seed.deletion_only {
                ", deletions only"
            } else {
                ""
            }
        );
    }
    if report.seeds.len() > 20 {
        outln!("- … {} more", report.seeds.len() - 20);
    }

    if !report.unattributed.is_empty() {
        outln!("");
        outln!("## Unattributed");
        outln!(
            "{} range(s) landed in no symbol — the impact below cannot account for them:",
            report.unattributed.len()
        );
        for entry in report.unattributed.iter().take(20) {
            if entry.start_line == 0 {
                outln!("- `{}`: {}", entry.path, entry.reason.describe());
            } else {
                outln!(
                    "- `{}:{}-{}`: {}",
                    entry.path,
                    entry.start_line,
                    entry.end_line,
                    entry.reason.describe()
                );
            }
        }
        if report.unattributed.len() > 20 {
            outln!("- … {} more", report.unattributed.len() - 20);
        }
    }

    outln!("");
    outln!("## Impacted");
    if report.impacted.is_empty() {
        if report.complete {
            outln!("Nothing outside the changed symbols depends on them.");
        } else {
            outln!(
                "No dependents found — but the analysis was incomplete, so this is not \
                 evidence that none exist."
            );
        }
    } else {
        for symbol in report.impacted.iter().take(30) {
            outln!("- d{} `{}`", symbol.distance, symbol.qualified_name);
        }
        if report.impacted.len() > 30 {
            outln!("- … {} more", report.impacted.len() - 30);
        }
    }

    if !report.modules.is_empty() {
        outln!("");
        outln!("## Modules");
        for module in report.modules.iter().take(30) {
            outln!(
                "- d{} `{}` ({} file(s), {} symbol(s){}, tests: {})",
                module.nearest_distance,
                module.path,
                module.files,
                module.symbols,
                if module.changed { ", changed" } else { "" },
                test_signal_label(module.test_signal)
            );
        }
        if report.modules.len() > 30 {
            outln!("- … {} more", report.modules.len() - 30);
        }
    }

    if !report.tests.is_empty() {
        outln!("");
        outln!("## Tests");
        for test in report.tests.iter().take(30) {
            outln!("- d{} `{}`", test.distance, test.path);
        }
        if report.tests.len() > 30 {
            outln!("- … {} more", report.tests.len() - 30);
        }
    }

    outln!("");
    outln!("## Owners");
    if report.owners.is_empty() {
        if report
            .unavailable
            .iter()
            .any(|u| matches!(u, dc_regress::Unavailable::OwnersUnavailable { .. }))
        {
            outln!("Owners could not be read — see the refusals above.");
        } else {
            outln!("No owners recorded for the changed paths.");
        }
    } else {
        for owner in report.owners.iter().take(30) {
            outln!("- {} `<{}>`", owner.name, owner.email);
        }
        if report.owners.len() > 30 {
            outln!("- … {} more", report.owners.len() - 30);
        }
    }
}

fn test_signal_label(signal: dc_regress::TestSignal) -> &'static str {
    match signal {
        dc_regress::TestSignal::Changed => "changed",
        dc_regress::TestSignal::Stale => "stale",
        dc_regress::TestSignal::None => "none",
        dc_regress::TestSignal::Na => "n/a",
        dc_regress::TestSignal::Unavailable => "unavailable",
    }
}
