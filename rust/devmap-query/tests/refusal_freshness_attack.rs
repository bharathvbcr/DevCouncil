//! Adversarial probes for the refusal-freshness change (ft-b79f84e2).
//!
//! The invariant under attack: on a store that holds a generation, EVERY
//! `Response` a `StoreQueryEngine` method returns -- success or refusal, nested
//! or top level -- carries exactly the verdict `Store::query_source_freshness`
//! reports right now. `finish` is that verdict, so a refusal that says anything
//! else (the canned "no persisted generation", or a bare "not checked" while
//! the store holds a real verdict) is a path that bypassed `finish`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use devmap_analyze::analyze;
use devmap_extract::extract_tree_with_report;
use devmap_query::{QueryEngine, Request, Rung, SourceFreshness, StoreQueryEngine};
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};

const NO_GENERATION: &str = "no persisted generation";

const CHAIN: &str = "def a():\n    return b()\n\n\ndef b():\n    return c()\n\n\ndef c():\n    return d()\n\n\ndef d():\n    return 1\n";
const TESTS: &str = "from chain import a\n\n\ndef test_a():\n    assert a() == 1\n";
const DUP1: &str = "def twin_one(x):\n    total = 0\n    for item in x:\n        if item > 3:\n            total += item * 2\n        else:\n            total -= 1\n    return total\n";
const DUP2: &str = "def twin_two(y):\n    acc = 0\n    for thing in y:\n        if thing > 3:\n            acc += thing * 2\n        else:\n            acc -= 1\n    return acc\n";
const LITS: &str = "NEEDLE = \"needle-literal-xyz\"\n\n\ndef uses():\n    return NEEDLE\n";

fn tmp_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "devmap-refusal-fresh-{label}-{}-{seq}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

fn git(root: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

struct Fixture {
    root: PathBuf,
    store: Store,
}

fn fixture(label: &str, db: Option<&Path>) -> Fixture {
    let root = tmp_dir(label);
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "a@example.invalid"]);
    git(&root, &["config", "user.name", "attack"]);
    for (name, body) in [
        ("chain.py", CHAIN),
        ("test_chain.py", TESTS),
        ("dup1.py", DUP1),
        ("dup2.py", DUP2),
        ("lits.py", LITS),
    ] {
        fs::write(root.join(name), body).unwrap();
    }
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-q", "-m", "one"]);
    let (files, _report) = extract_tree_with_report(&root).unwrap();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&files);
    let resolution = resolver.resolve_all(&files).unwrap();
    let analysis = analyze(&files, &resolution);
    let store = match db {
        Some(path) => Store::open(path).unwrap(),
        None => Store::open_in_memory().unwrap(),
    };
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
    Fixture { root, store }
}

fn req(query: &str, max_depth: usize) -> Request<String> {
    Request {
        query: query.to_string(),
        token_budget: 8_000,
        min_confidence: 0.0,
        max_depth,
    }
}

fn pair(from: &str, to: &str, max_depth: usize) -> Request<(String, String)> {
    Request {
        query: (from.to_string(), to.to_string()),
        token_budget: 8_000,
        min_confidence: 0.0,
        max_depth,
    }
}

fn s(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| v.to_string()).collect()
}

/// One observed `source_freshness`, named by the call and the arm that made it.
type Seen = Vec<(String, SourceFreshness)>;

