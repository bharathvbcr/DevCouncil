//! Where a Dev Map query's time goes, per surface and per phase.
//!
//! Measurement only, no assertions — `tests/` owns every property this touches.
//! Dependency-free by design: this workspace has no benchmark-harness
//! dependency and is not taking one, so the timing is `std::time::Instant` and
//! the statistics are a sort and an index. Same shape as
//! `devmap-extract/examples/budget_probe.rs`.
//!
//! ```sh
//! # generate (and cache) a synthetic corpus, then measure every surface
//! cargo run --release -p devmap-query --example query_bench
//!
//! # measure an existing store instead — a real repository's devmap.sqlite
//! cargo run --release -p devmap-query --example query_bench -- .devcouncil/codeintel/devmap.sqlite
//! ```
//!
//! Corpus size is `DEVMAP_BENCH_FILES` x `DEVMAP_BENCH_SYMS` (default
//! 4000 x 20, which is ~248k symbols); `DEVMAP_BENCH_DIR` moves the cache. A
//! generated corpus is written once and reused, so only the first run pays for
//! extraction.
//!
//! Release matters. A debug build's numbers are ten times larger and are not
//! comparable with anything here; the header line prints which profile ran.
//!
//! "cold" reopens the `Store`, which empties its per-instance edge cache, so a
//! cold row includes the SQL read. It does not evict the OS page cache — that
//! is not portable — so a cold row is "cold process, warm disk", which is what
//! a CLI invocation against a recently written store actually gets.
//!
//! ## Recorded results
//!
//! Machine: Apple M5 Pro (18 cores), macOS 27.0, rustc 1.98.0, `--release`.
//! Method: the before and after binaries were built from the same tree with
//! only the three fixes reverted, then run **interleaved** — before, after,
//! before, after — for five rounds on each corpus, and the medians of the
//! per-round warm p50 taken. Interleaving is the point: a single sequential
//! before-then-after run showed unchanged surfaces moving by ±10%, which is
//! machine drift being read as a result.
//!
//! Corpus A: generated, 4,000 files / 252,000 symbols / 660,000 edges
//! (`DEVMAP_BENCH_FILES=4000 DEVMAP_BENCH_SYMS=20`).
//! Corpus B: this repository indexed by `devmap build` — 1,379 files /
//! 15,684 symbols / 79,369 edges, three confidence tiers, six edge kinds.
//!
//! ```text
//! warm p50, median of 5      corpus A (660k edges)      corpus B (79k edges)
//!                            before    after            before    after
//! search                      23.8ms   23.2ms   -3%       2.5ms    3.2ms  (noise)
//! dependencies (one file)     55.2ms   54.6ms   -1%       8.9ms    9.4ms  (noise)
//! impact (symbol, depth 5)   215.9ms  142.6ms  -34%      37.3ms   27.8ms  -26%
//! trace (symbol, depth 1)    204.0ms  132.4ms  -35%      26.9ms   17.8ms  -34%
//! trace_between (a -> b)     211.5ms  203.2ms   -4%      28.1ms   28.6ms  (noise)
//! neighbors (8 targets)      2640 ms  777.5ms  -71%     399.1ms  106.3ms  -73%
//! dead_symbols                80.7ms   78.4ms   -3%       4.7ms    4.7ms
//! manifest (repo_map.json)    ~21 ms                      ~4 ms   (unchanged)
//! code_graph.json             ~1.9 s                    ~640 ms   (unchanged)
//!
//! impact phases, corpus A    before    after     corpus B  before   after
//! store.latest_edges          39.1ms   39.7ms                6.0ms   6.2ms
//! resolved_edge_from_stored   31.9ms   32.3ms                4.5ms   4.4ms
//! traversal_starts            52.0ms   13.8ms  -73%          7.6ms   1.8ms  -76%
//! traverse_graph index+walk   57.5ms   59.6ms                11.3ms  11.2ms
//! traversed_resolution_edges  43.6ms   11.9ms  -73%         10.3ms   5.2ms  -50%
//! ```
//!
//! Read the rows that did not move as the control: `search`, `dependencies`,
//! `dead_symbols`, `latest_edges` and `traverse_graph` are untouched code and
//! they stayed put, which is what makes the rows that did move credible.
//! Sub-millisecond rows on corpus B (`search`, `dead_symbols`) are inside the
//! run-order noise — the before binary always ran first in a round, so it paid
//! any page-cache warm-up — and should not be read as results in either
//! direction.
//!
//! What did NOT get fixed, and why, is in `examples/query_phase_ab.rs`: the
//! per-call adjacency index inside `traverse_graph` is ~100% of a walk's cost
//! and lives in `devmap-analyze`; `dead_symbols` reads all 160,000 dead rows
//! to show 66 and lives in `devmap-store`; `trace_between` is dominated by its
//! confidence sort, which is why it barely moved here.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use devmap_query::{Request, StoreQueryEngine};
use devmap_store::Store;

