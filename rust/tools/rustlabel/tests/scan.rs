//! Site enumeration: which positions rust-analyzer is asked about, and what
//! the labeller refuses to pretend it saw.

use rustlabel::classify::SiteShape;
use rustlabel::sites::{operator_candidates, scan, SiteKind};

fn at(source: &str, needle: &str, nth: usize) -> usize {
    source.match_indices(needle).nth(nth).expect(needle).0
}

#[test]
fn calls_are_asked_about_at_their_own_name() {
    let source = "fn a() {\n    b();\n    x.c();\n    m::d();\n    Self::e();\n    y.f::<u8>();\n    g::<u8>();\n}\n";
    let facts = scan(source, false).unwrap();
    let got: Vec<(&str, usize)> = facts
        .sites
        .iter()
        .map(|s| (s.name.as_str(), s.query_byte))
        .collect();
    let want = vec![
        ("b", at(source, "b()", 0)),
        ("c", at(source, "c()", 0)),
        ("d", at(source, "d()", 0)),
        ("e", at(source, "e()", 0)),
        ("f", at(source, "f::<u8>", 0)),
        ("g", at(source, "g::<u8>", 0)),
    ];
    assert_eq!(got, want);
    assert!(facts.sites.iter().all(|s| s.kind == SiteKind::Call));
    assert_eq!(facts.sites[0].shape, SiteShape::BareCall);
    assert_eq!(
        facts.sites[1].shape,
        SiteShape::MethodCall {
            receiver: "x".into()
        }
    );
    assert_eq!(
        facts.sites[2].shape,
        SiteShape::PathCall {
            qualifier: "m".into()
        }
    );
    assert_eq!(
        facts.sites[3].shape,
        SiteShape::PathCall {
            qualifier: "Self".into()
        }
    );
}

#[test]
fn a_call_after_non_ascii_text_is_asked_about_at_a_byte_that_starts_it() {
    let source = "fn a() {\n    let s = \"héllo → wörld\"; go();\n}\n";
    let facts = scan(source, false).unwrap();
    assert_eq!(facts.sites.len(), 1);
    let byte = facts.sites[0].query_byte;
    assert!(source.is_char_boundary(byte));
    assert!(source[byte..].starts_with("go()"));
}

/// A macro's arguments are a token tree the grammar never parses as calls, so
/// a call inside one is not a site. It is counted, and the identifiers a macro
/// spells are listed, so the run can refuse to call the enclosing function
/// fully read when one of them names a function of the corpus.
#[test]
fn a_call_inside_a_macro_is_not_a_site_but_is_never_invisible() {
    let source =
        "fn a() {\n    println!(\"{}\", f(1));\n    vec![g()];\n    println!(\"hi\");\n}\n";
    let facts = scan(source, false).unwrap();
    assert!(facts.sites.is_empty(), "{:?}", facts.sites);
    assert_eq!(facts.macro_invocations, 3);
    assert_eq!(
        facts.macro_idents,
        vec![
            (at(source, "println!(\"{}\"", 0), vec!["f".to_string()]),
            (at(source, "vec!", 0), vec!["g".to_string()]),
        ]
    );
}

/// A macro that names a function by value (`generate_handler![cmd]`) hides a
/// use as surely as one that calls it. (Fixture C39: `cmd_c39`.)
#[test]
fn a_function_named_without_parentheses_inside_a_macro_is_listed_too() {
    let source = "fn a() {\n    handler![cmd_one, cmd_two];\n    assert_eq!(x.len(), 3);\n}\n";
    let facts = scan(source, false).unwrap();
    assert_eq!(
        facts.macro_idents,
        vec![
            (
                at(source, "handler!", 0),
                vec!["cmd_one".to_string(), "cmd_two".to_string()]
            ),
            (
                at(source, "assert_eq!", 0),
                vec!["len".to_string(), "x".to_string()]
            ),
        ]
    );
}

#[test]
fn a_macro_rules_body_lists_its_identifiers_and_each_invocation_is_named() {
    let source = "macro_rules! log {\n    ($m:expr) => { $crate::emit($m) };\n}\nmacro_rules! twice {\n    ($e:expr) => { $e + $e };\n}\nfn a() {\n    log!(1);\n    mod_a::other!(2);\n}\n";
    let facts = scan(source, false).unwrap();
    assert_eq!(
        facts
            .macro_defs
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["log", "twice"]
    );
    let log_body = facts
        .macro_idents
        .iter()
        .find(|(at_byte, _)| *at_byte == at(source, "macro_rules! log", 0))
        .expect("log's body is listed");
    assert!(log_body.1.contains(&"emit".to_string()), "{log_body:?}");
    assert_eq!(
        facts.macro_calls,
        vec![
            (at(source, "log!(1)", 0), "log".to_string()),
            (at(source, "mod_a::other!", 0), "other".to_string()),
        ]
    );
}