/// Drive every `StoreQueryEngine` method that yields a `Response`, on a success
/// path and on every refusal path reachable from outside, and record every
/// freshness each one carries (including nested halves).
fn sweep(engine: &StoreQueryEngine) -> (Seen, Vec<String>) {
    let mut seen: Seen = Vec::new();
    let mut errored: Vec<String> = Vec::new();
    macro_rules! rec {
        ($name:expr, $fresh:expr) => {
            seen.push(($name.to_string(), $fresh.clone()))
        };
    }
    macro_rules! resp {
        ($name:expr, $call:expr) => {
            match $call {
                Ok(r) => rec!($name, r.source_freshness),
                Err(e) => errored.push(format!("{}: {e}", $name)),
            }
        };
    }

    for q in ["chain", "zzzqqq_nothing", "   "] {
        resp!(format!("search({q:?})"), engine.search(req(q, 3)));
    }
    let bad_filter =
        devmap_query::scope::NameQueryFilter::new(&s(&["no/such/dir"]), &[], &[]).unwrap();
    if let Some(filter) = bad_filter.as_ref() {
        resp!(
            "search_filtered(unknown prefix)",
            engine.search_filtered(req("chain", 3), Some(filter))
        );
    }

    for q in ["a", "chain.py", "nope_unknown", "test_chain.py"] {
        resp!(format!("dependencies({q:?})"), engine.dependencies(req(q, 3)));
        resp!(
            format!("dependencies_at_rung({q:?})"),
            engine.dependencies_at_rung(req(q, 3), Some(Rung::Deterministic))
        );
        resp!(format!("impact({q:?})"), engine.impact(req(q, 3)));
        resp!(
            format!("impact_at_rung({q:?})"),
            engine.impact_at_rung(req(q, 3), Some(Rung::Deterministic))
        );
        resp!(format!("trace({q:?})"), engine.trace(req(q, 3)));
        resp!(
            format!("trace_at_rung({q:?})"),
            engine.trace_at_rung(req(q, 3), Some(Rung::Deterministic))
        );
        match engine.impact_layered(req(q, 3)) {
            Ok(l) => {
                rec!(format!("impact_layered({q:?}).edges"), l.edges.source_freshness);
                rec!(
                    format!("impact_layered({q:?}).layers"),
                    l.blast_radius.layers.source_freshness
                );
            }
            Err(e) => errored.push(format!("impact_layered({q:?}): {e}")),
        }
    }
    for (from, to, depth) in [
        ("a", "d", 5),
        ("a", "d", 1),
        ("d", "a", 5),
        ("a", "a", 5),
        ("", "d", 5),
        ("a", "nope_unknown", 5),
        ("nope_unknown", "d", 5),
    ] {
        resp!(
            format!("trace_between({from:?},{to:?},{depth})"),
            engine.trace_between(pair(from, to, depth))
        );
    }
    for targets in [vec!["a", "nope_unknown"], vec!["nope_unknown"], vec!["chain.py"]] {
        match engine.neighbors(&s(&targets), 4_000, 0.0, 2) {
            Ok(list) => {
                for n in list {
                    rec!(format!("neighbors({targets:?}).{}.callers", n.target), n.callers.source_freshness);
                    rec!(format!("neighbors({targets:?}).{}.callees", n.target), n.callees.source_freshness);
                }
            }
            Err(e) => errored.push(format!("neighbors({targets:?}): {e}")),
        }
        match engine.neighbors_at_rung(&s(&targets), 4_000, 0.0, 2, Some(Rung::Deterministic)) {
            Ok(list) => {
                for n in list {
                    rec!(format!("neighbors_at_rung({targets:?}).{}.callers", n.target), n.callers.source_freshness);
                    rec!(format!("neighbors_at_rung({targets:?}).{}.callees", n.target), n.callees.source_freshness);
                }
            }
            Err(e) => errored.push(format!("neighbors_at_rung({targets:?}): {e}")),
        }
    }
    for q in ["a", "nope_unknown", "  "] {
        match engine.explore(q, 5, 8_000, 0.0, 2) {
            Ok(r) => {
                rec!(format!("explore({q:?}).definitions"), r.definitions.source_freshness);
                rec!(format!("explore({q:?}).layers"), r.blast_radius.layers.source_freshness);
                for d in &r.definitions.items {
                    rec!(format!("explore({q:?}).{}.callers", d.symbol_name), d.callers.source_freshness);
                    rec!(format!("explore({q:?}).{}.callees", d.symbol_name), d.callees.source_freshness);
                }
            }
            Err(e) => errored.push(format!("explore({q:?}): {e}")),
        }
    }
    for targets in [vec!["chain.py"], vec!["d"], vec!["nope_unknown"], vec![]] {
        match engine.affected_tests(&s(&targets), 4_000, 0.0, 3) {
            Ok(r) => {
                rec!(format!("affected_tests({targets:?}).tests"), r.tests.source_freshness);
                rec!(format!("affected_tests({targets:?}).layers"), r.blast_radius.layers.source_freshness);
            }
            Err(e) => errored.push(format!("affected_tests({targets:?}): {e}")),
        }
    }
    resp!("dead_symbols", engine.dead_symbols(4_000));
    match engine.clones(4_000, None, 3) {
        Ok(r) => rec!("clones.groups", r.groups.source_freshness),
        Err(e) => errored.push(format!("clones: {e}")),
    }
    for q in ["chain", "zzzqqq_nothing", "   "] {
        resp!(format!("search_semantic({q:?})"), engine.search_semantic(q, 4_000));
        resp!(format!("ask({q:?})"), engine.ask(q, 4_000, 0.0));
        match engine.ask_evidence(q, 4_000, 0.0) {
            Ok(p) => {
                rec!(format!("ask_evidence({q:?}).pack"), p.source_freshness);
                rec!(format!("ask_evidence({q:?}).related_tests"), p.related_tests.source_freshness);
            }
            Err(e) => errored.push(format!("ask_evidence({q:?}): {e}")),
        }
    }
    match engine.skeleton("chain.py", 4_000) {
        Ok(r) => rec!("skeleton(chain.py)", r.source_freshness),
        Err(e) => errored.push(format!("skeleton(chain.py): {e}")),
    }
    match engine.skeleton("not_there.py", 4_000) {
        Ok(r) => rec!("skeleton(not_there.py)", r.source_freshness),
        Err(e) => errored.push(format!("skeleton(not_there.py): {e}")),
    }
    match engine.preview("chain.py", "def a():\n    return 2\n", 4_000, 0.0) {
        Ok(r) => rec!("preview(chain.py).broken_callers", r.broken_callers.source_freshness),
        Err(e) => errored.push(format!("preview: {e}")),
    }
    match engine.preview("brand_new.py", "def n():\n    return 2\n", 4_000, 0.0) {
        Ok(r) => rec!("preview(new).broken_callers", r.broken_callers.source_freshness),
        Err(e) => errored.push(format!("preview(new): {e}")),
    }
    (seen, errored)
}

