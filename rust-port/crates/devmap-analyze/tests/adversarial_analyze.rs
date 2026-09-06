//! An adversarial pass over `devmap-analyze`.
//!
//! Analysis is where a bounded question is asked of an unbounded graph. Three
//! shapes are hunted:
//!
//! * **Stack.** `pdg::validate_statements` and `PdgBuilder::build_sequence`
//!   recurse once per nesting level of a statement tree they accept from
//!   outside the crate, and `FunctionPdgInput` is `Deserialize`. Unbounded
//!   recursion over caller-supplied data is a crash, not an error, and a crash
//!   in a daemon is a denial of service for every consumer behind it.
//! * **Bounds.** `traverse_graph` takes `max_depth` and `max_nodes`, so its
//!   *result* is bounded; the question here is whether its *cost* is, on a
//!   graph far larger than the walk.
//! * **Class A.** A walk that stopped early must say so. `TraversalStop` exists
//!   for exactly that, and its coverage is asserted here over the pathological
//!   shapes rather than the tidy ones its own unit tests use.

use std::time::{Duration, Instant};

use devmap_analyze::pdg::{
    build_function_pdg, FunctionPdgInput, PdgStatement, PdgStatementKind, MAX_PDG_NESTING_DEPTH,
};
use devmap_analyze::traversal::{traverse_graph, TraversalOptions};
use devmap_extract::model::{Confidence, EdgeKind};
use devmap_resolve::model::ResolvedEdge;

/// Ceiling for a single bounded walk over a large graph.
///
/// A walk capped at 1,000 nodes should not care how big the graph is beyond
/// the cost of finding its starts. Ten seconds is far above any correct
/// implementation and far below "the daemon is wedged", which is the only
/// distinction this needs to draw.
const WALK_BUDGET: Duration = Duration::from_secs(10);

fn edge(source: &str, target: &str, kind: EdgeKind) -> ResolvedEdge {
    ResolvedEdge {
        source_file: "a.go".to_string(),
        target_file: "b.go".to_string(),
        source_symbol: source.to_string(),
        target_symbol: target.to_string(),
        edge_kind: kind,
        confidence: Confidence::DETERMINISTIC,
        resolution: None,
        details: None,
        evidence: None,
    }
}

// ---------------------------------------------------------------------------
// Stack.
// ---------------------------------------------------------------------------

