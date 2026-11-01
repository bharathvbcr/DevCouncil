//! Svelte template text: a bare `&` or `>` no longer fails the file.
//!
//! tree-sitter-svelte-ng reads `Code Age & Half-Life` and `for > 6 months` as
//! ERROR nodes that swallow the rest of the text run. `crate::sveltetext`
//! parses a same-width copy with a letter written over the character and keeps
//! that tree only if it holds strictly less error; the extractors still read
//! the file's own text. These tests state the behaviour from outside the crate;
//! the unit tests in `sveltetext.rs` pin the rewrite itself.
//!
//! Measured on the frozen GitPulse corpus (11 partial Svelte files, 12 charged
//! files in all): see the commit message.

use std::time::Duration;

use devmap_extract::model::{Extraction, ParseOutcome};
use devmap_extract::extract_file_with_budget;

const BUDGET: Duration = Duration::from_secs(600);

fn ex(path: &str, source: &str) -> Extraction {
    extract_file_with_budget(path, source, BUDGET)
}

fn is_clean(e: &Extraction) -> bool {
    matches!(e.parse_outcome, ParseOutcome::Clean)
}

fn has_symbol(e: &Extraction, name: &str) -> bool {
    e.symbols.iter().any(|s| s.name == name)
}

fn callees(e: &Extraction) -> Vec<&str> {
    e.calls.iter().map(|c| c.callee_name.as_str()).collect()
}

#[test]
fn a_bare_ampersand_in_template_text_parses_clean_and_keeps_the_script() {
    let source = "<script lang=\"ts\">function f(){}</script>\n<h3>Code Age & Half-Life</h3>\n";
    let e = ex("src/Pulse.svelte", source);
    assert!(is_clean(&e), "outcome: {:?}", e.parse_outcome);
    assert!(has_symbol(&e, "f"), "the script's function is declared");
    assert!(
        !callees(&e).iter().any(|c| c.contains("Half")),
        "template words are not calls: {:?}",
        callees(&e)
    );
}

#[test]
fn a_parenthesised_word_after_a_bare_ampersand_is_not_a_call() {
    // `(DORA)` after the error reads as a call to `DORA` when the ERROR node is
    // walked as code; with the text parsed as text it is just text.
    let source = "<h3>Delivery & Operations (DORA)</h3>\n";
    let e = ex("src/Dora.svelte", source);
    assert!(is_clean(&e), "outcome: {:?}", e.parse_outcome);
    assert!(
        !callees(&e).contains(&"DORA"),
        "a phantom call: {:?}",
        callees(&e)
    );
}

#[test]
fn a_greater_than_in_template_text_parses_clean() {
    let e = ex("src/Age.svelte", "<p>for > 6 months</p>\n");
    assert!(is_clean(&e), "outcome: {:?}", e.parse_outcome);
}

#[test]
fn several_greater_thans_in_one_run_are_revealed_one_pass_at_a_time() {
    let e = ex("src/Chain.svelte", "<p>a > b > c > d</p>\n");
    assert!(is_clean(&e), "outcome: {:?}", e.parse_outcome);
}

#[test]
fn the_ampersand_and_the_greater_than_together_in_a_nested_template() {
    let source = "<script>function n(){}</script>\n<div>\n  <section>\n    <h4>Scan & Clean</h4>\n    <p>\n      inactive for > 6 months\n    </p>\n  </section>\n</div>\n";
    let e = ex("src/Both.svelte", source);
    assert!(is_clean(&e), "outcome: {:?}", e.parse_outcome);
    assert!(has_symbol(&e, "n"));
}

#[test]
fn a_symbol_after_the_repaired_text_keeps_the_files_own_offsets() {
    // The repaired copy has the same length, so a span is a span of the file.
    let source = "<script>function f(){}</script>\n<p>a & b</p>\n<script context=\"module\">export function g(){}</script>\n";
    let e = ex("src/Offsets.svelte", source);
    let g = e
        .symbols
        .iter()
        .find(|s| s.name == "g")
        .expect("g is declared");
    assert_eq!(
        &source[g.span.start_byte..g.span.end_byte],
        "function g(){}",
        "span {:?}",
        g.span
    );
}

#[test]
fn text_is_read_from_the_file_not_from_the_repaired_copy() {
    // A word that holds the character must come out as the file wrote it.
    let source = "<script>function f(){}</script>\n<p>a & b</p>\n";
    let e = ex("src/Text.svelte", source);
    let json = serde_json::to_string(&e).expect("serializes");
    assert!(!json.contains("a x b"), "the copy leaked: {json}");
}

#[test]
fn a_real_error_of_its_own_is_still_partial() {
    // An unclosed block is nobody's `&`: nothing is written over, the failed
    // tree is kept, and the file is reported partial as before.
    let e = ex("src/Unclosed.svelte", "{#if x}\n<p>hi</p>\n");
    assert!(
        matches!(e.parse_outcome, ParseOutcome::Partial { .. }),
        "outcome: {:?}",
        e.parse_outcome
    );
}

#[test]
fn a_real_error_beside_a_bare_ampersand_stays_partial_for_the_real_error() {
    let e = ex("src/Both.svelte", "{#if x}\n<p>a & b</p>\n");
    assert!(
        matches!(e.parse_outcome, ParseOutcome::Partial { .. }),
        "outcome: {:?}",
        e.parse_outcome
    );
}

#[test]
fn code_that_holds_an_ampersand_is_untouched() {
    for source in [
        "<p>{a && b}</p>\n",
        "<p class={a && b}>t</p>\n",
        "<p>x &amp; y &#38; &#x26;</p>\n",
        "<script>if (a && b) { f(); }\nfunction f(){}</script>\n<style>a::after{content:\"&\"}</style>\n",
    ] {
        let e = ex("src/Code.svelte", source);
        assert!(is_clean(&e), "{source:?}: {:?}", e.parse_outcome);
    }
}