fn expected(store: &Store) -> SourceFreshness {
    SourceFreshness::from_store(store.query_source_freshness())
}

fn offenders(seen: &Seen, want: &SourceFreshness) -> Vec<String> {
    seen.iter()
        .filter(|(_, got)| got != want)
        .map(|(name, got)| format!("{name}: {got:?}"))
        .collect()
}

fn assert_all_match(seen: &Seen, want: &SourceFreshness, scenario: &str) {
    assert!(seen.len() > 60, "sweep too thin to mean anything: {}", seen.len());
    let bad = offenders(seen, want);
    assert!(
        bad.is_empty(),
        "[{scenario}] {} of {} responses carry a verdict other than the store's {want:?}:\n{}",
        bad.len(),
        seen.len(),
        bad.join("\n")
    );
}

// Probe 1: priming the cache with `status` gives every response a real verdict.
#[test]
fn p1_every_response_carries_the_stores_verified_verdict() {
    let fx = fixture("p1", None);
    let status = fx.store.status("t").unwrap();
    assert_eq!(status.source_freshness, Some(true), "{status:?}");
    let want = expected(&fx.store);
    assert_eq!(want.fresh, Some(true), "precondition: a real verdict {want:?}");
    let engine = StoreQueryEngine::new(&fx.store);
    let (seen, errored) = sweep(&engine);
    eprintln!("errored (no Response to inspect): {errored:#?}");
    assert_all_match(&seen, &want, "fresh");
    assert!(seen.iter().all(|(_, f)| !f.reason.clone().unwrap_or_default().contains(NO_GENERATION)));
}

// Probe 1b: with the cache never primed, "not checked" is the store's own
// verdict, and every response must agree with it (and none say "no generation").
#[test]
fn p1b_unprimed_store_every_response_says_not_checked_never_no_generation() {
    let fx = fixture("p1b", None);
    let want = expected(&fx.store);
    assert_eq!(want.fresh, None);
    assert!(want.reason.clone().unwrap().contains("not checked"), "{want:?}");
    let (seen, _) = sweep(&StoreQueryEngine::new(&fx.store));
    assert_all_match(&seen, &want, "unprimed");
}

// Probe 3: the tree changed after the build and status noticed.
#[test]
fn p3_stale_store_refusals_report_stale_like_successes() {
    let fx = fixture("p3", None);
    fx.store.status("t").unwrap();
    fs::write(fx.root.join("chain.py"), format!("{CHAIN}\n\ndef extra():\n    return 9\n")).unwrap();
    fs::write(fx.root.join("brand_new_file.py"), "def z():\n    return 0\n").unwrap();
    let status = fx.store.status("t").unwrap();
    assert_eq!(status.source_freshness, Some(false), "{status:?}");
    let want = expected(&fx.store);
    assert_eq!(want.fresh, Some(false), "precondition: stale verdict {want:?}");
    assert!(want.reason.is_some());
    let (seen, _) = sweep(&StoreQueryEngine::new(&fx.store));
    assert_all_match(&seen, &want, "stale");
    // And explicitly: no refusal reports "not checked" beside a stale store.
    for (name, f) in &seen {
        assert_eq!(f.fresh, Some(false), "{name} lost the stale verdict: {f:?}");
    }
}

