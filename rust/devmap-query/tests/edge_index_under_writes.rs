//! The per-generation edge index under concurrent readers and a moving generation.
//!
//! The index exists because materialising the whole generation per question was
//! the cost of every graph query. Caching it introduces the failure a cache
//! always introduces: serving a generation that no longer exists. The cache is
//! keyed by generation id so that cannot happen by construction — this file is
//! the evidence, not the argument.
//!
//! Each generation is made *self-identifying*: every caller of `helper` in
//! generation `g` is named `caller_g{g}_{n}`. An answer that mixed two
//! generations would therefore contain two different markers, which is a thing
//! a test can see. "Internally consistent" is otherwise unfalsifiable.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use devmap_extract::extract_file;
use devmap_query::{Request, StoreQueryEngine};
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};

/// Callers per generation. Enough that a walk is real work and an index that
/// served the wrong generation would be visible, small enough that twenty
/// rewrites finish in seconds.
const CALLERS: usize = 60;

/// Generations the writer commits while the readers run.
const GENERATIONS: usize = 20;

/// Concurrent readers.
const READERS: usize = 100;

/// No individual answer may take this long. A deadlock shows up as a hang;
/// this turns a hang into a failure with a number attached.
const ANSWER_BUDGET: Duration = Duration::from_secs(20);

fn commit(store: &Store, generation: usize) {
    let mut sources = vec![(
        "core.py".to_string(),
        "def helper(rows):\n    return sum(rows)\n".to_string(),
    )];
    for index in 0..CALLERS {
        sources.push((
            format!("mod{index}.py"),
            format!(
                "from core import helper\n\n\ndef caller_g{generation}_{index}(rows):\n    \
                 return helper(rows)\n"
            ),
        ));
    }
    let extractions: Vec<_> = sources
        .iter()
        .map(|(path, body)| extract_file(path, body))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let analysis = devmap_analyze::analyze(&extractions, &resolution);
    store
        .save_generation_with_opts(
            &extractions,
            &resolution,
            &analysis,
            GenerationWriteOpts::default(),
        )
        .expect("generation");
}

/// The generation marker every caller in an answer carries, or `None` when the
/// answer names no caller at all.
fn markers(response: &devmap_query::Response<devmap_resolve::model::ResolvedEdge>) -> Vec<String> {
    let mut found: Vec<String> = response
        .items
        .iter()
        .filter_map(|edge| {
            let symbol = edge.source_symbol.rsplit("::").next()?;
            let rest = symbol.strip_prefix("caller_g")?;
            let (generation, _) = rest.split_once('_')?;
            Some(generation.to_string())
        })
        .collect();
    found.sort();
    found.dedup();
    found
}

/// 100 readers, 20 generation rewrites, one store: every answer comes from one
/// generation, and none of them hangs or panics.
#[test]
fn concurrent_impact_never_mixes_two_generations() {
    let dir = std::env::temp_dir().join(format!(
        "devmap-edgeindex-{}-{}",
        std::process::id(),
        Instant::now().elapsed().as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("fixture dir");
    let db = dir.join("index.sqlite");

    let store = Arc::new(Store::open(&db).expect("store"));
    commit(&store, 0);

    let stop = Arc::new(AtomicBool::new(false));
    let mixed = Arc::new(AtomicUsize::new(0));
    let answered = Arc::new(AtomicUsize::new(0));
    let unavailable = Arc::new(AtomicUsize::new(0));
    // Readers must be running *before* the writer starts, or the interleaving
    // this test is about never happens and it passes on a quiet store.
    let start = Arc::new(Barrier::new(READERS + 1));

    let mut readers = Vec::new();
    for _ in 0..READERS {
        let store = Arc::clone(&store);
        let stop = Arc::clone(&stop);
        let mixed = Arc::clone(&mixed);
        let answered = Arc::clone(&answered);
        let unavailable = Arc::clone(&unavailable);
        let start = Arc::clone(&start);
        readers.push(std::thread::spawn(move || {
            start.wait();
            while !stop.load(Ordering::Relaxed) {
                let engine = StoreQueryEngine::new(&store);
                let began = Instant::now();
                let response = engine
                    .impact(Request {
                        query: "helper".to_string(),
                        token_budget: 100_000,
                        min_confidence: 0.0,
                        max_depth: 3,
                    })
                    .expect("impact must answer or refuse, never panic");
                let elapsed = began.elapsed();
                assert!(
                    elapsed < ANSWER_BUDGET,
                    "one impact took {elapsed:?} while the generation moved"
                );

                match &response.resolution {
                    devmap_query::ResolutionAvailability::Unavailable { .. } => {
                        unavailable.fetch_add(1, Ordering::Relaxed);
                    }
                    _ => {
                        answered.fetch_add(1, Ordering::Relaxed);
                        // The counters must describe the list that was
                        // returned, whichever generation produced it.
                        assert_eq!(response.shown as usize, response.items.len());
                        assert_eq!(response.total, response.shown + response.hidden);
                        assert_eq!(response.truncated, response.hidden > 0);
                        if markers(&response).len() > 1 {
                            mixed.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            }
        }));
    }

    start.wait();
    for generation in 1..=GENERATIONS {
        commit(&store, generation);
    }
    stop.store(true, Ordering::Relaxed);
    for reader in readers {
        reader.join().expect("a reader panicked");
    }

    assert_eq!(
        mixed.load(Ordering::Relaxed),
        0,
        "an answer named callers from two generations at once"
    );
    let answered = answered.load(Ordering::Relaxed);
    assert!(
        answered > 0,
        "no reader got an answer ({} unavailable); the assertions above held \
         vacuously",
        unavailable.load(Ordering::Relaxed)
    );

    // And after the writer stops, the index must have moved with it: a cache
    // keyed by anything weaker than the generation id would still be serving
    // generation 0.
    let engine = StoreQueryEngine::new(&store);
    let response = engine
        .impact(Request {
            query: "helper".to_string(),
            token_budget: 100_000,
            min_confidence: 0.0,
            max_depth: 3,
        })
        .expect("impact");
    assert_eq!(
        markers(&response),
        vec![GENERATIONS.to_string()],
        "after {GENERATIONS} rewrites the index still answers from an older \
         generation"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A generation committed between two calls is visible to the second one.
///
/// The narrow version of the test above, without threads, because "the cache
/// went stale" and "two threads raced" are different failures and a single
/// concurrent test cannot tell them apart.
#[test]
fn a_new_generation_is_visible_to_the_next_query() {
    let dir = std::env::temp_dir().join(format!(
        "devmap-edgeindex-serial-{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("fixture dir");
    let db = dir.join("index.sqlite");
    let store = Store::open(&db).expect("store");

    for generation in 0..4 {
        commit(&store, generation);
        let engine = StoreQueryEngine::new(&store);
        let response = engine
            .impact(Request {
                query: "helper".to_string(),
                token_budget: 100_000,
                min_confidence: 0.0,
                max_depth: 3,
            })
            .expect("impact");
        assert_eq!(
            markers(&response),
            vec![generation.to_string()],
            "generation {generation} was committed and the answer came from an \
             older one"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}
