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
    let inputs = devmap_analyze::pdgsrc::python_function_pdgs(&source, &qualifier, 0, 0);

    let mut graphs = Vec::new();
    let mut refused = Vec::new();
    for input in &inputs {
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

    let sinks_of = |input: &devmap_analyze::pdg::FunctionPdgInput| -> Vec<String> {
        fn walk(statements: &[devmap_analyze::pdg::PdgStatement], out: &mut Vec<String>) {
            use devmap_analyze::pdg::PdgStatementKind as K;
            for statement in statements {
                for sink in &statement.taint_sinks {
                    out.push(format!("{}:{}", statement.line, sink));
                }
                match &statement.kind {
                    K::Branch {
                        then_body,
                        else_body,
                    } => {
                        walk(then_body, out);
                        walk(else_body, out);
                    }
                    K::Loop { body } => walk(body, out),
                    K::Try {
                        body,
                        handlers,
                        finally_body,
                    } => {
                        walk(body, out);
                        for handler in handlers {
                            walk(handler, out);
                        }
                        walk(finally_body, out);
                    }
                    K::Basic | K::Return | K::Raise => {}
                }
            }
        }
        let mut out = Vec::new();
        walk(&input.body, &mut out);
        out
    };

    let rows: Vec<serde_json::Value> = graphs
        .iter()
        .filter_map(|(input, graph)| {
            let sinks = sinks_of(input);
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
                "functions": rows,
                // Both numbers: a filtered view reported as a count
                // reads as a total.
                "shown": rows.len(),
                "total": graphs.len(),
                "refused": refused,
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
    }
    Ok(())
}
