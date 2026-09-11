//! Adversarial input for the route node and the view that reads it.
//!
//! A route's identity is `file::VERB path`, built from three strings the
//! *source file* controls: the indexed path, the HTTP method, and the path
//! pattern. None of them is validated by the extractor, because a route is
//! recorded from whatever the decorator or the registration call says. So every
//! separator in that format — the `::`, the space — can appear inside the parts
//! it separates, and the reader has to survive it.
//!
//! What must hold, whatever the input:
//!
//! * one node per route, and the `routes_to` edge finds it;
//! * `edge_endpoints_without_node` stays 0 for a bound route — a dangling
//!   endpoint is the defect the node emission exists to remove, and it must not
//!   come back through an odd path;
//! * a route that collides with an existing node id is *dropped and counted*,
//!   never emitted as a second node under the same id;
//! * the view never invents a verb, and never loses one.

use std::collections::BTreeMap;

use devmap_extract::extract_file;
use devmap_extract::model::{ExtractedRoute, Extraction, Span};
use devmap_query::{generate_code_graph_json, FreshnessInfo, StampedFreshness};
use serde_json::Value;

fn freshness() -> FreshnessInfo {
    FreshnessInfo {
        head_sha: String::new(),
        generation_id: 1,
        pending_count: 0,
        stamped: StampedFreshness {
            generated_head: None,
            indexed_hash: None,
            content_fingerprint: None,
        },
    }
}

/// A real extraction of `source`, with `routes` attached.
///
/// Parsed rather than synthesised so the symbols, spans and language are the
/// ones the kernel actually produces — the collision test below depends on a
/// real symbol node being emitted before the route loop runs.
fn extraction(path: &str, source: &str, routes: Vec<ExtractedRoute>) -> Extraction {
    let mut ext = extract_file(path, source);
    ext.routes = routes;
    ext
}

fn route(verb: &str, path: &str) -> ExtractedRoute {
    ExtractedRoute {
        framework: "flask".to_string(),
        http_method: verb.to_string(),
        path_pattern: path.to_string(),
        handler_name: "h".to_string(),
        span: Span {
            start_byte: 0,
            end_byte: 1,
        },
    }
}

fn graph(extractions: &[Extraction]) -> Value {
    let analysis = devmap_analyze::AnalysisSummary::default();
    let json = generate_code_graph_json(extractions, &analysis, &[], &freshness(), None)
        .expect("the graph must render");
    serde_json::from_str(&json).expect("valid JSON")
}

