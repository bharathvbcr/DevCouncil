//! Lane `verify-residue-2`: the alias family's last two breaks (ft-108444ad).
//!
//! * Row 3: a `macro_rules!` under `#[cfg(..)]` or `#[cfg_attr(..)]` may be
//!   compiled out, so it must not shadow a std alias of the same name.
//! * Row 4: the alias walk's work is bounded by the records, whatever flips a
//!   name's binding, and a cap that fires degrades the file.
//!
//! The two attack tests of the round (`m2_cfg_disabled_macro_rules_..` and
//! `m5_pingpong_alias_flip_is_bounded_in_time`) are in `attack2_round2.rs`;
//! these are the variants the round-2 brief names.

use std::time::{Duration, Instant};

use dc_verify::parse_unified;
use dc_verify::rigor::{Severity, StubReport, detect_stubs_report};

fn report(body: &str) -> StubReport {
    let lines: Vec<&str> = body.lines().collect();
    let mut diff = format!(
        "diff --git a/src/lib.rs b/src/lib.rs\nnew file mode 100644\n--- /dev/null\n+++ b/src/lib.rs\n@@ -0,0 +1,{} @@\n",
        lines.len()
    );
    for line in lines {
        diff.push('+');
        diff.push_str(line);
        diff.push('\n');
    }
    let files = parse_unified(&diff).expect("diff parses");
    let post = body.to_string();
    detect_stubs_report(&files, &|_| Some(post.clone()))
}

fn blocked(r: &StubReport) -> bool {
    r.findings
        .iter()
        .any(|f| f.gate == "stub_detection" && f.severity == Severity::Blocking)
}

fn not_clean(body: &str) {
    let r = report(body);
    assert!(
        blocked(&r) || !r.unparsed.is_empty(),
        "a stub scanned clean: {body:?}\nfindings={:?}",
        r.findings
    );
}

fn clean(body: &str) {
    let r = report(body);
    assert!(
        r.unparsed.is_empty() && !blocked(&r),
        "a clean file was degraded or blocked: {body:?}\nunparsed={:?} findings={:?}",
        r.unparsed,
        r.findings
    );
}

const CALL: &str = "pub fn f() -> u32 {\n    t!()\n}\n";
const ALIAS: &str = "use std::todo as t;\n";
const MACRO: &str = "macro_rules! t {\n    () => { 0 };\n}\n";

fn with_attrs(attrs: &str) -> String {
    format!("{ALIAS}{attrs}{MACRO}{CALL}")
}

#[test]
fn m2_cfg_attr_wrapping_cfg_does_not_hide_the_alias() {
    not_clean(&with_attrs("#[cfg_attr(all(), cfg(any()))]\n"));
}

#[test]
fn m2_cfg_attr_alone_does_not_hide_the_alias() {
    not_clean(&with_attrs("#[cfg_attr(feature = \"x\", allow(unused))]\n"));
}

#[test]
fn m2_plain_cfg_test_does_not_hide_the_alias() {
    not_clean(&with_attrs("#[cfg(test)]\n"));
}

#[test]
fn m2_doc_comments_between_the_cfg_and_the_macro_do_not_hide_the_alias() {
    not_clean(&with_attrs("#[cfg(any())]\n/// doc\n/// more doc\n"));
}

#[test]
fn m2_a_cfg_before_other_attributes_does_not_hide_the_alias() {
    not_clean(&with_attrs("#[cfg(any())]\n#[allow(unused_macros)]\n"));
}

#[test]
fn m2_a_cfg_gated_macro_does_not_feed_the_other_file_degrade() {
    // `t` is imported from another file and a cfg-gated macro of the same name
    // is not a definition this file can rely on: the call stays unresolved.
    let body = format!("use crate::t;\n#[cfg(any())]\n{MACRO}pub fn f() -> u32 {{\n    t!()\n}}\n");
    let r = report(&body);
    assert!(!r.unparsed.is_empty(), "{:?}", r.findings);
}

