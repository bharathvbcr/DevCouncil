use devmap_query::{ResolutionAvailability, StoreQueryEngine};

use crate::cli::Cli;
use crate::output::{
    emit_json, emit_scope, emit_search, emit_truncation, emit_unavailable, open_for_read,
};

#[derive(clap::Args)]
#[group(id = "Ask")]
pub(crate) struct Args {
    pub(crate) question: String,
    #[arg(short, long, default_value_t = 2000)]
    pub(crate) budget: u32,
    /// Minimum call-edge confidence. Defaults to the deterministic rung
    /// (1.0); edges below it are excluded. Lower this to include high or
    /// speculative edges in the PageRank walk.
    #[arg(long, default_value_t = devmap_query::ASK_DEFAULT_MIN_CONFIDENCE)]
    pub(crate) min_confidence: f32,
    /// Answer as an evidence pack: files in rank order with their role
    /// (implementation or test), the call edges between hits, each hit's
    /// verbatim source with line numbers, then the test files that reach
    /// the hits. A quarter of the budget is held for the test list.
    #[arg(long)]
    pub(crate) evidence: bool,
    /// Answer only from files under this repository-relative path prefix
    /// (repeatable). Applied before ranking, so term weights and the call
    /// graph are the scope's own. A prefix matching no indexed file is
    /// refused.
    #[arg(long = "path", value_name = "PREFIX")]
    pub(crate) paths: Vec<String>,
    /// Answer only from files in this language, as the index labels it
    /// (repeatable; `typescript` and `tsx` are distinct).
    #[arg(long = "language", value_name = "LANGUAGE")]
    pub(crate) languages: Vec<String>,
}

pub(crate) fn run(cli: &Cli, args: &Args) -> anyhow::Result<()> {
    match args {
        Args {
            question,
            budget,
            min_confidence,
            evidence: true,
            paths,
            languages,
        } => {
            let scope = devmap_query::SymbolScope::new(paths, languages)?;
            let store = open_for_read(cli)?;
            let engine = StoreQueryEngine::new(&store);
            let pack =
                engine.ask_evidence_scoped(question, *budget, *min_confidence, scope.as_ref())?;
            if cli.json {
                emit_json(cli, &serde_json::to_value(&pack)?)?;
            } else {
                emit_evidence_pack(&pack);
            }
        }
        Args {
            question,
            budget,
            min_confidence,
            evidence: false,
            paths,
            languages,
        } => {
            let scope = devmap_query::SymbolScope::new(paths, languages)?;
            let store = open_for_read(cli)?;
            let engine = StoreQueryEngine::new(&store);
            let resp = engine.ask_scoped(question, *budget, *min_confidence, scope.as_ref())?;
            if cli.json {
                emit_json(cli, &serde_json::to_value(&resp)?)?;
            } else {
                emit_search(&resp);
            }
        }
    }
    Ok(())
}