// Probe 3b: a file edited AFTER status primed a fresh verdict. The cache is the
// documented contract (status is the walker), so success and refusal must at
// least agree with each other.
#[test]
fn p3b_edit_after_status_successes_and_refusals_agree() {
    let fx = fixture("p3b", None);
    fx.store.status("t").unwrap();
    fs::write(fx.root.join("chain.py"), format!("{CHAIN}\n# edit\n")).unwrap();
    let (seen, _) = sweep(&StoreQueryEngine::new(&fx.store));
    let want = expected(&fx.store);
    assert_all_match(&seen, &want, "edit-after-status");
}

// A newer generation lands after status primed generation 1: the cache now
// describes another generation. Every response, refusal included, must say so.
#[test]
fn p3c_newer_generation_than_the_cache_refusals_say_so_too() {
    let fx = fixture("p3c", None);
    fx.store.status("t").unwrap();
    let (files, _) = extract_tree_with_report(&fx.root).unwrap();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&files);
    let resolution = resolver.resolve_all(&files).unwrap();
    let analysis = analyze(&files, &resolution);
    fx.store
        .save_generation_with_opts(
            &files,
            &resolution,
            &analysis,
            GenerationWriteOpts {
                repo_root: Some(fx.root.to_string_lossy().into_owned()),
                ..GenerationWriteOpts::default()
            },
        )
        .unwrap();
    let want = expected(&fx.store);
    assert!(want.reason.clone().unwrap_or_default().contains("cached source freshness"), "{want:?}");
    let (seen, _) = sweep(&StoreQueryEngine::new(&fx.store));
    assert_all_match(&seen, &want, "cache-older-than-generation");
}

// Probe 2a: a brand-new store.
#[test]
fn p2a_empty_store_every_method_says_no_generation_and_never_fresh() {
    let store = Store::open_in_memory().unwrap();
    let (seen, errored) = sweep(&StoreQueryEngine::new(&store));
    eprintln!("errored on empty store: {errored:#?}");
    assert!(!seen.is_empty());
    let bad: Vec<String> = seen
        .iter()
        .filter(|(_, f)| {
            f.fresh == Some(true)
                || f.generation_id.is_some()
                || !f.reason.clone().unwrap_or_default().contains(NO_GENERATION)
        })
        .map(|(n, f)| format!("{n}: {f:?}"))
        .collect();
    assert!(
        bad.is_empty(),
        "empty store: {} of {} responses do not say 'no persisted generation':\n{}",
        bad.len(),
        seen.len(),
        bad.join("\n")
    );
}

// Probe 2b: the generation vanishes mid-session, after the cache held a fresh
// verdict for it and after the engine read it once.
#[test]
fn p2b_generation_deleted_mid_session_is_never_fresh() {
    let dir = tmp_dir("p2b-db");
    let db = dir.join("devmap.sqlite");
    let fx = fixture("p2b", Some(&db));
    fx.store.status("t").unwrap();
    let engine = StoreQueryEngine::new(&fx.store);
    // Warm every per-generation cache the engine keeps.
    let (warm, _) = sweep(&engine);
    assert_eq!(warm[0].1.fresh, Some(true));
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.pragma_update(None, "foreign_keys", "OFF").unwrap();
        conn.execute_batch("DELETE FROM generations;").unwrap();
    }
    assert_eq!(fx.store.latest_generation_id().unwrap(), None, "precondition");
    let (seen, _) = sweep(&engine);
    let bad: Vec<String> = seen
        .iter()
        .filter(|(_, f)| f.fresh == Some(true) || f.generation_id.is_some())
        .map(|(n, f)| format!("{n}: {f:?}"))
        .collect();
    assert!(bad.is_empty(), "claims a generation after it was deleted:\n{}", bad.join("\n"));
    let canned: Vec<String> = seen
        .iter()
        .filter(|(_, f)| !f.reason.clone().unwrap_or_default().contains(NO_GENERATION))
        .map(|(n, f)| format!("{n}: {f:?}"))
        .collect();
    assert!(canned.is_empty(), "deleted generation not reported:\n{}", canned.join("\n"));
}

