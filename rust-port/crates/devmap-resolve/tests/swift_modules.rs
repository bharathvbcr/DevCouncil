//! Swift modules are Go packages: one unqualified namespace, named by import.
//!
//! `import Foundation` used to extract nothing, so every Swift call site that
//! named an SDK type or an XCTest assertion landed in `Unresolved` and
//! `devmap dead` set `walk_incomplete` over tens of thousands of them. Same-
//! module files import each other not at all, so `unwired_candidates` excluded
//! every `.swift` file as import-blind rather than asking whether anything
//! depended on it.
//!
//! The module is derived from the path (`Sources/<Name>/`, `app/MarkDevKit/…`).
//! A local `import Kit` lands on `module:Kit`; an SDK import that names no
//! indexed file is classified External. Same-module bare names reuse
//! `Resolution::SamePackage`.

use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, Extraction};
use devmap_resolve::model::{Resolution, ResolutionKind, ResolutionResult, UnresolvedClass};
use devmap_resolve::Resolver;

fn resolve(files: &[(&str, &str)]) -> ResolutionResult {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    resolver.resolve_all(&extractions)
}

fn kind_of(edge: &devmap_resolve::model::ResolvedEdge) -> ResolutionKind {
    edge.resolution
        .as_deref()
        .map(Resolution::kind)
        .expect("an edge the resolver built carries its own evidence")
}

#[test]
fn a_same_module_call_resolves_as_package_scope() {
    let result = resolve(&[
        ("Sources/App/Helper.swift", "func run() {}\n"),
        ("Sources/App/main.swift", "func start() {\n    run()\n}\n"),
        ("Sources/Other/Helper.swift", "func run() {}\n"),
    ]);
    let hits: Vec<_> = result
        .edges
        .iter()
        .filter(|edge| {
            edge.source_file == "Sources/App/main.swift"
                && edge.edge_kind == EdgeKind::Calls
                && edge
                    .target_symbol
                    .rsplit(['.', ':'])
                    .next()
                    .is_some_and(|tail| tail == "run")
        })
        .collect();
    assert_eq!(hits.len(), 1, "expected one same-module call, got {hits:?}");
    assert_eq!(hits[0].target_file, "Sources/App/Helper.swift");
    assert_eq!(kind_of(hits[0]), ResolutionKind::SamePackage);
}

#[test]
fn a_local_swift_import_lands_on_the_module_node() {
    let result = resolve(&[
        (
            "app/Host/main.swift",
            "import Kit\n\nfunc start() {\n    open()\n}\n",
        ),
        ("app/Kit/Store.swift", "public func open() {}\n"),
        ("app/Kit/Helpers.swift", "public func normalize() {}\n"),
    ]);
    let import_targets: Vec<&str> = result
        .edges
        .iter()
        .filter(|edge| {
            edge.edge_kind == EdgeKind::Imports && edge.source_file == "app/Host/main.swift"
        })
        .map(|edge| edge.target_file.as_str())
        .collect();
    assert_eq!(
        import_targets,
        vec!["module:Kit"],
        "import Kit must collapse onto the module, not onto every file in it"
    );
    for path in ["app/Kit/Store.swift", "app/Kit/Helpers.swift"] {
        assert!(
            result.edges.iter().any(|edge| {
                edge.edge_kind == EdgeKind::MemberOf
                    && edge.source_file == path
                    && edge.target_file == "module:Kit"
            }),
            "{path} must be a member of module:Kit"
        );
    }
}

#[test]
fn foundation_types_are_external_when_the_file_imported_foundation() {
    let result = resolve(&[(
        "Sources/App/main.swift",
        "import Foundation\n\nfunc load() {\n    _ = URL(string: \"x\")\n}\n",
    )]);
    let rows: Vec<_> = result
        .unresolved
        .iter()
        .filter(|row| row.callee_name == "URL")
        .collect();
    assert!(
        !rows.is_empty(),
        "URL() must be recorded so classification can see it: {:?}",
        result.unresolved
    );
    assert!(
        rows.iter()
            .all(|row| matches!(row.class, UnresolvedClass::External { .. })),
        "import Foundation makes URL an SDK type, not a defect: {rows:?}"
    );
}

