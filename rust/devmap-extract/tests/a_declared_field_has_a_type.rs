//! Every grammar that declares a typed field states one, in the same shape.
//!
//! `lease.revalidate()` names a method on whatever `lease` is, and a typed
//! field declaration is the file's own written answer. Two grammars produced
//! that evidence and twenty-nine did not, so a method reached only through a
//! typed field stayed callerless everywhere else — 57% of this workspace's
//! remaining unexplained receivers and 60% of an independent Swift corpus's are
//! a bare identifier of exactly that shape.
//!
//! These tests are the registry for `langfields`. Every case was written from a
//! dumped parse tree rather than from memory: six grammars spell a field
//! `field_declaration` and mean six different interiors, and three spell a
//! property `property_declaration` and likewise.

use devmap_extract::langfields::{extracts_field_types, FIELD_TYPE_LANGUAGES};
use devmap_extract::languages::LANGUAGE_SPECS;
use devmap_extract::model::{ExtractedReference, ReferenceKind};
use devmap_extract::{extract_file, linked_grammar_keys};

/// The field-type references a file states, as `(type, field, owner)`.
fn field_types(path: &str, source: &str) -> Vec<(String, String, String)> {
    extract_file(path, source)
        .references
        .into_iter()
        .filter(|reference| reference.kind == ReferenceKind::Type)
        .filter_map(|reference: ExtractedReference| {
            Some((
                reference.name,
                reference.assigned_to?,
                reference.enclosing_symbol?,
            ))
        })
        .collect()
}

fn names_a_field(path: &str, source: &str, field: &str) -> Option<String> {
    field_types(path, source)
        .into_iter()
        .find(|(_, name, _)| name == field)
        .map(|(type_name, _, _)| type_name)
}

/// One typed field, in every grammar that has one.
///
/// The table is the point: a reader adding a grammar adds a row here and finds
/// out immediately whether the arm they wrote reads the tree the grammar
/// actually produces.
#[test]
fn every_grammar_states_the_type_of_a_declared_field() {
    let cases: &[(&str, &str, &str)] = &[
        ("a.rs", "struct S {\n    lease: Lease,\n}\n", "lease"),
        (
            "a.go",
            "package p\ntype S struct {\n    Lease *pkg.Lease\n}\n",
            "Lease",
        ),
        (
            "a.java",
            "class S {\n    private Lease lease;\n}\n",
            "lease",
        ),
        ("a.cs", "class S {\n    private Lease lease;\n}\n", "lease"),
        ("a.swift", "struct S {\n    let lease: Lease\n}\n", "lease"),
        ("a.kt", "class S {\n    val lease: Lease = x\n}\n", "lease"),
        ("a.ts", "class S {\n    private lease: Lease;\n}\n", "lease"),
        (
            "a.tsx",
            "class S {\n    private lease: Lease;\n}\n",
            "lease",
        ),
        ("a.py", "class S:\n    lease: Lease\n", "lease"),
        ("a.scala", "class S {\n  val lease: Lease = x\n}\n", "lease"),
        (
            "a.php",
            "<?php\nclass S {\n    private Lease $lease;\n}\n",
            // Every PHP variable carries its sigil, and a receiver is spelled
            // `$lease` at the call site too, so the key must match.
            "$lease",
        ),
        ("a.cpp", "class S {\n    Lease lease;\n};\n", "lease"),
        ("a.c", "struct S {\n    struct Lease lease;\n};\n", "lease"),
        (
            "a.m",
            "@interface S : NSObject {\n    Lease *lease;\n}\n@end\n",
            "lease",
        ),
        ("a.dart", "class S {\n  final Lease lease;\n}\n", "lease"),
        ("a.sol", "contract S {\n    Lease lease;\n}\n", "lease"),
        (
            "a.pas",
            "type\n  S = class\n    lease: TLease;\n  end;\n",
            "lease",
        ),
    ];
    for (path, source, field) in cases {
        let found = names_a_field(path, source, field);
        let expected = if path.ends_with(".pas") {
            // Delphi's convention prefixes a type name with `T`.
            "TLease"
        } else {
            "Lease"
        };
        assert_eq!(
            found.as_deref(),
            Some(expected),
            "{path}: a declared field must state its type"
        );
    }
}