/// A statement tree deeper than the recursion bound is refused, not crashed on.
///
/// `validate_statements` and `build_sequence` both recurse once per nesting
/// level. `FunctionPdgInput` derives `Deserialize`, so the tree is
/// caller-supplied data — and there was no depth check anywhere between the
/// public entry point and the recursion. A tree a few tens of thousands deep
/// overflows the stack, which aborts the process: not an error a caller can
/// handle, not a result, and not something a `Result`-returning function is
/// allowed to do.
///
/// The refusal must name the depth, because a caller that hits it needs to know
/// whether it fed a pathological input or the bound is too low for real code.
///
/// The trees built here are `MAX_PDG_NESTING_DEPTH + 16` deep rather than the
/// 200,000 the original probe used. 200,000 reproduces the abort, but a tree
/// that deep also overflows the stack when it is *dropped* — `PdgStatement`'s
/// derived `Drop` is recursive too — so the test would abort on teardown
/// whether or not the bound works, and could only ever fail. The bound is what
/// is under test, and it is crossed just as definitively at 272.
#[test]
fn a_statement_tree_deeper_than_the_bound_is_refused_rather_than_overflowing() {
    fn nest(depth: usize) -> Vec<PdgStatement> {
        let mut body = vec![PdgStatement {
            line: 1,
            definitions: Vec::new(),
            uses: Vec::new(),
            taint_sinks: Vec::new(),
            kind: PdgStatementKind::Basic,
        }];
        for _ in 0..depth {
            body = vec![PdgStatement {
                line: 1,
                definitions: Vec::new(),
                uses: Vec::new(),
                taint_sinks: Vec::new(),
                kind: PdgStatementKind::Loop { body },
            }];
        }
        body
    }

    let input = |body: Vec<PdgStatement>| FunctionPdgInput {
        function_name: "f".to_string(),
        generation_id: 1,
        content_hash: 7,
        start_line: 1,
        end_line: 2,
        params: Vec::new(),
        body,
    };

    let over = MAX_PDG_NESTING_DEPTH + 16;
    let error = build_function_pdg(&input(nest(over)))
        .err()
        .unwrap_or_else(|| panic!("a {over}-deep statement tree must be refused"));
    let text = error.to_string();
    assert!(
        text.contains(&MAX_PDG_NESTING_DEPTH.to_string()),
        "the refusal must name the bound it enforced, got: {text}"
    );

    // The bound is a ceiling on hostile input, not a limit on real code: a
    // tree just inside it still builds.
    let ok = build_function_pdg(&input(nest(MAX_PDG_NESTING_DEPTH - 1)));
    assert!(
        ok.is_ok(),
        "a tree inside the bound must still build: {:?}",
        ok.err()
    );

    // Every recursive arm is covered, not just `Loop`: a bound applied to one
    // shape leaves the others reachable.
    for (label, wrap) in [
        (
            "Branch/then",
            (|body| PdgStatementKind::Branch {
                then_body: body,
                else_body: Vec::new(),
            }) as fn(Vec<PdgStatement>) -> PdgStatementKind,
        ),
        ("Branch/else", |body| PdgStatementKind::Branch {
            then_body: Vec::new(),
            else_body: body,
        }),
        ("Try/body", |body| PdgStatementKind::Try {
            body,
            handlers: Vec::new(),
            finally_body: Vec::new(),
        }),
        ("Try/handler", |body| PdgStatementKind::Try {
            body: Vec::new(),
            handlers: vec![body],
            finally_body: Vec::new(),
        }),
        ("Try/finally", |body| PdgStatementKind::Try {
            body: Vec::new(),
            handlers: Vec::new(),
            finally_body: body,
        }),
    ] {
        let mut body = vec![PdgStatement {
            line: 1,
            definitions: Vec::new(),
            uses: Vec::new(),
            taint_sinks: Vec::new(),
            kind: PdgStatementKind::Basic,
        }];
        for _ in 0..over {
            body = vec![PdgStatement {
                line: 1,
                definitions: Vec::new(),
                uses: Vec::new(),
                taint_sinks: Vec::new(),
                kind: wrap(body),
            }];
        }
        assert!(
            build_function_pdg(&input(body)).is_err(),
            "a {over}-deep {label} nesting was accepted; the depth bound does not \
             cover every recursive arm"
        );
    }
}

// ---------------------------------------------------------------------------
// Bounds.
// ---------------------------------------------------------------------------

/// One node with a very large in-degree does not defeat the caps.
///
/// A `Calls` edge is emitted per (call site x candidate), so a widely-called
/// symbol genuinely does accumulate tens of thousands of inbound edges (SC4).
/// Reverse traversal from it sorts that whole neighbour list — after cloning it
/// — on every visit.
#[test]
fn a_symbol_with_a_hundred_thousand_callers_stays_inside_its_caps() {
    let hub = "hub.go::Hub.serve".to_string();
    let edges: Vec<ResolvedEdge> = (0..100_000)
        .map(|i| edge(&format!("c{i}.go::caller{i}"), &hub, EdgeKind::Calls))
        .collect();

    let started = Instant::now();
    let walk = traverse_graph(
        std::slice::from_ref(&hub),
        &edges,
        &TraversalOptions {
            max_depth: 3,
            max_nodes: 500,
            reverse: true,
        },
    );
    let elapsed = started.elapsed();
    assert!(
        elapsed < WALK_BUDGET,
        "reverse impact from a 100k-caller hub took {elapsed:?}"
    );
    assert!(
        walk.visited_nodes.len() <= 500,
        "node cap exceeded: {}",
        walk.visited_nodes.len()
    );
    // Class A: the answer is a lower bound and must say so.
    assert!(
        walk.stop.is_incomplete(),
        "a walk that saw 500 of 100,000 callers reported itself complete"
    );
    let reason = walk
        .stop
        .reason(3, 500)
        .expect("an incomplete walk must carry a reason");
    assert!(
        reason.contains("lower bound"),
        "the reason must say the result is a lower bound, got: {reason}"
    );
}

