//! Cross-checks between answers that describe the same graph.
//!
//! Each query surface has its own tests for its own contract. What none of them
//! can see is a disagreement *between* two surfaces: `neighbors` is documented
//! as "exactly the [`impact`] and [`trace`] each target would have got on its
//! own, under the same budget and the same `min_confidence`", `dead` is
//! documented as symbols nothing calls, and `search` publishes an arithmetic
//! its clients enforce. Those are claims about relationships, and a relationship
//! is only broken by two implementations drifting apart — exactly what a
//! per-surface test keeps green on both sides.
//!
//! The corpus is generated rather than hand-written, so the invariants are
//! checked over hundreds of symbols of several shapes instead of the two or
//! three a fixture can carry, and it is seeded so a failure replays.

use std::collections::BTreeSet;

use devmap_extract::extract_file;
use devmap_query::{Request, StoreQueryEngine};
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};

/// A deterministic 64-bit PRNG. No `rand` dependency exists in this workspace
/// and none may be added; xorshift64* is enough to shuffle a fixture and its
/// seed is written down, so any failure here replays exactly.
struct Seeded(u64);

impl Seeded {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }
}

const SEED: u64 = 0x5EED_1A2151;
const MODULES: usize = 60;
const PER_MODULE: usize = 8;

/// A corpus with the shapes that actually produce different edge kinds: free
/// functions, classes with methods, cross-module imports and calls, a module
/// nothing imports, and a symbol declared under the same name in two modules.
fn corpus() -> Store {
    let mut random = Seeded(SEED);
    let mut extractions = Vec::new();

    for module in 0..MODULES {
        let mut source = String::new();
        // Import a couple of earlier modules, so the call graph has depth
        // rather than being one layer of leaves.
        for _ in 0..2 {
            if module > 0 {
                let target = random.below(module);
                source.push_str(&format!("from mod_{target:03} import fn_{target:03}_0\n"));
            }
        }
        source.push('\n');
        source.push_str(&format!(
            "class Widget_{module:03}:\n    def render(self):\n        return self.paint()\n\n    \
             def paint(self):\n        return {module}\n\n\n"
        ));
        // `collide` is declared in every module on purpose: a bare call to it
        // is ambiguous, which is where a resolver is tempted to pick one and
        // call the answer deterministic.
        source.push_str("def collide(rows):\n    return len(rows)\n\n\n");
        for index in 0..PER_MODULE {
            source.push_str(&format!("def fn_{module:03}_{index}(rows):\n"));
            if module > 0 && index % 3 == 0 {
                let target = random.below(module);
                source.push_str(&format!("    fn_{target:03}_0(rows)\n"));
            }
            if index % 2 == 0 {
                source.push_str("    collide(rows)\n");
            }
            source.push_str(&format!("    return Widget_{module:03}().render()\n\n\n"));
        }
        extractions.push(extract_file(&format!("mod_{module:03}.py"), &source));
    }
    // A module nothing imports, so `dead` has something real to find.
    extractions.push(extract_file(
        "orphan.py",
        "def never_called_anywhere(rows):\n    return rows\n",
    ));

    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let analysis = devmap_analyze::analyze(&extractions, &resolution);
    let store = Store::open_in_memory().expect("in-memory store");
    store
        .save_generation_with_opts(
            &extractions,
            &resolution,
            &analysis,
            GenerationWriteOpts::default(),
        )
        .expect("generation writes");
    store
}

/// The identity of an edge, for set comparison: everything the wire form
/// carries except the confidence float, which is compared separately so a
/// formatting difference cannot look like a missing edge.
fn key(edge: &devmap_resolve::model::ResolvedEdge) -> (String, String, String, String, String) {
    (
        edge.source_file.clone(),
        edge.target_file.clone(),
        edge.source_symbol.clone(),
        edge.target_symbol.clone(),
        format!("{:?}", edge.edge_kind),
    )
}

fn targets(store: &Store, count: usize) -> Vec<String> {
    let mut random = Seeded(SEED ^ 0x9E37_79B9);
    let mut picked = BTreeSet::new();
    for _ in 0..count {
        let module = random.below(MODULES);
        let index = random.below(PER_MODULE);
        picked.insert(format!("mod_{module:03}.py::fn_{module:03}_{index}"));
        picked.insert(format!("mod_{module:03}.py::Widget_{module:03}.render"));
    }
    // Prove the fixture is not answering about nothing.
    let edges = store.latest_edges(0.0).expect("edges");
    assert!(
        edges.len() > 1_000,
        "the corpus must hold a real graph, got {} edge(s)",
        edges.len()
    );
    picked.into_iter().collect()
}

fn request(query: &str, budget: u32, min_confidence: f32, depth: usize) -> Request<String> {
    Request {
        query: query.to_string(),
        token_budget: budget,
        min_confidence,
        max_depth: depth,
    }
}