#[test]
fn m2_an_ungated_attribute_still_lets_the_real_macro_hide_the_alias() {
    clean(&with_attrs("#[allow(unused_macros)]\n"));
    clean(&with_attrs("#[macro_export]\n"));
    clean(&with_attrs("/// documented\n"));
}

#[test]
fn m2_a_cfg_on_the_previous_item_is_not_this_macros_cfg() {
    clean(&format!(
        "{ALIAS}#[cfg(test)]\nfn other() {{}}\n{MACRO}{CALL}"
    ));
}

// ------------------------------------------------------------------ row 4

fn pingpong(k: usize, m: usize) -> String {
    let mut body = String::new();
    for _ in 0..k {
        body.push_str("use std::todo as t;\nuse std::unimplemented as t;\n");
    }
    for _ in 0..m {
        body.push_str("use t as u;\n");
    }
    for _ in 0..m {
        body.push_str("use u as t;\n");
    }
    body.push_str(CALL);
    body
}

fn timed(body: &str) -> (Duration, StubReport) {
    let start = Instant::now();
    let r = report(body);
    (start.elapsed(), r)
}

/// The 67 s attack of round 2: 36,000 records, 582 KB, inside both caps.
#[test]
fn m5_the_67_second_pingpong_is_bounded_and_still_blocks() {
    let (took, r) = timed(&pingpong(6_000, 12_000));
    eprintln!("pingpong 6000/12000: {took:?} blocked={}", blocked(&r));
    // Loose 10 s backstop only: the exact bound is the step count asserted by
    // stub_ast::step_count_tests, which a loaded machine cannot flake.
    assert!(took < Duration::from_secs(10), "took {took:?}");
    assert!(blocked(&r) || !r.unparsed.is_empty());
}

/// Ten times the attack size: past the record cap, so the cap fires before the
/// walk starts. It must be degraded or blocked, never clean, and quick.
#[test]
fn m5_ten_times_the_attack_is_degraded_or_blocked_in_time() {
    let (took, r) = timed(&pingpong(20_000, 20_000));
    eprintln!(
        "pingpong 20000/20000: {took:?} blocked={} unparsed={:?}",
        blocked(&r),
        r.unparsed
    );
    // Loose 10 s backstop only: the exact bound is the step count asserted by
    // stub_ast::step_count_tests, which a loaded machine cannot flake.
    assert!(took < Duration::from_secs(10), "took {took:?}");
    assert!(blocked(&r) || !r.unparsed.is_empty());
}

/// A flip through `panic`: a name bound to `panic`, then `todo`, then `panic`
/// again must end on the strong stub, and not re-queue its dependents on each
/// flip.
#[test]
fn m5_flips_through_panic_end_on_the_strong_stub_and_block() {
    let mut body = String::new();
    for _ in 0..3_000 {
        body.push_str("use std::panic as t;\nuse std::todo as t;\n");
    }
    for _ in 0..3_000 {
        body.push_str("use t as u;\nuse u as t;\n");
    }
    body.push_str(CALL);
    let (took, r) = timed(&body);
    eprintln!("panic flips: {took:?}");
    // Loose 10 s backstop only: the exact bound is the step count asserted by
    // stub_ast::step_count_tests, which a loaded machine cannot flake.
    assert!(took < Duration::from_secs(10), "took {took:?}");
    assert!(blocked(&r), "{:?} {:?}", r.findings, r.unparsed);
}

/// One first segment carrying many names: every leaf hangs off `std`.
#[test]
fn m5_one_first_segment_with_many_names_is_linear() {
    let mut body = String::new();
    for i in 0..20_000 {
        body.push_str(&format!("use std::todo as a{i};\n"));
    }
    body.push_str("use a19999 as t;\n");
    body.push_str(CALL);
    let (took, r) = timed(&body);
    eprintln!("many names: {took:?}");
    // Loose 10 s backstop only: the exact bound is the step count asserted by
    // stub_ast::step_count_tests, which a loaded machine cannot flake.
    assert!(took < Duration::from_secs(10), "took {took:?}");
    assert!(blocked(&r), "{:?} {:?}", r.findings, r.unparsed);
}