/// A cycle does not make a bounded walk run forever.
///
/// Mutual recursion is ordinary in real code, and the `visited`/`enqueued`
/// split (G21) is what stops a cycle from re-entering the queue. A dense cycle
/// exercises both sets at once.
#[test]
fn a_dense_cycle_terminates_and_reports_its_cap() {
    // 400 nodes, every one calling every other: 160k edges and a cycle through
    // every pair.
    let mut edges = Vec::new();
    for i in 0..400 {
        for j in 0..400 {
            if i != j {
                edges.push(edge(&format!("n{i}"), &format!("n{j}"), EdgeKind::Calls));
            }
        }
    }
    let started = Instant::now();
    let walk = traverse_graph(
        &["n0".to_string()],
        &edges,
        &TraversalOptions {
            max_depth: 1_000,
            max_nodes: 100,
            reverse: false,
        },
    );
    assert!(
        started.elapsed() < WALK_BUDGET,
        "a fully connected 400-node cycle took {:?}",
        started.elapsed()
    );
    assert!(walk.visited_nodes.len() <= 100);
    assert!(
        walk.stop.node_capped,
        "the walk stopped at its node cap and must say so: {:?}",
        walk.stop
    );
}

/// Self-loops and duplicate edges do not inflate the answer or hang the walk.
#[test]
fn self_loops_and_duplicate_edges_do_not_inflate_a_walk() {
    let mut edges = vec![edge("n0", "n0", EdgeKind::Calls); 5_000];
    edges.extend(vec![edge("n0", "n1", EdgeKind::Calls); 5_000]);
    let walk = traverse_graph(
        &["n0".to_string()],
        &edges,
        &TraversalOptions {
            max_depth: 10,
            max_nodes: 1_000,
            reverse: false,
        },
    );
    assert_eq!(
        walk.visited_nodes.len(),
        2,
        "a self-loop and a duplicated edge produced {:?}",
        walk.visited_nodes
    );
}

/// Hostile node identities are matched byte-for-byte, never reinterpreted.
///
/// `is_symbol_node` inspects the identity's text to decide whether package
/// topology may be followed. A name containing `::`, a dot, a slash or a
/// directory-traversal sequence must be treated as the opaque identity it is —
/// a walk that reinterpreted one would return edges belonging to a different
/// node, which is a Class C fabrication with the fabrication done at query time.
#[test]
fn hostile_node_identities_are_matched_verbatim() {
    let hostile = [
        "../../etc/passwd::secret",
        "a\u{202e}b.go::f",
        "sym\u{200b}bol",
        "\u{1f4a5}.go::boom",
        &"L".repeat(10_000),
        "x'; DROP TABLE generation_edges; --",
    ];
    for name in hostile {
        let target = format!("{name}::callee");
        let edges = vec![edge(name, &target, EdgeKind::Calls)];
        let walk = traverse_graph(
            &[name.to_string()],
            &edges,
            &TraversalOptions {
                max_depth: 4,
                max_nodes: 100,
                reverse: false,
            },
        );
        assert!(
            walk.visited_nodes.contains(name),
            "a hostile start identity was not matched: {:?}",
            name.chars().take(40).collect::<String>()
        );
        assert!(
            walk.visited_nodes.contains(&target),
            "the walk did not follow the edge out of a hostile identity"
        );
    }
}

/// An empty or degenerate option set is answered, not panicked on.
///
/// `max_nodes: 0` and `max_depth: 0` are reachable from a caller that computed
/// a budget and got nothing left. Both must produce a walk that says it
/// declined, rather than an empty walk that reads as "nothing is connected".
#[test]
fn degenerate_caps_decline_visibly_rather_than_reading_as_an_empty_graph() {
    let edges = vec![
        edge("a", "b", EdgeKind::Calls),
        edge("b", "c", EdgeKind::Calls),
    ];

    let none = traverse_graph(
        &["a".to_string()],
        &edges,
        &TraversalOptions {
            max_depth: 4,
            max_nodes: 0,
            reverse: false,
        },
    );
    assert!(none.visited_nodes.is_empty());
    assert_eq!(
        none.stop.starts_dropped, 1,
        "a walk with no node budget dropped its start and must count it"
    );
    assert!(none.stop.is_incomplete());

    let shallow = traverse_graph(
        &["a".to_string()],
        &edges,
        &TraversalOptions {
            max_depth: 0,
            max_nodes: 100,
            reverse: false,
        },
    );
    assert_eq!(shallow.visited_nodes.len(), 1);
    assert!(
        shallow.stop.depth_capped,
        "a zero-depth walk over a node with neighbours must report the cap"
    );

    // No starts at all is a complete answer about nothing, not a decline.
    let empty = traverse_graph(
        &[],
        &edges,
        &TraversalOptions {
            max_depth: 4,
            max_nodes: 100,
            reverse: false,
        },
    );
    assert!(!empty.stop.is_incomplete(), "{:?}", empty.stop);
}