/// `neighbors` promises each target "exactly the `impact` and `trace` it would
/// have got on its own". Asserted as a set equality on both halves, for every
/// target, at three confidence floors.
#[test]
fn a_composed_answer_equals_the_two_answers_it_composes() {
    let store = corpus();
    let engine = StoreQueryEngine::new(&store);
    let all = targets(&store, 40);

    for min_confidence in [0.0f32, 0.4, 0.9] {
        for chunk in all.chunks(devmap_query::MAX_NEIGHBOR_TARGETS) {
            let composed = engine
                .neighbors(chunk, 4_000, min_confidence, 2)
                .expect("neighbors");
            assert_eq!(composed.len(), chunk.len());
            for (entry, target) in composed.iter().zip(chunk) {
                assert_eq!(&entry.target, target, "answers must stay in target order");

                let separate_callers = engine
                    .impact(request(target, 4_000, min_confidence, 2))
                    .expect("impact");
                let separate_callees = engine
                    .trace(request(target, 4_000, min_confidence, 2))
                    .expect("trace");

                for (half, composed_half, separate) in [
                    ("callers", &entry.callers, &separate_callers),
                    ("callees", &entry.callees, &separate_callees),
                ] {
                    let left: BTreeSet<_> = composed_half.items.iter().map(key).collect();
                    let right: BTreeSet<_> = separate.items.iter().map(key).collect();
                    assert_eq!(
                        left, right,
                        "the {half} half of the composed answer for {target} at \
                         min_confidence {min_confidence} is not the answer the \
                         separate call gives"
                    );
                    assert_eq!(
                        (
                            composed_half.shown,
                            composed_half.hidden,
                            composed_half.total
                        ),
                        (separate.shown, separate.hidden, separate.total),
                        "{half} counts differ for {target}"
                    );
                }
            }
        }
    }
}

/// Every edge a walk reports must exist in the generation it walked, at a
/// confidence at or above the floor the caller asked for. A traversal that
/// invents, relabels or under-filters an edge is worse than one that misses it.
#[test]
fn every_edge_a_walk_reports_is_in_the_generation_at_the_floor_it_claimed() {
    let store = corpus();
    let engine = StoreQueryEngine::new(&store);
    let stored: BTreeSet<_> = store
        .latest_edges(0.0)
        .expect("edges")
        .iter()
        .map(|edge| {
            (
                edge.source_file.clone(),
                edge.target_file.clone(),
                edge.source_symbol.clone(),
                edge.target_symbol.clone(),
                edge.edge_kind.clone(),
            )
        })
        .collect();

    for min_confidence in [0.0f32, 0.7] {
        for target in targets(&store, 20) {
            for (name, answer) in [
                (
                    "impact",
                    engine
                        .impact(request(&target, 4_000, min_confidence, 3))
                        .expect("impact"),
                ),
                (
                    "trace",
                    engine
                        .trace(request(&target, 4_000, min_confidence, 3))
                        .expect("trace"),
                ),
            ] {
                for edge in &answer.items {
                    let identity = (
                        edge.source_file.clone(),
                        edge.target_file.clone(),
                        edge.source_symbol.clone(),
                        edge.target_symbol.clone(),
                        format!("{:?}", edge.edge_kind),
                    );
                    assert!(
                        stored.contains(&identity),
                        "{name}({target}) reported an edge the generation does not \
                         hold: {identity:?}"
                    );
                    assert!(
                        edge.confidence.0 + 1e-6 >= min_confidence,
                        "{name}({target}) reported {identity:?} at confidence {} \
                         under a floor of {min_confidence}",
                        edge.confidence.0
                    );
                }
            }
        }
    }
}

/// `dead` proposes deletions. A symbol with any inbound `Calls` edge in the
/// generation, at any confidence, is not one — the floor a report chooses is a
/// ranking decision and must never turn an edge that exists into an absence.
#[test]
fn nothing_dead_has_a_caller_in_the_edge_table() {
    let store = corpus();
    let engine = StoreQueryEngine::new(&store);

    let called: BTreeSet<String> = store
        .latest_edges(0.0)
        .expect("edges")
        .iter()
        .filter(|edge| matches!(edge.edge_kind.as_str(), "Calls"))
        .map(|edge| edge.target_symbol.clone())
        .collect();

    let report = engine.dead_symbols(40_000).expect("dead");
    assert!(
        report.shown > 0,
        "the corpus contains an uncalled function, so this must find something"
    );
    for finding in &report.items {
        if finding.is_exempt {
            continue;
        }
        assert!(
            !called.contains(&finding.symbol_name),
            "`dead` proposed {} in {}, but the generation holds a Calls edge \
             naming it as a target",
            finding.symbol_name,
            finding.file_path
        );
    }
}

