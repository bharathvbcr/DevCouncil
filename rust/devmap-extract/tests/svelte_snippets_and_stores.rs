//! Svelte templates: the top of a long template, `{#snippet}`, and `$store`.
//!
//! GitPulse's `TaskBoard.svelte` (about 600 template expressions) and
//! `RepoTabBar.svelte` were reported as grammar failures at their top-level
//! `{#snippet}` blocks. The grammar parses both. Two things in the extractor
//! lost the text:
//!
//! * the template expressions were capped at 256 and walked last-first, so the
//!   cap discarded the *top* of the template, where the snippets are;
//! * a snippet's parameter list was parsed as an expression, so a typed
//!   parameter (`status: TaskStatus`, `info?: Item`) was an error range and an
//!   untyped one (`card`) was a phantom call.
//!
//! Every test here fails against the code before that change.

use std::time::Duration;

use devmap_extract::extract_file_with_budget;
use devmap_extract::model::{Extraction, ParseOutcome, ReferenceKind, SymbolKind, WiringKind};

fn ex(source: &str) -> Extraction {
    extract_file_with_budget("src/Board.svelte", source, Duration::from_secs(600))
}

fn is_clean(e: &Extraction) -> bool {
    matches!(e.parse_outcome, ParseOutcome::Clean)
}

fn callees(e: &Extraction) -> Vec<&str> {
    e.calls.iter().map(|c| c.callee_name.as_str()).collect()
}

fn name_references(e: &Extraction) -> Vec<&str> {
    e.references
        .iter()
        .filter(|r| r.kind == ReferenceKind::Name)
        .map(|r| r.name.as_str())
        .collect()
}

const SCRIPT: &str = "<script lang=\"ts\">\n  import { label } from './format';\n</script>\n";

#[test]
fn a_snippet_ahead_of_three_hundred_expressions_is_still_read() {
    let mut source = format!("{SCRIPT}{{#snippet title(s: string)}}\n  <h2>{{label(s)}}</h2>\n{{/snippet}}\n");
    for index in 0..300 {
        source.push_str(&format!("<li>{{label({index})}}</li>\n"));
    }
    let e = ex(&source);
    assert!(is_clean(&e), "outcome: {:?}", e.parse_outcome);
    let first = e.calls.iter().find(|c| c.callee_name == "label").expect("a label call");
    assert!(
        source[first.span.start_byte..].starts_with("label(s)"),
        "the call inside the snippet is the first one read: {first:?}"
    );
}

#[test]
fn a_snippet_is_a_function_symbol_and_render_calls_it() {
    let e = ex(&format!(
        "{SCRIPT}{{#snippet row(card)}}\n  <li>{{label(card)}}</li>\n{{/snippet}}\n{{@render row(1)}}\n"
    ));
    let row = e
        .symbols
        .iter()
        .find(|s| s.name == "row")
        .expect("the snippet is a symbol");
    assert_eq!(row.kind, SymbolKind::Function);
    assert_eq!(row.qualified_name, "src/Board.svelte::row");
    assert!(callees(&e).contains(&"row"), "{:?}", callees(&e));
}

#[test]
fn typed_optional_and_multiline_snippet_parameters_parse_clean_and_call_nothing() {
    let e = ex(&format!(
        "{SCRIPT}{{#snippet a(status: Status)}}x{{/snippet}}\n\
         {{#snippet b(info?: Item)}}x{{/snippet}}\n\
         {{#snippet c(\n  label: string,\n  current: Tab | null,\n  onPick: (c: Tab) => void\n)}}x{{/snippet}}\n\
         {{#snippet d(card)}}x{{/snippet}}\n"
    ));
    assert!(is_clean(&e), "outcome: {:?}", e.parse_outcome);
    for phantom in ["status", "info", "label", "current", "onPick", "card"] {
        assert!(
            !callees(&e).contains(&phantom),
            "a parameter is not a call: {:?}",
            callees(&e)
        );
    }
}

#[test]
fn a_default_value_in_a_snippet_parameter_is_still_a_call() {
    let e = ex(&format!("{SCRIPT}{{#snippet a(s = label(1))}}x{{/snippet}}\n"));
    assert!(is_clean(&e), "outcome: {:?}", e.parse_outcome);
    assert!(callees(&e).contains(&"label"), "{:?}", callees(&e));
}

#[test]
fn a_snippet_given_to_a_component_is_reached_by_the_runtime() {
    let e = ex(&format!(
        "{SCRIPT}<Dialog>\n  {{#snippet header()}}x{{/snippet}}\n</Dialog>\n\
         <svelte:boundary>{{#snippet failed()}}x{{/snippet}}</svelte:boundary>\n\
         <div>{{#snippet plain()}}x{{/snippet}}</div>\n{{#snippet top()}}x{{/snippet}}\n"
    ));
    let entry = |name: &str| {
        e.wiring.iter().any(|w| {
            w.kind == WiringKind::RuntimeEntryPoint
                && w.target_symbol == format!("src/Board.svelte::{name}")
        })
    };
    assert!(entry("header") && entry("failed"), "{:?}", e.wiring);
    assert!(!entry("plain") && !entry("top"), "{:?}", e.wiring);
}

#[test]
fn a_store_read_is_a_reference_to_the_store_not_a_call() {
    let e = ex("<script lang=\"ts\">\n  import { markers } from './stores';\n  let a = $state(1);\n  const n = $markers.length;\n</script>\n\
                <p title={$markers.join(',')}>{$markers.length}</p>\n{$markers}\n");
    let reads = name_references(&e).iter().filter(|n| **n == "markers").count();
    assert_eq!(reads, 4, "script, attribute, text and lone read: {:?}", e.references);
    assert!(
        !callees(&e).iter().any(|c| c.starts_with('$') && *c != "$state"),
        "{:?}",
        callees(&e)
    );
}

#[test]
fn runes_strings_and_properties_are_not_store_reads() {
    let e = ex("<script lang=\"ts\">\n  let a = $state(1);\n  let b = $derived(a + 1);\n  const s = '$notAStore';\n  const t = `cost ${$real}`;\n  const o = x.$prop;\n  // $commented\n</script>\n");
    let names = name_references(&e);
    assert!(names.contains(&"real"), "{names:?}");
    for not_a_store in ["state", "derived", "notAStore", "commented", "prop"] {
        assert!(!names.contains(&not_a_store), "{not_a_store}: {names:?}");
    }
}
