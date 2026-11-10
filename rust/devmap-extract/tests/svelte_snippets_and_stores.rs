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

// ------------------------------------------------------------------ round 2

const PROBE: &str = "<script lang=\"ts\">\n\
  const pattern = /^\\$foo|\\$bar$/;\n\
  const obj = { $item: 1, $zap: 2 };\n\
  const s = \"$foo\";\n\
  const t = `cost $foo and ${1}`;\n\
  // $foo in a comment\n\
  const m = obj.$foo;\n\
  const { $foo: x } = obj;\n\
  $bar: for (;;) { break $bar; }\n\
  let a = $state(0);\n\
  function foo() {}\n\
  function bar() {}\n\
  function item() {}\n\
  function zap() {}\n\
  function state() {}\n\
</script>\n\
<p title={\"$foo\"}>{`$zap`}{obj.$foo}{/\\$bar/.test(s)}</p>\n";

#[test]
fn only_an_identifier_in_expression_position_is_a_store_read() {
    let e = ex(PROBE);
    let names = name_references(&e);
    for local in ["foo", "bar", "item", "zap", "state"] {
        assert!(!names.contains(&local), "{local} read as a store: {:?}", e.references);
    }
}

/// An IMPORTED store: a real target for a read. `PROBE` above declares each of
/// its names as a function of the file, and `store_can_be_a_target` drops a
/// function before the syntax tree is consulted, so it would pass for a text
/// scan too. Here only the tree stands between a `$foo` spelling and a read.
const IMPORTED_DECOYS: &str = "<script lang=\"ts\">\n\
  import { foo } from './stores';\n\
  const pattern = /^\\$foo|\\$foo$/;\n\
  const text = \"$foo\";\n\
  const template = `cost $foo and ${text}`;\n\
  const obj = { $foo: 1 };\n\
  $foo: for (;;) { break $foo; }\n\
  // $foo in a comment\n\
</script>\n\
<p title={\"$foo\"}>{`$foo`}{/\\$foo/ instanceof RegExp}{`${text}`}</p>\n";

fn reads_of(e: &Extraction, store: &str) -> usize {
    name_references(e).iter().filter(|n| **n == store).count()
}

#[test]
fn a_dollar_spelling_that_is_not_in_expression_position_never_reads_an_imported_store() {
    let e = ex(IMPORTED_DECOYS);
    assert_eq!(
        reads_of(&e, "foo"),
        0,
        "a regex, a string, a template-literal text part, an object key, a label and a \
         comment are not reads: {:?}",
        e.references
    );
}

#[test]
fn one_true_read_among_the_decoys_is_the_only_read_of_an_imported_store() {
    let script = IMPORTED_DECOYS.replace("</script>", "  const read = $foo + 1;\n</script>");
    let e = ex(&script);
    assert_eq!(reads_of(&e, "foo"), 1, "{:?}", e.references);
    let markup = IMPORTED_DECOYS.replace("</p>", "{$foo}</p>");
    let e = ex(&markup);
    assert_eq!(reads_of(&e, "foo"), 1, "{:?}", e.references);
}

#[test]
fn a_store_read_never_binds_a_function_of_the_component() {
    let e = ex("<script lang=\"ts\">\n  import { markers } from './stores';\n  function helper() {}\n  let n = $markers;\n</script>\n{$helper}{$helper.x}{$markers}\n");
    let names = name_references(&e);
    assert!(!names.contains(&"helper"), "{:?}", e.references);
    assert!(names.contains(&"markers"), "{:?}", e.references);
}

#[test]
fn a_shorthand_property_and_an_assignment_are_store_reads() {
    let e = ex("<script lang=\"ts\">\n  import { a, b } from './stores';\n  const o = { $a };\n  $b = 1;\n</script>\n");
    let names = name_references(&e);
    assert!(names.contains(&"a") && names.contains(&"b"), "{:?}", e.references);
}

