//! Stubs the line and tree checks both used to miss: a panic whose message is
//! assembled by another macro, and a wrapper macro that is not named
//! `todo`, `unimplemented`, or `panic`.

use dc_verify::parse_unified;
use dc_verify::rigor::{Severity, detect_stubs_report};

fn whole_file(path: &str, body: &str) -> String {
    let lines: Vec<&str> = body.lines().collect();
    let mut diff = format!(
        "diff --git a/{path} b/{path}\nnew file mode 100644\n--- /dev/null\n+++ b/{path}\n@@ -0,0 +1,{} @@\n",
        lines.len()
    );
    for line in lines {
        diff.push('+');
        diff.push_str(line);
        diff.push('\n');
    }
    diff
}

fn blocks_or_degrades(body: &str) {
    let diff = whole_file("src/lib.rs", body);
    let files = parse_unified(&diff).expect("diff");
    let report = detect_stubs_report(&files, &|_| None);
    let finding = report
        .findings
        .iter()
        .any(|f| f.gate == "stub_detection" && f.severity == Severity::Blocking);
    let degraded = !report.unparsed.is_empty();
    assert!(
        finding || degraded,
        "scanned clean: findings={:?} unparsed={:?}",
        report.findings,
        report.unparsed
    );
}

#[test]
fn panic_concat_of_a_placeholder_is_not_clean() {
    blocks_or_degrades("pub fn f() {\n    panic!(concat!(\"not \", \"implemented\"));\n}\n");
}

#[test]
fn a_wrapper_macro_that_is_not_named_todo_is_not_clean() {
    blocks_or_degrades(
        "macro_rules! unfinished {\n    () => { unimplemented!() };\n}\npub fn work() { unfinished!(); }\n",
    );
}

fn adv_blocking(body: &str) -> bool {
    let diff = whole_file("src/lib.rs", body);
    let files = parse_unified(&diff).expect("diff");
    let report = detect_stubs_report(&files, &|_| None);
    report
        .findings
        .iter()
        .any(|f| f.gate == "stub_detection" && f.severity == Severity::Blocking)
}

#[test]
fn adv_real_function_mentioning_todo_is_not_blocking() {
    let body = "\
pub fn work(n: i32) -> i32 {
    println!(\"see todo!() in the docs\");
    // the docs also say unimplemented!()
    n + 1
}
";
    assert!(
        !adv_blocking(body),
        "a string or comment that names a stub token was a blocking stub"
    );
}

#[test]
fn adv_macro_rules_quoting_todo_is_not_blocking() {
    let body = "\
macro_rules! announce {
    () => { println!(\"see todo!() in the docs\"); };
}
pub fn work() -> i32 {
    announce!();
    1 + 2
}
";
    assert!(
        !adv_blocking(body),
        "a wrapper whose text only quotes todo!() was a blocking stub"
    );
}

#[test]
fn adv_panic_concat_with_non_literal_is_not_not_implemented() {
    let body = "\
pub fn f(detail: &str) {
    panic!(concat!(\"not \", detail));
}
";
    assert!(
        !adv_blocking(body),
        "panic!(concat!(\"not \", detail)) was treated as \"not implemented\""
    );
}

#[test]
fn adv_panic_concat_joining_across_a_non_literal_is_not_a_placeholder() {
    let body = "\
pub fn f(detail: &str) {
    panic!(concat!(\"not \", detail, \"implemented\"));
}
";
    assert!(
        !adv_blocking(body),
        "literals joined across a non-literal were treated as \"not implemented\""
    );
}

#[test]
fn adv_panic_concat_queue_invariant_stays_clean() {
    let body = "\
pub fn f(len: usize) {
    panic!(concat!(\"queue invariant \", \"violated\"), len);
}
";
    assert!(
        !adv_blocking(body),
        "panic!(concat!(\"queue invariant \", \"violated\")) was a blocking stub"
    );
}

#[test]
fn a_panic_of_a_real_error_stays_clean() {
    let diff = whole_file(
        "src/lib.rs",
        "pub fn f(len: usize) {\n    panic!(\"queue invariant violated: len {}\", len);\n}\n",
    );
    let files = parse_unified(&diff).expect("diff");
    let report = detect_stubs_report(&files, &|_| None);
    let blocking = report
        .findings
        .iter()
        .any(|f| f.gate == "stub_detection" && f.severity == Severity::Blocking);
    assert!(
        !blocking,
        "a real panic was reported as a stub: {:?}",
        report.findings
    );
}

#[test]
fn a_char_literal_quote_does_not_hide_a_stub_in_a_wrapper_macro() {
    let body = "\
macro_rules! work {
    ($($t:tt)*) => {};
}
pub fn f() {
    work!('\"', todo!());
}
";
    assert!(
        adv_blocking(body),
        "a `'\"'` char literal opened a string that hid the todo!() after it"
    );
}

#[test]
fn an_escaped_quote_char_literal_does_not_hide_a_stub_either() {
    let body = "\
macro_rules! work {
    ($($t:tt)*) => {};
}
pub fn f() {
    work!('\\\"', '\\'', todo!());
}
";
    assert!(
        adv_blocking(body),
        "an escaped-quote char literal hid the todo!() after it"
    );
}

#[test]
fn a_raw_string_holding_a_quote_does_not_expose_text_after_it() {
    let body = "\
pub fn f() -> usize {
    println!(r#\"a \" todo!() b\"#);
    1
}
";
    assert!(
        !adv_blocking(body),
        "text inside a raw string was read as code and flagged as a stub"
    );
}

#[test]
fn a_lifetime_is_not_a_char_literal() {
    let body = "\
macro_rules! work {
    ($($t:tt)*) => {};
}
pub fn f<'a>(x: &'a str) -> &'a str {
    work!(x, todo!());
    x
}
";
    assert!(
        adv_blocking(body),
        "a lifetime tick swallowed the rest of the line"
    );
}

#[test]
fn a_non_ascii_identifier_before_a_stub_does_not_panic_the_line_scan() {
    let body = "\
macro_rules! work {
    ($($t:tt)*) => {};
}
pub fn f() {
    work!(café, todo!());
}
";
    assert!(
        adv_blocking(body),
        "a stub after a non-ASCII identifier was not found"
    );
}

#[test]
fn a_multiline_block_comment_naming_a_stub_is_not_code() {
    let body = "\
macro_rules! work {
    ($($t:tt)*) => {};
}
pub fn f() -> i32 {
    work!(1, /* a note
        todo!() would go here
    */ 2);
    3
}
";
    assert!(
        !adv_blocking(body),
        "text inside a multi-line block comment was read as code"
    );
}

#[test]
fn a_comparison_with_a_todo_identifier_is_not_a_stub() {
    let body = "\
macro_rules! work {
    ($($t:tt)*) => {};
}
pub fn f(todo: i32) -> i32 {
    work!(todo != 3);
    todo
}
";
    assert!(
        !adv_blocking(body),
        "`todo != 3` was read as the todo! macro"
    );
}

#[test]
fn trivia_between_the_macro_name_and_its_bang_is_still_the_macro() {
    let body = "\
macro_rules! work {
    ($($t:tt)*) => {};
}
pub fn f() {
    work!(todo /* why */ !());
}
";
    assert!(adv_blocking(body), "a comment before the bang hid todo!()");
}
