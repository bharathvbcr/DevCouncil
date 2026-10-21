//! Adversarial probes for the build cancellation in `extract_scanned_for_generation`.

use devmap_extract::{detect_language, extract_file, BuildCancelled, Extraction, ScannedTree};
use devmap_extract::cache::CacheKey;
use devmap_store::{extract_scanned_for_generation, Store};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

fn corpus(n: usize) -> ScannedTree {
    let mut scanned = ScannedTree::default();
    for i in 0..n {
        let next = (i + 1) % n;
        scanned.sources.push((
            format!("src/m{i}.py"),
            format!(
                "from src.m{next} import f{next}\n\ndef f{i}():\n    return f{next}()\n\n\
                 class C{i}:\n    def m(self, x):\n        return [f{next}() for _ in range(x)]\n"
            ),
        ));
    }
    scanned
}

fn xorshift(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

/// 200 cancels at random points of the rayon pass. Each run must end in a
/// complete, unflagged result or `BuildCancelled`; never a panic, a hang, a
/// short vector, or a cancelled file recorded as a refusal. Nothing may be
/// admitted to the payload cache by a cancelled (or any) generation build.
#[test]
fn cancel_at_a_random_point_never_yields_a_partial_or_poisoned_result() {
    let scanned = corpus(600);
    let baseline: Vec<Extraction> = scanned
        .sources
        .iter()
        .map(|(p, s)| extract_file(p, s))
        .collect();
    let baseline_json: Vec<String> = baseline
        .iter()
        .map(|e| serde_json::to_string(e).unwrap())
        .collect();
    let (tx, rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let store = Store::open_in_memory().unwrap();
        let mut rng = 0x2545F4914F6CDD1Du64;
        let (mut cancelled, mut completed) = (0, 0);
        for round in 0..200 {
            let flag = AtomicBool::new(false);
            let delay_us = xorshift(&mut rng) % 40_000;
            let outcome = std::thread::scope(|scope| {
                scope.spawn(|| {
                    spin_for(delay_us);
                    flag.store(true, Ordering::SeqCst);
                });
                extract_scanned_for_generation(&store, &scanned, None, Some(&flag))
            });
            match outcome {
                Ok(extractions) => {
                    completed += 1;
                    assert_eq!(extractions.len(), scanned.sources.len(), "round {round}");
                    for (i, e) in extractions.iter().enumerate() {
                        assert!(
                            !e.is_parse_failure() && !e.is_budget_refusal(),
                            "round {round}: {} came back refused/failed",
                            e.file_path
                        );
                        // Cache admission is at generation commit, so no row
                        // exists and the output must equal the oracle exactly
                        // (the path-bound `source_code` is stripped only by the
                        // cache, which is cold here).
                        assert_eq!(
                            serde_json::to_string(e).unwrap(),
                            baseline_json[i],
                            "round {round} file {i}"
                        );
                    }
                }
                Err(error) => {
                    cancelled += 1;
                    assert!(error.is::<BuildCancelled>(), "round {round}: {error:#}");
                }
            }
        }
        // No cancelled or completed run admitted anything.
        for (path, src) in scanned.sources.iter().take(50) {
            let key = CacheKey::for_source(detect_language(std::path::Path::new(path)), src);
            assert!(
                store.try_get_cached_extraction(&key).unwrap().is_none(),
                "{path} was admitted to the payload cache by a generation-bound build"
            );
        }
        assert_eq!(store.latest_generation_id().unwrap(), None);
        tx.send((cancelled, completed)).unwrap();
    });
    let (cancelled, completed) = rx
        .recv_timeout(Duration::from_secs(600))
        .expect("a cancel-at-random-point loop hung or panicked");
    worker.join().unwrap();
    eprintln!("cancelled={cancelled} completed={completed}");
    assert!(cancelled > 0, "the loop never landed a cancel mid-pass");
}

fn spin_for(us: u64) {
    let end = std::time::Instant::now() + Duration::from_micros(us);
    while std::time::Instant::now() < end {
        std::hint::spin_loop();
    }
}

/// An empty tree with the flag already tripped: nothing to cancel, so Ok(empty)
/// (and in particular no panic). The CLI's own post-check turns it into a
/// cancel.
#[test]
fn a_tripped_flag_over_an_empty_tree_is_a_clean_empty_result() {
    let store = Store::open_in_memory().unwrap();
    let flag = AtomicBool::new(true);
    let out = extract_scanned_for_generation(&store, &ScannedTree::default(), None, Some(&flag));
    assert!(out.unwrap().is_empty());
}

/// Progress counters of a cancelled build must not claim files as extracted
/// that were never extracted.
#[test]
fn a_cancelled_build_does_not_report_unextracted_files_as_completed() {
    let store = Store::open_in_memory().unwrap();
    let scanned = corpus(50);
    let flag = AtomicBool::new(true);
    let progress = devmap_extract::progress::FileProgress::default();
    let out = extract_scanned_for_generation(&store, &scanned, Some(&progress), Some(&flag));
    assert!(out.is_err());
    let snap = progress.snapshot();
    assert_eq!(
        snap.completed, 0,
        "a pre-cancelled build reports {} of {} files completed: {snap:?}",
        snap.completed,
        scanned.sources.len()
    );
}