/// Warm repetitions for a surface that costs less than ~50 ms.
const WARM_FAST: usize = 20;
/// Warm repetitions for a surface that costs more.
const WARM_SLOW: usize = 5;

fn main() -> anyhow::Result<()> {
    let profile = if cfg!(debug_assertions) {
        "debug (numbers are ~10x a release build; do not compare across profiles)"
    } else {
        "release"
    };
    println!("profile: {profile}");

    let corpus = match std::env::args().nth(1) {
        Some(path) => Corpus::existing(PathBuf::from(path))?,
        None => Corpus::generated()?,
    };

    let store = corpus.open()?;
    let status = store.status(&corpus.db.display().to_string())?;
    println!(
        "store: {} nodes, {} edges, generation {:?}",
        status.node_count, status.edge_count, status.latest_generation
    );
    if status.edge_count == 0 {
        anyhow::bail!("refusing to report a fast query over an empty store");
    }

    // Targets are picked out of the store, not hardcoded, so this runs against
    // a generated corpus and against a real repository unchanged. A surface
    // whose target could not be found is SKIPPED and says so — a benchmark
    // that quietly measures a miss reports the miss's speed as the query's.
    let targets = Targets::pick(&store)?;
    println!("{targets}\n");

    header();
    search(&corpus, &targets)?;
    dependencies(&corpus, &targets)?;
    impact(&corpus, &targets)?;
    trace(&corpus, &targets)?;
    trace_between(&corpus, &targets)?;
    neighbors(&corpus, &targets)?;
    dead_symbols(&corpus)?;
    artifacts(&corpus)?;

    impact_phases(&store, &targets)?;
    Ok(())
}

// ---------------------------------------------------------------- reporting

fn header() {
    println!(
        "{:<34} {:>11} {:>11} {:>11}  answer",
        "surface", "cold", "warm p50", "warm p90"
    );
    println!("{}", "-".repeat(96));
}

fn report(label: &str, cold: Duration, warm: &[Duration], answer: String) {
    let (p50, p90) = percentiles(warm);
    println!("{label:<34} {cold:>11.2?} {p50:>11.2?} {p90:>11.2?}  {answer}");
}

fn skipped(label: &str, why: &str) {
    println!(
        "{label:<34} {:>11} {:>11} {:>11}  SKIPPED: {why}",
        "-", "-", "-"
    );
}

fn percentiles(samples: &[Duration]) -> (Duration, Duration) {
    let mut sorted = samples.to_vec();
    sorted.sort();
    if sorted.is_empty() {
        return (Duration::ZERO, Duration::ZERO);
    }
    let p50 = sorted[sorted.len() / 2];
    let p90 = sorted[(sorted.len() * 9 / 10).min(sorted.len() - 1)];
    (p50, p90)
}

