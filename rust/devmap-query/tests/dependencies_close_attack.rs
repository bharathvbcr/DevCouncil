//! Closing adversarial probes for `lane/deps-outbound` @ 41a0d9be (delta
//! 38fcbc77..41a0d9be). A failing test here is a finding; nothing is fixed.

use crate::common;

use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
use devmap_query::rung::Rung;
use devmap_query::{QueryEngine, Request, Response, StoreQueryEngine};
use devmap_resolve::model::{ResolutionResult, ResolvedEdge};
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};

struct Fx {
    extractions: Vec<Extraction>,
    resolution: ResolutionResult,
    store: Store,
}

fn fx_with(files: &[(&str, String)], dispatch: bool) -> Fx {
    let extractions: Vec<_> = files.iter().map(|(p, b)| extract_file(p, b)).collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let mut resolution = resolver.resolve_all(&extractions).unwrap();
    if dispatch {
        common::dispatch_ambiguous_sites(&mut resolution);
    }
    let analysis = devmap_analyze::analyze(&extractions, &resolution);
    let store = Store::open_in_memory().unwrap();
    store
        .save_generation_with_opts(&extractions, &resolution, &analysis, GenerationWriteOpts::default())
        .unwrap();
    Fx { extractions, resolution, store }
}

fn req(q: &str, budget: u32) -> Request<String> {
    Request { query: q.to_string(), token_budget: budget, min_confidence: 0.0, max_depth: 1 }
}

fn store_ans(fx: &Fx, q: &str, budget: u32) -> Result<Response<ResolvedEdge>, String> {
    StoreQueryEngine::new(&fx.store).dependencies(req(q, budget)).map_err(|e| e.to_string())
}

fn store_rung(fx: &Fx, q: &str, rung: Rung) -> Response<ResolvedEdge> {
    StoreQueryEngine::new(&fx.store)
        .dependencies_at_rung(req(q, 100_000), Some(rung))
        .unwrap()
}

fn mem_ans(fx: &Fx, q: &str, budget: u32) -> Response<ResolvedEdge> {
    QueryEngine::new(&fx.extractions, &fx.resolution).dependencies(req(q, budget))
}

fn key(e: &ResolvedEdge) -> String {
    format!(
        "{}|{}|{}|{}|{:?}|{}",
        e.source_file, e.source_symbol, e.target_file, e.target_symbol, e.edge_kind, e.confidence.0
    )
}

const LIB: &str = "RETRY = 2\n\n\ndef helper():\n    return 1\n\n\ndef worker():\n    helper()\n    return RETRY\n\n\ndef leaf():\n    return RETRY\n";
const CALLERS: &str = "from lib import worker, helper\n\ncb = helper\n\n\ndef c1():\n    return worker()\n\n\ndef c2():\n    cb2 = helper\n    return helper() + worker()\n";

fn std_fx() -> Fx {
    fx_with(&[("lib.py", LIB.to_string()), ("callers.py", CALLERS.to_string())], false)
}

fn control_free(s: &str) -> bool {
    !s.chars().any(|c| c.is_control())
}

// ------------------------------------------------------------- probe 7

/// Probe 7a. A symbol query that carries a newline (a trailing one survives
/// into the note because the note quotes the *untrimmed* query) must not put a
/// raw control character into the informational note. Severity: low.
#[test]
fn p7_a_control_character_in_the_query_is_escaped_in_the_note() {
    let fx = std_fx();
    // RETRY has callers and nothing leaving it: the inbound note fires.
    let mut report = String::new();
    for q in ["RETRY\n", "RETRY\r\n", "RETRY\t"] {
        let s = store_ans(&fx, q, 8_000);
        let m = mem_ans(&fx, q, 8_000);
        if let Ok(r) = &s {
            if let Some(n) = &r.note {
                if !control_free(n) {
                    report.push_str(&format!("store {q:?}: {n:?}\n"));
                }
            }
        }
        if let Some(n) = &m.note {
            if !control_free(n) {
                report.push_str(&format!("memory {q:?}: {n:?}\n"));
            }
        }
    }
    assert!(report.is_empty(), "raw control characters reach the note:\n{report}");
}

