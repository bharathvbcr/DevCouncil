use std::path::PathBuf;

use crate::cli::Cli;
use crate::output::emit_json;
use crate::render_error;

#[derive(clap::Args)]
#[group(id = "Pdg")]
pub(crate) struct Args {
    /// File to analyse. Read from disk, not from the index, so it answers
    /// about the buffer on disk right now.
    pub(crate) file: PathBuf,
    /// Report only statements that reach a security-sensitive sink.
    ///
    /// A sink is evidence. Its *absence* is not a safety claim: the patterns
    /// are a heuristic list of well-known sinks, transcribed from the
    /// implementation this replaces.
    #[arg(long)]
    pub(crate) taint: bool,
}

pub(crate) fn run(cli: &Cli, args: &Args) -> anyhow::Result<()> {
    let Args { file, taint } = args;
    let language = devmap_extract::detect_language(file);
    if language != "python" {
        anyhow::bail!(
            "pdg supports python; {} is {language}. Refusing rather than returning an \
empty graph, which would read as 'this file has no control flow'.",
            file.display()
        );
    }
    let source = devmap_extract::read_source(file)
        .map_err(|error| anyhow::anyhow!("cannot read {}: {error}", file.display()))?;
    let qualifier = file.to_string_lossy().replace('\\', "/");
    let extraction = devmap_analyze::pdgsrc::python_function_pdgs(&source, &qualifier, 0, 0);

    let mut graphs = Vec::new();
    let mut refused = Vec::new();
    for input in &extraction.functions {
        match devmap_analyze::pdg::build_function_pdg(input) {
            Ok(graph) => graphs.push((input, graph)),
            // A function the builder refuses is named with its reason
            // rather than dropped: a short list that looks complete is
            // the failure this whole analysis is careful about.
            Err(error) => refused.push(serde_json::json!({
                "function": input.function_name,
                "reason": render_error(&error),
            })),
        }
    }
    // What the producer could not read — a grammar failure, a syntax error, a
    // function nested past the cap — is a gap in the answer exactly as a
    // refused function is. Without it, an unparseable file answered
    // `functions: []` like an empty module.
    let complete = extraction.is_complete() && refused.is_empty();

    let rows: Vec<serde_json::Value> = graphs
        .iter()
        .filter_map(|(input, graph)| {
            let sinks: Vec<String> = input
                .taint_sinks()
                .into_iter()
                .map(|(line, variable)| format!("{line}:{variable}"))
                .collect();
            if *taint && sinks.is_empty() {
                return None;
            }
            Some(serde_json::json!({
                "function": graph.function_name,
                "start_line": input.start_line,
                "end_line": input.end_line,
                "params": input.params,
                "nodes": graph.nodes.len(),
                "edges": graph.edges.len(),
                "taint_sinks": sinks,
            }))
        })
        .collect();

    if cli.json {
        emit_json(
            cli,
            &serde_json::json!({
                "file": file,
                "language": language,
                "parse": extraction.parse,
                "functions": rows,
                // Both numbers: a filtered view reported as a count
                // reads as a total.
                "shown": rows.len(),
                "total": graphs.len(),
                // `total` counts what was built. It is the file's whole
                // function count only when `complete` is true.
                "complete": complete,
                "refused": refused,
                "skipped": extraction.skipped,
                "skipped_total": extraction.skipped_total,
            }),
        )?;
    } else {
        for row in &rows {
            outln!(
                "{}  lines {}-{}  {} node(s), {} edge(s)",
                row["function"].as_str().unwrap_or(""),
                row["start_line"],
                row["end_line"],
                row["nodes"],
                row["edges"]
            );
            for sink in row["taint_sinks"].as_array().into_iter().flatten() {
                outln!("    sink {}", sink.as_str().unwrap_or(""));
            }
        }
        outln!("  {} of {} function(s)", rows.len(), graphs.len());
        for entry in &refused {
            outln!(
                "  refused {}: {}",
                entry["function"].as_str().unwrap_or(""),
                entry["reason"].as_str().unwrap_or("")
            );
        }
        for skipped in &extraction.skipped {
            let reason = serde_json::to_value(skipped.reason)?;
            outln!(
                "  skipped lines {}-{}{}: {}",
                skipped.start_line,
                skipped.end_line,
                skipped
                    .function
                    .as_deref()
                    .map(|name| format!(" ({name})"))
                    .unwrap_or_default(),
                reason.as_str().unwrap_or("")
            );
        }
        if extraction.skipped_total > extraction.skipped.len() {
            outln!(
                "  … {} more skipped region(s) not listed",
                extraction.skipped_total - extraction.skipped.len()
            );
        }
        if !complete {
            let parse = serde_json::to_value(extraction.parse)?;
            outln!(
                "  incomplete (parse: {}): the count above is a lower bound, not the file's \
function count.",
                parse.as_str().unwrap_or("")
            );
        }
    }
    Ok(())
}
