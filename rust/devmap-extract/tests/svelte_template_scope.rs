//! Svelte templates bind names, and a bound name is not its namesake.
//!
//! `{#each rows as item}{item}{/each}` reads the block's `item`. The extractor
//! used to record a call to `item` for the header and again for the use, so the
//! resolver bound both to whatever `item` the module declared or imported: a
//! confident edge to the wrong function. The same holds for an index, a
//! destructured name, a snippet parameter, a `{@const}`, an `{#await}` result
//! and a `let:` directive.
//!
//! Every test that names a binding fails against the extractor before the
//! template walk carried a scope.

use std::time::Duration;

use devmap_extract::extract_file_with_budget;
use devmap_extract::model::{Extraction, ParseOutcome, ReferenceKind, SymbolKind};

fn ex(source: &str) -> Extraction {
    extract_file_with_budget("src/Scope.svelte", source, Duration::from_secs(600))
}

fn is_clean(e: &Extraction) -> bool {
    matches!(e.parse_outcome, ParseOutcome::Clean)
}

const HEAD: &str =
    "<script lang=\"ts\">\n  import { item, row, v, e, w } from './format';\n</script>\n";

fn bare_calls(e: &Extraction, name: &str) -> usize {
    e.calls
        .iter()
        .filter(|c| c.callee_name == name && c.receiver_expr.is_none())
        .count()
}

fn call_references(e: &Extraction, name: &str) -> usize {
    e.references
        .iter()
        .filter(|r| r.kind == ReferenceKind::Call && r.name == name)
        .count()
}

/// The leading identifier of an expression: `item.sub[0]` -> `item`.
fn root_of(expr: &str) -> &str {
    let end = expr
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '$'))
        .unwrap_or(expr.len());
    &expr[..end]
}

/// Every call and reference whose name, or whose receiver's root, is `name` and
/// that carries no local binding at its start: the sites the resolver is free
/// to bind to a namesake.
fn unshadowed(e: &Extraction, name: &str) -> Vec<String> {
    let bound = |at: usize| {
        e.local_bindings
            .iter()
            .any(|b| b.start_byte == at && b.name == name)
    };
    let mut open = Vec::new();
    for call in &e.calls {
        let root = call
            .receiver_expr
            .as_deref()
            .map_or(call.callee_name.as_str(), root_of);
        if root == name && !bound(call.span.start_byte) {
            open.push(format!("call {call:?}"));
        }
    }
    for reference in &e.references {
        if !matches!(
            reference.kind,
            ReferenceKind::Name | ReferenceKind::Call | ReferenceKind::Constructor
        ) {
            continue;
        }
        let root = reference
            .receiver_expr
            .as_deref()
            .map_or(reference.name.as_str(), root_of);
        if root == name && !bound(reference.span.start_byte) {
            open.push(format!("reference {reference:?}"));
        }
    }
    open
}

#[test]
fn an_each_item_is_not_a_call_to_its_namesake() {
    let e = ex(&format!(
        "{HEAD}{{#each rows as item}}<b>{{item}}</b>{{/each}}<i>{{item}}</i>\n"
    ));
    assert!(is_clean(&e), "{:?}", e.parse_outcome);
    assert_eq!(
        bare_calls(&e, "item"),
        1,
        "only the use after the block: {:?}",
        e.calls
    );
    assert_eq!(call_references(&e, "item"), 1, "{:?}", e.references);
}

#[test]
fn an_each_pattern_an_index_and_nesting_bind_inside_the_block_only() {
    let e = ex(&format!(
        "{HEAD}{{#each rows as {{ a, b: c }}, i (i)}}{{a}}{{c}}{{i}}\
         {{#each a.list as inner}}{{inner}}{{a}}{{/each}}{{/each}}\
         <p>{{a}}{{c}}{{i}}{{inner}}</p>\n"
    ));
    assert!(is_clean(&e), "{:?}", e.parse_outcome);
    for bound in ["a", "c", "i", "inner"] {
        assert_eq!(
            bare_calls(&e, bound),
            1,
            "{bound}: only the use after the block: {:?}",
            e.calls
        );
    }
    // `b` is a property key, not a binding: nothing else is affected by it.
    assert_eq!(bare_calls(&e, "b"), 0, "{:?}", e.calls);
}

