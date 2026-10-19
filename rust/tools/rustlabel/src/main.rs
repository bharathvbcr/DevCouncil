//! `rustlabel` writes the Rust half of a DevMap accuracy labels file: what
//! rust-analyzer says every in-repo function, method and operator use in a
//! checkout binds to.
//!
//! It is the oracle the corpus record is scored against
//! (`rust/devmap-analyze/tests/real_corpus_accuracy.rs` reads the output as
//! `<checkout>/.devmap-accuracy/labels.json`). The labels name private paths
//! and symbols, so for a private corpus they are written beside it, never into
//! this repository.
//!
//! ```text
//! cargo run -p devmap-rustlabel --release -- -root <checkout> [-dead <dead.json>] \
//!   -o <checkout>/.devmap-accuracy/labels.json -report <checkout>/.devmap-accuracy/rustlabel-report.json
//! ```
//!
//! Exit codes: 0 labelled (the report says how much of the checkout was read) ·
//! 1 it could not label (no rust-analyzer, no std sources, a workspace that
//! does not load) · 2 usage.

use std::io::Write;
use std::process::ExitCode;

use rustlabel::cli::{parse, USAGE};
use rustlabel::labels::DeadLabel;
use rustlabel::server::{label_checkout, with_dead, Prerequisites};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let options = match parse(&args) {
        Ok(options) => options,
        Err(why) => {
            eprintln!("rustlabel: {why}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match execute(&options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("rustlabel: {why}");
            ExitCode::from(1)
        }
    }
}

fn execute(options: &rustlabel::cli::Options) -> Result<(), String> {
    let dead: Vec<DeadLabel> = match &options.dead {
        Some(path) => {
            let raw =
                std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
            serde_json::from_str(&raw).map_err(|e| format!("{}: {e}", path.display()))?
        }
        None => Vec::new(),
    };
    Prerequisites::check_in(&options.root)?;
    let outcome = label_checkout(&options.root, &options.config)?;
    let labels = with_dead(outcome.labels, dead);
    let encoded = labels.encode().map_err(|e| e.to_string())?;
    match &options.out {
        Some(path) => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("{}: {e}", parent.display()))?;
            }
            std::fs::write(path, &encoded).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        None => std::io::stdout()
            .write_all(&encoded)
            .map_err(|e| e.to_string())?,
    }
    let report = serde_json::to_string_pretty(&outcome.report).map_err(|e| e.to_string())?;
    if let Some(path) = &options.report {
        std::fs::write(path, format!("{report}\n"))
            .map_err(|e| format!("{}: {e}", path.display()))?;
    }
    if let Some(path) = &options.trace {
        let mut lines = outcome.trace.join("\n");
        lines.push('\n');
        std::fs::write(path, lines).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    eprintln!("{report}");
    Ok(())
}
