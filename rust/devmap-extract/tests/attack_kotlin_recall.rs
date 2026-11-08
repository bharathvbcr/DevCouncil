//! Adversarial pass on the Kotlin node-recall lane (card ft-4617a335).
//!
//! Each test states what correct behaviour is. A failing test is a break and
//! its name is the repro. Nothing here fixes anything.

use devmap_extract::extract_file;
use devmap_extract::model::{Extraction, ParseOutcome, SymbolKind};

fn kinds(extraction: &Extraction) -> Vec<(String, SymbolKind)> {
    let mut found: Vec<_> = extraction
        .symbols
        .iter()
        .filter(|symbol| symbol.kind != SymbolKind::File)
        .map(|symbol| (symbol.qualified_name.clone(), symbol.kind))
        .collect();
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

fn names(extraction: &Extraction) -> Vec<String> {
    kinds(extraction).into_iter().map(|(n, _)| n).collect()
}

fn kind_of(extraction: &Extraction, qualified: &str) -> Option<SymbolKind> {
    kinds(extraction)
        .into_iter()
        .find(|(n, _)| n == qualified)
        .map(|(_, k)| k)
}

fn clean(extraction: &Extraction) -> bool {
    matches!(extraction.parse_outcome, ParseOutcome::Clean)
}

/// Every symbol's span must slice the *original* text and contain the symbol's
/// bare name: the length-preservation invariant of the `;` rewrite.
fn assert_spans_are_the_originals(path: &str, source: &str) {
    let extraction = extract_file(path, source);
    for symbol in extraction
        .symbols
        .iter()
        .filter(|symbol| symbol.kind != SymbolKind::File)
    {
        let text = source
            .get(symbol.span.start_byte..symbol.span.end_byte)
            .unwrap_or_else(|| panic!("{}: span off a char boundary", symbol.qualified_name));
        let bare = symbol
            .qualified_name
            .rsplit("::")
            .next()
            .unwrap()
            .rsplit('.')
            .next()
            .unwrap();
        assert!(
            text.contains(bare),
            "{path}: span of {} is {text:?}, which does not hold {bare:?}",
            symbol.qualified_name
        );
    }
}

// ---- kotlinsemi: the accessor rewrite ----------------------------------------

#[test]
fn kotlinsemi_accessor_rewrite_leaves_a_call_named_set_in_a_body_alone() {
    // The class needs the accessor repair (the `;` before `private set`). The
    // `;` in `f` ends `val k = 1` in a function body; `set(k)` is a CALL to the
    // member below. Writing a space over that `;` makes it `1 set (k)`, an
    // infix call, and the `Calls` site to `C.set` is gone.
    let source = "class C {\n    var m: Int = 0; private set\n    fun set(x: Int) = x\n    fun f() { val k = 1; set(k) }\n}\n";
    let extraction = extract_file("C.kt", source);
    assert!(clean(&extraction), "{:?}", extraction.parse_outcome);
    let sites: Vec<_> = extraction
        .calls
        .iter()
        .filter(|call| call.callee_name == "set")
        .collect();
    assert!(
        !sites.is_empty(),
        "the call set(k) in C.f is no longer a call site: diagnostics {:?}",
        extraction.diagnostics
    );
}

#[test]
fn kotlinsemi_get_after_a_local_val_in_a_body_is_not_an_accessor() {
    let source = "class C {\n    var m: Int = 0; private set\n    fun get(x: Int) = x\n    fun f() { val k = 1; get(k) }\n}\n";
    let extraction = extract_file("C.kt", source);
    assert!(clean(&extraction), "{:?}", extraction.parse_outcome);
    assert!(
        extraction.calls.iter().any(|call| call.callee_name == "get"),
        "the call get(k) is gone: {:?}",
        extraction.diagnostics
    );
}

#[test]
fn kotlinsemi_semicolons_inside_raw_strings_and_comments_survive() {
    let source = "class C {\n    val raw = \"\"\"\n        var q = 1; private set\n    \"\"\"\n    /* var z = 1; set */\n    var m: Int = 0; private set\n    fun f() = 1\n}\n";
    let extraction = extract_file("C.kt", source);
    assert!(clean(&extraction), "{:?}", extraction.parse_outcome);
    assert!(names(&extraction).contains(&"C.kt::C.f".to_string()));
    assert_spans_are_the_originals("C.kt", source);
}

#[test]
fn kotlinsemi_two_vals_on_one_line_and_an_accessor() {
    let source = "class C {\n    val a = 1; val b = 2\n    var m: Int = 0; private set\n    fun f() = 1\n}\n";
    let extraction = extract_file("C.kt", source);
    assert!(clean(&extraction), "{:?}", extraction.parse_outcome);
    let found = names(&extraction);
    for expected in ["C.kt::C.a", "C.kt::C.b", "C.kt::C.m", "C.kt::C.f"] {
        assert!(found.contains(&expected.to_string()), "{expected} in {found:?}");
    }
}

#[test]
fn kotlinsemi_val_with_an_inline_getter_after_a_semicolon() {
    let source = "class C {\n    val x = 1; get() = field + 1\n    fun f() = 1\n}\nfun g() = 2\n";
    let extraction = extract_file("C.kt", source);
    assert!(names(&extraction).contains(&"C.kt::g".to_string()), "{:?}", names(&extraction));
    assert_spans_are_the_originals("C.kt", source);
}

#[test]
fn kotlinsemi_crlf_line_endings() {
    let source = "class C {\r\n    var m: Int = 0; private set\r\n    fun f() = 1\r\n}\r\nfun g() = 2\r\n";
    let extraction = extract_file("C.kt", source);
    assert!(clean(&extraction), "{:?} {:?}", extraction.parse_outcome, extraction.diagnostics);
    assert!(names(&extraction).contains(&"C.kt::g".to_string()));
    assert_spans_are_the_originals("C.kt", source);
}

#[test]
fn kotlinsemi_cr_only_line_endings() {
    let source = "class C {\r    var m: Int = 0; private set\r    fun f() = 1\r}\rfun g() = 2\r";
    let extraction = extract_file("C.kt", source);
    assert!(names(&extraction).contains(&"C.kt::g".to_string()), "{:?}", names(&extraction));
}

#[test]
fn kotlinsemi_a_tab_between_the_semicolon_and_the_accessor() {
    let source = "class C {\n    var m: Int = 0;\tprivate set\n    fun f() = 1\n}\nfun g() = 2\n";
    let extraction = extract_file("C.kt", source);
    assert!(clean(&extraction), "{:?}", extraction.parse_outcome);
}

#[test]
fn kotlinsemi_semicolon_directly_before_the_accessor_with_no_space() {
    let source = "class C {\n    var m: Int = 0;private set\n    fun f() = 1\n}\nfun g() = 2\n";
    let extraction = extract_file("C.kt", source);
    assert!(clean(&extraction), "{:?}", extraction.parse_outcome);
}

#[test]
fn kotlinsemi_non_ascii_text_keeps_every_span_on_a_char_boundary() {
    let source = "class Café {\n    var nom: String = \"é\"; private set\n    fun f() = \"日本\"\n}\nfun g() = 2\n";
    let extraction = extract_file("C.kt", source);
    assert!(clean(&extraction), "{:?}", extraction.parse_outcome);
    assert_spans_are_the_originals("C.kt", source);
}

#[test]
fn kotlinsemi_the_edit_cap_is_reported_not_silent() {
    // 600 accessor `;` in one class: past MAX_EDITS (512). The file must not
    // panic, and a file left Partial must say so.
    let mut source = String::from("class C {\n");
    for n in 0..600 {
        source.push_str(&format!("    var p{n}: Int = 0; private set\n"));
    }
    source.push_str("    fun f() = 1\n}\nfun g() = 2\n");
    let extraction = extract_file("C.kt", &source);
    let found = names(&extraction);
    let outcome_is_honest = clean(&extraction) || matches!(extraction.parse_outcome, ParseOutcome::Partial { .. });
    assert!(outcome_is_honest, "{:?}", extraction.parse_outcome);
    // At the cap the class and `g` should not simply vanish without a Partial.
    if !found.contains(&"C.kt::g".to_string()) {
        assert!(
            matches!(extraction.parse_outcome, ParseOutcome::Partial { .. }),
            "g lost on a Clean outcome"
        );
    }
}

#[test]
#[ignore = "ft-4617a335: kotlinsemi repair stops between 30 and 40 accessors"]
fn kotlinsemi_how_many_accessor_semicolons_repair() {
    let mut bad = Vec::new();
    for count in [1usize, 5, 10, 12, 15, 20, 25, 30, 40] {
        let mut source = String::from("class C {\n");
        for n in 0..count {
            source.push_str(&format!("    var p{n}: Int = 0; private set\n"));
        }
        source.push_str("    fun f() = 1\n}\nfun g() = 2\n");
        let extraction = extract_file("C.kt", &source);
        if !clean(&extraction) {
            bad.push((count, extraction.diagnostics.clone()));
        }
    }
    assert!(bad.is_empty(), "left Partial: {bad:?}");
}

#[test]
#[ignore = "ft-4617a335: kotlinsemi repair stops between 30 and 40 accessors"]
fn kotlinsemi_exactly_512_accessor_semicolons_still_repair() {
    let mut source = String::from("class C {\n");
    for n in 0..512 {
        source.push_str(&format!("    var p{n}: Int = 0; private set\n"));
    }
    source.push_str("    fun f() = 1\n}\nfun g() = 2\n");
    let extraction = extract_file("C.kt", &source);
    assert!(clean(&extraction), "{:?}", extraction.parse_outcome);
}

#[test]
fn kotlinsemi_a_one_line_class_and_an_accessor_and_a_second_property() {
    let source = "class A { var x = 1; private set; var y = 2 }\nfun top() {}\n";
    let extraction = extract_file("A.kt", source);
    assert!(names(&extraction).contains(&"A.kt::top".to_string()), "{:?}", names(&extraction));
}

// ---- property rule (P2) ------------------------------------------------------

const SHAPES: &str = "\
interface Base { val id: Int }
abstract class Shapes : Base {
    override val id: Int = 1
    lateinit var repo: String
    @JvmField val jf = 2
    val lazyV: Int by lazy { compute() }
    var observed: Int by kotlin.properties.Delegates.observable(0) { _, _, _ -> }
    val `back tick` = 3
    val
        ownLine = 4
    val Shapes.ext: Int get() = 5
    fun compute() = 1
    companion object Named { val inNamed = 6; const val K = 7 }
    inner class In { val innerProp = 8 }
    class Nested { val nestedProp = 9 }
}
object Obj { val objProp = 1; var objVar = 2 }
enum class E(val label: String) { A(\"a\"), B(\"b\"); val body = 1; fun f() = label }
value class V(val raw: Int)
data object Dobj { val d = 1 }
class Vararg(vararg val xs: Int, internal val i: Int, protected val p: Int)
sealed interface S { val k: Int; sealed class Leaf : S { override val k = 1 } }
";

#[test]
fn property_shapes_are_fields_with_the_owner_a_call_would_name() {
    let extraction = extract_file("S.kt", SHAPES);
    assert!(clean(&extraction), "{:?}", extraction.parse_outcome);
    let found = kinds(&extraction);
    let want = |name: &str| {
        let q = format!("S.kt::{name}");
        let kind = kind_of(&extraction, &q);
        assert_eq!(kind, Some(SymbolKind::Field), "{q} in {found:?}");
    };
    for name in [
        "Base.id",
        "Shapes.id",
        "Shapes.repo",
        "Shapes.jf",
        "Shapes.lazyV",
        "Shapes.observed",
        "Shapes.`back tick`",
        "Shapes.ownLine",
        "Shapes.inNamed",
        "Shapes.K",
        "In.innerProp",
        "Nested.nestedProp",
        "Obj.objProp",
        "Obj.objVar",
        "E.label",
        "E.body",
        "V.raw",
        "Dobj.d",
        "Vararg.xs",
        "Vararg.i",
        "Vararg.p",
        "S.k",
        "Leaf.k",
    ] {
        want(name);
    }
}

#[test]
fn property_an_extension_property_is_a_callable_not_a_field() {
    let extraction = extract_file("S.kt", SHAPES);
    assert_ne!(kind_of(&extraction, "S.kt::Shapes.ext"), Some(SymbolKind::Field));
}

#[test]
#[ignore = "ft-4617a335: NIT-6, nested owners with one bare name share an id; diagnostic is emitted"]
fn property_two_nested_classes_with_one_name_keep_both_properties() {
    // NIT-6: `A.Inner.x` and `B.Inner.x` are both `Inner.x` (bare owner), so the
    // second property is withheld and its node, with any edge to it, is gone.
    let source = "class A { class Inner { val x = 1 } }\nclass B { class Inner { val x = 2 } }\n";
    let extraction = extract_file("N.kt", source);
    let xs = extraction
        .symbols
        .iter()
        .filter(|symbol| symbol.qualified_name.ends_with("Inner.x"))
        .count();
    assert_eq!(xs, 2, "{:?}", names(&extraction));
}

#[test]
fn property_withholding_is_not_silent() {
    let source = "class A { class Inner { val x = 1 } }\nclass B { class Inner { val x = 2 } }\n";
    let extraction = extract_file("N.kt", source);
    assert!(
        extraction.diagnostics.iter().any(|d| d.contains("property id withheld")),
        "{:?}",
        extraction.diagnostics
    );
}

#[test]
fn property_a_class_and_a_property_of_one_name_in_one_owner() {
    let source = "object O { val Foo = 1; class Foo }\n";
    let extraction = extract_file("O.kt", source);
    let class = kinds(&extraction)
        .into_iter()
        .filter(|(n, _)| n.ends_with("Foo"))
        .collect::<Vec<_>>();
    // The class keeps `O.Foo`; the property is withheld, with a diagnostic.
    assert!(class.iter().any(|(_, k)| *k == SymbolKind::Class), "{class:?}");
    assert!(
        !class.iter().any(|(_, k)| *k == SymbolKind::Field),
        "a Field and a Class share an id: {class:?}"
    );
}

#[test]
fn property_withheld_for_a_companion_name_clash_keeps_the_first() {
    let source = "class H {\n    val z = 1\n    companion object { val z = 2 }\n}\n";
    let extraction = extract_file("H.kt", source);
    let zs: Vec<_> = extraction
        .symbols
        .iter()
        .filter(|s| s.qualified_name == "H.kt::H.z")
        .collect();
    assert_eq!(zs.len(), 1);
}

#[test]
#[ignore = "ft-4617a335: val /*c*/ name hides the keyword from is_property_symbol, documented residual"]
fn property_with_a_comment_between_val_and_name_is_still_settled_against_a_function() {
    // The documented residual: `val /*c*/ size` beside `fun size()` leaves two
    // nodes with one id (the property is not recognised as a property).
    let source = "class C {\n    val /*c*/ size = 1\n    fun size() = 2\n}\n";
    let extraction = extract_file("C.kt", source);
    let n = extraction
        .symbols
        .iter()
        .filter(|s| s.qualified_name == "C.kt::C.size")
        .count();
    assert_eq!(n, 1, "two symbols share C.kt::C.size: {:?}", kinds(&extraction));
}

#[test]
fn property_with_an_annotation_between_val_and_name_is_settled() {
    let source = "class C {\n    val size = 1\n    fun size() = 2\n    @get:JvmName(\"q\") val q = 1\n    fun q() = 3\n}\n";
    let extraction = extract_file("C.kt", source);
    for id in ["C.kt::C.size", "C.kt::C.q"] {
        let n = extraction.symbols.iter().filter(|s| s.qualified_name == id).count();
        assert_eq!(n, 1, "{id}: {:?}", kinds(&extraction));
    }
}

#[test]
fn property_constructor_parameter_with_val_and_a_function_of_the_same_name() {
    let source = "class C(val size: Int) {\n    fun size() = size\n}\n";
    let extraction = extract_file("C.kt", source);
    let n = extraction
        .symbols
        .iter()
        .filter(|s| s.qualified_name == "C.kt::C.size")
        .count();
    assert_eq!(n, 1, "{:?}", kinds(&extraction));
}

#[test]
fn property_expect_actual_files_each_own_their_property() {
    let common = extract_file("common/P.kt", "expect val platform: String\nexpect class Q { val x: Int }\n");
    let jvm = extract_file("jvm/P.kt", "actual val platform: String = \"j\"\nactual class Q { actual val x: Int = 1 }\n");
    assert!(names(&common).contains(&"common/P.kt::platform".to_string()), "{:?}", names(&common));
    assert!(names(&jvm).contains(&"jvm/P.kt::platform".to_string()), "{:?}", names(&jvm));
    assert!(names(&common).contains(&"common/P.kt::Q.x".to_string()), "{:?}", names(&common));
    assert!(names(&jvm).contains(&"jvm/P.kt::Q.x".to_string()), "{:?}", names(&jvm));
}

#[test]
fn property_private_nested_owner_members_are_still_nodes_but_a_private_class_param_is_not() {
    let source = "class C(private val a: Int, val b: Int, private var c: Int)\n";
    let extraction = extract_file("C.kt", source);
    let found = names(&extraction);
    assert!(found.contains(&"C.kt::C.b".to_string()));
    assert!(!found.contains(&"C.kt::C.a".to_string()));
    assert!(!found.contains(&"C.kt::C.c".to_string()));
}

#[test]
#[ignore = "ft-4617a335: a local class property is a node, same family as the local class at base"]
fn property_a_property_in_a_local_class_or_an_anonymous_object_is_not_a_node() {
    let source = "fun f() {\n    class Local { val inLocal = 1 }\n    val o = object { val anon = 2 }\n}\n";
    let extraction = extract_file("L.kt", source);
    let found = names(&extraction);
    assert!(!found.iter().any(|n| n.ends_with(".anon")), "{found:?}");
    // A local class's member is not reachable from outside: not a node either.
    assert!(!found.iter().any(|n| n.ends_with("Local.inLocal")), "{found:?}");
}

#[test]
fn property_the_initializer_calls_stay_attributed_to_the_owner_not_the_property() {
    let source = "class C {\n    val v = compute()\n    fun compute() = 1\n}\n";
    let extraction = extract_file("C.kt", source);
    let call = extraction
        .calls
        .iter()
        .find(|c| c.callee_name == "compute")
        .expect("compute() call");
    assert_ne!(
        call.caller_symbol.as_deref(),
        Some("C.kt::C.v"),
        "a property is a leaf: its initializer belongs to the class"
    );
}

#[test]
fn property_in_a_file_with_an_error_region_is_not_invented_from_garbage() {
    let source = "class C {\n    val a = 1\n    fun broken( = \n    val b = 2\n}\n";
    let extraction = extract_file("C.kt", source);
    let found = names(&extraction);
    assert!(found.contains(&"C.kt::C.a".to_string()), "{found:?}");
}

// ---- typealias shapes (extraction) ---------------------------------------------

#[test]
fn typealias_shapes_are_nodes_and_never_break_the_file() {
    let source = "package p\ntypealias L = List<Int>\ntypealias Handler<T> = (T) -> Unit\ntypealias Pt = Pair<Int, Int>\ninternal typealias I = Int\nclass Real\ntypealias R2 = Real\nfun usesL(l: L) = l\n";
    let extraction = extract_file("A.kt", source);
    assert!(clean(&extraction), "{:?}", extraction.parse_outcome);
    for alias in ["L", "Handler", "Pt", "I", "R2"] {
        assert_eq!(
            kind_of(&extraction, &format!("A.kt::{alias}")),
            Some(SymbolKind::Struct),
            "{alias}: {:?}",
            names(&extraction)
        );
    }
}

#[test]
fn typealias_inside_a_class_or_function_body_is_not_a_node_or_not_a_crash() {
    // Kotlin forbids a nested typealias, but a parser may still produce it.
    let source = "class C {\n    typealias Inner = Int\n    fun f() = 1\n}\nfun g() = 2\n";
    let extraction = extract_file("A.kt", source);
    assert!(names(&extraction).contains(&"A.kt::g".to_string()), "{:?}", names(&extraction));
}

#[test]
fn typealias_same_name_as_a_function_in_one_file_is_one_node() {
    let source = "typealias Pt = Pair<Int, Int>\nfun Pt(x: Int): Pt = Pair(x, x)\n";
    let extraction = extract_file("A.kt", source);
    let n = extraction
        .symbols
        .iter()
        .filter(|s| s.qualified_name == "A.kt::Pt")
        .count();
    assert_eq!(n, 1, "alias and factory share one id: {:?}", kinds(&extraction));
}