/// One cold sample (fresh `Store`) plus `repeats` warm samples on one engine.
///
/// The cold sample reopens the store, which is what empties its edge cache.
/// Doing that inside the timed region is deliberate: opening is part of what a
/// CLI invocation pays.
fn measure<T>(
    corpus: &Corpus,
    repeats: usize,
    mut call: impl FnMut(&StoreQueryEngine<'_>) -> anyhow::Result<T>,
) -> anyhow::Result<(Duration, Vec<Duration>, T)> {
    let cold_store = corpus.open()?;
    let started = Instant::now();
    let engine = StoreQueryEngine::new(&cold_store);
    let first = call(&engine)?;
    let cold = started.elapsed();
    std::hint::black_box(&first);

    let store = corpus.open()?;
    let engine = StoreQueryEngine::new(&store);
    // One untimed call so the warm samples measure a warm cache, not the
    // transition into one.
    std::hint::black_box(call(&engine)?);
    let mut warm = Vec::with_capacity(repeats);
    let mut last = None;
    for _ in 0..repeats {
        let started = Instant::now();
        let answer = call(&engine)?;
        warm.push(started.elapsed());
        last = Some(answer);
    }
    Ok((cold, warm, last.expect("repeats >= 1")))
}

// ----------------------------------------------------------------- surfaces

fn search(corpus: &Corpus, targets: &Targets) -> anyhow::Result<()> {
    let query = targets.symbol_name.clone();
    let (cold, warm, answer) = measure(corpus, WARM_FAST, |engine| {
        engine.search(Request {
            query: query.clone(),
            token_budget: devmap_query::Budget::SEARCH,
            min_confidence: 0.0,
            max_depth: 3,
        })
    })?;
    report(
        &format!("search {:?}", targets.symbol_name),
        cold,
        &warm,
        format!("shown={} total={}", answer.shown, answer.total),
    );
    Ok(())
}

fn dependencies(corpus: &Corpus, targets: &Targets) -> anyhow::Result<()> {
    let file = targets.file.clone();
    let (cold, warm, answer) = measure(corpus, WARM_FAST, |engine| {
        engine.dependencies(Request {
            query: file.clone(),
            token_budget: devmap_query::Budget::DEPS,
            min_confidence: 0.0,
            max_depth: 3,
        })
    })?;
    report(
        "dependencies (one file)",
        cold,
        &warm,
        format!("shown={} total={}", answer.shown, answer.total),
    );
    Ok(())
}

fn impact(corpus: &Corpus, targets: &Targets) -> anyhow::Result<()> {
    let target = targets.symbol.clone();
    let (cold, warm, answer) = measure(corpus, WARM_SLOW, |engine| {
        engine.impact(Request {
            query: target.clone(),
            token_budget: devmap_query::Budget::DEPS,
            min_confidence: 0.0,
            max_depth: 5,
        })
    })?;
    report(
        "impact (symbol, depth 5)",
        cold,
        &warm,
        format!("shown={} total={}", answer.shown, answer.total),
    );
    Ok(())
}

fn trace(corpus: &Corpus, targets: &Targets) -> anyhow::Result<()> {
    let target = targets.symbol.clone();
    let (cold, warm, answer) = measure(corpus, WARM_SLOW, |engine| {
        engine.trace(Request {
            query: target.clone(),
            token_budget: devmap_query::Budget::DEPS,
            min_confidence: 0.0,
            max_depth: 1,
        })
    })?;
    report(
        "trace (symbol, depth 1)",
        cold,
        &warm,
        format!("shown={} total={}", answer.shown, answer.total),
    );
    Ok(())
}

fn trace_between(corpus: &Corpus, targets: &Targets) -> anyhow::Result<()> {
    let Some((from, to)) = targets.path_endpoints.clone() else {
        skipped(
            "trace_between (a -> b)",
            "no connected pair found in this store",
        );
        return Ok(());
    };
    let (cold, warm, answer) = measure(corpus, WARM_SLOW, |engine| {
        engine.trace_between(Request {
            query: (from.clone(), to.clone()),
            token_budget: devmap_query::Budget::DEPS,
            min_confidence: 0.0,
            max_depth: 8,
        })
    })?;
    report(
        "trace_between (a -> b)",
        cold,
        &warm,
        format!(
            "shown={} resolution={}",
            answer.shown,
            match &answer.resolution {
                devmap_query::ResolutionAvailability::Available => "available".to_string(),
                devmap_query::ResolutionAvailability::Unavailable { reason } =>
                    format!("unavailable ({reason})"),
            }
        ),
    );
    Ok(())
}

fn neighbors(corpus: &Corpus, targets: &Targets) -> anyhow::Result<()> {
    let fanout = targets.neighbors.clone();
    if fanout.is_empty() {
        skipped("neighbors", "no targets");
        return Ok(());
    }
    let count = fanout.len();
    let (cold, warm, answer) = measure(corpus, WARM_SLOW, |engine| {
        engine.neighbors(&fanout, devmap_query::Budget::DEPS, 0.0, 5)
    })?;
    let edges: u32 = answer
        .iter()
        .map(|n| n.callers.shown + n.callees.shown)
        .sum();
    report(
        &format!("neighbors ({count} targets)"),
        cold,
        &warm,
        format!("{} answers, {edges} edges shown", answer.len()),
    );
    Ok(())
}

fn dead_symbols(corpus: &Corpus) -> anyhow::Result<()> {
    let (cold, warm, answer) = measure(corpus, WARM_FAST, |engine| {
        engine.dead_symbols(devmap_query::Budget::DEAD)
    })?;
    report(
        "dead_symbols",
        cold,
        &warm,
        format!("shown={} total={}", answer.shown, answer.total),
    );
    Ok(())
}

/// `repo_map.json` and `code_graph.json`, exactly as `dev map manifest` builds
/// them: read the generation back out of the store, then render.
fn artifacts(corpus: &Corpus) -> anyhow::Result<()> {
    let store = corpus.open()?;

    let started = Instant::now();
    let extractions = store.latest_extractions()?;
    let read_extractions = started.elapsed();

    let analysis = store
        .latest_analysis()?
        .ok_or_else(|| anyhow::anyhow!("store has no analysis summary"))?;
    let generation_id = store
        .latest_generation_id()?
        .ok_or_else(|| anyhow::anyhow!("store has no generation"))?;

    let started = Instant::now();
    let edges = store
        .latest_edges(0.0)?
        .into_iter()
        .map(devmap_query::resolved_edge_from_stored)
        .collect::<anyhow::Result<Vec<_>>>()?;
    let read_edges = started.elapsed();

    let freshness = devmap_query::FreshnessInfo {
        head_sha: store
            .latest_generation_head()?
            .unwrap_or_else(|| "unavailable".to_string()),
        generation_id,
        pending_count: 0,
        stamped: devmap_query::StampedFreshness::default(),
    };

    let started = Instant::now();
    let (_manifest, manifest_json) = devmap_query::generate_manifest_with_edges(
        &extractions,
        &analysis,
        freshness.clone(),
        &edges,
        None,
    );
    let manifest = started.elapsed();

    let repo_root = store.latest_repo_root()?;
    let started = Instant::now();
    let (graph_json, _compact) = devmap_query::generate_code_graph_encodings(
        &extractions,
        &analysis,
        &edges,
        &freshness,
        repo_root.as_deref(),
        false,
    )?;
    let graph = started.elapsed();

    println!(
        "{:<34} {:>11.2?} {:>11} {:>11}  {} extractions",
        "  read latest_extractions",
        read_extractions,
        "-",
        "-",
        extractions.len()
    );
    println!(
        "{:<34} {:>11.2?} {:>11} {:>11}  {} edges",
        "  read + convert latest_edges",
        read_edges,
        "-",
        "-",
        edges.len()
    );
    println!(
        "{:<34} {:>11.2?} {:>11} {:>11}  {} bytes",
        "manifest (repo_map.json)",
        manifest,
        "-",
        "-",
        manifest_json.len()
    );
    println!(
        "{:<34} {:>11.2?} {:>11} {:>11}  {} bytes",
        "code_graph.json",
        graph,
        "-",
        "-",
        graph_json.len()
    );
    Ok(())
}

// -------------------------------------------------------- phase attribution

/// Split one `impact` call into the phases it is actually made of.
///
/// Every phase below is the engine's own code, called directly — not a
/// reimplementation of it — so a number here points at a function that exists.
fn impact_phases(store: &Store, targets: &Targets) -> anyhow::Result<()> {
    println!("\nimpact phase breakdown (warm store, p50 of {WARM_SLOW})");
    println!("{}", "-".repeat(96));

    let mut read = Vec::new();
    let mut convert = Vec::new();
    let mut starts_timing = Vec::new();
    let mut walk = Vec::new();
    let mut select = Vec::new();

    let mut edges = Vec::new();
    let mut start_count = 0usize;
    let mut traversed_count = 0usize;
    let mut selected_count = 0usize;
    for _ in 0..WARM_SLOW {
        let t = Instant::now();
        let rows = store.latest_edges(0.0)?;
        read.push(t.elapsed());

        let t = Instant::now();
        edges = rows
            .into_iter()
            .map(devmap_query::resolved_edge_from_stored)
            .collect::<anyhow::Result<Vec<_>>>()?;
        convert.push(t.elapsed());

        let t = Instant::now();
        let starts: Vec<String> = devmap_query::traversal_starts(&edges, &targets.symbol, true)
            .into_iter()
            .map(|(symbol, _)| symbol)
            .collect();
        starts_timing.push(t.elapsed());
        start_count = starts.len();

        let t = Instant::now();
        let result = devmap_analyze::traversal::traverse_graph(
            &starts,
            &edges,
            &devmap_analyze::traversal::TraversalOptions {
                max_depth: 5,
                max_nodes: 5_000,
                reverse: true,
            },
        );
        walk.push(t.elapsed());
        traversed_count = result.traversed_edges.len();

        let t = Instant::now();
        let picked = devmap_query::traversed_resolution_edges(&result, &edges, 0.0);
        select.push(t.elapsed());
        selected_count = picked.len();
        std::hint::black_box(&picked);
    }

    let phase = |label: &str, samples: &[Duration], note: String| {
        let (p50, p90) = percentiles(samples);
        println!("{label:<34} {p50:>11.2?} {p90:>11.2?}  {note}");
    };
    println!("{:<34} {:>11} {:>11}  note", "phase", "p50", "p90");
    phase(
        "store.latest_edges (SQL/cache)",
        &read,
        format!("{} rows", edges.len()),
    );
    phase(
        "resolved_edge_from_stored",
        &convert,
        format!("{} rows", edges.len()),
    );
    phase(
        "traversal_starts",
        &starts_timing,
        format!("scans {} edges -> {start_count} starts", edges.len()),
    );
    phase(
        "traverse_graph (index + walk)",
        &walk,
        format!("{traversed_count} traversed edges"),
    );
    phase(
        "traversed_resolution_edges",
        &select,
        format!("scans {} edges -> {selected_count} kept", edges.len()),
    );
    Ok(())
}

// ------------------------------------------------------------------ targets

/// Query inputs chosen from the store's own contents.
struct Targets {
    /// A `file::symbol` identity that appears as an edge target.
    symbol: String,
    /// Its bare name, for `search`.
    symbol_name: String,
    /// An indexed file path, for `dependencies`.
    file: String,
    /// Two node ids with an edge between them, for `trace_between`.
    path_endpoints: Option<(String, String)>,
    /// Eight distinct edge targets, for the composed `neighbors` fan-out.
    neighbors: Vec<String>,
}

impl Targets {
    fn pick(store: &Store) -> anyhow::Result<Self> {
        let edges = store.latest_edges(0.0)?;
        // The busiest target: the node with the most inbound edges, so `impact`
        // measures a real traversal rather than a leaf with one caller.
        let mut inbound: std::collections::BTreeMap<&str, usize> = Default::default();
        for edge in &edges {
            if edge.edge_kind == "Calls" {
                *inbound.entry(edge.target_symbol.as_str()).or_default() += 1;
            }
        }
        let symbol = inbound
            .iter()
            .max_by_key(|(name, count)| (**count, std::cmp::Reverse(*name)))
            .map(|(name, _)| (*name).to_string())
            .ok_or_else(|| anyhow::anyhow!("store has no Calls edges to traverse"))?;
        let symbol_name = symbol
            .rsplit("::")
            .next()
            .unwrap_or(&symbol)
            .rsplit('.')
            .next()
            .unwrap_or(&symbol)
            .to_string();

        let file = edges
            .iter()
            .map(|edge| edge.source_file.clone())
            .find(|path| !path.is_empty())
            .ok_or_else(|| anyhow::anyhow!("store has no edge with a source file"))?;

        // A two-hop pair: some x -> symbol -> y. Endpoints far enough apart
        // that the walk does work, close enough that it terminates.
        let inbound_edge = edges
            .iter()
            .find(|edge| edge.target_symbol == symbol && edge.source_symbol != symbol);
        let outbound_edge = edges
            .iter()
            .find(|edge| edge.source_symbol == symbol && edge.target_symbol != symbol);
        let path_endpoints = match (inbound_edge, outbound_edge) {
            (Some(into), Some(out)) => {
                Some((into.source_symbol.clone(), out.target_symbol.clone()))
            }
            _ => None,
        };

        let mut seen = BTreeSet::new();
        let mut neighbors = Vec::new();
        for (name, _) in inbound.iter().rev() {
            if seen.insert(*name) {
                neighbors.push((*name).to_string());
            }
            if neighbors.len() == 8 {
                break;
            }
        }

        Ok(Self {
            symbol,
            symbol_name,
            file,
            path_endpoints,
            neighbors,
        })
    }
}

impl std::fmt::Display for Targets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "targets: symbol={:?} search={:?} file={:?} pair={:?} fanout={}",
            self.symbol,
            self.symbol_name,
            self.file,
            self.path_endpoints,
            self.neighbors.len()
        )
    }
}