#[test]
fn xctest_assertions_are_external_when_the_file_imported_xctest() {
    let result = resolve(&[(
        "Tests/AppTests/AppTests.swift",
        "import XCTest\n\nfunc testIt() {\n    XCTAssertEqual(1, 1)\n}\n",
    )]);
    let rows: Vec<_> = result
        .unresolved
        .iter()
        .filter(|row| row.callee_name == "XCTAssertEqual")
        .collect();
    assert!(
        !rows.is_empty(),
        "XCTAssertEqual must be recorded: {:?}",
        result.unresolved
    );
    assert!(
        rows.iter()
            .all(|row| matches!(row.class, UnresolvedClass::External { .. })),
        "import XCTest makes XCTAssertEqual library API, not a defect: {rows:?}"
    );
}

#[test]
fn swift_prelude_constructors_are_language_declared() {
    let result = resolve(&[(
        "Sources/App/main.swift",
        "func build() {\n    _ = String()\n}\n",
    )]);
    let rows: Vec<_> = result
        .unresolved
        .iter()
        .filter(|row| row.callee_name == "String")
        .collect();
    assert!(
        !rows.is_empty(),
        "String() must be recorded: {:?}",
        result.unresolved
    );
    assert!(
        rows.iter()
            .all(|row| matches!(row.class, UnresolvedClass::Builtin)),
        "String is a prelude type and needs no import: {rows:?}"
    );
}

#[test]
fn a_typed_swift_method_call_binds_the_declared_receiver() {
    let result = resolve(&[
        (
            "Sources/Kit/Reader.swift",
            "public final class Reader {\n    public func read() -> Int { return 1 }\n}\n",
        ),
        (
            "Sources/Kit/Other.swift",
            "public final class Other {\n    public func read() -> Int { return 2 }\n}\n",
        ),
        (
            "Sources/Kit/Load.swift",
            "func load(_ reader: Reader) -> Int {\n    return reader.read()\n}\n",
        ),
    ]);
    let hits: Vec<_> = result
        .edges
        .iter()
        .filter(|edge| {
            edge.edge_kind == EdgeKind::Calls
                && edge.source_file == "Sources/Kit/Load.swift"
                && edge.target_symbol.ends_with("Reader.read")
        })
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "reader.read() is typed; it must bind, not fall to AmbiguousGlobal: {:?}",
        result
            .edges
            .iter()
            .filter(|edge| edge.source_file == "Sources/Kit/Load.swift")
            .map(|edge| format!(
                "{:?} {} -> {} ({:?})",
                edge.edge_kind,
                edge.source_symbol,
                edge.target_symbol,
                kind_of(edge)
            ))
            .collect::<Vec<_>>()
    );
    assert_eq!(kind_of(hits[0]), ResolutionKind::ReceiverType);
}

#[test]
fn a_type_shared_method_call_binds_the_type_as_receiver() {
    let result = resolve(&[(
        "Sources/Kit/Renderer.swift",
        "public final class Renderer {\n    public static let shared = Renderer()\n    public func isScalable() -> Bool { return true }\n    public func check() -> Bool {\n        return Renderer.shared.isScalable()\n    }\n}\n",
    )]);
    let hits: Vec<_> = result
        .edges
        .iter()
        .filter(|edge| {
            edge.edge_kind == EdgeKind::Calls && edge.target_symbol.ends_with("isScalable")
        })
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "Renderer.shared.isScalable() names Renderer; it must bind: {:?}",
        result
            .edges
            .iter()
            .filter(|edge| edge.edge_kind == EdgeKind::Calls)
            .map(|edge| format!(
                "{} -> {} ({:?})",
                edge.source_symbol,
                edge.target_symbol,
                kind_of(edge)
            ))
            .collect::<Vec<_>>()
    );
}

/// `Type.method()` across files of one module, when another type also
/// declares the method. The global rung is AmbiguousGlobal; same-module
/// corroboration of the written type is what binds it.
#[test]
fn a_same_module_type_method_binds_across_files() {
    let result = resolve(&[
        (
            "Sources/Kit/Normalizer.swift",
            "public enum Normalizer {\n    public static func normalize() {}\n}\n",
        ),
        (
            "Sources/Kit/Other.swift",
            "public enum Other {\n    public static func normalize() {}\n}\n",
        ),
        (
            "Sources/Kit/Renderer.swift",
            "func render() {\n    Normalizer.normalize()\n}\n",
        ),
    ]);
    let hits: Vec<_> = result
        .edges
        .iter()
        .filter(|edge| {
            edge.edge_kind == EdgeKind::Calls
                && edge.source_file == "Sources/Kit/Renderer.swift"
                && edge.target_symbol.ends_with("Normalizer.normalize")
        })
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "Normalizer.normalize() is a same-module type method; it must bind, \
         not fall to AmbiguousGlobal: {:?}",
        result
            .edges
            .iter()
            .filter(|edge| edge.source_file == "Sources/Kit/Renderer.swift")
            .map(|edge| format!(
                "{:?} {} -> {} ({:?})",
                edge.edge_kind,
                edge.source_symbol,
                edge.target_symbol,
                kind_of(edge)
            ))
            .collect::<Vec<_>>()
    );
    assert_eq!(kind_of(hits[0]), ResolutionKind::ReceiverType);
    assert_eq!(hits[0].target_file, "Sources/Kit/Normalizer.swift");
}

