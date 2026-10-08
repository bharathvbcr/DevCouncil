use crate::cli::Cli;
use crate::output::{emit_json, open_for_read};

#[derive(clap::Args)]
#[group(id = "History")]
pub(crate) struct Args {
    #[arg(short, long, default_value_t = 10)]
    pub(crate) last: usize,
}

pub(crate) fn run(cli: &Cli, args: &Args) -> anyhow::Result<()> {
    let Args { last } = args;
    let store = open_for_read(cli)?;
    let rows = store.build_history(*last)?;

    if cli.json {
        // Deltas are reported against the next-older row, so the oldest
        // row in the window carries none rather than a fabricated zero.
        let entries: Vec<serde_json::Value> = rows
            .iter()
            .enumerate()
            .map(|(index, row)| {
                let previous = rows.get(index + 1);
                serde_json::json!({
                    "generation_id": row.generation_id,
                    "built_at": row.built_at,
                    "head_sha": row.head_sha,
                    "files": row.files,
                    "symbols": row.symbols,
                    "edges": row.edges,
                    "dead_confident": row.dead_confident,
                    "dead_ambiguous": row.dead_ambiguous,
                    "parse_failed": row.parse_failed,
                    "languages_covered": row.languages_covered,
                    "build_ms": row.build_ms,
                    "db_bytes": row.db_bytes,
                    "delta": previous.map(|prev| serde_json::json!({
                        "symbols": row.symbols as i64 - prev.symbols as i64,
                        "edges": row.edges as i64 - prev.edges as i64,
                        "dead_confident": row.dead_confident as i64 - prev.dead_confident as i64,
                        "build_ms": match (row.build_ms, prev.build_ms) {
                            (Some(current), Some(previous)) => Some(current as i128 - previous as i128),
                            _ => None,
                        },
                    })),
                })
            })
            .collect();
        emit_json(
            cli,
            &serde_json::json!({ "shown": rows.len(), "history": entries }),
        )?;
    } else if rows.is_empty() {
        outln!("No build history yet — run `devmap build` first.");
    } else {
        outln!(
            "{:>5}  {:<12} {:>7} {:>8} {:>8} {:>6} {:>9} {:>8}",
            "gen",
            "head",
            "files",
            "symbols",
            "edges",
            "dead",
            "build_ms",
            "db_MiB"
        );
        for (index, row) in rows.iter().enumerate() {
            let head: String = row.head_sha.chars().take(12).collect();
            let delta = rows
                .get(index + 1)
                .map(|prev| {
                    format!(
                        "  ({:+} sym, {:+} edge, {:+} dead)",
                        row.symbols as i64 - prev.symbols as i64,
                        row.edges as i64 - prev.edges as i64,
                        row.dead_confident as i64 - prev.dead_confident as i64,
                    )
                })
                .unwrap_or_default();
            outln!(
                "{:>5}  {:<12} {:>7} {:>8} {:>8} {:>6} {:>9} {:>8.2}{}",
                row.generation_id,
                head,
                row.files,
                row.symbols,
                row.edges,
                row.dead_confident,
                row.build_ms
                    .map_or_else(|| "-".to_string(), |value| value.to_string()),
                row.db_bytes as f64 / (1024.0 * 1024.0),
                delta
            );
        }
    }
    Ok(())
}
