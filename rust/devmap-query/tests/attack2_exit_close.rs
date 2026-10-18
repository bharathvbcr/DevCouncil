//! Closing attack (attack2) on ft-b79f84e2: `StoreQueryEngine::exit`.
//!
//! 1. `exit_is_the_whole_body` -- the guard accepts `body.contains("self.exit(")`,
//!    so a method with a refusal returned BEFORE its `self.exit(` still passes
//!    it. This asserts what the guard only implies: the exit is the entire body.
//! 2. `default_rung_is_the_unnarrowed_walk` -- `impact`/`trace` now delegate to
//!    `_at_rung(.., None)`; the two must agree on every field but the verdict.
//! 3. `an_answer_never_carries_another_generations_verified_verdict` -- the exit
//!    reads the verdict AFTER the answer is built. A writer lands generations
//!    while a reader queries and a status loop refreshes the reader handle's
//!    verdict cache (what the daemon's tick does); no answer built from
//!    generation N may carry a VERIFIED verdict that names generation M != N.

use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use devmap_analyze::analyze;
use devmap_extract::extract_tree_with_report;
use devmap_query::{Request, Rung, StoreQueryEngine};
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};

use super::refusal_freshness_attack::fixture;

const ENGINE: &str = include_str!("../src/engine.rs");

fn req(query: &str, max_depth: usize) -> Request<String> {
    Request {
        query: query.to_string(),
        token_budget: 8_000,
        min_confidence: 0.0,
        max_depth,
    }
}

/// Names of public methods whose body does not open with `self.exit(` and is not
/// a bare one-expression delegate, for every method that returns an answer type.
#[test]
fn exit_is_the_whole_body() {
    let start = ENGINE.find("impl<'a> StoreQueryEngine<'a> {").unwrap();
    let text = &ENGINE[start..];
    let mut offenders = Vec::new();
    let mut checked = 0;
    let mut at = 0;
    while let Some(rel) = text[at..].find("\n    pub fn ") {
        let sig = at + rel + "\n    pub fn ".len();
        if text[..sig].contains("\n}\n") {
            break;
        }
        let name: String = text[sig..]
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        let Some(arrow) = text[sig..].find(") -> ").map(|i| sig + i + 5) else {
            at = sig;
            continue;
        };
        let Some(open) = text[arrow..].find("{\n").map(|i| arrow + i) else {
            at = sig;
            continue;
        };
        let returns = text[arrow..open].to_string();
        let mut depth = 0usize;
        let mut end = open;
        for (i, c) in text[open..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = open + i;
                        break;
                    }
                }
                _ => {}
            }
        }
        let body = text[open + 1..end].trim();
        let answers = [
            "Response<", "LayeredImpact", "Vec<Neighbors>", "ExploreReport",
            "AffectedTestsReport", "CloneReport", "SkeletonReport", "EvidencePack",
            "PreviewReport",
        ]
        .iter()
        .any(|t| returns.contains(t));
        if answers {
            checked += 1;
            let opens_with_exit = body.starts_with("self.exit(");
            let closes_exit = body.ends_with("})");
            let delegate = body.starts_with("self.") && !body.contains(';');
            if !(opens_with_exit && closes_exit) && !delegate {
                offenders.push(name);
            }
        }
        at = end;
    }
    assert!(checked >= 20, "scan found only {checked}");
    assert!(
        offenders.is_empty(),
        "public answer methods whose body is more than `self.exit(|| ..)`: {offenders:?}"
    );
}

#[test]
fn default_rung_is_the_unnarrowed_walk() {
    let fx = fixture("attack2-default-rung", None);
    let engine = StoreQueryEngine::new(&fx.store);
    for q in ["a", "b", "d", "chain.py", "test_chain.py", "nope_unknown"] {
        for depth in [1usize, 2, 5] {
            let pairs = [
                (
                    engine.impact(req(q, depth)).unwrap(),
                    engine.impact_at_rung(req(q, depth), None).unwrap(),
                    "impact",
                ),
                (
                    engine.trace(req(q, depth)).unwrap(),
                    engine.trace_at_rung(req(q, depth), None).unwrap(),
                    "trace",
                ),
            ];
            for (plain, rung, what) in pairs {
                let (a, b) = (
                    serde_json::to_value(&plain).unwrap(),
                    serde_json::to_value(&rung).unwrap(),
                );
                assert_eq!(a, b, "{what}({q:?}, {depth}) differs by rung=None");
                assert!(
                    plain.rungs.is_some() || plain.items.is_empty(),
                    "{what}({q:?}) lost its rung summary"
                );
            }
            // A named rung must still narrow: it may only drop items.
            let narrowed = engine
                .impact_at_rung(req(q, depth), Some(Rung::Deterministic))
                .unwrap();
            let all = engine.impact(req(q, depth)).unwrap();
            assert!(narrowed.items.len() <= all.items.len(), "{q:?}");
        }
    }
}

