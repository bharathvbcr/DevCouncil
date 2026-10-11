//! Swift protocols, receivers and targets: what a call written against a
//! protocol means, which names a callable binds itself, and which target a
//! folder compiled into two of them belongs to.
//!
//! The accuracy fixture (`rust/testdata/accuracy/swift`) is the measuring stick;
//! these tests pin the mechanisms behind its rows in isolation, so a regression
//! names the rule that broke rather than a row id.

use devmap_extract::model::{EdgeKind, Extraction};
use devmap_extract::{extract_file, ProjectManifests, SwiftTarget};
use devmap_resolve::model::{Resolution, ResolutionKind, ResolutionResult, ResolvedEdge};
use devmap_resolve::Resolver;

fn resolve_with(files: &[(&str, &str)], manifests: Option<&ProjectManifests>) -> ResolutionResult {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    if let Some(manifests) = manifests {
        resolver.index_project_manifests(manifests);
    }
    resolver.index_extractions(&extractions);
    resolver.resolve_all(&extractions).unwrap()
}

fn resolve(files: &[(&str, &str)]) -> ResolutionResult {
    resolve_with(files, None)
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

const PROTOCOL: &str = "protocol Greeter {\n    func greet() -> String\n}\n\
struct English: Greeter {\n    func greet() -> String { \"hi\" }\n}\n\
struct French: Greeter {\n    func greet() -> String { \"salut\" }\n}\n\
func greetAll() {\n    let g: Greeter = French()\n    _ = g.greet()\n}\n";

#[test]
fn a_call_through_a_protocol_is_a_direct_edge_and_a_dispatch_per_conformer() {
    let result = resolve(&[("Sources/App/G.swift", PROTOCOL)]);
    let edges = calls_from(&result, "greetAll");
    let direct: Vec<_> = edges
        .iter()
        .filter(|edge| edge.target_symbol.ends_with("Greeter.greet"))
        .collect();
    assert_eq!(direct.len(), 1, "{edges:?}");
    assert_eq!(kind(direct[0]), ResolutionKind::ReceiverType);
    let mut dispatched: Vec<&str> = edges
        .iter()
        .filter(|edge| kind(edge) == ResolutionKind::Dispatch)
        .map(|edge| edge.target_symbol.rsplit("::").next().unwrap())
        .collect();
    dispatched.sort();
    assert_eq!(dispatched, vec!["English.greet", "French.greet"]);
    for edge in edges.iter().filter(|e| kind(e) == ResolutionKind::Dispatch) {
        assert!(
            edge.details
                .as_deref()
                .is_some_and(|via| via.contains("greet")),
            "a dispatch edge names the protocol method it was reached through: {edge:?}"
        );
        let resolution = edge.resolution.as_deref().unwrap();
        assert!(
            resolution.is_unproven_candidate_set(),
            "two conformers are candidates"
        );
    }
}

#[test]
fn dispatch_past_the_fan_out_cap_is_one_ledger_row_and_no_edges() {
    let mut source = String::from("protocol Greeter {\n    func greet() -> String\n}\n");
    for index in 0..17 {
        source.push_str(&format!(
            "struct T{index}: Greeter {{\n    func greet() -> String {{ \"{index}\" }}\n}}\n"
        ));
    }
    source.push_str("func run(_ g: Greeter) {\n    _ = g.greet()\n}\n");
    let result = resolve(&[("Sources/App/G.swift", &source)]);
    let edges = calls_from(&result, "run");
    assert!(
        edges
            .iter()
            .all(|edge| kind(edge) != ResolutionKind::Dispatch),
        "17 candidates is past the cap of 16: {edges:?}"
    );
    assert!(
        edges
            .iter()
            .any(|edge| edge.target_symbol.ends_with("Greeter.greet")),
        "the direct edge to the protocol method stays"
    );
    assert!(
        result
            .unresolved
            .iter()
            .any(|row| row.callee_name == "greet" && row.source_symbol.ends_with("run")),
        "the capped fan-out is recorded, not dropped"
    );
}

#[test]
fn a_conformance_to_an_indexed_protocol_is_implements_not_extends() {
    let result = resolve(&[("Sources/App/G.swift", PROTOCOL)]);
    assert!(
        result
            .edges
            .iter()
            .any(|edge| edge.edge_kind == EdgeKind::Implements
                && edge.source_symbol.ends_with("English")
                && edge.target_symbol.ends_with("Greeter")),
        "{:?}",
        result
            .edges
            .iter()
            .map(|e| (&e.source_symbol, &e.target_symbol, e.edge_kind))
            .collect::<Vec<_>>()
    );
    assert!(
        !result
            .edges
            .iter()
            .any(|edge| edge.edge_kind == EdgeKind::Extends
                && edge.target_symbol.ends_with("Greeter")),
        "a protocol is implemented, never extended"
    );
}

#[test]
fn a_protocol_extension_default_is_reached_from_a_conformer_value() {
    let result = resolve(&[(
        "Sources/App/G.swift",
        "protocol Greeter {\n    func greet() -> String\n}\n\
         extension Greeter {\n    func hello() -> String { greet() }\n}\n\
         struct English: Greeter {\n    func greet() -> String { \"hi\" }\n}\n\
         func use() {\n    let e = English()\n    _ = e.hello()\n}\n",
    )]);
    assert!(
        calls_from(&result, "use")
            .iter()
            .any(|edge| edge.target_symbol.ends_with("Greeter.hello")),
        "e.hello() names the default the conformer inherits"
    );
    // And the default's own `greet()` is a call through the protocol.
    assert!(
        calls_from(&result, "Greeter.hello")
            .iter()
            .any(|edge| kind(edge) == ResolutionKind::Dispatch
                && edge.target_symbol.ends_with("English.greet")),
        "implicit self inside a protocol extension dispatches to the conformers"
    );
}

#[test]
fn a_parameter_of_another_function_never_types_a_local_of_this_one() {
    let result = resolve(&[(
        "Sources/App/C.swift",
        "struct Alpha { func run() -> Int { 1 } }\n\
         struct Beta { func run() -> Int { 2 } }\n\
         func external() -> Beta { Beta() }\n\
         func usesParam(_ s: Alpha) -> Int { s.run() }\n\
         func usesLocal() -> Int { let s = external(); return s.run() }\n\
         func usesClosure(_ xs: [Beta]) -> Int { xs.map { s in s.run() }.count }\n",
    )]);
    for caller in ["usesLocal", "usesClosure"] {
        let edges = calls_from(&result, caller);
        assert!(
            !edges
                .iter()
                .any(|edge| edge.target_symbol.ends_with("Alpha.run")),
            "{caller} bound Alpha.run from another function's parameter: {edges:?}"
        );
        assert!(
            edges
                .iter()
                .any(|edge| edge.target_symbol.ends_with("Beta.run")),
            "{caller} should reach Beta.run: {edges:?}"
        );
    }
    assert!(calls_from(&result, "usesParam")
        .iter()
        .any(|edge| edge.target_symbol.ends_with("Alpha.run")));
}

#[test]
fn a_closure_property_called_in_a_method_is_not_a_free_function_of_that_name() {
    let result = resolve(&[(
        "Sources/App/J.swift",
        "struct Job {\n    let onDone: () -> Void\n    func finish() { onDone() }\n}\n\
         func onDone() {}\n",
    )]);
    assert!(
        !calls_from(&result, "Job.finish")
            .iter()
            .any(|edge| edge.target_symbol.ends_with("onDone")),
        "the call is of the stored property"
    );
}

#[test]
fn a_call_that_is_the_right_operand_of_a_sum_is_still_a_call() {
    let result = resolve(&[(
        "Sources/App/O.swift",
        "func tail() -> String { \"\" }\nfunc build() -> String { \"x\" + tail() }\n",
    )]);
    assert!(
        calls_from(&result, "build")
            .iter()
            .any(|edge| edge.target_symbol.ends_with("tail")),
        "`\"x\" + tail()` is a call of tail"
    );
}

#[test]
fn an_operator_overload_is_called_by_its_operator_syntax() {
    let result = resolve(&[(
        "Sources/App/V.swift",
        "struct Vec { var x: Int\n    static func + (l: Vec, r: Vec) -> Vec { Vec(x: l.x + r.x) } }\n\
         func add(_ a: Vec, _ b: Vec) -> Vec { a + b }\n",
    )]);
    assert!(
        calls_from(&result, "add")
            .iter()
            .any(|edge| edge.target_symbol.ends_with("Vec.+")),
        "a + b on two Vec values is Vec.+"
    );
}

#[test]
fn a_function_named_by_its_type_is_a_reference() {
    let result = resolve(&[(
        "Sources/App/R.swift",
        "struct Meter { static func double(_ x: Int) -> Int { x * 2 } }\n\
         func run() -> [Int] { [1, 2].map(Meter.double) }\n",
    )]);
    assert!(
        result
            .edges
            .iter()
            .any(|edge| edge.source_symbol.ends_with("run")
                && edge.target_symbol.ends_with("Meter.double")),
        "Meter.double passed by name is a use of the method"
    );
}

fn two_targets() -> ProjectManifests {
    ProjectManifests {
        swift_targets: vec![
            SwiftTarget {
                name: "AppTarget".to_string(),
                paths: vec!["ios/AppTarget".to_string(), "ios/Shared".to_string()],
                package: false,
            },
            SwiftTarget {
                name: "WidgetTarget".to_string(),
                paths: vec!["ios/WidgetTarget".to_string(), "ios/Shared".to_string()],
                package: false,
            },
        ],
        ..ProjectManifests::default()
    }
}

const TARGET_FILES: &[(&str, &str)] = &[
    (
        "ios/AppTarget/Theme.swift",
        "struct Theme { static func accent() -> String { \"app\" } }\n",
    ),
    (
        "ios/WidgetTarget/Theme.swift",
        "struct Theme { static func accent() -> String { \"widget\" } }\n",
    ),
    (
        "ios/Shared/Formatter.swift",
        "func sharedFormat() -> String { Theme.accent() }\n",
    ),
];

#[test]
fn a_folder_compiled_into_two_targets_reaches_each_targets_own_type() {
    let result = resolve_with(TARGET_FILES, Some(&two_targets()));
    let mut reached: Vec<&str> = calls_from(&result, "sharedFormat")
        .iter()
        .map(|edge| edge.target_file.as_str())
        .collect();
    reached.sort();
    assert_eq!(
        reached,
        vec!["ios/AppTarget/Theme.swift", "ios/WidgetTarget/Theme.swift"]
    );
}

#[test]
fn without_the_manifest_the_shared_folder_names_neither_target() {
    // The path heuristic puts `Shared/` in a module of its own, so `Theme` is
    // two unrelated declarations and no edge is claimed.
    let result = resolve_with(TARGET_FILES, None);
    assert!(calls_from(&result, "sharedFormat").iter().all(|edge| {
        edge.target_file == "ios/Shared/Formatter.swift"
            || kind(edge) != ResolutionKind::ReceiverType
    }));
}

#[test]
fn an_own_module_type_shadows_the_same_name_an_import_brings_in() {
    let result = resolve(&[
        (
            "Sources/CoreKit/Engine.swift",
            "public struct Theme { public init() {} }\n",
        ),
        ("Sources/App/Theme.swift", "struct Theme { init() {} }\n"),
        (
            "Sources/App/Home.swift",
            "import CoreKit\nfunc make() -> Theme { Theme() }\n",
        ),
    ]);
    let reached: Vec<&str> = result
        .edges
        .iter()
        .filter(|edge| {
            edge.source_file == "Sources/App/Home.swift" && edge.source_symbol.ends_with("make")
        })
        .map(|edge| edge.target_file.as_str())
        .collect();
    assert!(reached.contains(&"Sources/App/Theme.swift"), "{reached:?}");
    assert!(
        !reached.contains(&"Sources/CoreKit/Engine.swift"),
        "an imported Theme must not outrank the module's own: {reached:?}"
    );
}

#[test]
fn a_nested_function_is_not_a_function_of_the_module() {
    let result = resolve(&[
        (
            "Sources/App/A.swift",
            "func outer() {\n    func helper() {}\n    helper()\n}\n",
        ),
        (
            "Sources/App/B.swift",
            "func unrelated() {\n    helper()\n}\n",
        ),
    ]);
    assert!(
        !result
            .edges
            .iter()
            .any(|edge| edge.source_symbol.ends_with("unrelated")
                && edge.target_symbol.ends_with("helper")),
        "a call in B.swift reached a function local to outer()"
    );
    let declared = extract_file(
        "Sources/App/A.swift",
        "func outer() {\n    func helper() {}\n    helper()\n}\n",
    );
    assert!(
        !declared
            .symbols
            .iter()
            .any(|symbol| symbol.name == "helper"),
        "a nested function is no module-level symbol"
    );
}