// ------------------------------------------------------------------- corpus

struct Corpus {
    db: PathBuf,
}

impl Corpus {
    fn existing(db: PathBuf) -> anyhow::Result<Self> {
        if !db.exists() {
            anyhow::bail!("no store at {}", db.display());
        }
        println!("corpus: existing store at {}", db.display());
        Ok(Self { db })
    }

    fn open(&self) -> anyhow::Result<Store> {
        Store::open_existing(&self.db)?
            .ok_or_else(|| anyhow::anyhow!("no store at {}", self.db.display()))
    }

    /// Build (or reuse) a synthetic corpus large enough for the numbers to mean
    /// something: thousands of files, hundreds of thousands of symbols, and a
    /// call graph with real fan-in rather than a chain.
    fn generated() -> anyhow::Result<Self> {
        let files: usize = env_usize("DEVMAP_BENCH_FILES", 4_000);
        let syms: usize = env_usize("DEVMAP_BENCH_SYMS", 20);
        let root = match std::env::var_os("DEVMAP_BENCH_DIR") {
            Some(dir) => PathBuf::from(dir),
            None => std::env::temp_dir().join("devmap-query-bench"),
        };
        let root = root.join(format!("f{files}-s{syms}"));
        let db = root.join("devmap.sqlite");
        if db.exists() {
            println!("corpus: cached at {} (delete to rebuild)", root.display());
            return Ok(Self { db });
        }

        println!(
            "corpus: generating {files} files x ~{} symbols at {}",
            syms * 3 + 2,
            root.display()
        );
        std::fs::create_dir_all(&root)?;

        let started = Instant::now();
        let sources = write_sources(&root, files, syms)?;
        println!("  generate sources        {:>11.2?}", started.elapsed());

        let started = Instant::now();
        let extractions: Vec<_> = sources
            .iter()
            .map(|(path, source)| devmap_extract::extract_file(path, source))
            .collect();
        let symbol_count: usize = extractions.iter().map(|e| e.symbols.len()).sum();
        println!(
            "  extract                 {:>11.2?}  {symbol_count} symbols",
            started.elapsed()
        );

        let started = Instant::now();
        let mut resolver = devmap_resolve::Resolver::new();
        resolver.index_extractions(&extractions);
        let resolution = resolver.resolve_all(&extractions);
        println!(
            "  resolve                 {:>11.2?}  {} edges",
            started.elapsed(),
            resolution.edges.len()
        );

        let started = Instant::now();
        let analysis = devmap_analyze::analyze(&extractions, &resolution);
        println!("  analyze                 {:>11.2?}", started.elapsed());

        let started = Instant::now();
        let store = Store::open(&db)?;
        store.save_generation_with_opts(
            &extractions,
            &resolution,
            &analysis,
            devmap_store::GenerationWriteOpts {
                repo_root: Some(root.display().to_string()),
                ..Default::default()
            },
        )?;
        println!("  save_generation         {:>11.2?}", started.elapsed());

        Ok(Self { db })
    }
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(default)
}

