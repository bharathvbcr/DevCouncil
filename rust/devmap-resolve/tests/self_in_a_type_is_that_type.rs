//! `Self.m()` written in a type's own body means that type.
//!
//! X42 dispatches an implicit receiver on "the type the call is written
//! inside", and read that type by asking for the caller's **parent**. That is
//! right for a call inside a method and blind for a call the extractor
//! attributes to the type itself — a Swift stored-property initializer, a
//! Kotlin companion object, a Ruby constant assignment, a Scala `val` — where
//! the parent is the file and the enclosing type is the caller's own name.
//!
//! Measured on a 306-file Swift corpus: 9 targets held no non-structural
//! inbound edge but their `Self.staticMethod()` call sites, and every one of
//! those callers was the `struct`, so the rung abstained and the targets read
//! as dead code. These tests pin the repair and the abstentions around it.

use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
use devmap_resolve::Resolver;

fn calls_from(files: &[(&str, &str)], source: &str) -> Vec<String> {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, body)| extract_file(path, body))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    resolver
        .resolve_all(&extractions)
        .expect("resolution")
        .edges
        .into_iter()
        .filter(|edge| edge.source_symbol == source && format!("{:?}", edge.edge_kind) == "Calls")
        .map(|edge| edge.target_symbol)
        .collect()
}

const SWIFT: &str = "struct Blocks {\n\
     \x20   private static func zzcheck(_ v: Int) -> Bool { v > 0 }\n\
     \x20   static let zzgate: Bool = Self.zzcheck(1)\n\
     }\n";

/// The regression itself: a `Self.` call attributed to the struct resolves.
///
/// `zzcheck` is `private` and called nowhere else, so before the fix it was a
/// symbol with no inbound edge but its container's — the shape that reads as
/// dead code.
#[test]
fn a_self_call_attributed_to_the_type_finds_the_types_method() {
    let targets = calls_from(&[("a.swift", SWIFT)], "a.swift::Blocks");
    assert_eq!(
        targets,
        vec!["a.swift::Blocks.zzcheck".to_string()],
        "a call written in the type's own body must dispatch on that type"
    );
}

/// A guard against passing vacuously: the caller really is the type.
///
/// If a future extractor change parents this call under a synthetic member
/// instead, the test above would pass through `declaring_type_of` and stop
/// exercising the fallback. This fails loudly at that point rather than going
/// quiet.
#[test]
fn the_caller_in_that_shape_is_the_type_and_not_a_member() {
    let extraction = extract_file("a.swift", SWIFT);
    let call = extraction
        .calls
        .iter()
        .find(|call| call.callee_name == "zzcheck")
        .expect("the call is extracted");
    assert_eq!(
        call.caller_symbol.as_deref(),
        Some("a.swift::Blocks"),
        "this fixture exists to exercise the caller-is-the-type path"
    );
    assert_eq!(call.receiver_expr.as_deref(), Some("Self"));
}

/// Not a Swift patch: every grammar that attributes a self-receiver call to
/// the enclosing type is repaired by the same line.
///
/// Measured shapes, not guesses — TypeScript, JavaScript and a Python
/// class-body assignment reach the extractor with **no** caller symbol at all,
/// which is a different and untouched gap, so they are deliberately absent.
#[test]
fn the_repair_is_not_swift_specific() {
    for (path, source) in [
        ("a.swift", SWIFT),
        (
            "a.kt",
            "class Blocks {\n\
             \x20   companion object {\n\
             \x20       fun zzcheck(v: Int) = v > 0\n\
             \x20       val zzgate = this.zzcheck(1)\n\
             \x20   }\n\
             }\n",
        ),
        (
            "a.rb",
            "class Blocks\n\
             \x20 def self.zzcheck(v)\n\
             \x20   v > 0\n\
             \x20 end\n\
             \x20 ZZGATE = self.zzcheck(1)\n\
             end\n",
        ),
        (
            "a.scala",
            "class Blocks {\n\
             \x20 def zzcheck(v: Int) = v > 0\n\
             \x20 val zzgate = this.zzcheck(1)\n\
             }\n",
        ),
    ] {
        let call = extract_file(path, source)
            .calls
            .into_iter()
            .find(|call| call.callee_name == "zzcheck")
            .unwrap_or_else(|| panic!("{path}: the call is extracted"));
        assert_eq!(
            call.caller_symbol.as_deref(),
            Some(format!("{path}::Blocks").as_str()),
            "{path}: the fixture must exercise the caller-is-the-type path"
        );
        let targets = calls_from(&[(path, source)], &format!("{path}::Blocks"));
        assert!(
            targets
                .iter()
                .any(|target| target.ends_with("Blocks.zzcheck")),
            "{path}: got {targets:?}"
        );
    }
}