#[test]
fn a_snippet_passed_as_a_prop_is_a_reference_not_a_call() {
    let e = ex(&format!(
        "{SCRIPT}<Comp {{row}} cell={{row}} />\n{{#snippet row()}}x{{/snippet}}\n{{@render row()}}\n"
    ));
    let called = e.calls.iter().filter(|c| c.callee_name == "row").count();
    assert_eq!(called, 1, "only the render is a call: {:?}", e.calls);
    let referred = name_references(&e).iter().filter(|n| **n == "row").count();
    assert_eq!(referred, 2, "{:?}", e.references);
}

#[test]
fn a_second_snippet_of_one_name_is_declared_beside_the_first() {
    let source = format!("{SCRIPT}{{#snippet a()}}x{{/snippet}}\n<div>{{#snippet a()}}y{{/snippet}}</div>\n");
    let e = ex(&source);
    assert!(is_clean(&e), "{:?}", e.parse_outcome);
    let mut qualified: Vec<&str> = e
        .symbols
        .iter()
        .filter(|s| s.name == "a")
        .map(|s| s.qualified_name.as_str())
        .collect();
    qualified.sort_unstable();
    assert_eq!(qualified, ["src/Board.svelte::a", "src/Board.svelte::a#2"]);
}

#[test]
fn a_directive_names_the_function_it_runs() {
    let e = ex("<script lang=\"ts\">\n  import { focus } from './actions';\n  import { fade } from 'svelte/transition';\n  let x = 1;\n</script>\n\
<div use:focus={1} transition:fade|local in:fly out:fade animate:flip class:active bind:value={x} on:click={() => 1} use:ns.pick></div>\n");
    let names = name_references(&e);
    for used in ["focus", "fade", "fly", "flip", "ns"] {
        assert!(names.contains(&used), "{used}: {:?}", e.references);
    }
    for not_a_function in ["active", "value", "click"] {
        assert!(!names.contains(&not_a_function), "{not_a_function}: {:?}", e.references);
    }
}

fn vue_with(count: usize) -> String {
    let mut source = String::from("<script setup lang=\"ts\">\nfunction f(n: number) { return n; }\n</script>\n<template>\n");
    for index in 0..count {
        source.push_str(&format!("  <li>{{{{ f({index}) }}}}</li>\n"));
    }
    source.push_str("</template>\n");
    source
}

#[test]
fn vue_reads_the_same_below_the_old_bound_and_all_of_it_above() {
    let small = extract_file_with_budget("src/A.vue", &vue_with(50), Duration::from_secs(600));
    assert!(is_clean(&small), "{:?}", small.parse_outcome);
    assert_eq!(small.calls.iter().filter(|c| c.callee_name == "f").count(), 50);
    // Between the old bound (256) and the new one: now read whole.
    let large = extract_file_with_budget("src/A.vue", &vue_with(300), Duration::from_secs(600));
    assert!(is_clean(&large), "{:?}", large.parse_outcome);
    assert_eq!(large.calls.iter().filter(|c| c.callee_name == "f").count(), 300);
}

/// Astro's grammar hands its `{...}` expressions back as text, which this walk
/// does not read at all, so the bound on template expressions cannot change
/// what an Astro file yields: below it and above it the answer is the frontmatter.
#[test]
fn astro_is_untouched_by_the_template_expression_bound() {
    let build = |count: usize| {
        let mut source = String::from("---\nfunction f(n: number) { return n; }\n---\n<ul>\n");
        for index in 0..count {
            source.push_str(&format!("  <li>{{f({index})}}</li>\n"));
        }
        source.push_str("</ul>\n");
        extract_file_with_budget("src/A.astro", &source, Duration::from_secs(600))
    };
    let (small, large) = (build(50), build(300));
    assert!(is_clean(&small) && is_clean(&large));
    assert_eq!(small.calls.len(), large.calls.len(), "template expressions are not walked");
    assert!(small.symbols.iter().any(|s| s.name == "f"));
}