/// Every separator in the id format, inside the parts it separates.
///
/// `::` in the path, a space in the path, a newline, a tab, a lone `:`, an
/// empty path, an empty verb, and four-byte UTF-8. Each is one route and must
/// produce exactly one node, with the path preserved byte for byte in
/// `extras.route` — the extras are what the reader uses, and they are what
/// keeps an odd path from having to survive a round trip through a formatted
/// string.
#[test]
fn a_route_whose_path_contains_the_id_separators_is_still_one_node() {
    let hostile = [
        ("GET", "/a/b::c/d"),
        ("POST", "/has a space"),
        ("PUT", "/tab\there"),
        ("DELETE", "/new\nline"),
        ("PATCH", "/colon:only"),
        ("HEAD", ""),
        ("", "/no/verb"),
        ("GET", "/emoji/🧭/path"),
        ("GET", "/::"),
        ("ANY", "::"),
    ];
    let routes: Vec<ExtractedRoute> = hostile
        .iter()
        .map(|(verb, path)| route(verb, path))
        .collect();
    let value = graph(&[extraction("api.py", "def h(): pass\n", routes)]);

    let nodes: Vec<&Value> = value["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|node| node["kind"] == "route")
        .collect();
    assert_eq!(
        nodes.len(),
        hostile.len(),
        "one node per route, whatever the path: {nodes:?}"
    );

    let by_path: BTreeMap<&str, &Value> = nodes
        .iter()
        .map(|node| (node["extras"]["route"].as_str().unwrap(), *node))
        .collect();
    for (verb, path) in hostile {
        let node = by_path
            .get(path)
            .unwrap_or_else(|| panic!("no node for {path:?}: {by_path:?}"));
        assert_eq!(
            node["extras"]["route"], path,
            "the path survives byte for byte"
        );
        assert_eq!(node["extras"]["verb"], verb, "the verb is never invented");
        assert_eq!(node["extras"]["framework"], "flask");
    }
    assert_eq!(
        value["meta"]["devmap_rust"]["duplicate_node_ids_dropped"], 0,
        "these are ten distinct routes"
    );
}

/// The same method and path twice in one file is one node and a counted drop.
///
/// Two nodes sharing an id is worse than one: a consumer indexing by id keeps
/// whichever came last, so every edge naming that id points at a coin flip.
#[test]
fn a_route_declared_twice_in_one_file_is_dropped_and_counted() {
    let value = graph(&[extraction(
        "api.py",
        "def h(): pass\n",
        vec![
            route("GET", "/dup"),
            route("GET", "/dup"),
            route("PUT", "/dup"),
        ],
    )]);
    let routes: Vec<&Value> = value["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|node| node["kind"] == "route")
        .collect();
    assert_eq!(routes.len(), 2, "GET /dup once, PUT /dup once");
    assert_eq!(
        value["meta"]["devmap_rust"]["duplicate_node_ids_dropped"], 1,
        "the drop is reported, not silent"
    );
}

/// The same method and path in two files is two nodes, not one.
///
/// This is why the file is in the identity. Two Flask blueprints each serving
/// `GET /health` are two routes; collapsing them would drop a node and strand
/// whichever edge named it.
#[test]
fn one_method_and_path_in_two_files_stays_two_nodes() {
    let value = graph(&[
        extraction("a.py", "def h(): pass\n", vec![route("GET", "/health")]),
        extraction("b.py", "def h(): pass\n", vec![route("GET", "/health")]),
    ]);
    let ids: Vec<&str> = value["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|node| node["kind"] == "route")
        .map(|node| node["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["a.py::GET /health", "b.py::GET /health"]);
    assert_eq!(
        value["meta"]["devmap_rust"]["duplicate_node_ids_dropped"],
        0
    );
}

/// A symbol whose qualified name is exactly a route's id wins, and the route is
/// counted as the drop.
///
/// Contrived in Python and reachable in a language with liberal identifiers,
/// but the direction matters more than the likelihood: symbols are emitted
/// first, so the collision resolves toward the *declaration*, and the route is
/// dropped through the same counter as any other duplicate rather than
/// overwriting a node another edge already names.
#[test]
fn a_symbol_colliding_with_a_route_id_is_not_overwritten() {
    let mut ext = extraction("api.py", "def h(): pass\n", vec![route("GET", "/x")]);
    // A declaration whose qualified name is exactly the id the route would
    // claim. Contrived in Python; reachable in a language with liberal
    // identifiers, and the *direction* of the resolution is what this pins.
    let target = ext
        .symbols
        .iter_mut()
        .find(|symbol| symbol.name == "h")
        .expect("the parsed file declares h");
    target.name = "GET /x".to_string();
    target.qualified_name = "api.py::GET /x".to_string();
    let value = graph(&[ext]);

    let matching: Vec<&Value> = value["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|node| node["id"] == "api.py::GET /x")
        .collect();
    assert_eq!(matching.len(), 1, "one node per id, always");
    assert_eq!(
        matching[0]["kind"], "function",
        "the declaration keeps the id it was emitted under"
    );
    assert_eq!(
        value["meta"]["devmap_rust"]["duplicate_node_ids_dropped"],
        1
    );
}

/// A thousand routes in one file render, and every one is a node.
///
/// The route loop runs per file inside the node loop; nothing about it is
/// bounded, and nothing about it should be — a route dropped for want of budget
/// would under-report the API surface. This is the assertion that it scales
/// linearly rather than, say, re-scanning the node index per route.
#[test]
fn a_thousand_routes_in_one_file_all_become_nodes() {
    let routes: Vec<ExtractedRoute> = (0..1000)
        .map(|i| route("GET", &format!("/r/{i}")))
        .collect();
    let value = graph(&[extraction("api.py", "def h(): pass\n", routes)]);
    let count = value["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|node| node["kind"] == "route")
        .count();
    assert_eq!(count, 1000);
    assert_eq!(
        value["meta"]["devmap_rust"]["duplicate_node_ids_dropped"],
        0
    );
}

/// A bound route with a hostile path leaves no dangling endpoint.
///
/// The whole point of the node is that the `routes_to` edge names something. A
/// path carrying the id format's own separators is where that would break
/// first, so this runs the real resolver over a real file rather than
/// hand-building an edge: if `ExtractedRoute::node_id` and the graph export
/// ever disagree about how to spell one of these, the count moves off zero.
#[test]
fn a_bound_route_with_a_hostile_path_leaves_no_dangling_endpoint() {
    use std::collections::BTreeSet;

    let source = "def h():\n    return 1\n";
    let mut ext = extract_file("api.py", source);
    ext.routes = vec![
        route("GET", "/a/b::c/d"),
        route("POST", "/has a space"),
        route("ANY", "::"),
    ];
    let extractions = [ext];

    let mut resolver = devmap_resolve::Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    // Three route edges. The resolver also emits the file's `Contains` edge,
    // which is not what this counts.
    let bound = resolution
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == devmap_extract::model::EdgeKind::HandlesRoute)
        .count();
    assert_eq!(
        bound, 3,
        "each route binds to the one handler named h: {:?}",
        resolution.edges
    );

    let analysis = devmap_analyze::AnalysisSummary::default();
    let json = generate_code_graph_json(
        &extractions,
        &analysis,
        &resolution.edges,
        &freshness(),
        None,
    )
    .unwrap();
    let value: Value = serde_json::from_str(&json).unwrap();

    let ids: BTreeSet<&str> = value["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| node["id"].as_str().unwrap())
        .collect();
    let routes_to: Vec<&Value> = value["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|edge| edge["kind"] == "routes_to")
        .collect();
    assert_eq!(routes_to.len(), 3);
    for edge in routes_to {
        for endpoint in ["source", "target"] {
            let name = edge[endpoint].as_str().unwrap();
            assert!(
                ids.contains(name),
                "the edge's {endpoint} {name:?} names no node: {ids:?}"
            );
        }
    }
    assert_eq!(
        value["meta"]["devmap_rust"]["edge_endpoints_without_node"], 0,
        "a bound route leaves nothing dangling, whatever its path"
    );
}
