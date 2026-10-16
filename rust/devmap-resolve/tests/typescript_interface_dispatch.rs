//! A TypeScript call written against an interface: a direct edge to the
//! interface method, and one `Dispatch` edge per in-repo implementer.
//!
//! Precision of those edges is bounded by the implementer count, not by
//! correctness. The fan-out cap of 16 is the shared emission
//! `dispatch_through_interface` pins in `swift_protocol_dispatch`.

use devmap_extract::model::{EdgeKind, SymbolKind};
use devmap_extract::{extract_file, Extraction};
use devmap_resolve::model::{Resolution, ResolutionKind, ResolutionResult, ResolvedEdge};
use devmap_resolve::Resolver;

fn resolve(files: &[(&str, &str)]) -> (Vec<Extraction>, ResolutionResult) {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    (extractions, resolution)
}

fn calls_from<'a>(result: &'a ResolutionResult, caller: &str) -> Vec<&'a ResolvedEdge> {
    result
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls && edge.source_symbol.ends_with(caller))
        .collect()
}

fn kind(edge: &ResolvedEdge) -> ResolutionKind {
    edge.resolution
        .as_deref()
        .map(Resolution::kind)
        .expect("evidence")
}

const SOURCE: &str = "\
interface Client { get(k: string): string }\n\
interface Paper { title: string }\n\
class MemClient implements Client { get(k: string): string { return k } }\n\
class DiskClient implements Client { get(k: string): string { return k } }\n\
function useClient(c: Client): string { return c.get('a') }\n\
function viaConcrete(m: MemClient): string { return m.get('b') }\n\
class Overload {\n\
  area(): number;\n\
  area(): number { return 1 }\n\
}\n";

#[test]
fn a_call_through_a_typescript_interface_is_a_direct_edge_and_a_dispatch_per_implementer() {
    let (extractions, result) = resolve(&[("c.ts", SOURCE)]);
    let symbols = &extractions[0].symbols;
    assert!(
        symbols.iter().any(|symbol| {
            symbol.kind == SymbolKind::Method && symbol.qualified_name.ends_with("Client.get")
        }),
        "an interface method is a symbol: {symbols:?}"
    );
    assert!(
        !symbols
            .iter()
            .any(|symbol| symbol.qualified_name.ends_with("Paper.title")),
        "a property signature is not a method"
    );
    assert_eq!(
        symbols
            .iter()
            .filter(|symbol| symbol.qualified_name.ends_with("Overload.area"))
            .count(),
        1,
        "a class overload signature is not a second symbol: {symbols:?}"
    );

    let edges = calls_from(&result, "useClient");
    let direct: Vec<_> = edges
        .iter()
        .filter(|edge| {
            kind(edge) == ResolutionKind::ReceiverType
                && edge.target_symbol.ends_with("::Client.get")
        })
        .collect();
    assert_eq!(direct.len(), 1, "{edges:?}");
    assert_eq!(kind(direct[0]), ResolutionKind::ReceiverType);
    let mut dispatched: Vec<&str> = edges
        .iter()
        .filter(|edge| kind(edge) == ResolutionKind::Dispatch)
        .map(|edge| edge.target_symbol.rsplit("::").next().unwrap())
        .collect();
    dispatched.sort();
    assert_eq!(dispatched, vec!["DiskClient.get", "MemClient.get"]);
    for edge in edges
        .iter()
        .filter(|edge| kind(edge) == ResolutionKind::Dispatch)
    {
        assert!(
            edge.details
                .as_deref()
                .is_some_and(|via| via.contains("get")),
            "a dispatch edge names the interface method: {edge:?}"
        );
        assert!(
            edge.resolution
                .as_deref()
                .is_some_and(Resolution::is_unproven_candidate_set),
            "two implementers are candidates"
        );
    }

    let concrete = calls_from(&result, "viaConcrete");
    assert!(
        concrete
            .iter()
            .any(|edge| kind(edge) == ResolutionKind::ReceiverType
                && edge.target_symbol.ends_with("MemClient.get")),
        "a class-typed receiver stays a direct edge: {concrete:?}"
    );
    assert!(
        concrete
            .iter()
            .all(|edge| kind(edge) != ResolutionKind::Dispatch),
        "a class is not dispatched: {concrete:?}"
    );
}