/// Files first, then source: the order a reader uses an answer in.
///
/// The file list is short enough to decide what to read; the source blocks
/// below it mean the obvious reads are already done. A folded unit points at
/// the block that shows it instead of printing its lines twice.
fn emit_evidence_pack(pack: &devmap_query::EvidencePack) {
    if let ResolutionAvailability::Unavailable { reason } = &pack.resolution {
        emit_unavailable(reason);
        return;
    }
    emit_scope(pack.scope.as_ref());
    let symbols: usize = pack.files.iter().map(|file| file.units.len()).sum();
    // Qualified names lead with the file path, which the list already shows.
    let short = |file: &str, name: &'_ str| -> String {
        name.strip_prefix(file)
            .and_then(|rest| rest.strip_prefix("::"))
            .unwrap_or(name)
            .to_string()
    };
    let shorten = |file: &str, names: &[String]| -> String {
        names
            .iter()
            .map(|name| short(file, name))
            .collect::<Vec<_>>()
            .join(", ")
    };
    outln!(
        "Evidence: {} file{}, {} symbol{}.",
        pack.files.len(),
        if pack.files.len() == 1 { "" } else { "s" },
        symbols,
        if symbols == 1 { "" } else { "s" }
    );
    for file in &pack.files {
        outln!("- {}  ({})", file.file_path, file.role.as_str());
        for unit in &file.units {
            let mut line = format!(
                "    {}-{}  {}  {}",
                unit.hit.span.0,
                unit.hit.span.1,
                unit.hit.kind,
                short(&file.file_path, &unit.qualified_name)
            );
            if !unit.calls.is_empty() {
                line.push_str(&format!(
                    "  calls: {}",
                    shorten(&file.file_path, &unit.calls)
                ));
            }
            if !unit.called_by.is_empty() {
                line.push_str(&format!(
                    "  called by: {}",
                    shorten(&file.file_path, &unit.called_by)
                ));
            }
            if unit.role != file.role {
                line.push_str(&format!("  [{}]", unit.role.as_str()));
            }
            if let Some(container) = &unit.contained_in {
                line.push_str(&format!(
                    "  (source shown in {})",
                    short(&file.file_path, container)
                ));
            }
            outln!("{line}");
        }
    }
    outln!("End file list.");
    for file in &pack.files {
        for unit in &file.units {
            if unit.contained_in.is_some() {
                continue;
            }
            outln!("");
            if let Some(reason) = &unit.hit.source_unavailable_reason {
                outln!(
                    "Source {} ({}): {reason}",
                    file.file_path,
                    unit.qualified_name
                );
                continue;
            }
            if unit.hit.source_span.is_empty() {
                // A lead with no text: a whole file too large for the budget.
                if let Some(omitted) = unit.hit.source_span_omitted_bytes {
                    outln!(
                        "Source {} lines {}-{} ({}): {omitted} bytes, too large to show; read the file",
                        file.file_path,
                        unit.hit.span.0,
                        unit.hit.span.1,
                        unit.qualified_name
                    );
                }
                continue;
            }
            outln!(
                "Source {} lines {}-{} ({}):",
                file.file_path,
                unit.hit.span.0,
                unit.hit.span.1,
                unit.qualified_name
            );
            let indent = unit.hit.source_indent.as_deref().unwrap_or("");
            for (offset, text) in unit.hit.source_span.lines().enumerate() {
                // The span starts at the symbol, not the line; give the first
                // line back the indentation its neighbours kept.
                let lead = if offset == 0 { indent } else { "" };
                outln!("{}: {lead}{text}", unit.hit.span.0 as usize + offset);
            }
            if let Some(omitted) = unit.hit.source_span_omitted_bytes {
                outln!("... {omitted} more bytes not shown (budget); read the file for the rest");
            }
        }
    }
    outln!("");
    emit_truncation(pack.shown, pack.hidden, pack.total, pack.truncated);
    if let Some(reason) = &pack.walk_incomplete {
        outln!("warning: {reason}");
    }
    let tests = &pack.related_tests;
    if !tests.items.is_empty() || tests.hidden > 0 || tests.walk_incomplete.is_some() {
        outln!("");
        outln!("Related tests (reach the hits over call edges; not run):");
        for test in &tests.items {
            // A symbol named after its file is the file's own top level.
            let symbols: Vec<String> = test
                .symbols
                .iter()
                .map(|symbol| {
                    if symbol == &test.path {
                        "(top level)".to_string()
                    } else {
                        short(&test.path, symbol)
                    }
                })
                .collect();
            let more =
                test.reached_symbols as usize - symbols.len().min(test.reached_symbols as usize);
            outln!(
                "- {}  depth {}  {}{}",
                test.path,
                test.depth,
                symbols.join(", "),
                if more > 0 {
                    format!(" (+{more} more)")
                } else {
                    String::new()
                }
            );
        }
        emit_truncation(tests.shown, tests.hidden, tests.total, tests.truncated);
        if let Some(reason) = &tests.walk_incomplete {
            outln!("warning: related tests: {reason}");
        }
    }
    if let Some(gap) = &pack.coverage_gap {
        outln!("note: call-graph coverage: {gap}");
    }
}