#[test]
fn the_else_branch_of_an_each_does_not_see_the_item() {
    let e = ex(&format!(
        "{HEAD}{{#each rows as item}}{{item}}{{:else}}{{item}}{{/each}}\n"
    ));
    assert_eq!(
        bare_calls(&e, "item"),
        1,
        "only the else branch: {:?}",
        e.calls
    );
}

#[test]
fn a_snippet_parameter_is_bound_in_the_snippet_only() {
    let e = ex(&format!(
        "{HEAD}{{#snippet cell(item, other = row())}}{{item}}{{other}}{{/snippet}}\
         {{@render cell(1)}}{{item}}\n"
    ));
    assert!(is_clean(&e), "{:?}", e.parse_outcome);
    assert_eq!(
        bare_calls(&e, "item"),
        1,
        "only the use after the snippet: {:?}",
        e.calls
    );
    assert_eq!(bare_calls(&e, "other"), 0, "{:?}", e.calls);
    assert_eq!(
        bare_calls(&e, "row"),
        1,
        "a default value is still an expression: {:?}",
        e.calls
    );
    assert_eq!(bare_calls(&e, "cell"), 1, "{:?}", e.calls);
}

#[test]
fn a_const_tag_binds_for_the_rest_of_its_block() {
    let e = ex(&format!(
        "{HEAD}{{#if go}}{{@const item = row(1)}}<b>{{item}}</b>{{/if}}<i>{{item}}</i>\n"
    ));
    assert!(is_clean(&e), "{:?}", e.parse_outcome);
    assert_eq!(
        bare_calls(&e, "item"),
        1,
        "only the use after the block: {:?}",
        e.calls
    );
    assert_eq!(
        bare_calls(&e, "row"),
        1,
        "the initialiser is an expression: {:?}",
        e.calls
    );
}

#[test]
fn an_await_result_is_bound_in_its_branch_only() {
    let e = ex(&format!(
        "{HEAD}{{#await load() then w}}{{w}}{{/await}}\
         {{#await load()}}p{{:then v}}{{v}}{{:catch e}}{{e}}{{/await}}\
         {{w}}{{v}}{{e}}\n"
    ));
    assert!(is_clean(&e), "{:?}", e.parse_outcome);
    for bound in ["w", "v", "e"] {
        assert_eq!(
            bare_calls(&e, bound),
            1,
            "{bound}: only the use after the block: {:?}",
            e.calls
        );
    }
    assert_eq!(
        bare_calls(&e, "load"),
        2,
        "both headers still call load: {:?}",
        e.calls
    );
}

#[test]
fn a_let_directive_binds_for_the_children_of_its_element() {
    let e = ex(&format!(
        "{HEAD}<svelte:fragment let:item let:foo={{row}} let:z={{{{ v, e }}}}>{{item}}{{row}}{{v}}{{e}}{{z}}</svelte:fragment>\
         {{item}}{{row}}{{v}}{{e}}\n"
    ));
    for bound in ["item", "row", "v", "e"] {
        assert_eq!(
            bare_calls(&e, bound),
            1,
            "{bound}: only the use after the element: {:?}",
            e.calls
        );
    }
    // `foo` is the slot prop's name and `z` is destructured away: only `z` is read.
    assert_eq!(bare_calls(&e, "z"), 1, "{:?}", e.calls);
    assert_eq!(bare_calls(&e, "foo"), 0, "{:?}", e.calls);
}

#[test]
fn a_call_or_member_use_of_a_bound_name_carries_a_local_binding() {
    let inside = ex(&format!(
        "{HEAD}{{#each rows as item}}{{item()}}{{item.save()}}{{fmt(item.label)}}{{/each}}\n"
    ));
    assert!(is_clean(&inside), "{:?}", inside.parse_outcome);
    assert_eq!(
        unshadowed(&inside, "item"),
        Vec::<String>::new(),
        "every use of the block's `item` is marked local"
    );
    let outside = ex(&format!(
        "{HEAD}{{#each rows as item}}x{{/each}}{{item()}}{{item.save()}}\n"
    ));
    assert!(
        !unshadowed(&outside, "item").is_empty(),
        "after the block `item` is the module's again: {:?}",
        outside.local_bindings
    );
}

