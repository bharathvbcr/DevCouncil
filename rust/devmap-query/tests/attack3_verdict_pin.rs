//! attack/verdict-pin probes on 7c732848: `StoreQueryEngine::exit` pins the
//! freshness verdict to `latest_generation_id()` read before the answer.

use std::fs;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

use devmap_analyze::analyze;
use devmap_extract::extract_tree_with_report;
use devmap_query::{Request, StoreQueryEngine};
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};

use super::refusal_freshness_attack::fixture;

fn req(query: &str, max_depth: usize) -> Request<String> {
    Request {
        query: query.to_string(),
        token_budget: 8_000,
        min_confidence: 0.0,
        max_depth,
    }
}

const CHAIN: &str = "def a():\n    return b()\n\n\ndef b():\n    return c()\n\n\ndef c():\n    return d()\n\n\ndef d():\n    return 1\n";

/// Generation `n` has `n` callers of `d` (c plus n-1 extras).
fn land_generation(root: &std::path::Path, store: &Store, generation: usize) {
    let mut source = CHAIN.to_string();
    for i in 1..generation {
        source.push_str(&format!("\n\ndef extra_{i}():\n    return d()\n"));
    }
    fs::write(root.join("chain.py"), source).unwrap();
    let (files, _) = extract_tree_with_report(root).unwrap();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&files);
    let resolution = resolver.resolve_all(&files).unwrap();
    let analysis = analyze(&files, &resolution);
    store
        .save_generation_with_opts(
            &files,
            &resolution,
            &analysis,
            GenerationWriteOpts {
                repo_root: Some(root.to_string_lossy().into_owned()),
                ..GenerationWriteOpts::default()
            },
        )
        .unwrap();
}

/// Probe 1: the pin's soundness rests on generation ids never decreasing or
/// being reused. The answer reads generation 2 (2 callers). The hook rewinds the
/// sequence (what a restore does) and lands a different generation that gets id
/// 2 again, then refreshes the verdict cache. `before == 2` and the verdict
/// names 2, so a verified verdict is kept for content that no longer exists.
#[test]
#[ignore = "requires in-place generation table wipe; tracked on ft-c13da1b7 (store no longer matches)"]
fn a_reused_generation_id_does_not_keep_a_verified_verdict() {
    let db = std::env::temp_dir().join(format!("attack3-reuse-{}.sqlite", std::process::id()));
    let _ = fs::remove_file(&db);
    let fx = fixture("attack3-reuse", Some(&db));
    let reader = Store::open(&db).unwrap();
    let writer = Store::open(&db).unwrap();
    reader.status("t").unwrap();
    assert_eq!(reader.latest_generation_id().unwrap(), Some(1));
    // The hook must be `Send + Sync`; a bare connection is not `Sync`.
    let raw = Mutex::new(rusqlite::Connection::open(&db).unwrap());
    let engine = StoreQueryEngine::new(&reader).with_interleave_hook(|| {
        let raw = raw.lock().unwrap();
        let tables: Vec<String> = {
            let mut stmt = raw
                .prepare("SELECT name FROM sqlite_master WHERE type='table'")
                .unwrap();
            let rows = stmt.query_map([], |r| r.get::<_, String>(0)).unwrap();
            rows.map(|r| r.unwrap()).collect()
        };
        raw.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
        for t in tables {
            let keep = t.starts_with("sqlite_")
                || t.starts_with("nodes_fts") && t != "nodes_fts_map"
                || t == "paths"
                || t == "reader_compat"
                || t == "pending_state"
                || t == "build_history"
                || t == "meta";
            if !keep {
                if let Err(e) = raw.execute(&format!("DELETE FROM \"{t}\""), []) {
                    eprintln!("wipe {t}: {e}");
                }
            }
        }
        raw.execute("DELETE FROM nodes_fts", []).unwrap();
        raw.execute("DELETE FROM sqlite_sequence WHERE name = 'generations'", [])
            .unwrap();
        land_generation(&fx.root, &writer, 5);
        reader.status("t").unwrap();
    });
    let answer = engine.impact(req("d", 1)).unwrap();
    assert_eq!(
        reader.latest_generation_id().unwrap(),
        Some(1),
        "id was not reused"
    );
    let verdict = &answer.source_freshness;
    assert_eq!(
        verdict.fresh,
        None,
        "verified verdict {verdict:?} kept for an answer read from the pre-wipe generation 1 \
         ({} caller(s); id 1 now names a 5-caller generation)",
        answer.items.len(),
    );
}

/// Probe 3: an empty store (no generation) answers unverified with no id, and
/// an interleave hook that does nothing changes nothing.
#[test]
fn an_empty_store_answer_is_unverified_with_no_generation() {
    let store = Store::open_in_memory().unwrap();
    let engine = StoreQueryEngine::new(&store).with_interleave_hook(|| {});
    let answer = engine.impact(req("d", 1)).unwrap();
    let v = &answer.source_freshness;
    assert_eq!((v.fresh, v.generation_id), (None, None), "{v:?}");
}