// Probe 4: both halves of a layered impact carry the same freshness, on every
// arm, and the same freshness the plain `impact` of the same query carries.
#[test]
fn p4_layered_halves_agree_with_each_other_and_with_plain_impact() {
    for prime in [false, true] {
        let fx = fixture(if prime { "p4a" } else { "p4b" }, None);
        if prime {
            fx.store.status("t").unwrap();
        }
        let engine = StoreQueryEngine::new(&fx.store);
        for q in ["a", "d", "b", "chain.py", "nope_unknown", "test_a"] {
            let layered = engine.impact_layered(req(q, 3)).unwrap();
            let plain = engine.impact(req(q, 3)).unwrap();
            assert_eq!(
                layered.edges.source_freshness, layered.blast_radius.layers.source_freshness,
                "halves diverge for {q:?} (primed={prime})"
            );
            assert_eq!(
                layered.edges.source_freshness, plain.source_freshness,
                "layered and plain impact diverge for {q:?} (primed={prime})"
            );
            assert_eq!(layered.edges.source_freshness, expected(&fx.store));
        }
    }
}

// Probe 4b: one half refuses while the other succeeds: a max_depth of zero and
// an oversized request are the only ways to make the halves differ in outcome.
#[test]
fn p4b_halves_with_different_outcomes_still_share_freshness() {
    let fx = fixture("p4c", None);
    fx.store.status("t").unwrap();
    let engine = StoreQueryEngine::new(&fx.store);
    for (q, depth, budget) in [("a", 0usize, 8_000u32), ("a", 3, 0), ("a", 3, 1), ("a", 3, 2)] {
        let mut r = req(q, depth);
        r.token_budget = budget;
        let layered = engine.impact_layered(r).unwrap();
        assert_eq!(
            layered.edges.source_freshness, layered.blast_radius.layers.source_freshness,
            "q={q:?} depth={depth} budget={budget}: edges {:?} vs layers {:?} (edges.resolution={:?}, layers.resolution={:?})",
            layered.edges.source_freshness,
            layered.blast_radius.layers.source_freshness,
            layered.edges.resolution,
            layered.blast_radius.layers.resolution
        );
        assert_eq!(layered.edges.source_freshness, expected(&fx.store));
    }
}

// Probe 7: the in-memory engine has no store: "not checked" must never ride
// with a generation id, nor with an unresolved_namesakes stamped with one.
#[test]
fn p7_in_memory_engine_never_implies_a_generation() {
    let root = tmp_dir("p7");
    fs::write(root.join("chain.py"), CHAIN).unwrap();
    let (files, _) = extract_tree_with_report(&root).unwrap();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&files);
    let resolution = resolver.resolve_all(&files).unwrap();
    let engine = QueryEngine::new(&files, &resolution);
    let mut all: Vec<(String, SourceFreshness, bool)> = Vec::new();
    for q in ["a", "d", "nope_unknown", "chain.py", "  "] {
        let r = engine.search(req(q, 3));
        all.push((format!("search({q:?})"), r.source_freshness, r.unresolved_namesakes.is_some()));
        for (name, r) in [
            ("impact", engine.impact(req(q, 3))),
            ("trace", engine.trace(req(q, 3))),
            ("dependencies", engine.dependencies(req(q, 3))),
        ] {
            all.push((format!("{name}({q:?})"), r.source_freshness, r.unresolved_namesakes.is_some()));
        }
    }
    let bad: Vec<String> = all
        .iter()
        .filter(|(_, f, namesakes)| {
            f.fresh.is_some()
                || f.generation_id.is_some()
                || *namesakes
                || !f.reason.clone().unwrap_or_default().contains("not checked")
        })
        .map(|(n, f, ns)| format!("{n}: {f:?} namesakes={ns}"))
        .collect();
    assert!(bad.is_empty(), "in-memory freshness implies a generation:\n{}", bad.join("\n"));
}

// Probe 1 (store side): the refusal's unresolved_namesakes, when present, must
// name the generation the freshness names -- a refusal that reads generation N
// for its ledger and generation M (or none) for its verdict is the contradiction
// this change set out to remove.
#[test]
fn p1c_namesakes_generation_and_freshness_never_contradict() {
    let fx = fixture("p1c", None);
    fx.store.status("t").unwrap();
    let engine = StoreQueryEngine::new(&fx.store);
    for q in ["nope_unknown", "a", "d", "b", "chain.py"] {
        let r = engine.impact(req(q, 3)).unwrap();
        if let Some(n) = &r.unresolved_namesakes {
            assert_eq!(
                r.source_freshness.generation_id,
                Some(n.generation_id),
                "{q:?}: freshness {:?} vs namesakes generation {}",
                r.source_freshness,
                n.generation_id
            );
        }
        let l = engine.impact_layered(req(q, 3)).unwrap();
        if let Some(n) = &l.edges.unresolved_namesakes {
            assert_eq!(l.edges.source_freshness.generation_id, Some(n.generation_id), "{q:?} layered");
        }
        if let Some(n) = &l.blast_radius.layers.unresolved_namesakes {
            assert_eq!(
                l.blast_radius.layers.source_freshness.generation_id,
                Some(n.generation_id),
                "{q:?} layered bands"
            );
        }
    }
}