/// The owner is the declaring type, so the key is scoped to it.
///
/// Objective-C is in the list for a reason cargo-mutants found: its interface
/// names itself with a bare `identifier` child rather than a `name:` field, so
/// the walk picks the *first* identifier — and inverting that comparison yields
/// the brace block's whole source text as the owner instead of `S`. Nothing
/// else in this file looked at an Objective-C owner.
#[test]
fn a_field_is_recorded_against_the_type_that_declares_it() {
    for (path, source) in [
        ("a.swift", "struct Holder {\n    let lease: Lease\n}\n"),
        ("a.rs", "struct Holder {\n    lease: Lease,\n}\n"),
        ("a.java", "class Holder {\n    private Lease lease;\n}\n"),
        ("a.py", "class Holder:\n    lease: Lease\n"),
        (
            "a.m",
            "@interface Holder : NSObject {\n    Lease *lease;\n}\n@end\n",
        ),
    ] {
        let owner = field_types(path, source)
            .into_iter()
            .find(|(_, name, _)| name == "lease")
            .map(|(_, _, owner)| owner);
        assert_eq!(
            owner.as_deref(),
            Some(format!("{path}::Holder").as_str()),
            "{path}: a field belongs to its type, not to the file"
        );
    }
}

/// A collection is not its element, and that is the line the reducer holds.
///
/// `many.first()` is a call on `Array`, not on `Lease`. Answering `Lease` would
/// hand the resolver a confident wrong type, and a wrong `DETERMINISTIC` edge
/// is worse than the missing one it replaces.
#[test]
fn a_collection_field_states_nothing() {
    for (path, source, field) in [
        ("a.swift", "struct S {\n    var many: [Lease]\n}\n", "many"),
        (
            "a.swift",
            "struct S {\n    var byName: [String: Lease]\n}\n",
            "byName",
        ),
        ("a.ts", "class S {\n    many: Lease[];\n}\n", "many"),
        (
            "a.go",
            "package p\ntype S struct {\n    Many []pkg.Lease\n}\n",
            "Many",
        ),
        (
            "a.go",
            "package p\ntype S struct {\n    ByName map[string]pkg.Lease\n}\n",
            "ByName",
        ),
    ] {
        assert_eq!(
            names_a_field(path, source, field),
            None,
            "{path}: {field} is a collection, and its element is not its type"
        );
    }
}

/// A wrapper denoting the same value is unwrapped; a generic reduces to its head.
#[test]
fn a_wrapper_reduces_and_a_generic_keeps_its_head() {
    for (path, source, field, expected) in [
        // Unwrapped: the operand denotes the same value.
        ("a.rs", "struct S {\n    r: &'a Lease,\n}\n", "r", "Lease"),
        (
            "a.swift",
            "struct S {\n    var o: Lease?\n}\n",
            "o",
            "Lease",
        ),
        (
            "a.kt",
            "class S {\n    var o: Lease? = null\n}\n",
            "o",
            "Lease",
        ),
        (
            "a.php",
            "<?php\nclass S {\n    private ?Lease $o = null;\n}\n",
            "$o",
            "Lease",
        ),
        (
            "a.go",
            "package p\ntype S struct {\n    P *pkg.Lease\n}\n",
            "P",
            "Lease",
        ),
        // Head of a generic: a `Box<Lease>` is a `Box`.
        ("a.rs", "struct S {\n    b: Box<Lease>,\n}\n", "b", "Box"),
        (
            "a.java",
            "class S {\n    List<Lease> many;\n}\n",
            "many",
            "List",
        ),
        (
            "a.cpp",
            "class S {\n    std::vector<Lease> many;\n};\n",
            "many",
            "vector",
        ),
    ] {
        assert_eq!(
            names_a_field(path, source, field).as_deref(),
            Some(expected),
            "{path}: {field}"
        );
    }
}

/// A builtin is refused: no corpus symbol can bear that name, and answering
/// would pre-empt the evidence that comes next.
#[test]
fn a_primitive_field_states_nothing() {
    for (path, source, field) in [
        ("a.rs", "struct S {\n    n: u64,\n    ok: bool,\n}\n", "n"),
        ("a.rs", "struct S {\n    n: u64,\n    ok: bool,\n}\n", "ok"),
        ("a.ts", "class S {\n    n: number;\n}\n", "n"),
        ("a.sol", "contract S {\n    uint256 n;\n}\n", "n"),
    ] {
        assert_eq!(
            names_a_field(path, source, field),
            None,
            "{path}: {field} is a builtin and names no corpus type"
        );
    }
}