/// Probe 7b. The note quotes the query verbatim; a query padded with whitespace
/// makes the note unbounded. Severity: low.
#[test]
fn p7_the_note_has_a_length_cap() {
    let fx = std_fx();
    let q = format!("RETRY{}", " ".repeat(200_000));
    let mut lens = vec![];
    if let Ok(r) = store_ans(&fx, &q, 8_000) {
        lens.push(("store", r.note.map_or(0, |n| n.len())));
    }
    lens.push(("memory", mem_ans(&fx, &q, 8_000).note.map_or(0, |n| n.len())));
    for (engine, len) in &lens {
        assert!(*len < 4_096, "{engine} note is {len} bytes for a padded query");
    }
    assert!(lens.iter().any(|(_, l)| *l > 0), "the probe did not reach the note: {lens:?}");
}

// ------------------------------------------------------------- probe 5

fn rung_fx() -> Fx {
    fx_with(
        &[
            ("lib_a.py", "def shared():\n    return 1\n".to_string()),
            ("lib_b.py", "def shared():\n    return 2\n".to_string()),
            (
                "app.py",
                concat!(
                    "def a():\n    return 1\n\n\n",
                    "def b():\n    return a()\n\n\n",
                    "def helper():\n    return shared()\n\n\n",
                    "def main():\n    b()\n    return helper()\n"
                )
                .to_string(),
            ),
        ],
        true,
    )
}

/// Probe 5a. Partially filtered file answer: some outbound edges shown, some
/// hidden by the rung. The hidden count must be stated somewhere a reader of
/// `note` sees (the histogram carries it, the note does not). Severity: low.
#[test]
fn p5_a_partially_filtered_answer_states_the_hidden_count_in_the_note() {
    let fx = rung_fx();
    let r = store_rung(&fx, "app.py", Rung::Deterministic);
    let hist = r.rungs.clone().expect("rungs");
    assert!(hist.filtered_out > 0 && !r.items.is_empty(), "fixture is not partial: {hist:?}");
    let note = r.note.clone().unwrap_or_default();
    assert!(
        note.contains(&hist.filtered_out.to_string()),
        "{} of {} edges hidden by min_rung but note={:?}",
        hist.filtered_out,
        hist.total(),
        r.note
    );
}

/// Probe 5b. Emptied wholly by the rung: the count is stated (control).
#[test]
fn p5_an_emptied_answer_states_the_hidden_count() {
    let fx = rung_fx();
    let r = store_rung(&fx, "app.py::helper", Rung::Deterministic);
    assert!(r.items.is_empty());
    let note = r.note.expect("a note");
    assert!(note.contains("hidden by min_rung=deterministic"), "{note}");
}

/// Probe 5c. min_rung on a FILE query that is emptied by the rung must say so.
#[test]
fn p5_a_file_query_emptied_by_the_rung_says_so() {
    // A file whose only outbound edge is speculative and which declares nothing.
    let fx = fx_with(
        &[
            ("lib_a.py", "def shared():\n    return 1\n".to_string()),
            ("lib_b.py", "def shared():\n    return 2\n".to_string()),
            ("app.py", "x = shared()\n".to_string()),
        ],
        true,
    );
    let open = store_ans(&fx, "app.py", 8_000).unwrap();
    let floored = store_rung(&fx, "app.py", Rung::Deterministic);
    assert!(!open.items.is_empty(), "fixture has no outbound edges: {open:?}");
    if floored.items.is_empty() {
        let note = floored.note.clone().unwrap_or_default();
        assert!(note.contains("hidden by min_rung"), "emptied by the rung, note={:?}", floored.note);
    }
}

// ------------------------------------------------------------- probe 4

fn wide(n: usize) -> Fx {
    let mut app = String::new();
    for i in 0..n {
        app.push_str(&format!("def f{i:04}():\n    return {i}\n\n\n"));
    }
    app.push_str("def main():\n");
    for i in (0..n).rev() {
        app.push_str(&format!("    f{i:04}()\n"));
    }
    app.push_str("    return 0\n");
    fx_with(&[("app.py", app)], false)
}

