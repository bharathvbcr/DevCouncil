//! A Java record is a type, its components are methods, and an enum constant is
//! a member of its enum.
//!
//! `record_declaration` was a node kind the declaration table did not list, so a
//! record emitted no symbol and everything inside it was owned by the file (or
//! by the class that happened to enclose it). Measured on Orbit's
//! `java/records` fixture before this change: `Point.java` produced nothing,
//! `Range.java` produced `Range` and `length` as bare file-level functions, and
//! every call into a record — `new Point(0, 0)`, `p.x()`, `r.length()` — had no
//! target. An enum constant (`EARTH` in `enum Planet`) produced nothing either,
//! so `Planet.EARTH` named no node.
//!
//! The rules under test live in `devmap-extract`'s `langdecl/java.rs`; the
//! resolver half is checked here because a call edge is what proves a symbol is
//! joinable.

use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, Extraction, SymbolKind};
use devmap_resolve::model::{ResolutionResult, UnresolvedClass, UnresolvedKind};
use devmap_resolve::Resolver;

const POINT: &str = "package p;\npublic record Point(int x, int y) {}\n";
const POINT_USER: &str = "package p;\n\
public class PointUser {\n\
    public Point origin() { return new Point(0, 0); }\n\
    public String describe(Point q) { return q.x() + \",\" + q.y(); }\n\
}\n";
const RANGE: &str = "package p;\n\
public record Range(int start, int end) {\n\
    public Range(int start, int end) {\n\
        this.start = start;\n\
        this.end = end;\n\
    }\n\
    public int length() { return end - start; }\n\
}\n";
const RANGE_USER: &str = "package p;\n\
public class RangeUser {\n\
    public int span(Range r) { return r.length(); }\n\
}\n";
const BOUNDS: &str = "package p;\n\
public record Bounds(int lo, int hi) {\n\
    public Bounds {\n\
        if (lo > hi) throw new IllegalArgumentException(\"lo\");\n\
    }\n\
}\n";
const WRAPPED: &str = "package p;\n\
public record Wrapped(int raw) {\n\
    public int raw() { return raw < 0 ? 0 : raw; }\n\
}\n";
const WRAPPED_USER: &str = "package p;\n\
public class WrappedUser {\n\
    public int read(Wrapped w) { return w.raw(); }\n\
}\n";
const SHAPE: &str = "package p;\npublic sealed interface Shape permits Circle {}\n";
const CIRCLE: &str = "package p;\npublic record Circle(int radius) implements Shape {}\n";
const OUTER: &str = "package p;\n\
public class Outer {\n\
    public record Inner(int value) {\n\
        public int doubled() { return value * 2; }\n\
    }\n\
    public Inner make(int v) { return new Inner(v); }\n\
    public int use(Inner i) { return i.doubled(); }\n\
}\n";
const PLANET: &str = "package p;\n\
public enum Planet implements Describable {\n\
    MERCURY(1), EARTH(3) {\n\
        public String describe() { return \"home\"; }\n\
    };\n\
    private final int order;\n\
    Planet(int order) { this.order = order; }\n\
    public String describe() { return \"planet\"; }\n\
}\n";
const DESCRIBABLE: &str = "package p;\npublic interface Describable { String describe(); }\n";

fn files(all: &[(&str, &str)]) -> Vec<Extraction> {
    all.iter()
        .map(|(path, source)| extract_file(path, source))
        .collect()
}

fn resolve(all: &[(&str, &str)]) -> (Vec<Extraction>, ResolutionResult) {
    let extractions = files(all);
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let result = resolver.resolve_all(&extractions).unwrap();
    (extractions, result)
}

fn corpus() -> Vec<(&'static str, &'static str)> {
    vec![
        ("p/Point.java", POINT),
        ("p/PointUser.java", POINT_USER),
        ("p/Range.java", RANGE),
        ("p/RangeUser.java", RANGE_USER),
        ("p/Bounds.java", BOUNDS),
        ("p/Wrapped.java", WRAPPED),
        ("p/WrappedUser.java", WRAPPED_USER),
        ("p/Shape.java", SHAPE),
        ("p/Circle.java", CIRCLE),
        ("p/Outer.java", OUTER),
        ("p/Planet.java", PLANET),
        ("p/Describable.java", DESCRIBABLE),
    ]
}