#[test]
fn a_callee_that_is_not_a_path_or_a_field_is_unsupported_and_counted() {
    let source = "fn a() {\n    (self.cb)(1);\n    make()(2);\n    ok();\n}\n";
    let facts = scan(source, false).unwrap();
    // `make()` is a site; the call of its result and the parenthesised
    // field are not.
    let names: Vec<&str> = facts.sites.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["make", "ok"]);
    assert_eq!(
        facts.unsupported_callees,
        vec![at(source, "(self.cb)(1)", 0), at(source, "make()(2)", 0)]
    );
}

#[test]
fn function_declarations_are_found_by_their_name_byte_in_traits_impls_and_inline_modules() {
    let source = "trait Speak {\n    fn speak(&self);\n    fn twice(&self) { self.speak(); }\n}\nstruct Dog;\nimpl Speak for Dog {\n    fn speak(&self) {}\n}\nmod m {\n    pub fn helper() {}\n}\nfn top() {\n    fn inner() {}\n}\n";
    let facts = scan(source, false).unwrap();
    let decl = |needle: &str, nth: usize| {
        let byte = at(source, needle, nth);
        let d = facts
            .fn_decls
            .get(&byte)
            .unwrap_or_else(|| panic!("no declaration at {needle}#{nth}: {:?}", facts.fn_decls));
        (d.name.clone(), d.in_trait)
    };
    assert_eq!(decl("speak(&self);", 0), ("speak".into(), true));
    assert_eq!(decl("twice", 0), ("twice".into(), true));
    assert_eq!(decl("speak(&self) {}", 0), ("speak".into(), false));
    assert_eq!(decl("helper", 0), ("helper".into(), false));
    assert_eq!(decl("top", 0), ("top".into(), false));
    assert_eq!(decl("inner", 0), ("inner".into(), false));
    // A call to `speak` is a use, not a declaration.
    assert!(!facts.fn_decls.contains_key(&at(source, "speak();", 0)));
}

#[test]
fn a_use_alias_is_recorded_because_it_renames_a_callee() {
    let facts = scan("use a::b as c;\nuse d::{e as f, g};\nfn x() {}\n", false).unwrap();
    assert_eq!(
        facts.aliases.iter().map(String::as_str).collect::<Vec<_>>(),
        vec!["c", "f"]
    );
}

#[test]
fn references_are_functions_used_as_values_and_only_when_asked_for() {
    let source = "fn a() {\n    xs.map(helper);\n    run(Self::step);\n    let f = util::go;\n    let n = 1 + count;\n    helper();\n}\n";
    let without = scan(source, false).unwrap();
    assert!(without.sites.iter().all(|s| s.kind != SiteKind::Value));

    let with = scan(source, true).unwrap();
    let values: Vec<(&str, SiteShape)> = with
        .sites
        .iter()
        .filter(|s| s.kind == SiteKind::Value)
        .map(|s| (s.name.as_str(), s.shape.clone()))
        .collect();
    assert_eq!(
        values,
        vec![
            ("helper", SiteShape::ValuePath { qualifier: None }),
            (
                "step",
                SiteShape::ValuePath {
                    qualifier: Some("Self".into())
                }
            ),
            (
                "go",
                SiteShape::ValuePath {
                    qualifier: Some("util".into())
                }
            ),
        ]
    );
    // `helper()` is a call, and the callee of a call is never also a value.
    let helper_sites = with.sites.iter().filter(|s| s.name == "helper").count();
    assert_eq!(helper_sites, 2, "{:?}", with.sites);
}

#[test]
fn operators_are_asked_about_at_the_operator_token() {
    let source = "fn a() {\n    a + b;\n    a += 1;\n    -a;\n    !a;\n    *a;\n    v[0];\n    r?;\n    a == b;\n    a && b;\n    a || b;\n}\n";
    let facts = scan(source, false).unwrap();
    let ops: Vec<(&str, usize)> = facts
        .sites
        .iter()
        .filter(|s| s.kind == SiteKind::Operator)
        .map(|s| (s.name.as_str(), s.query_byte))
        .collect();
    let want = vec![
        ("+", at(source, "+ b", 0)),
        ("+=", at(source, "+=", 0)),
        ("-", at(source, "-a", 0)),
        ("!", at(source, "!a", 0)),
        ("*", at(source, "*a", 0)),
        ("[", at(source, "[0]", 0)),
        ("?", at(source, "?;", 0)),
        ("==", at(source, "==", 0)),
    ];
    assert_eq!(ops, want);
}

#[test]
fn an_operator_names_the_trait_methods_it_can_reach() {
    assert_eq!(operator_candidates("+"), &["add"]);
    assert_eq!(operator_candidates("+="), &["add_assign"]);
    assert_eq!(operator_candidates("["), &["index", "index_mut"]);
    assert_eq!(
        operator_candidates("?"),
        &["from", "branch", "from_residual"]
    );
    assert_eq!(operator_candidates("*"), &["deref", "deref_mut", "mul"]);
    assert!(operator_candidates("&&").is_empty());
}