/// Probe 4. 1000 equal-confidence same-kind edges: both engines, three runs,
/// one identical order; declaration edges last on a file query even though
/// their confidence is at least as high.
#[test]
fn p4_order_is_total_and_engine_independent() {
    let fx = wide(1000);
    for q in ["app.py::main", "app.py"] {
        let first: Vec<String> =
            store_ans(&fx, q, 10_000_000).unwrap().items.iter().map(key).collect();
        assert!(first.len() >= 1000, "{q}: {} items", first.len());
        for run in 0..3 {
            let again: Vec<String> =
                store_ans(&fx, q, 10_000_000).unwrap().items.iter().map(key).collect();
            assert_eq!(first, again, "{q}: store run {run} reordered");
            let mem: Vec<String> = mem_ans(&fx, q, 10_000_000).items.iter().map(key).collect();
            assert_eq!(first, mem, "{q}: engines order differently");
        }
        let mut seen_decl = false;
        for k in &first {
            let decl = k.contains("|Contains|") || k.contains("|MemberOf|");
            assert!(!(seen_decl && !decl), "{q}: a real dependency follows a declaration: {k}");
            seen_decl |= decl;
        }
    }
}

/// Probe 4b. A truncated page has the same content in both engines.
#[test]
fn p4_a_truncated_page_is_the_same_page_in_both_engines() {
    let fx = wide(300);
    for q in ["app.py::main", "app.py"] {
        for budget in [50u32, 400, 1_500] {
            let s: Vec<String> = store_ans(&fx, q, budget).unwrap().items.iter().map(key).collect();
            let m: Vec<String> = mem_ans(&fx, q, budget).items.iter().map(key).collect();
            assert_eq!(s, m, "{q} budget {budget}: truncated pages differ");
        }
    }
}

// ------------------------------------------------------------- probe 3

/// Probe 3. Every symbol with nothing leaving it: the inbound count both
/// engines state equals the count of raw inbound rows in the resolution (each
/// distinct row once; Calls and References to the same pair both count).
#[test]
fn p3_inbound_count_is_the_raw_row_count() {
    let fx = std_fx();
    let mut symbols: Vec<(String, String)> = fx
        .resolution
        .edges
        .iter()
        .flat_map(|e| {
            [
                (e.source_symbol.clone(), e.source_file.clone()),
                (e.target_symbol.clone(), e.target_file.clone()),
            ]
        })
        .collect();
    symbols.sort();
    symbols.dedup();
    let mut report = String::new();
    let mut checked = 0;
    for (symbol, file) in &symbols {
        if !symbol.contains("::") && !symbol.chars().next().is_some_and(char::is_alphabetic) {
            continue;
        }
        let query = format!("{file}::{symbol}");
        let query = if symbol.contains("::") { symbol.clone() } else { query };
        let raw = fx
            .resolution
            .edges
            .iter()
            .filter(|e| {
                e.target_symbol == *symbol && e.target_file == *file && e.source_symbol != *symbol
            })
            .count();
        let outbound = fx
            .resolution
            .edges
            .iter()
            .filter(|e| e.source_symbol == *symbol && e.source_file == *file)
            .count();
        if outbound != 0 || raw == 0 {
            continue;
        }
        checked += 1;
        let stated = |n: &Option<String>| -> Option<usize> {
            let n = n.as_deref()?;
            let head = &n[..n.find(" inbound edge(s)")?];
            head.rsplit(' ').next()?.parse().ok()
        };
        let m = mem_ans(&fx, &query, 8_000);
        if stated(&m.note) != Some(raw) {
            report.push_str(&format!("memory {query}: raw={raw} note={:?}\n", m.note));
        }
        match store_ans(&fx, &query, 8_000) {
            Ok(s) if stated(&s.note) == Some(raw) => {}
            other => report.push_str(&format!(
                "store {query}: raw={raw} got={:?}\n",
                other.map(|r| r.note)
            )),
        }
    }
    assert!(checked >= 2, "the probe covered {checked} symbols");
    assert!(report.is_empty(), "inbound counts differ from raw rows:\n{report}");
}

/// Probe 3b. A name two files define: two start nodes reachable from one query;
/// an edge reachable from both starts counts once, and the engines agree.
#[test]
fn p3_two_starts_one_edge_counts_once() {
    let fx = fx_with(
        &[
            ("a.py", "def dup():\n    return 1\n".to_string()),
            ("b.py", "def dup():\n    return 2\n".to_string()),
            ("c.py", "def caller():\n    return dup()\n".to_string()),
        ],
        true,
    );
    let raw_total = fx
        .resolution
        .edges
        .iter()
        .filter(|e| e.target_symbol.ends_with("dup") && e.source_symbol != e.target_symbol)
        .count();
    let m = mem_ans(&fx, "dup", 8_000);
    let s = store_ans(&fx, "dup", 8_000);
    let count = |n: &Option<String>| -> Option<usize> {
        let n = n.as_deref()?;
        n[..n.find(" inbound edge(s)")?].rsplit(' ').next()?.parse().ok()
    };
    let s_note = s.as_ref().ok().and_then(|r| r.note.clone());
    assert_eq!(count(&m.note), count(&s_note), "memory={:?} store={:?} raw={raw_total}", m.note, s_note);
    if let Some(c) = count(&m.note) {
        assert!(c <= raw_total, "{c} stated for {raw_total} raw inbound rows");
    }
}

