//! Serialise every query surface's answer, so two builds can be diffed.
//!
//! A performance change is only a performance change if the answers do not
//! move. Counts are not enough — a reordering, a dropped tie-break or a lost
//! `walk_incomplete` reason all keep the counts and change the answer — so this
//! writes the **content** of every surface as JSON and leaves the comparison to
//! `diff`.
//!
//! ```sh
//! cargo run --release -p devmap-query --example query_snapshot -- <devmap.sqlite> <out-dir>
//! # ... change the engine, rebuild ...
//! diff -r before/ after/
//! ```
//!
//! `<out-dir>/answers.json` holds every bounded surface; `repo_map.json` and
//! `code_graph.json` are written beside it because they are large enough that
//! diffing them inline is unhelpful.
//!
//! Targets are read out of the store the same way `query_bench` reads them, so
//! both runs ask the same questions of the same generation without a hardcoded
//! name that might not exist in someone else's corpus.
//!
//! No assertions and no dependencies: it prints what it wrote and exits.

use std::collections::BTreeMap;
use std::path::PathBuf;

use devmap_query::{Request, StoreQueryEngine};
use devmap_store::Store;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let db = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("usage: query_snapshot <devmap.sqlite> <out-dir>"))?;
    let out = PathBuf::from(
        args.next()
            .ok_or_else(|| anyhow::anyhow!("usage: query_snapshot <devmap.sqlite> <out-dir>"))?,
    );
    std::fs::create_dir_all(&out)?;

    let store = Store::open_existing(&db)?.ok_or_else(|| anyhow::anyhow!("no store at {db}"))?;
    let edges = store.latest_edges(0.0)?;
    if edges.is_empty() {
        anyhow::bail!("refusing to snapshot an empty store");
    }

    // Same target selection as `query_bench`, so the two examples interrogate
    // the same corners of the corpus.
    let mut inbound: BTreeMap<&str, usize> = BTreeMap::new();
    for edge in &edges {
        if edge.edge_kind == "Calls" {
            *inbound.entry(edge.target_symbol.as_str()).or_default() += 1;
        }
    }
    let symbol = inbound
        .iter()
        .max_by_key(|(name, count)| (**count, std::cmp::Reverse(*name)))
        .map(|(name, _)| (*name).to_string())
        .ok_or_else(|| anyhow::anyhow!("store has no Calls edges"))?;
    let bare = symbol.rsplit("::").next().unwrap_or(&symbol).to_string();
    let file = edges
        .iter()
        .map(|edge| edge.source_file.clone())
        .find(|path| !path.is_empty())
        .ok_or_else(|| anyhow::anyhow!("store has no edge with a source file"))?;
    let fanout: Vec<String> = inbound
        .iter()
        .rev()
        .take(8)
        .map(|(name, _)| (*name).to_string())
        .collect();
    let pair = {
        let into = edges
            .iter()
            .find(|edge| edge.target_symbol == symbol && edge.source_symbol != symbol);
        let out_edge = edges
            .iter()
            .find(|edge| edge.source_symbol == symbol && edge.target_symbol != symbol);
        into.zip(out_edge)
            .map(|(a, b)| (a.source_symbol.clone(), b.target_symbol.clone()))
    };

    let engine = StoreQueryEngine::new(&store);
    let request = |query: &str, depth: usize| Request {
        query: query.to_string(),
        token_budget: 20_000,
        min_confidence: 0.0,
        max_depth: depth,
    };

    let mut answers = serde_json::Map::new();
    answers.insert(
        "targets".to_string(),
        serde_json::json!({
            "symbol": symbol, "bare": bare, "file": file,
            "fanout": fanout, "pair": pair,
        }),
    );
    answers.insert(
        "search".to_string(),
        serde_json::to_value(engine.search(request(&bare, 3))?)?,
    );
    answers.insert(
        "dependencies".to_string(),
        serde_json::to_value(engine.dependencies(request(&file, 3))?)?,
    );
    // Both target shapes: a qualified id resolves through the path matcher, a
    // bare name does not, and the rewrite of that matcher has to leave both
    // alone.
    for (label, target) in [
        ("symbol", symbol.as_str()),
        ("bare", bare.as_str()),
        ("file", file.as_str()),
    ] {
        answers.insert(
            format!("impact_{label}"),
            serde_json::to_value(engine.impact(request(target, 5))?)?,
        );
        answers.insert(
            format!("trace_{label}"),
            serde_json::to_value(engine.trace(request(target, 3))?)?,
        );
    }
    if let Some((from, to)) = pair.clone() {
        answers.insert(
            "trace_between".to_string(),
            serde_json::to_value(engine.trace_between(Request {
                query: (from, to),
                token_budget: 20_000,
                min_confidence: 0.0,
                max_depth: 8,
            })?)?,
        );
    }
    answers.insert(
        "neighbors".to_string(),
        serde_json::to_value(engine.neighbors(&fanout, 20_000, 0.0, 5)?)?,
    );
    // A second fan-out under a confidence filter: `min_confidence` is now
    // applied once, in the hoisted load, so an answer that silently widened
    // would show up here.
    answers.insert(
        "neighbors_filtered".to_string(),
        serde_json::to_value(engine.neighbors(&fanout, 20_000, 0.9, 5)?)?,
    );
    answers.insert(
        "neighbors_empty".to_string(),
        serde_json::to_value(engine.neighbors(&[], 20_000, 0.0, 5)?)?,
    );
    answers.insert(
        "dead_symbols".to_string(),
        serde_json::to_value(engine.dead_symbols(20_000)?)?,
    );
    answers.insert(
        "explore".to_string(),
        serde_json::to_value(engine.explore(&bare, 5, 40_000, 0.0, 3)?)?,
    );
    answers.insert(
        "affected_tests".to_string(),
        serde_json::to_value(engine.affected_tests(&fanout, 20_000, 0.0, 3)?)?,
    );

    let answers_path = out.join("answers.json");
    std::fs::write(
        &answers_path,
        serde_json::to_string_pretty(&serde_json::Value::Object(answers))?,
    )?;

    // The two artifacts, exactly as `dev map manifest` renders them.
    let extractions = store.latest_extractions()?;
    let analysis = store
        .latest_analysis()?
        .ok_or_else(|| anyhow::anyhow!("store has no analysis summary"))?;
    let generation_id = store
        .latest_generation_id()?
        .ok_or_else(|| anyhow::anyhow!("store has no generation"))?;
    let resolved = edges
        .into_iter()
        .map(devmap_query::resolved_edge_from_stored)
        .collect::<anyhow::Result<Vec<_>>>()?;
    let freshness = devmap_query::FreshnessInfo {
        head_sha: store
            .latest_generation_head()?
            .unwrap_or_else(|| "unavailable".to_string()),
        generation_id,
        pending_count: 0,
        stamped: devmap_query::StampedFreshness::default(),
    };
    // The tree this generation indexed, as `write_consumer_artifacts` reads it,
    // so the snapshot's `package_managers` / `test_commands` are the same
    // answer `dev map manifest` would write.
    let repo_root = store.latest_repo_root()?.map(std::path::PathBuf::from);
    let (_lean, manifest_json) = devmap_query::generate_manifest_with_edges(
        &extractions,
        &analysis,
        freshness.clone(),
        &resolved,
        repo_root.as_deref(),
    );
    std::fs::write(out.join("repo_map.json"), &manifest_json)?;
    let (graph_json, _) = devmap_query::generate_code_graph_encodings(
        &extractions,
        &analysis,
        &resolved,
        &freshness,
        store.latest_repo_root()?.as_deref(),
        false,
    )?;
    std::fs::write(out.join("code_graph.json"), &graph_json)?;

    println!(
        "wrote {} ({} bytes), repo_map.json ({} bytes), code_graph.json ({} bytes)",
        answers_path.display(),
        std::fs::metadata(&answers_path)?.len(),
        manifest_json.len(),
        graph_json.len()
    );
    // Named so a diff that comes back empty cannot be mistaken for a run that
    // compared nothing.
    println!("surfaces snapshotted: search, dependencies, impact x3, trace x3, trace_between, neighbors, neighbors_filtered, neighbors_empty, dead_symbols, explore, affected_tests, repo_map, code_graph");
    Ok(())
}