/// A Ruby `module` is a receiver `self` can denote, and is admitted.
///
/// `SymbolKind::Module` is shared with Go packages, TypeScript and C#
/// namespaces and Terraform blocks. Admitting it was checked against each
/// rather than assumed: a namespace-level call reaches the extractor with **no
/// caller symbol**, and the rest have no `self` keyword, so none of them can
/// reach this rung. Ruby's `module M; def self.m` can, and there `self` is `M`.
#[test]
fn a_ruby_module_is_a_receiver_self_can_denote() {
    let source = "module M\n\
         \x20 def self.zzcheck(v)\n\
         \x20   v > 0\n\
         \x20 end\n\
         \x20 GATE = self.zzcheck(1)\n\
         end\n";
    assert_eq!(
        calls_from(&[("a.rb", source)], "a.rb::M"),
        vec!["a.rb::M.zzcheck".to_string()]
    );
}

/// A namespace-level call carries no caller at all, so the widened kind list
/// cannot reach it. Pinned because that is the whole safety argument for
/// admitting `Module`.
#[test]
fn a_namespace_level_call_has_no_caller_to_widen_from() {
    for (path, source) in [
        (
            "a.ts",
            "namespace N {\n\
             \x20   export function zzcheck(v: number) { return v > 0 }\n\
             \x20   export const g = this.zzcheck(1);\n\
             }\n",
        ),
        (
            "a.js",
            "class B {\n\
             \x20   static zzcheck(v) { return v > 0 }\n\
             \x20   static g = this.zzcheck(1);\n\
             }\n",
        ),
    ] {
        let call = extract_file(path, source)
            .calls
            .into_iter()
            .find(|call| call.callee_name == "zzcheck")
            .unwrap_or_else(|| panic!("{path}: the call is extracted"));
        assert_eq!(
            call.caller_symbol, None,
            "{path}: if this ever gains a caller, the `Module` kind needs \
             re-checking against it"
        );
    }
}

/// A nested Swift type stays unresolved, and that is an **extractor** gap.
///
/// `struct Outer { struct Inner { … } }` gives the nested type the qualified
/// name `Outer.Inner` but gives its method the parent `Inner`, so the two
/// halves of `type_methods`' key disagree before any rung runs. X42 missed this
/// before the fix too — `declaring_type_of` answered `Outer` — so nothing here
/// regressed; the case is pinned so the day the extractor is repaired is the
/// day this test tells someone the rung now covers it.
#[test]
fn a_nested_type_is_a_known_extractor_gap_not_a_rung_gap() {
    let source = "struct Outer {\n\
         \x20   struct Inner {\n\
         \x20       static func zzcheck(_ v: Int) -> Bool { v > 0 }\n\
         \x20       static let zzgate: Bool = Self.zzcheck(1)\n\
         \x20   }\n\
         }\n";
    let extraction = extract_file("a.swift", source);
    let method = extraction
        .symbols
        .iter()
        .find(|symbol| symbol.name == "zzcheck")
        .expect("the method is extracted");
    assert_eq!(
        method.parent_symbol.as_deref(),
        Some("a.swift::Inner"),
        "the gap is here: the nested type is `Outer.Inner` but its method is \
         parented to `Inner`"
    );
    assert!(
        calls_from(&[("a.swift", source)], "a.swift::Outer.Inner").is_empty(),
        "the rung cannot bridge a key the extractor spells two ways"
    );
}