mod autotraits {
    use std::marker::PhantomData;
    pub struct W<T>(PhantomData<T>);
    // Inherent, so `W::<T>::IS_SEND` exists only when `T: Send`. A blanket
    // trait const of `false` was never consulted by that path.
    impl<T: Send> W<T> {
        pub const IS_SEND: bool = true;
    }
    impl<T: Sync> W<T> {
        pub const IS_SYNC: bool = true;
    }
}

/// Probe 4: the hook is `Box<dyn Fn() + 'a>`, neither Send nor Sync, so the
/// public `StoreQueryEngine` loses both auto traits it held before.
#[test]
fn the_engine_is_still_send_and_sync() {
    use autotraits::*;
    let send = W::<StoreQueryEngine<'static>>::IS_SEND;
    let sync = W::<StoreQueryEngine<'static>>::IS_SYNC;
    assert!(send && sync, "StoreQueryEngine: Send={send} Sync={sync}");
}

/// Probe 5: 8 readers on one handle, a fast writer, a status-refresh loop.
#[test]
fn eight_readers_never_get_a_mis_verified_answer() {
    const PER_READER: usize = 700;
    let db = std::env::temp_dir().join(format!("attack3-hard-{}.sqlite", std::process::id()));
    let _ = fs::remove_file(&db);
    let fx = fixture("attack3-hard", Some(&db));
    let reader = Store::open(&db).unwrap();
    reader.status("t").unwrap();
    let writer = Store::open(&db).unwrap();
    let stop = AtomicBool::new(false);
    let total = AtomicUsize::new(0);
    let verified = AtomicUsize::new(0);
    let moved = AtomicUsize::new(0);
    let wrong: Mutex<Vec<(usize, Option<u32>)>> = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        scope.spawn(|| {
            while !stop.load(Ordering::Relaxed) {
                let _ = reader.status("t");
            }
        });
        let readers: Vec<_> = (0..8)
            .map(|_| {
                scope.spawn(|| {
                    let engine = StoreQueryEngine::new(&reader);
                    for _ in 0..PER_READER {
                        let Ok(r) = engine.impact(req("d", 1)) else {
                            continue;
                        };
                        total.fetch_add(1, Ordering::Relaxed);
                        let v = &r.source_freshness;
                        if v.fresh.is_some() {
                            verified.fetch_add(1, Ordering::Relaxed);
                            if v.generation_id.map(|g| g as usize) != Some(r.items.len()) {
                                wrong.lock().unwrap().push((r.items.len(), v.generation_id));
                            }
                        } else if v.reason.as_deref().unwrap_or("").contains("index moved") {
                            moved.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                })
            })
            .collect();
        scope.spawn(|| {
            let mut generation = 2;
            while !stop.load(Ordering::Relaxed) && generation <= 400 {
                land_generation(&fx.root, &writer, generation);
                generation += 1;
            }
        });
        for h in readers {
            h.join().unwrap();
        }
        stop.store(true, Ordering::Relaxed);
    });
    let wrong = wrong.into_inner().unwrap();
    eprintln!(
        "queries={} verified={} index-moved={} mis-verified={}",
        total.load(Ordering::Relaxed),
        verified.load(Ordering::Relaxed),
        moved.load(Ordering::Relaxed),
        wrong.len()
    );
    assert!(
        total.load(Ordering::Relaxed) >= 5000,
        "only {} queries",
        total.load(Ordering::Relaxed)
    );
    assert!(
        wrong.is_empty(),
        "{} mis-verified: {:?}",
        wrong.len(),
        &wrong[..wrong.len().min(8)]
    );
}

/// Probe 6: the reason names both ids and is short and control-free.
#[test]
fn the_moved_reason_names_both_ids_and_stays_short() {
    let db = std::env::temp_dir().join(format!("attack3-word-{}.sqlite", std::process::id()));
    let _ = fs::remove_file(&db);
    let fx = fixture("attack3-word", Some(&db));
    let reader = Store::open(&db).unwrap();
    reader.status("t").unwrap();
    let writer = Store::open(&db).unwrap();
    let engine = StoreQueryEngine::new(&reader).with_interleave_hook(|| {
        land_generation(&fx.root, &writer, 2);
        reader.status("t").unwrap();
    });
    let answer = engine.impact(req("d", 1)).unwrap();
    let reason = answer.source_freshness.reason.clone().unwrap_or_default();
    assert!(
        reason.contains("generation 1") && reason.contains("to 2"),
        "{reason}"
    );
    assert!(
        reason.len() < 200 && !reason.chars().any(|c| c.is_control()),
        "{reason}"
    );
}