#[test]
fn each_render_of_a_duplicate_snippet_names_the_snippet_of_its_own_block() {
    let e = ex("<script lang=\"ts\">\n  let c = true;\n</script>\n\
         {#if c}{#snippet r()}x{/snippet}{@render r()}{:else}{#snippet r()}y{/snippet}{@render r()}{/if}\n");
    assert!(is_clean(&e), "{:?}", e.parse_outcome);
    let declared: Vec<(&str, &str)> = e
        .symbols
        .iter()
        .filter(|s| s.kind == SymbolKind::Function)
        .map(|s| (s.name.as_str(), s.qualified_name.as_str()))
        .collect();
    assert_eq!(
        declared,
        [
            ("r", "src/Scope.svelte::r"),
            ("r#2", "src/Scope.svelte::r#2")
        ]
    );
    let mut renders: Vec<(usize, &str)> = e
        .calls
        .iter()
        .filter(|c| c.callee_name.starts_with('r'))
        .map(|c| (c.span.start_byte, c.callee_name.as_str()))
        .collect();
    renders.sort_unstable();
    let names: Vec<&str> = renders.iter().map(|(_, name)| *name).collect();
    assert_eq!(
        names,
        ["r", "r#2"],
        "the render in the else branch is the else branch's snippet"
    );
}

#[test]
fn a_render_before_its_snippet_in_one_block_reaches_it() {
    let e = ex("{#if go}{@render r()}{#snippet r()}x{/snippet}{/if}{#if go}{#snippet r()}y{/snippet}{@render r()}{/if}\n");
    let mut renders: Vec<(usize, &str)> = e
        .calls
        .iter()
        .filter(|c| c.callee_name.starts_with('r'))
        .map(|c| (c.span.start_byte, c.callee_name.as_str()))
        .collect();
    renders.sort_unstable();
    let names: Vec<&str> = renders.iter().map(|(_, name)| *name).collect();
    assert_eq!(names, ["r", "r#2"], "{:?}", e.calls);
}

#[test]
fn a_default_inside_a_binding_pattern_is_read_in_the_scope_outside_it() {
    for (construct, source, bound) in [
        ("each", "{#each rows as { a = fromEach() }}{a}{/each}", "a"),
        (
            "const",
            "{#if go}{@const { c = fromConst() } = o}{c}{/if}",
            "c",
        ),
        ("then", "{#await p then { d = fromThen() }}{d}{/await}", "d"),
        (
            "let",
            "<svelte:fragment let:x={{ e = fromLet() }}>{e}</svelte:fragment>",
            "e",
        ),
    ] {
        let e = ex(source);
        let default = format!("from{}", construct[..1].to_uppercase() + &construct[1..]);
        assert_eq!(bare_calls(&e, &default), 1, "{construct}: {:?}", e.calls);
        assert_eq!(bare_calls(&e, bound), 0, "{construct}: {:?}", e.calls);
    }
}

#[test]
fn a_component_tag_named_by_a_bound_name_records_nothing_of_that_name() {
    let e = ex(&format!(
        "{HEAD}{{#each rows as item}}<item.Comp />{{/each}}\n"
    ));
    assert!(is_clean(&e), "{:?}", e.parse_outcome);
    assert_eq!(bare_calls(&e, "item"), 0, "{:?}", e.calls);
    assert!(
        e.calls.iter().all(|c| c.callee_name != "Comp")
            && e.references
                .iter()
                .all(|r| r.name != "Comp" && r.name != "item"),
        "calls {:?} references {:?}",
        e.calls,
        e.references
    );
}

#[test]
fn a_name_the_template_does_not_bind_is_still_a_call() {
    let e = ex(&format!(
        "{HEAD}{{#each rows as item}}{{row}}{{/each}}<button onclick={{row}}>x</button>\n"
    ));
    assert_eq!(bare_calls(&e, "row"), 2, "{:?}", e.calls);
    assert_eq!(unshadowed(&e, "row").len(), 4, "{:?}", e.local_bindings);
}