// ------------------------------------------------------------- probe 1

/// Probe 1. No other query's envelope may carry `note`, not even as null.
#[test]
fn p1_other_tools_do_not_serialize_note() {
    let fx = std_fx();
    let engine = StoreQueryEngine::new(&fx.store);
    let mut seen = vec![];
    let impact = serde_json::to_value(engine.impact(req("lib.py::worker", 8_000)).unwrap()).unwrap();
    let neighbors = serde_json::to_value(
        engine.neighbors(&["lib.py::worker".to_string()], 8_000, 0.0, 1).unwrap(),
    )
    .unwrap();
    for (name, v) in [("impact", impact), ("neighbors", neighbors)] {
        if v.get("note").is_some() {
            seen.push(name);
        }
    }
    let deps = serde_json::to_value(store_ans(&fx, "lib.py", 8_000).unwrap()).unwrap();
    let silent = !deps.as_object().unwrap().contains_key("note")
        || deps["note"].is_string();
    assert!(seen.is_empty(), "note leaked into {seen:?}");
    assert!(silent, "deps serialises a non-string note: {}", deps["note"]);
}

// ------------------------------------------------------------- probe 3, live

/// Probe 3, live. Against a real `devmap build` of this repository (set
/// `DEVMAP_LIVE_STORE` to the store path; without it the probe cannot run and
/// FAILS rather than passing unexamined): 10 leaf symbols with callers, the
/// inbound count in the note equals the raw stored rows pointing at them.
#[test]
#[ignore = "needs DEVMAP_LIVE_STORE; run with --ignored"]
fn p3_live_inbound_counts_match_raw_stored_rows() {
    let path = std::env::var("DEVMAP_LIVE_STORE")
        .expect("DEVMAP_LIVE_STORE is not set: the live probe did not run");
    let store = Store::open_read_only(&path).unwrap();
    let rows = store.latest_edges(0.0).unwrap();
    let mut outbound = std::collections::BTreeSet::new();
    let mut inbound: std::collections::BTreeMap<(String, String), usize> = Default::default();
    for e in &rows {
        outbound.insert((e.source_symbol.clone(), e.source_file.clone()));
    }
    for e in &rows {
        if e.source_symbol != e.target_symbol {
            *inbound.entry((e.target_symbol.clone(), e.target_file.clone())).or_default() += 1;
        }
    }
    let mut names: std::collections::BTreeMap<&str, usize> = Default::default();
    for (s, _) in inbound.keys() {
        *names.entry(s.as_str()).or_default() += 1;
    }
    let candidates: Vec<_> = inbound
        .iter()
        .filter(|((s, f), n)| {
            **n >= 2 && s.contains("::") && !outbound.contains(&(s.clone(), f.clone())) && names[s.as_str()] == 1
        })
        .collect();
    assert!(candidates.len() >= 10, "only {} candidates", candidates.len());
    let stride = candidates.len() / 10;
    let engine = StoreQueryEngine::new(&store);
    let mut report = String::new();
    let mut checked = 0;
    for ((symbol, _file), raw) in candidates.iter().step_by(stride).take(10) {
        let r = match engine.dependencies(req(symbol, 8_000)) {
            Ok(r) => r,
            Err(e) => {
                report.push_str(&format!("{symbol}: refused {e}\n"));
                continue;
            }
        };
        checked += 1;
        let stated: Option<usize> = r.note.as_deref().and_then(|n| {
            n[..n.find(" inbound edge(s)")?].rsplit(' ').next()?.parse().ok()
        });
        if stated != Some(**raw) {
            report.push_str(&format!(
                "{symbol}: raw rows={raw}, note={:?}, items={}\n",
                r.note,
                r.items.len()
            ));
        }
    }
    assert!(checked >= 5, "only {checked} symbols answered:\n{report}");
    assert!(report.is_empty(), "{report}");
}
