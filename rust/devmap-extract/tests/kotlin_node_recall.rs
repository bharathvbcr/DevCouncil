//! Kotlin node recall (card ft-4617a335): which declarations are nodes.
//!
//! The rulings these tests pin, each against the PSI-measured gap:
//!
//! * `var x = 1; private set` on one line no longer makes the file an ERROR.
//! * `typealias` (and `actual typealias`) is a `Struct`, named from the grammar's
//!   `type` field.
//! * A non-private member, top-level or constructor property is a `Field` owned
//!   by its class's bare name; private, local and destructured ones are not
//!   nodes, and a property never takes an id a function or type already holds.
//! * Constructors and companion objects are not nodes, by design.

use devmap_extract::extract_file;
use devmap_extract::model::{Extraction, ParseOutcome, SymbolKind};

fn symbols(extraction: &Extraction) -> Vec<(String, SymbolKind)> {
    let mut found: Vec<(String, SymbolKind)> = extraction
        .symbols
        .iter()
        .filter(|symbol| symbol.kind != SymbolKind::File)
        .map(|symbol| (symbol.qualified_name.clone(), symbol.kind))
        .collect();
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

fn names(extraction: &Extraction) -> Vec<String> {
    symbols(extraction)
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

fn is_clean(extraction: &Extraction) -> bool {
    matches!(extraction.parse_outcome, ParseOutcome::Clean)
}

// ---- `; private set` -------------------------------------------------------

const ERR_KT: &str =
    "class Track {\n    var maxM: Double? = null; private set\n    fun f() = 1\n}\nfun g() = 2\n";
const SC3_KT: &str =
    "class T3 {\n    var m: Double? = null; private set\n    fun f() = 1\n}\nfun g3() = 2\n";

#[test]
fn a_semicolon_before_a_private_setter_does_not_lose_the_file() {
    for (path, source, class, function) in [
        ("Err.kt", ERR_KT, "Track", "g"),
        ("Sc3.kt", SC3_KT, "T3", "g3"),
    ] {
        let extraction = extract_file(path, source);
        assert!(
            is_clean(&extraction),
            "{path}: {:?} {:?}",
            extraction.parse_outcome,
            extraction.diagnostics
        );
        let found = names(&extraction);
        for expected in [
            format!("{path}::{class}"),
            format!("{path}::{class}.f"),
            format!("{path}::{function}"),
        ] {
            assert!(found.contains(&expected), "{path}: {expected} in {found:?}");
        }
        assert!(
            extraction
                .diagnostics
                .iter()
                .any(|note| note.contains("before a property accessor")),
            "{path}: the repair is recorded: {:?}",
            extraction.diagnostics
        );
    }
}

#[test]
fn the_accessor_repair_keeps_every_offset_and_the_original_text() {
    let extraction = extract_file("Err.kt", ERR_KT);
    let f = extraction
        .symbols
        .iter()
        .find(|symbol| symbol.qualified_name == "Err.kt::Track.f")
        .expect("Track.f");
    assert_eq!(&ERR_KT[f.span.start_byte..f.span.end_byte], "fun f() = 1");
    let semi = ERR_KT.find("; private").unwrap();
    assert!(
        extraction
            .recovered_decl_ranges
            .iter()
            .any(|range| range.start_byte <= semi && semi < range.end_byte),
        "the statement the `;` was in is a recorded repair: {:?}",
        extraction.recovered_decl_ranges
    );
}

#[test]
fn the_accessor_repair_covers_each_visibility_and_both_accessors() {
    for accessor in [
        "private set",
        "internal set",
        "protected set",
        "public set",
        "set",
        "get",
        "set(value) { field = value }",
        "private   set",
    ] {
        let source = format!(
            "class C {{\n    var m: Int = 0; {accessor}\n    fun f() = 1\n}}\nfun g() = 2\n"
        );
        let extraction = extract_file("C.kt", &source);
        assert!(
            is_clean(&extraction),
            "{accessor}: {:?}",
            extraction.parse_outcome
        );
        assert!(
            names(&extraction).contains(&"C.kt::g".to_string()),
            "{accessor}: {:?}",
            names(&extraction)
        );
    }
}

#[test]
fn a_semicolon_in_a_string_or_comment_is_never_rewritten() {
    let source = "class C {\n    val a = \"x; set\"\n    // y; private set\n    var m: Int = 0; private set\n    fun f() = 1\n}\n";
    let extraction = extract_file("C.kt", source);
    assert!(is_clean(&extraction), "{:?}", extraction.parse_outcome);
    assert!(names(&extraction).contains(&"C.kt::C.f".to_string()));
}

#[test]
fn a_file_the_rewrite_cannot_make_clean_stays_partial() {
    let source = "class C {\n    var m: Int = 0; private set\n    fun f( = \n}\nfun g() = 2\n";
    let extraction = extract_file("C.kt", source);
    assert!(
        matches!(extraction.parse_outcome, ParseOutcome::Partial { .. }),
        "{:?}",
        extraction.parse_outcome
    );
}

#[test]
fn a_one_line_class_with_a_private_setter_takes_both_repairs() {
    let source = "class A { var x = 1; private set }\nfun top() {}\n";
    let extraction = extract_file("A.kt", source);
    assert!(is_clean(&extraction), "{:?}", extraction.parse_outcome);
    assert!(names(&extraction).contains(&"A.kt::top".to_string()));
}

// ---- typealias --------------------------------------------------------------

#[test]
fn a_typealias_is_a_struct_named_from_the_type_field() {
    let source =
        "typealias Id = Long\nactual typealias P = Path\nprivate typealias T = Int\nclass K\n";
    let extraction = extract_file("A.kt", source);
    let alias = |name: &str| {
        extraction
            .symbols
            .iter()
            .find(|symbol| symbol.qualified_name == format!("A.kt::{name}"))
            .unwrap_or_else(|| panic!("{name} in {:?}", names(&extraction)))
    };
    for name in ["Id", "P", "T"] {
        assert_eq!(alias(name).kind, SymbolKind::Struct, "{name}");
        assert_eq!(alias(name).parent_symbol.as_deref(), Some("A.kt"));
    }
    assert!(alias("Id").is_exported);
    assert!(alias("P").is_exported);
    assert!(!alias("T").is_exported, "a private alias is not exported");
}

// ---- properties -------------------------------------------------------------

const PROPS: &str = "\
const val MAX = 3
private const val HIDDEN_MAX = 4
val top: Int = 1
private val hiddenTop = 2
class Trip(val name: String, var n: Int, plain: Int, private val secret: Int) {
    val extra = 2
    private val hidden = 3
    var count: Int = 0
        private set
    val size = 1
    fun size() = 2
    constructor(x: Int) : this(\"a\", x, x, x)
    private constructor() : this(\"\", 0, 0, 0)
    init { val inInit = 1 }
    fun body() {
        val local = 1
        val (a, b) = Pair(1, 2)
    }
    val computed: Int get() { val inGetter = 1; return inGetter }
    companion object {
        fun make() = Trip(\"a\", 1, 1, 1)
        val z = 1
    }
}
data class D(val a: Int, val b: Int = a)
object Single { val v = 1 }
interface I { val p: Int }
enum class Mode(val label: String) { FAST(\"f\"), SLOW(\"s\"); val extra2 = 1 }
val o = object { val anon = 1 }
fun host() { val inHost = 1 }
val pair = Pair(1, 2)
val (da, db) = pair
";

#[test]
fn every_non_private_member_top_level_and_constructor_property_is_a_field() {
    let extraction = extract_file("T.kt", PROPS);
    assert!(is_clean(&extraction), "{:?}", extraction.parse_outcome);
    let found = symbols(&extraction);
    let fields: Vec<&str> = found
        .iter()
        .filter(|(_, kind)| *kind == SymbolKind::Field)
        .map(|(name, _)| name.as_str())
        .collect();
    assert_eq!(
        fields,
        [
            "T.kt::D.a",
            "T.kt::D.b",
            "T.kt::I.p",
            "T.kt::MAX",
            "T.kt::Mode.FAST",
            "T.kt::Mode.SLOW",
            "T.kt::Mode.extra2",
            "T.kt::Mode.label",
            "T.kt::Single.v",
            "T.kt::Trip.computed",
            "T.kt::Trip.count",
            "T.kt::Trip.extra",
            "T.kt::Trip.n",
            "T.kt::Trip.name",
            "T.kt::Trip.z",
            "T.kt::o",
            "T.kt::pair",
            "T.kt::top",
        ],
        "{found:?}"
    );
    for symbol in extraction
        .symbols
        .iter()
        .filter(|symbol| symbol.kind == SymbolKind::Field)
    {
        assert!(symbol.is_exported, "{} is exported", symbol.qualified_name);
    }
}

#[test]
fn a_property_is_owned_by_the_bare_name_and_the_top_level_by_the_file() {
    let extraction = extract_file("T.kt", PROPS);
    let parent = |qualified: &str| {
        extraction
            .symbols
            .iter()
            .find(|symbol| symbol.qualified_name == qualified)
            .and_then(|symbol| symbol.parent_symbol.clone())
            .unwrap_or_else(|| panic!("{qualified} in {:?}", names(&extraction)))
    };
    assert_eq!(parent("T.kt::Trip.name"), "T.kt::Trip");
    assert_eq!(parent("T.kt::MAX"), "T.kt");
    // The companion is skipped in the owner walk: `Trip.z`, like `Trip.make`.
    assert_eq!(parent("T.kt::Trip.z"), "T.kt::Trip");
    assert!(names(&extraction).contains(&"T.kt::Trip.make".to_string()));
}

#[test]
fn private_local_destructured_and_anonymous_properties_are_not_nodes() {
    let extraction = extract_file("T.kt", PROPS);
    let found = names(&extraction);
    for absent in [
        "HIDDEN_MAX",
        "hiddenTop",
        "Trip.secret",
        "Trip.hidden",
        "Trip.plain",
        "Trip.local",
        "Trip.a",
        "Trip.inInit",
        "Trip.inGetter",
        "host.inHost",
        "inHost",
        "local",
        "anon",
        "da",
        "db",
        "a",
        "b",
    ] {
        assert!(
            !found.contains(&format!("T.kt::{absent}")),
            "{absent} must not be a node: {found:?}"
        );
    }
}

#[test]
fn a_property_never_takes_the_id_of_a_function_of_the_same_owner() {
    let extraction = extract_file("T.kt", PROPS);
    let size: Vec<_> = extraction
        .symbols
        .iter()
        .filter(|symbol| symbol.qualified_name == "T.kt::Trip.size")
        .collect();
    assert_eq!(size.len(), 1, "one id, one node: {size:?}");
    assert_eq!(
        size[0].kind,
        SymbolKind::Method,
        "the callable keeps the id"
    );
    assert!(
        extraction
            .diagnostics
            .iter()
            .any(|note| note.contains("kotlin property id withheld: 1 shared")),
        "the withholding is counted: {:?}",
        extraction.diagnostics
    );
}

#[test]
fn a_top_level_property_yields_the_id_to_a_class_or_function() {
    let extraction = extract_file(
        "T.kt",
        "val Foo = 1\nclass Foo\nval bar = 1\nfun bar() = 2\n",
    );
    let found = symbols(&extraction);
    assert_eq!(
        found,
        [
            ("T.kt::Foo".to_string(), SymbolKind::Class),
            ("T.kt::bar".to_string(), SymbolKind::Function),
        ]
    );
}

#[test]
fn the_first_of_two_properties_with_one_id_wins() {
    let source = "class Holder {\n    val z = 1\n    companion object { val z = 2 }\n}\n";
    let extraction = extract_file("H.kt", source);
    let z: Vec<_> = extraction
        .symbols
        .iter()
        .filter(|symbol| symbol.qualified_name == "H.kt::Holder.z")
        .collect();
    assert_eq!(z.len(), 1, "{z:?}");
    assert_eq!(z[0].span.start_byte, source.find("z = 1").unwrap());
    assert!(extraction
        .diagnostics
        .iter()
        .any(|note| note.contains("1 of an earlier property")));
}

#[test]
fn a_property_is_not_the_caller_of_the_calls_in_its_initializer() {
    let source = "fun helper() = 1\nclass C {\n    val x = helper()\n}\n";
    let extraction = extract_file("C.kt", source);
    let call = extraction
        .calls
        .iter()
        .find(|call| call.callee_name == "helper")
        .expect("the call is recorded");
    assert_eq!(call.caller_symbol.as_deref(), Some("C.kt::C"), "{call:?}");
}

// ---- constructors and companions -------------------------------------------

#[test]
fn constructors_and_companion_objects_are_not_nodes() {
    let source = "\
data class Trip(val name: String) {
    constructor(x: Int) : this(\"a\")
    private constructor() : this(\"\")
    companion object { fun make() = Trip(\"a\") }
}
class Holder {
    companion object Named { fun build() = 1 }
}
";
    let extraction = extract_file("A.kt", source);
    assert!(is_clean(&extraction), "{:?}", extraction.parse_outcome);
    assert_eq!(
        symbols(&extraction),
        [
            ("A.kt::Holder".to_string(), SymbolKind::Class),
            ("A.kt::Holder.build".to_string(), SymbolKind::Method),
            ("A.kt::Trip".to_string(), SymbolKind::Class),
            ("A.kt::Trip.make".to_string(), SymbolKind::Method),
            ("A.kt::Trip.name".to_string(), SymbolKind::Field),
        ]
    );
}

#[test]
fn a_multi_line_twin_with_the_semicolon_before_an_accessor_is_repaired_too() {
    for source in [
        "class A {\nvar v = 0; set(x) { }; fun f() { g() }\n}\n",
        "class A {\nvar v = 0; set(x) { }\n fun f() { g() }\n}\n",
    ] {
        let extraction = extract_file("A.kt", source);
        assert!(
            is_clean(&extraction),
            "{source:?}: {:?}",
            extraction.parse_outcome
        );
        assert!(names(&extraction).contains(&"A.kt::A.f".to_string()));
    }
}

// ---- the two remaining TrailBlazer triggers, isolated -----------------------
//
// Neither is repaired here: a repair would have to rename an identifier the
// grammar reads as a modifier, which is a different rewrite from a separator.
// These pin what the reduced shapes are, so a later repair (or a grammar bump)
// turns them red on purpose and the lost declarations are named, not guessed.

/// `LocationRepository.kt:124` (`locationSource`): a lambda parameter named with
/// the soft keyword `external` (`{ external -> .. }`) is read as the modifier.
/// Eight bytes (`external`) are the whole error, inside the surviving function.
#[test]
fn a_lambda_parameter_named_external_is_an_error_inside_a_surviving_function() {
    let source = "fun f() {\n    val r = y.flatMapLatest { external ->\n        if (external) 1 else 2\n    }\n}\nfun g() = 2\n";
    let extraction = extract_file("L.kt", source);
    let ParseOutcome::Partial { error_ranges, .. } = &extraction.parse_outcome else {
        panic!("{:?}", extraction.parse_outcome);
    };
    let texts: Vec<&str> = error_ranges
        .iter()
        .map(|range| &source[range.start_byte..range.end_byte])
        .collect();
    assert_eq!(texts, ["external"]);
    let found = names(&extraction);
    assert!(found.contains(&"L.kt::f".to_string()) && found.contains(&"L.kt::g".to_string()));
}

/// `NmeaParser.kt` (`accept`, the `open` property): a statement that starts with
/// the soft keyword `open` as an assignment target (`open = it`) is read as a
/// modifier, and the error swallows what follows it.
#[test]
fn a_statement_assigning_to_a_variable_named_open_is_an_error() {
    let source = "fun f() {\n    open = 1\n}\nfun g() = 2\n";
    let extraction = extract_file("N.kt", source);
    assert!(
        matches!(extraction.parse_outcome, ParseOutcome::Partial { .. }),
        "{:?}",
        extraction.parse_outcome
    );
    // `this.open = 1` is the same assignment spelled so the grammar reads it.
    let spelled = extract_file(
        "N.kt",
        "class C {\n    var open = 0\n    fun f() { this.open = 1 }\n}\nfun g() = 2\n",
    );
    assert!(is_clean(&spelled), "{:?}", spelled.parse_outcome);
}