/// The ordinary shape still works, so the fallback did not displace it.
#[test]
fn a_self_call_inside_a_method_still_finds_its_own_type() {
    let source = "struct Blocks {\n\
         \x20   private func zzinst(_ v: Int) -> Bool { v > 0 }\n\
         \x20   func run(v: Int) -> Bool { return self.zzinst(v) }\n\
         }\n";
    assert_eq!(
        calls_from(&[("a.swift", source)], "a.swift::Blocks.run"),
        vec!["a.swift::Blocks.zzinst".to_string()]
    );
}

/// The fallback is keyed on *this* file, not on the bare name globally.
///
/// Two files declare `Blocks`; only one declares `zzcheck`. A `Self.zzcheck()`
/// written in the other one must not reach across — the receiver is that
/// file's own type, which has no such method.
#[test]
fn a_namesake_type_in_another_file_is_not_reached() {
    let with_method = "struct Blocks {\n\
         \x20   static func zzcheck(_ v: Int) -> Bool { v > 0 }\n\
         }\n";
    let without = "struct Blocks {\n\
         \x20   static let zzgate: Bool = Self.zzcheck(1)\n\
         }\n";
    let files = [("a.swift", with_method), ("b.swift", without)];
    assert!(
        calls_from(&files, "b.swift::Blocks").is_empty(),
        "`Self` in b.swift means b.swift's `Blocks`"
    );
}

/// A caller that is a file-level *function* is not a type, and gets nothing.
///
/// This is the line the widening must not cross: `declaring_type_of` returning
/// `None` is not on its own a licence to guess, and a free function's `Self`
/// has no enclosing type to dispatch on.
#[test]
fn a_file_level_function_is_not_an_enclosing_type() {
    let source = "struct Blocks {\n\
         \x20   static func zzcheck(_ v: Int) -> Bool { v > 0 }\n\
         }\n\
         func zzfree() -> Bool { return Self.zzcheck(1) }\n";
    assert!(
        calls_from(&[("a.swift", source)], "a.swift::zzfree").is_empty(),
        "a free function has no `Self`"
    );
}

/// A caller that is a type declaring nothing by that name still abstains.
#[test]
fn a_type_without_the_method_resolves_to_nothing() {
    let source = "struct Blocks {\n\
         \x20   static let zzgate: Bool = Self.zzabsent(1)\n\
         }\n";
    assert!(calls_from(&[("a.swift", source)], "a.swift::Blocks").is_empty());
}

/// Inheritance still reaches a supertype's method from the type-as-caller path.
///
/// The fallback hands X42 a type name and nothing else, so the supertype walk
/// that follows must behave exactly as it does for a method caller.
#[test]
fn the_supertype_walk_runs_from_a_type_caller_too() {
    let source = "class Base {\n\
         \x20   static func zzbase(_ v: Int) -> Bool { v > 0 }\n\
         }\n\
         class Derived: Base {\n\
         \x20   static let zzgate: Bool = Self.zzbase(1)\n\
         }\n";
    assert_eq!(
        calls_from(&[("a.swift", source)], "a.swift::Derived"),
        vec!["a.swift::Base.zzbase".to_string()],
        "an inherited static is still a method of `Self`"
    );
}

/// Two types of one name in one file leave the rung with no single answer.
///
/// The fallback narrows to the file and stops; choosing between two
/// declarations inside it is the downstream ambiguity guard's job, and the
/// guard must still fire rather than picking whichever merged first.
#[test]
fn two_namesake_types_in_one_file_abstain() {
    let source = "struct Blocks {\n\
         \x20   static func zzcheck(_ v: Int) -> Bool { v > 0 }\n\
         \x20   static let zzgate: Bool = Self.zzcheck(1)\n\
         }\n\
         struct Blocks {\n\
         \x20   static func zzcheck(_ v: Int) -> Bool { v < 0 }\n\
         }\n";
    assert!(
        calls_from(&[("a.swift", source)], "a.swift::Blocks").is_empty(),
        "one name, two declarations, no answer this resolver may give"
    );
}