/// A local is not a field, however alike the two look in a grammar.
///
/// Swift spells a local `let` with the same `property_declaration` node as a
/// stored property, so the owner walk is the only thing telling them apart.
#[test]
fn a_local_inside_a_function_is_not_a_field() {
    for (path, source) in [
        ("a.swift", "func f() {\n    let lease: Lease = x\n}\n"),
        (
            "a.swift",
            "struct S {\n    func f() {\n        let lease: Lease = x\n    }\n}\n",
        ),
        ("a.kt", "fun f() {\n    val lease: Lease = x\n}\n"),
    ] {
        let owners: Vec<_> = field_types(path, source)
            .into_iter()
            .filter(|(_, name, _)| name == "lease")
            .collect();
        assert!(
            owners.is_empty(),
            "{path}: a function's local is not a member of the enclosing type, got {owners:?}"
        );
    }
}

/// Python's own idiom is a field written against `self` in `__init__`.
#[test]
fn a_python_self_annotation_is_a_field_of_its_class() {
    let source = "class Holder:\n\
         \x20   def __init__(self):\n\
         \x20       self.lease: Lease = make()\n";
    assert_eq!(
        field_types("a.py", source)
            .into_iter()
            .find(|(_, name, _)| name == "lease"),
        Some((
            "Lease".to_string(),
            "lease".to_string(),
            "a.py::Holder".to_string()
        )),
        "`self.lease: Lease` in a method declares a field, not a local"
    );
}

/// ...and a plain annotated local in the same method is not.
#[test]
fn a_python_bare_annotation_in_a_method_is_a_local() {
    let source = "class Holder:\n\
         \x20   def run(self):\n\
         \x20       lease: Lease = make()\n";
    let owners: Vec<_> = field_types("a.py", source)
        .into_iter()
        .filter(|(_, name, _)| name == "lease")
        .collect();
    assert!(
        owners.iter().all(|(_, _, owner)| owner != "a.py::Holder"),
        "a bare annotation in a method body is a local, got {owners:?}"
    );
}

/// Go still records the package a foreign field type is named through, and
/// records it only when the type resolved beside it.
#[test]
fn go_records_the_package_a_field_type_comes_from() {
    let qualifiers: Vec<_> =
        extract_file("a.go", "package p\ntype S struct {\n    L *pkg.Lease\n}\n")
            .references
            .into_iter()
            .filter(|reference| reference.kind == ReferenceKind::TypeQualifier)
            .map(|reference| (reference.name, reference.assigned_to))
            .collect();
    assert_eq!(qualifiers, vec![("pkg".to_string(), Some("L".to_string()))]);
    // A slice states no type, so it states no qualifier either.
    let sliced: Vec<_> = extract_file("b.go", "package p\ntype S struct {\n    L []pkg.Lease\n}\n")
        .references
        .into_iter()
        .filter(|reference| reference.kind == ReferenceKind::TypeQualifier)
        .collect();
    assert!(
        sliced.is_empty(),
        "a qualifier without a type beside it says where a field came from \
         without saying what it is, got {sliced:?}"
    );
}

/// A grammar with nothing to read is **refused**, not merely unlisted.
///
/// Without this, a dispatcher stubbed to say "yes" to everything satisfies both
/// the list check and the coverage sweep below — each only ever asks whether a
/// key is *in*. cargo-mutants found exactly that stub surviving.
#[test]
fn a_grammar_with_no_typed_field_is_refused() {
    for lang in [
        "javascript",
        "ruby",
        "lua",
        "luau",
        "shell",
        "erlang",
        "hcl",
        "nix",
        "r",
        "cobol",
        "sql",
    ] {
        assert!(
            !extracts_field_types(lang),
            "{lang} declares no typed field, so the dispatcher must refuse it"
        );
    }
    assert!(
        !extracts_field_types("vb"),
        "VB.NET has no linked grammar at all, so no tree-sitter walk can serve it"
    );
    assert!(!extracts_field_types(""), "an empty key names no language");
}