/// The arithmetic every client of a budgeted answer enforces.
#[test]
fn every_budgeted_answer_accounts_for_what_it_withheld() {
    let store = corpus();
    let engine = StoreQueryEngine::new(&store);

    let mut checked = 0usize;
    for query in ["fn_0", "collide", "Widget", "render", "never_called", "zzz"] {
        // Budgets chosen to straddle the cut: 40 tokens cannot hold a page,
        // 40,000 holds everything this corpus has.
        for budget in [40u32, 400, 4_000, 40_000] {
            let answer = engine
                .search(request(query, budget, 0.0, 1))
                .expect("search");
            assert_eq!(
                answer.shown as usize,
                answer.items.len(),
                "search({query}, {budget}) says it shows {} items and carries {}",
                answer.shown,
                answer.items.len()
            );
            assert_eq!(
                answer.shown.saturating_add(answer.hidden),
                answer.total,
                "search({query}, {budget}): {} shown + {} hidden != {} total",
                answer.shown,
                answer.hidden,
                answer.total
            );
            assert_eq!(
                answer.truncated,
                answer.hidden > 0,
                "search({query}, {budget}) reports truncated={} with {} hidden",
                answer.truncated,
                answer.hidden
            );
            checked += 1;
        }
    }

    for target in targets(&store, 8) {
        for budget in [40u32, 400, 4_000] {
            for (name, answer) in [
                (
                    "impact",
                    engine.impact(request(&target, budget, 0.0, 2)).expect("i"),
                ),
                (
                    "trace",
                    engine.trace(request(&target, budget, 0.0, 2)).expect("t"),
                ),
                (
                    "deps",
                    engine
                        .dependencies(request(&target, budget, 0.0, 1))
                        .expect("d"),
                ),
            ] {
                assert_eq!(
                    answer.shown as usize,
                    answer.items.len(),
                    "{name}({target}, {budget}) miscounts its own items"
                );
                assert_eq!(
                    answer.shown.saturating_add(answer.hidden),
                    answer.total,
                    "{name}({target}, {budget}): {} + {} != {}",
                    answer.shown,
                    answer.hidden,
                    answer.total
                );
                assert_eq!(
                    answer.truncated,
                    answer.hidden > 0,
                    "{name}({target}, {budget}) reports truncated={} with {} hidden",
                    answer.truncated,
                    answer.hidden
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 100, "only {checked} answers were checked");
}

/// The count that stands in for a listing must equal the listing, at every
/// floor and for every batch shape — including a batch large enough to be
/// chunked, where the two could disagree by double-counting.
#[test]
fn the_caller_count_equals_the_caller_listing_it_stands_in_for() {
    let store = corpus();
    let mut names: Vec<String> = (0..MODULES)
        .flat_map(|module| (0..PER_MODULE).map(move |index| format!("fn_{module:03}_{index}")))
        .collect();
    names.push("collide".to_string());
    names.push("render".to_string());
    // Duplicates on purpose: `IN` ignores them and the chunked count must too.
    names.push("collide".to_string());

    for min_confidence in [0.0f32, 0.4, 0.9, 1.0] {
        for exclude in ["", "mod_000.py"] {
            let listed = store
                .callers_of(&names, exclude, min_confidence)
                .expect("callers_of");
            let counted = store
                .count_callers_of(&names, exclude, min_confidence)
                .expect("count_callers_of");
            assert_eq!(
                counted,
                listed.len(),
                "count_callers_of and callers_of disagree at floor \
                 {min_confidence} excluding {exclude:?}"
            );
        }
    }
}

/// `neighbors` takes a `max_depth` and must spend it on both directions.
///
/// The sibling assertion above finds this too, across every target and floor.
/// This case states it on its own because the disagreement is one parameter,
/// not a set difference: the composition walked the caller's depth inbound and
/// a hardcoded single hop outbound, so one answer described two different
/// questions and nothing in it said which half had been cut short.
///
/// It is the same defect the `min_confidence` note one line above in
/// `neighbors_once` describes and closes — "it was hardcoded to 0.0 here, which
/// silently discarded the caller's filter on the inbound side" — surviving in
/// the parameter next to it. `neighbors_composition.rs` cannot see it: every
/// comparison there is made at depth 1, the one depth where the hardcoded value
/// and the requested one agree.
#[test]
fn both_halves_of_a_composition_walk_to_the_depth_the_caller_asked_for() {
    let store = corpus();
    let engine = StoreQueryEngine::new(&store);
    // `fn_000_3` calls `Widget_000()` and `.render()`, and `render` calls
    // `paint` — so there is a second hop to find, and a one-hop answer is
    // visibly short rather than merely differently ordered.
    let target = "mod_000.py::fn_000_3";

    let mut reach = Vec::new();
    for depth in [1usize, 2, 3] {
        let composed = engine
            .neighbors(&[target.to_string()], 20_000, 0.0, depth)
            .expect("neighbors");
        let separate = engine
            .trace(request(target, 20_000, 0.0, depth))
            .expect("trace");
        let composed_edges: BTreeSet<_> = composed[0].callees.items.iter().map(key).collect();
        let separate_edges: BTreeSet<_> = separate.items.iter().map(key).collect();
        assert_eq!(
            composed_edges, separate_edges,
            "at depth {depth} the composed callees are not the answer `trace` \
             gives for the same target, budget and floor"
        );
        reach.push(composed[0].callees.total);
    }

    // And the depths must actually differ on this fixture, or the equality
    // above is satisfied by a walk that ignores depth in both places.
    assert!(
        reach[1] > reach[0],
        "depth 2 reached {} edges against depth 1's {} — the fixture has no \
         second hop, so this test proves nothing",
        reach[1],
        reach[0]
    );
}
