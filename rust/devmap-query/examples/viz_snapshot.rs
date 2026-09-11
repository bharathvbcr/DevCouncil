//! Reproduce a visualization payload from an existing graph without rebuilding its index.
//! cargo run -p devmap-query --no-default-features --example viz_snapshot -- graph.json [--symbols]
use anyhow::{bail, Context, Result};
use devmap_query::viz::{build_payload, VizOptions};
use serde_json::Value;
use std::io::Write;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args.len() > 2 || (args.len() == 2 && args[1] != "--symbols") {
        bail!("usage: viz_snapshot graph.json [--symbols]");
    }
    let graph: Value = serde_json::from_slice(&std::fs::read(&args[0]).context("read graph")?)?;
    let started = std::time::Instant::now();
    let payload = build_payload(
        &graph,
        &VizOptions {
            symbols: args.len() == 2,
            ..Default::default()
        },
    );
    eprintln!("projection: {:?}; {}", started.elapsed(), payload["counts"]);
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());
    serde_json::to_writer(&mut out, &payload)?;
    out.write_all(b"\n")?;
    Ok(())
}