/// A pathological type expression terminates, and abstains rather than guessing.
///
/// Both reducers are depth-bounded, and the bound is the whole defence against
/// a generated or adversarial file: unbounded recursion here is a stack
/// overflow in the extractor, not a wrong answer. The cases sit either side of
/// each bound so that widening it, narrowing it, or failing to advance the
/// counter at all are three distinguishable failures rather than one.
#[test]
fn a_pathological_type_expression_is_bounded() {
    // `nominal_type_name`: one `reference_type` per `&`.
    let refs = |count: usize| format!("struct S {{\n    r: {}Lease,\n}}\n", "&".repeat(count));
    assert_eq!(
        names_a_field("a.rs", &refs(16), "r").as_deref(),
        Some("Lease"),
        "sixteen references is inside the bound and must still reduce"
    );
    assert_eq!(
        names_a_field("a.rs", &refs(17), "r"),
        None,
        "seventeen is past it, and past it the answer is no answer"
    );
    assert_eq!(
        names_a_field("a.rs", &refs(400), "r"),
        None,
        "four hundred must terminate, not recurse to exhaustion"
    );

    // `declarator_name`: one `pointer_declarator` per `*`.
    let stars = |count: usize| {
        format!(
            "struct S {{\n    struct Lease {}lease;\n}};\n",
            "*".repeat(count)
        )
    };
    assert_eq!(
        names_a_field("a.c", &stars(8), "lease").as_deref(),
        Some("Lease"),
        "eight pointers is inside the bound"
    );
    assert_eq!(
        names_a_field("a.c", &stars(9), "lease"),
        None,
        "nine is past it"
    );
    assert_eq!(
        names_a_field("a.c", &stars(400), "lease"),
        None,
        "four hundred must terminate"
    );
}

/// The dispatcher and its published list cannot disagree.
#[test]
fn the_language_list_is_the_dispatcher() {
    for lang in FIELD_TYPE_LANGUAGES {
        assert!(
            extracts_field_types(lang),
            "{lang} is listed but the dispatcher has no arm for it"
        );
    }
    let mut sorted = FIELD_TYPE_LANGUAGES.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        sorted.as_slice(),
        FIELD_TYPE_LANGUAGES,
        "the list must be sorted and free of duplicates so a reader can scan it"
    );
}

/// Every linked grammar is either covered or named as having nothing to cover.
///
/// The sweep that stops this from rotting: a grammar linked later shows up here
/// as an unclassified key rather than as a silent hole, which is how the gap
/// this module closes survived twenty-nine grammars in the first place.
#[test]
fn every_linked_grammar_is_classified() {
    // Grammars with no typed field declaration to read. Each is a fact about
    // the language, not a backlog entry.
    const NO_TYPED_FIELDS: &[&str] = &[
        "javascript", // untyped; a class field has no annotation to read
        "jsx",
        "ruby", // `@ivar` carries no declared type
        "lua",  // untyped tables
        "luau",
        "r", // untyped
        "shell",
        "erlang", // records are typed only by optional specs, not by the field
        "cobol",
        "cfml",
        "nix", // attribute sets are untyped
        "hcl", // block arguments are untyped
        "sql",
        "svelte", // markup; its `<script>` is merged as typescript
        "vue",
        "astro",
        "liquid",
        "markdown",
        "json",
        "yaml",
        "toml",
    ];
    let mut unclassified: Vec<&str> = Vec::new();
    for key in linked_grammar_keys() {
        if extracts_field_types(key) || NO_TYPED_FIELDS.contains(&key) {
            continue;
        }
        unclassified.push(key);
    }
    assert!(
        unclassified.is_empty(),
        "these linked grammars are neither read for field types nor recorded as \
         having none: {unclassified:?} — add an arm, or a line to NO_TYPED_FIELDS \
         saying why the language has nothing to read"
    );
}

/// A registry language whose grammar this module reads must actually parse.
///
/// The sweep above asks the *grammar* keys; this asks the registry, so a
/// language routed to a covered grammar (ArkTS to `typescript`, Metal to `cpp`)
/// is proved to reach the walk rather than assumed to. Written as a real
/// extraction because a name-only check would pass for a language whose
/// extension never reaches `extract_file`.
#[test]
fn a_registry_language_routed_to_a_covered_grammar_reads_its_fields() {
    let linked = linked_grammar_keys();
    let covered: Vec<&devmap_extract::languages::LanguageSpec> = LANGUAGE_SPECS
        .iter()
        .filter(|spec| linked.contains(&spec.grammar) && extracts_field_types(spec.grammar))
        .collect();
    assert!(
        covered.len() >= 15,
        "expected the registry to route at least fifteen languages to a covered \
         grammar, got {}",
        covered.len()
    );
    // ArkTS borrows TypeScript's grammar and must read fields through it.
    let arkts = LANGUAGE_SPECS
        .iter()
        .find(|spec| spec.name == "ArkTS")
        .expect("ArkTS is registered");
    assert!(extracts_field_types(arkts.grammar));
    let extension = arkts.extensions.first().expect("ArkTS has an extension");
    let path = format!("a{extension}");
    assert_eq!(
        names_a_field(&path, "class S {\n    private lease: Lease;\n}\n", "lease").as_deref(),
        Some("Lease"),
        "{path}: a borrowed grammar must still reach the field walk"
    );
}