const CHAIN: &str = "def a():\n    return b()\n\n\ndef b():\n    return c()\n\n\ndef c():\n    return d()\n\n\ndef d():\n    return 1\n";

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

#[test]
fn an_answer_never_carries_another_generations_verified_verdict() {
    let db = std::env::temp_dir().join(format!("attack2-race-{}.sqlite", std::process::id()));
    let _ = fs::remove_file(&db);
    let fx = fixture("attack2-race", Some(&db));
    let reader = Store::open(&db).unwrap();
    reader.status("t").unwrap();
    let writer = Store::open(&db).unwrap();
    let stop = AtomicBool::new(false);
    // (callers in the answer == generation it was built from, verdict gen, fresh)
    let seen: Mutex<Vec<(usize, Option<u32>, Option<bool>)>> = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        scope.spawn(|| {
            while !stop.load(Ordering::Relaxed) {
                let _ = reader.status("t");
            }
        });
        scope.spawn(|| {
            let engine = StoreQueryEngine::new(&reader);
            while !stop.load(Ordering::Relaxed) {
                if let Ok(r) = engine.impact(req("d", 1)) {
                    seen.lock()
                        .unwrap()
                        .push((r.items.len(), r.source_freshness.generation_id, r.source_freshness.fresh));
                }
            }
        });
        for generation in 2..=40 {
            land_generation(&fx.root, &writer, generation);
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
        stop.store(true, Ordering::Relaxed);
    });
    let seen = seen.into_inner().unwrap();
    let mismatched: Vec<_> = seen
        .iter()
        .filter(|(callers, verdict_gen, _)| verdict_gen.map(|g| g as usize) != Some(*callers))
        .collect();
    let verified_wrong: Vec<_> = mismatched
        .iter()
        .filter(|(_, _, fresh)| fresh.is_some())
        .collect();
    eprintln!(
        "answers={} mismatched_generation={} of which VERIFIED={}: {:?}",
        seen.len(),
        mismatched.len(),
        verified_wrong.len(),
        &verified_wrong[..verified_wrong.len().min(8)]
    );
    assert!(seen.len() > 50, "reader barely ran: {}", seen.len());
    assert!(
        verified_wrong.is_empty(),
        "{} answers carried generation N's items with a verified verdict naming another \
         generation: {:?}",
        verified_wrong.len(),
        &verified_wrong[..verified_wrong.len().min(8)]
    );
}

/// Deterministic form of the race above: a hook lands generations 2 and 3 and
/// refreshes the verdict cache after the answer was read from generation 1 and
/// before `exit` reads the verdict. No timing involved.
#[test]
fn a_verdict_for_another_generation_is_never_attached_as_verified() {
    let db = std::env::temp_dir().join(format!("attack2-pin-{}.sqlite", std::process::id()));
    let _ = fs::remove_file(&db);
    let fx = fixture("attack2-pin", Some(&db));
    let reader = Store::open(&db).unwrap();
    reader.status("t").unwrap();
    let writer = Store::open(&db).unwrap();
    let moved = std::cell::Cell::new(false);
    let engine = StoreQueryEngine::new(&reader).with_interleave_hook(|| {
        for generation in 2..=3 {
            land_generation(&fx.root, &writer, generation);
        }
        reader.status("t").unwrap();
        moved.set(true);
    });
    let answer = engine.impact(req("d", 1)).unwrap();
    assert!(moved.get(), "the interleave hook never ran");
    assert_eq!(
        reader.latest_generation_id().unwrap(),
        Some(3),
        "the hook did not land the generations"
    );
    let verdict = &answer.source_freshness;
    assert_eq!(verdict.fresh, None, "claimed {verdict:?} for a generation not read");
    assert_eq!(verdict.generation_id, None, "{verdict:?}");
    let reason = verdict.reason.as_deref().unwrap_or_default();
    assert!(reason.contains("index moved from generation 1 to 3"), "{reason}");
}

/// The guard above must not degrade every answer: when no generation lands
/// during the query, the verified verdict for the generation read is kept.
#[test]
fn a_verdict_for_the_generation_read_stays_verified() {
    let db = std::env::temp_dir().join(format!("attack2-same-{}.sqlite", std::process::id()));
    let _ = fs::remove_file(&db);
    let fx = fixture("attack2-same", Some(&db));
    let reader = Store::open(&db).unwrap();
    reader.status("t").unwrap();
    let hooked = std::cell::Cell::new(false);
    let engine = StoreQueryEngine::new(&reader).with_interleave_hook(|| {
        reader.status("t").unwrap();
        hooked.set(true);
    });
    let answer = engine.impact(req("d", 1)).unwrap();
    assert!(hooked.get());
    let _ = &fx;
    let verdict = &answer.source_freshness;
    assert!(verdict.fresh.is_some(), "matching generations lost their verdict: {verdict:?}");
    assert_eq!(verdict.generation_id, reader.latest_generation_id().unwrap());
}