fn refused<T>(r: &devmap_query::Response<T>) -> bool {
    matches!(r.resolution, devmap_query::ResolutionAvailability::Unavailable { .. })
}

/// Collect (name, refused, freshness) for the surfaces the sweep flagged.
fn flagged_surfaces(engine: &StoreQueryEngine) -> Vec<(String, bool, SourceFreshness)> {
    let mut out = Vec::new();
    for q in ["a", "nope_unknown", "  "] {
        let r = engine.explore(q, 5, 8_000, 0.0, 2).unwrap();
        out.push((format!("explore({q:?}).definitions"), refused(&r.definitions), r.definitions.source_freshness.clone()));
        out.push((format!("explore({q:?}).layers"), refused(&r.blast_radius.layers), r.blast_radius.layers.source_freshness.clone()));
    }
    for targets in [vec!["chain.py"], vec!["nope_unknown"], vec![]] {
        if let Ok(r) = engine.affected_tests(&s(&targets), 4_000, 0.0, 3) {
            out.push((format!("affected_tests({targets:?}).tests"), refused(&r.tests), r.tests.source_freshness.clone()));
            out.push((format!("affected_tests({targets:?}).layers"), refused(&r.blast_radius.layers), r.blast_radius.layers.source_freshness.clone()));
        }
    }
    let r = engine.clones(4_000, None, 3).unwrap();
    out.push(("clones.groups".to_string(), refused(&r.groups), r.groups.source_freshness.clone()));
    let r = engine.preview("chain.py", "def a():\n    return 2\n", 4_000, 0.0).unwrap();
    out.push(("preview.broken_callers".to_string(), refused(&r.broken_callers), r.broken_callers.source_freshness.clone()));
    out
}

// Success arms of explore / affected_tests / clones / preview skip `finish`.
#[test]
fn p1d_success_arms_of_explore_affected_clones_preview_carry_the_verdict() {
    let fx = fixture("p1d", None);
    fx.store.status("t").unwrap();
    let want = expected(&fx.store);
    let rows = flagged_surfaces(&StoreQueryEngine::new(&fx.store));
    let bad: Vec<String> = rows
        .iter()
        .filter(|(_, refused, f)| !refused && f != &want)
        .map(|(n, _, f)| format!("{n}: {f:?}"))
        .collect();
    assert!(bad.is_empty(), "success arms say 'not checked' beside {want:?}:\n{}", bad.join("\n"));
}

// The refusal half: a response whose resolution is Unavailable, on a store
// holding a real verdict, must carry that verdict.
#[test]
fn p1e_refusal_arms_of_explore_affected_clones_preview_carry_the_verdict() {
    let fx = fixture("p1e", None);
    fx.store.status("t").unwrap();
    let want = expected(&fx.store);
    let rows = flagged_surfaces(&StoreQueryEngine::new(&fx.store));
    eprintln!("{rows:#?}");
    let refusals = rows.iter().filter(|(_, refused, _)| *refused).count();
    assert!(refusals >= 2, "probe reached too few refusal arms: {refusals}");
    let bad: Vec<String> = rows
        .iter()
        .filter(|(_, refused, f)| *refused && f != &want)
        .map(|(n, _, f)| format!("{n}: {f:?}"))
        .collect();
    assert!(bad.is_empty(), "refusal arms say 'not checked' beside {want:?}:\n{}", bad.join("\n"));
}

// preview on a store with no generation: the canned sentence belongs here.
#[test]
fn p2c_preview_on_an_empty_store_says_no_generation() {
    let store = Store::open_in_memory().unwrap();
    let r = StoreQueryEngine::new(&store)
        .preview("chain.py", "def a():\n    return 2\n", 4_000, 0.0)
        .unwrap();
    let reason = r.broken_callers.source_freshness.reason.clone().unwrap_or_default();
    assert!(reason.contains(NO_GENERATION), "{:?}", r.broken_callers.source_freshness);
}