fn symbol<'a>(
    extractions: &'a [Extraction],
    file: &str,
    qualified: &str,
) -> Vec<&'a devmap_extract::model::ExtractedSymbol> {
    extractions
        .iter()
        .filter(|extraction| extraction.file_path == file)
        .flat_map(|extraction| extraction.symbols.iter())
        .filter(|symbol| symbol.qualified_name == qualified)
        .collect()
}

/// What the resolver left unresolved, compactly, so a failure names the call.
fn gaps(result: &ResolutionResult) -> String {
    result
        .unresolved
        .iter()
        .filter(|row| {
            !matches!(
                row.callee_name.as_str(),
                "String" | "IllegalArgumentException"
            )
        })
        .map(|row| {
            format!(
                "{} -> {}{} {:?}",
                row.source_symbol,
                row.receiver
                    .as_deref()
                    .map(|receiver| format!("{receiver}."))
                    .unwrap_or_default(),
                row.callee_name,
                row.class
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn edge(result: &ResolutionResult, kind: EdgeKind, from: &str, to: &str) -> bool {
    result.edges.iter().any(|edge| {
        edge.edge_kind == kind && edge.source_symbol == from && edge.target_symbol == to
    })
}

#[test]
fn a_record_is_a_class_symbol_and_owns_its_methods() {
    let extractions = files(&corpus());
    let point = symbol(&extractions, "p/Point.java", "p/Point.java::Point");
    assert_eq!(point.len(), 1, "a record is a symbol");
    assert_eq!(point[0].kind, SymbolKind::Class);
    // An explicit method is owned by the record, not by the file.
    let length = symbol(&extractions, "p/Range.java", "p/Range.java::Range.length");
    assert_eq!(length.len(), 1);
    assert_eq!(length[0].kind, SymbolKind::Method);
    assert_eq!(
        length[0].parent_symbol.as_deref(),
        Some("p/Range.java::Range")
    );
}

#[test]
fn a_record_component_is_an_accessor_method_anchored_at_the_header() {
    let extractions = files(&corpus());
    for name in ["x", "y"] {
        let accessor = symbol(
            &extractions,
            "p/Point.java",
            &format!("p/Point.java::Point.{name}"),
        );
        assert_eq!(accessor.len(), 1, "accessor {name}");
        assert_eq!(accessor[0].kind, SymbolKind::Method);
        assert_eq!(
            accessor[0].parent_symbol.as_deref(),
            Some("p/Point.java::Point")
        );
        // Line 2 is the record header.
        assert_eq!(
            accessor[0].span.line_range(POINT).0,
            2,
            "anchored to the component"
        );
    }
}

#[test]
fn an_explicit_accessor_replaces_the_implicit_one_without_a_duplicate_identity() {
    let extractions = files(&corpus());
    let raw = symbol(
        &extractions,
        "p/Wrapped.java",
        "p/Wrapped.java::Wrapped.raw",
    );
    assert_eq!(raw.len(), 1, "two declarations of one qualified name");
    // The surviving declaration is the written method, on its own line.
    assert_eq!(raw[0].span.line_range(WRAPPED).0, 3);
}

#[test]
fn an_overload_with_parameters_does_not_replace_the_accessor() {
    let source = "package p;\n\
public record Box(int raw) {\n\
    public int raw(int scale) { return raw * scale; }\n\
}\n";
    let extractions = files(&[("p/Box.java", source)]);
    let raw = symbol(&extractions, "p/Box.java", "p/Box.java::Box.raw");
    assert_eq!(raw.len(), 2, "the accessor and the overload both exist");
}

#[test]
fn a_method_parameter_is_not_a_component() {
    let source = "package p;\n\
public record Box(int raw) {\n\
    public int twice(int factor) { return raw * factor; }\n\
}\n";
    let extractions = files(&[("p/Box.java", source)]);
    assert!(symbol(&extractions, "p/Box.java", "p/Box.java::Box.factor").is_empty());
    assert_eq!(
        symbol(&extractions, "p/Box.java", "p/Box.java::Box.raw").len(),
        1
    );
}

#[test]
fn a_compact_constructor_is_a_constructor_of_its_record() {
    let extractions = files(&corpus());
    let constructor = symbol(
        &extractions,
        "p/Bounds.java",
        "p/Bounds.java::Bounds.Bounds",
    );
    assert_eq!(constructor.len(), 1);
    assert_eq!(constructor[0].kind, SymbolKind::Method);
    let explicit = symbol(&extractions, "p/Range.java", "p/Range.java::Range.Range");
    assert_eq!(explicit.len(), 1, "an explicit canonical constructor");
}

#[test]
fn calls_into_a_record_resolve() {
    let (_, result) = resolve(&corpus());
    let unresolved = gaps(&result);
    assert!(
        edge(
            &result,
            EdgeKind::Calls,
            "p/PointUser.java::PointUser.describe",
            "p/Point.java::Point.x"
        ),
        "accessor x: {unresolved}"
    );
    assert!(
        edge(
            &result,
            EdgeKind::Calls,
            "p/PointUser.java::PointUser.describe",
            "p/Point.java::Point.y"
        ),
        "accessor y: {unresolved}"
    );
    assert!(
        result.edges.iter().any(|edge| {
            edge.source_symbol == "p/PointUser.java::PointUser.origin"
                && edge.target_symbol == "p/Point.java::Point"
        }),
        "new Point(..) names the record: {unresolved}"
    );
    assert!(
        edge(
            &result,
            EdgeKind::Calls,
            "p/RangeUser.java::RangeUser.span",
            "p/Range.java::Range.length"
        ),
        "explicit method: {unresolved}"
    );
    assert!(
        edge(
            &result,
            EdgeKind::Calls,
            "p/WrappedUser.java::WrappedUser.read",
            "p/Wrapped.java::Wrapped.raw"
        ),
        "explicit accessor: {unresolved}"
    );
}

#[test]
fn a_record_nested_in_a_class_is_reached_through_its_own_methods() {
    let (extractions, result) = resolve(&corpus());
    assert_eq!(
        symbol(&extractions, "p/Outer.java", "p/Outer.java::Inner.doubled").len(),
        1,
        "the nested record owns doubled"
    );
    assert!(
        result.edges.iter().any(|edge| {
            edge.source_symbol == "p/Outer.java::Outer.use"
                && edge.target_symbol == "p/Outer.java::Inner.doubled"
        }),
        "{}",
        gaps(&result)
    );
}

#[test]
fn a_record_implements_its_interface() {
    let (_, result) = resolve(&corpus());
    assert!(
        result.edges.iter().any(|edge| {
            edge.edge_kind == EdgeKind::Implements
                && edge.source_symbol == "p/Circle.java::Circle"
                && edge.target_symbol == "p/Shape.java::Shape"
        }),
        "{:?}",
        result
            .edges
            .iter()
            .filter(|edge| edge.source_file == "p/Circle.java")
            .collect::<Vec<_>>()
    );
}

#[test]
fn an_enum_constant_is_a_field_of_its_enum_and_the_enum_implements_its_interface() {
    let (extractions, result) = resolve(&corpus());
    for constant in ["MERCURY", "EARTH"] {
        let found = symbol(
            &extractions,
            "p/Planet.java",
            &format!("p/Planet.java::Planet.{constant}"),
        );
        assert_eq!(found.len(), 1, "{constant}");
        assert_eq!(found[0].kind, SymbolKind::Field);
        assert_eq!(
            found[0].parent_symbol.as_deref(),
            Some("p/Planet.java::Planet")
        );
    }
    assert!(
        result.edges.iter().any(|edge| {
            edge.edge_kind == EdgeKind::Implements
                && edge.source_symbol == "p/Planet.java::Planet"
                && edge.target_symbol == "p/Describable.java::Describable"
        }),
        "an enum names its interfaces"
    );
}

#[test]
fn a_local_record_keeps_its_components_under_itself() {
    // A record declared inside a method has no type owner (the walk ends at the
    // callable), but its own components still belong to it.
    let source = "package p;\n\
public class Host {\n\
    public int run() {\n\
        record Pair(int a, int b) {}\n\
        return new Pair(1, 2).a();\n\
    }\n\
}\n";
    let extractions = files(&[("p/Host.java", source)]);
    for name in ["a", "b"] {
        assert_eq!(
            symbol(
                &extractions,
                "p/Host.java",
                &format!("p/Host.java::Pair.{name}")
            )
            .len(),
            1,
            "{name}"
        );
    }
}

// `java.lang` is the last route of a Java type name's lookup: declaration, same
// package, on-demand import, then `java.lang`. A name this table owns that no
// earlier route bound is the JDK's, so its miss is `External`, as `Map` and
// `PrintStream` are through their imports. A namesake in a *different language*
// in the same corpus (a Go `String()` method, which Orbit has) must not turn the
// Java type into an unexplained row.

const PERSON: &str = "package p;\n\
import java.util.Map;\n\
public record Person(String name, int age, Map<String, Integer> tags) {\n\
    public String label() { return name; }\n\
}\n";
const GO_STRINGER: &str =
    "package pb\ntype Fmt int\nfunc (f Fmt) String() string { return \"\" }\n";

/// The class of every `Type` reference row for `name` in `file`.
fn type_rows(result: &ResolutionResult, file: &str, name: &str) -> Vec<(String, UnresolvedClass)> {
    result
        .unresolved
        .iter()
        .filter(|row| {
            row.source_file == file
                && row.callee_name == name
                && row.kind == UnresolvedKind::Reference
        })
        .map(|row| (row.source_symbol.clone(), row.class.clone()))
        .collect()
}

#[test]
fn string_in_a_record_is_external_java_lang_beside_a_foreign_namesake() {
    let (_, result) = resolve(&[("p/Person.java", PERSON), ("pb/fmt.go", GO_STRINGER)]);
    let rows = type_rows(&result, "p/Person.java", "String");
    // The component field, its implicit accessor, the explicit accessor's return
    // type and the generic argument: a record has no fewer than the component
    // and the accessor.
    assert!(
        rows.iter()
            .any(|(symbol, _)| symbol == "p/Person.java::Person.name")
            && rows
                .iter()
                .any(|(symbol, _)| symbol == "p/Person.java::Person.label"),
        "the component and the accessor's return type are rows: {rows:?}"
    );
    for (symbol, class) in &rows {
        assert_eq!(
            class,
            &UnresolvedClass::External {
                module: "java.lang.String".to_string()
            },
            "{symbol}"
        );
    }
    // The imported `Map` is the control: same lookup, already external.
    assert!(
        type_rows(&result, "p/Person.java", "Map")
            .iter()
            .all(|(_, class)| matches!(class, UnresolvedClass::External { module } if module == "java.util.Map")),
    );
}

#[test]
fn a_java_declaration_of_the_name_keeps_the_miss_unexplained() {
    // Fail open: a JVM-language declaration of the name anywhere in the corpus
    // is evidence the miss may be a bind failure, not the JDK's type. (A method
    // is the declaration that does not bind a type reference, so the miss stays.)
    let (_, result) = resolve(&[
        ("p/Person.java", PERSON),
        (
            "q/Util.java",
            "package q;\npublic class Util { public static int String() { return 1; } }\n",
        ),
    ]);
    let rows = type_rows(&result, "p/Person.java", "String");
    assert!(
        !rows.is_empty()
            && rows
                .iter()
                .all(|(_, class)| matches!(class, UnresolvedClass::Unresolved)),
        "{rows:?}"
    );
}

#[test]
fn a_csharp_string_is_not_java_lang() {
    // `CStyle` is one family for C, C++, C#, Objective-C and Java; the table is
    // Java's alone.
    let (_, result) = resolve(&[
        (
            "n/Person.cs",
            "namespace N { public class Person { public String Name; } }\n",
        ),
        ("pb/fmt.go", GO_STRINGER),
    ]);
    assert!(
        type_rows(&result, "n/Person.cs", "String")
            .iter()
            .all(|(_, class)| !matches!(class, UnresolvedClass::External { module } if module.starts_with("java."))),
    );
}

// Fail closed on a possible type parameter. The extractor records no
// type-parameter facts for a class, so `class Box<Record> { Record held; }`
// cannot be told from the JDK's `Record`: a java.lang name in a file that
// declares any type-parameter list is not claimed as `java.lang`. It keeps the
// class the name would have without the java.lang rung (here `Unresolved`, a Go
// `Record` is in the corpus), and the ledger row says why.

const GO_RECORD: &str = "package pb\ntype Record struct{}\ntype Process struct{}\n";

/// `(class, reason)` of every `Type` reference row for `name` in `file`.
fn type_row_reasons(
    result: &ResolutionResult,
    file: &str,
    name: &str,
) -> Vec<(UnresolvedClass, String)> {
    result
        .unresolved
        .iter()
        .filter(|row| {
            row.source_file == file
                && row.callee_name == name
                && row.kind == UnresolvedKind::Reference
        })
        .map(|row| {
            let reason = match &row.resolution {
                devmap_resolve::model::Resolution::Unresolved { reason } => reason.clone(),
                other => format!("{other:?}"),
            };
            (row.class.clone(), reason)
        })
        .collect()
}

#[test]
fn a_class_type_parameter_named_like_a_jdk_type_is_not_java_lang() {
    let (_, result) = resolve(&[
        (
            "p/Box.java",
            "package p;\npublic class Box<Record> {\n    private Record held;\n}\n",
        ),
        ("pb/types.go", GO_RECORD),
    ]);
    let rows = type_row_reasons(&result, "p/Box.java", "Record");
    assert!(!rows.is_empty(), "the field's type is a row");
    for (class, reason) in &rows {
        assert_eq!(class, &UnresolvedClass::Unresolved, "{reason}");
        assert!(reason.contains("possibly a type parameter"), "{reason}");
    }
}

#[test]
fn a_non_generic_java_class_still_claims_java_lang() {
    let (_, result) = resolve(&[
        (
            "p/Plain.java",
            "package p;\npublic class Plain {\n    private String held;\n    public String get() { return held; }\n}\n",
        ),
        ("pb/fmt.go", GO_STRINGER),
    ]);
    let rows = type_row_reasons(&result, "p/Plain.java", "String");
    assert!(!rows.is_empty());
    for (class, reason) in &rows {
        assert_eq!(
            class,
            &UnresolvedClass::External {
                module: "java.lang.String".to_string()
            },
            "{reason}"
        );
    }
}

#[test]
fn a_method_type_parameter_stays_a_local_binding() {
    let (_, result) = resolve(&[
        (
            "p/Ids.java",
            "package p;\npublic class Ids {\n    public <Process> Process id(Process x) { return x; }\n}\n",
        ),
        ("pb/types.go", GO_RECORD),
    ]);
    let rows = type_row_reasons(&result, "p/Ids.java", "Process");
    assert!(!rows.is_empty());
    assert!(
        rows.iter()
            .all(|(class, _)| *class == UnresolvedClass::LocalBinding),
        "{rows:?}"
    );
}

#[test]
fn a_generic_file_does_not_claim_java_lang_for_any_name() {
    // The cost of failing closed without per-name facts: `T` is not `String`,
    // but nothing here says so.
    let (_, result) = resolve(&[
        (
            "p/Holder.java",
            "package p;\npublic class Holder<T> {\n    private String label;\n}\n",
        ),
        ("pb/fmt.go", GO_STRINGER),
    ]);
    let rows = type_row_reasons(&result, "p/Holder.java", "String");
    assert!(!rows.is_empty());
    assert!(
        rows.iter().all(|(class, reason)| {
            *class == UnresolvedClass::Unresolved && reason.contains("possibly a type parameter")
        }),
        "{rows:?}"
    );
}