/// One package per 100 modules, each module importing two others, so the call
/// graph has cross-file fan-in instead of being a set of islands.
fn write_sources(root: &Path, files: usize, syms: usize) -> anyhow::Result<Vec<(String, String)>> {
    let per_package = 100;
    let mut out = Vec::with_capacity(files);
    let mut current_dir = None;
    for index in 0..files {
        let package = index / per_package;
        let module = index % per_package;
        let dir = root.join(format!("pkg{package}"));
        if current_dir.as_ref() != Some(&dir) {
            std::fs::create_dir_all(&dir)?;
            std::fs::write(dir.join("__init__.py"), "")?;
            current_dir = Some(dir.clone());
        }
        let rel = format!("pkg{package}/mod_{module:03}.py");
        // Two imports, chosen to wrap around the corpus so the last module
        // still points at real code.
        let (up_pkg, up_mod) = {
            let other = (index + 37) % files;
            (other / per_package, other % per_package)
        };
        let (far_pkg, far_mod) = {
            let other = (index + 1013) % files;
            (other / per_package, other % per_package)
        };
        let source = module_source(package, module, syms, up_pkg, up_mod, far_pkg, far_mod);
        std::fs::write(root.join(&rel), &source)?;
        out.push((rel, source));
    }
    Ok(out)
}

