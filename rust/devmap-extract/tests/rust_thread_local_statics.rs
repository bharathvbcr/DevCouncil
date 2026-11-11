//! `thread_local!` declares the statics it names, read from the invocation's own
//! tokens: a `static` inside a string literal or a comment is not a declaration,
//! and neither is the `'static` lifetime or anything inside an initializer.

use devmap_extract::extract_file;

fn declared(source: &str) -> Vec<String> {
    let extraction = extract_file("src/lib.rs", source);
    let facts = extraction
        .rust_scope_facts
        .expect("scope facts")
        .macros
        .expect("macro facts");
    facts
        .calls
        .into_iter()
        .filter(|call| call.name == "thread_local")
        .flat_map(|call| call.declares)
        .collect()
}

#[test]
fn a_static_named_by_the_invocation_is_declared() {
    assert_eq!(
        declared("thread_local! {\n    static A: u8 = 1;\n    pub static B: u16 = 2;\n}\n"),
        vec!["A", "B"]
    );
}

#[test]
fn a_static_in_a_string_literal_or_a_comment_is_not_declared() {
    let source =
        "thread_local! {\n    // static LINE: u8 = 1;\n    /* static BLOCK: u8 = 1; */\n    \
                  static REAL: &'static str = \"static FAKE: u8 = 1;\";\n    \
                  static RAW: &str = r#\"static RAWFAKE\"#;\n}\n";
    assert_eq!(declared(source), vec!["REAL", "RAW"]);
}

#[test]
fn what_an_initializer_holds_declares_nothing_here() {
    let source = "thread_local! {\n    static OUTER: u8 = { static INNER: u8 = 1; INNER };\n}\n";
    assert_eq!(declared(source), vec!["OUTER"]);
}

#[test]
fn a_type_after_a_static_lifetime_is_not_a_declaration() {
    let source =
        "thread_local! {\n    static CFG: Cell<Option<&'static Config>> = Cell::new(None);\n    \
                  static MUT: Cell<&'static mut Other> = Cell::new(None);\n    \
                  static DYN: Box<dyn Handler + 'static> = Box::new(H);\n}\n";
    assert_eq!(declared(source), vec!["CFG", "MUT", "DYN"]);
}
