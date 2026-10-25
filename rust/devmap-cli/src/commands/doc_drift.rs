use devmap_query::{DocDriftReport, StoreQueryEngine};

use crate::cli::Cli;
use crate::output::{emit_json, emit_truncation, emit_unavailable, open_for_read};

#[derive(clap::Args)]
#[group(id = "DocDrift")]
pub(crate) struct Args {
    /// Repo-relative Markdown files or directory prefixes to scan. Default:
    /// every indexed Markdown file.
    pub(crate) paths: Vec<String>,
    /// Repo-relative prefix to leave out. May be repeated.
    #[arg(long = "exclude", value_name = "PREFIX")]
    pub(crate) exclude: Vec<String>,
    /// Scan `docs/archive` too. By default it is left out unless a PATH names
    /// something inside it.
    #[arg(long)]
    pub(crate) no_default_excludes: bool,
    #[arg(short, long, default_value_t = 2000)]
    pub(crate) budget: u32,
}

pub(crate) fn run(cli: &Cli, args: &Args) -> anyhow::Result<()> {
    let Args {
        paths,
        exclude,
        no_default_excludes,
        budget,
    } = args;
    let store = open_for_read(cli)?;
    let report =
        StoreQueryEngine::new(&store).doc_drift(paths, exclude, *budget, !no_default_excludes)?;
    if cli.json {
        emit_json(cli, &serde_json::to_value(&report)?)?;
    } else {
        emit_doc_drift(&report);
    }
    Ok(())
}

fn emit_doc_drift(report: &DocDriftReport) {
    if !report.available {
        emit_unavailable(report.walk_incomplete.as_deref().unwrap_or("no generation"));
        return;
    }
    for finding in &report.findings {
        outln!(
            "{}:{} {} {} — {}",
            finding.doc,
            finding.line,
            finding.kind.as_str(),
            finding.reference,
            finding.detail
        );
        if !finding.current_locations.is_empty() {
            outln!("    now in: {}", finding.current_locations.join(", "));
        }
    }
    emit_truncation(report.shown, report.hidden, report.total, report.truncated);
    if let Some(reason) = &report.walk_incomplete {
        outln!("warning: {reason}");
    }
    let counts = |map: &std::collections::BTreeMap<String, u32>| {
        if map.is_empty() {
            "none".to_string()
        } else {
            map.iter()
                .map(|(name, count)| format!("{name} {count}"))
                .collect::<Vec<_>>()
                .join(", ")
        }
    };
    if !report.default_excludes.is_empty() {
        outln!(
            "note: {} document(s) under {} were left out by default; name a path inside it or pass --no-default-excludes to scan them",
            report.docs_excluded_by_default,
            report.default_excludes.join(", ")
        );
    }
    outln!(
        "doc-drift: {} of {} docs scanned, {} references; checked: {}; skipped: {}",
        report.docs_scanned,
        report.docs_total,
        report.references,
        counts(&report.checked),
        counts(&report.skipped)
    );
}