fn module_source(
    package: usize,
    module: usize,
    syms: usize,
    up_pkg: usize,
    up_mod: usize,
    far_pkg: usize,
    far_mod: usize,
) -> String {
    let mut source = String::with_capacity(syms * 320);
    source.push_str(&format!(
        "from pkg{up_pkg}.mod_{up_mod:03} import svc_{up_pkg}_{up_mod:03}_0\n\
         from pkg{far_pkg}.mod_{far_mod:03} import svc_{far_pkg}_{far_mod:03}_0\n\n\n"
    ));
    source.push_str(&format!("class Service_{package}_{module:03}:\n"));
    source.push_str("    def __init__(self):\n        self.state = 0\n\n");
    for i in 0..syms {
        source.push_str(&format!(
            "    def handle_{i}(self, request):\n\
             \x20       value = svc_{up_pkg}_{up_mod:03}_0(request)\n\
             \x20       return helper_{package}_{module:03}_{i}(value)\n\n"
        ));
    }
    source.push('\n');
    for i in 0..syms {
        source.push_str(&format!(
            "def svc_{package}_{module:03}_{i}(payload):\n\
             \x20   staged = helper_{package}_{module:03}_{i}(payload)\n\
             \x20   return svc_{far_pkg}_{far_mod:03}_0(staged)\n\n\n"
        ));
    }
    for i in 0..syms {
        source.push_str(&format!(
            "def helper_{package}_{module:03}_{i}(payload):\n    return payload\n\n\n"
        ));
    }
    source
}