/// Overloads of one type method are several `type_methods` hits in one
/// file. They must still corroborate `Viewer.shared.present(...)`.
#[test]
fn a_type_shared_overloaded_method_still_binds() {
    let result = resolve(&[
        (
            "Sources/Kit/Viewer.swift",
            "public final class Viewer {\n    public static let shared = Viewer()\n    public func present(_ x: Int) {}\n    public func present(_ x: String) {}\n}\n",
        ),
        (
            "Sources/Kit/Other.swift",
            "public final class Other {\n    public func present(_ x: Int) {}\n}\n",
        ),
        (
            "Sources/Kit/Host.swift",
            "func show() {\n    Viewer.shared.present(1)\n}\n",
        ),
    ]);
    let hits: Vec<_> = result
        .edges
        .iter()
        .filter(|edge| {
            edge.edge_kind == EdgeKind::Calls
                && edge.source_file == "Sources/Kit/Host.swift"
                && edge.target_symbol.contains("Viewer.present")
        })
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "overloaded Viewer.present must bind through Viewer.shared: {:?}",
        result
            .edges
            .iter()
            .filter(|edge| edge.source_file == "Sources/Kit/Host.swift")
            .map(|edge| format!(
                "{:?} {} -> {} ({:?})",
                edge.edge_kind,
                edge.source_symbol,
                edge.target_symbol,
                kind_of(edge)
            ))
            .collect::<Vec<_>>()
    );
    assert_eq!(kind_of(hits[0]), ResolutionKind::ReceiverType);
}

#[test]
fn a_foundation_method_on_a_typed_url_is_external() {
    let result = resolve(&[(
        "Sources/App/Paths.swift",
        "import Foundation\n\nfunc child(_ url: URL) -> URL {\n    return url.appendingPathComponent(\"x\")\n}\n",
    )]);
    let rows: Vec<_> = result
        .unresolved
        .iter()
        .filter(|row| row.callee_name == "appendingPathComponent")
        .collect();
    assert!(
        !rows.is_empty(),
        "appendingPathComponent must be recorded: unresolved={:?}",
        result.unresolved
    );
    assert!(
        rows.iter()
            .all(|row| matches!(row.class, UnresolvedClass::External { .. })),
        "URL is a Foundation type and this file imported Foundation: {rows:?}"
    );
}

#[test]
fn map_on_an_array_is_not_classified_external() {
    let result = resolve(&[(
        "Sources/App/Ids.swift",
        "import Foundation\n\nfunc ids(_ xs: [Int]) -> [Int] {\n    return xs.map { $0 }\n}\nfunc map(_ xs: [Int]) -> [Int] { return xs }\n",
    )]);
    let rows: Vec<_> = result
        .unresolved
        .iter()
        .filter(|row| row.callee_name == "map")
        .collect();
    assert!(
        rows.iter()
            .all(|row| !matches!(row.class, UnresolvedClass::External { .. })),
        "map is a name the repository can own; Foundation must not claim it: {rows:?}"
    );
}

#[test]
fn a_url_without_importing_foundation_is_not_classified_external() {
    let result = resolve(&[(
        "Sources/App/main.swift",
        "func load() {\n    _ = URL(string: \"x\")\n}\n",
    )]);
    let rows: Vec<_> = result
        .unresolved
        .iter()
        .filter(|row| row.callee_name == "URL")
        .collect();
    assert!(
        !rows.is_empty(),
        "URL() without an import must still be recorded: {:?}",
        result.unresolved
    );
    assert!(
        rows.iter()
            .all(|row| !matches!(row.class, UnresolvedClass::External { .. })),
        "classifying URL as External without import Foundation would hide a \
         name the repository might own: {rows:?}"
    );
}
