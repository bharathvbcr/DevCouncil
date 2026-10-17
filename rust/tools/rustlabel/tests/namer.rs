//! Symbol ids in exactly the form the scorer joins on: `<file>::<qualified
//! name>`, taken from the extractor that names DevMap's own nodes, and the
//! file path itself for a site outside any function.

use rustlabel::namer::Namer;
use rustlabel::sites::scan;

const SOURCE: &str = "static TABLE: Option<u8> = init();\ntrait Speak {\n    fn speak(&self);\n}\nstruct Dog;\nimpl Speak for Dog {\n    fn speak(&self) {}\n}\nmod m {\n    pub fn helper() {}\n}\nfn top() {\n    fn inner() {}\n    let f = || helper();\n}\n";

fn namer() -> Namer {
    Namer::new(&[("lib.rs".to_string(), SOURCE.to_string())])
}

fn at(needle: &str) -> usize {
    SOURCE.find(needle).expect(needle)
}

#[test]
fn a_site_is_named_for_the_innermost_function_around_it() {
    let n = namer();
    assert_eq!(n.caller_at("lib.rs", at("helper();")), "lib.rs::top");
    // Inside the nested function the nested function is the caller.
    assert_eq!(
        n.caller_at("lib.rs", at("inner() {}") + 4),
        "lib.rs::top.inner"
    );
}

#[test]
fn a_site_outside_every_function_is_the_file() {
    let n = namer();
    assert_eq!(n.caller_at("lib.rs", at("init()")), "lib.rs");
    assert_eq!(n.caller_at("unknown.rs", 0), "unknown.rs");
}

#[test]
fn a_declaration_is_named_by_its_type_for_an_impl_and_its_trait_for_a_declaration() {
    let n = namer();
    let facts = scan(SOURCE, false).unwrap();
    let id_of = |needle: &str, nth: usize| {
        let byte = SOURCE.match_indices(needle).nth(nth).unwrap().0;
        assert!(facts.fn_decls.contains_key(&byte), "{needle}#{nth}");
        n.target_at("lib.rs", byte)
    };
    let trait_decl = id_of("speak(&self);", 0).unwrap();
    assert_eq!(trait_decl.id, "lib.rs::Speak.speak");
    assert!(trait_decl.is_method);
    let impl_method = id_of("speak(&self) {}", 0).unwrap();
    assert_eq!(impl_method.id, "lib.rs::Dog.speak");
    assert!(impl_method.is_method);
    let free = id_of("helper", 0).unwrap();
    assert_eq!(free.id, "lib.rs::helper");
    assert!(!free.is_method);
}

#[test]
fn a_position_in_no_function_has_no_target() {
    let n = namer();
    assert!(n.target_at("lib.rs", at("Option")).is_none());
    assert!(n.target_at("nowhere.rs", 0).is_none());
}

#[test]
fn every_callable_and_file_is_listed_for_coverage() {
    let n = namer();
    let callers: Vec<&str> = n.callers().collect();
    for want in [
        "lib.rs",
        "lib.rs::Speak.speak",
        "lib.rs::Dog.speak",
        "lib.rs::helper",
        "lib.rs::top",
        "lib.rs::top.inner",
    ] {
        assert!(callers.contains(&want), "{want} not in {callers:?}");
    }
}
