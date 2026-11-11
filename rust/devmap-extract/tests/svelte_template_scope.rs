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

/// Plain `Name` references (a value read, not a call) to `name`: what a lone
/// markup identifier such as `{item}` or `onclick={item}` records.
fn name_refs(e: &Extraction, name: &str) -> usize {
    e.references
        .iter()
        .filter(|r| r.kind == ReferenceKind::Name && r.name == name && r.receiver_expr.is_none())
        // A site that carries a local binding is the template's own name, which
        // the resolver never binds to a namesake.
        .filter(|r| {
            !e.local_bindings
                .iter()
                .any(|b| b.start_byte == r.span.start_byte && b.name == name)
        })
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
        0,
        "{{item}} is a read: {:?}",
        e.calls
    );
    assert_eq!(call_references(&e, "item"), 0, "{:?}", e.references);
    assert_eq!(
        name_refs(&e, "item"),
        1,
        "only the use after the block: {:?}",
        e.references
    );
    assert_eq!(unshadowed(&e, "item").len(), 1, "{:?}", e.local_bindings);
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
            0,
            "a lone identifier is a read: {:?}",
            e.calls
        );
        assert_eq!(
            name_refs(&e, bound),
            1,
            "{bound}: only the use after the block: {:?}",
            e.references
        );
        assert_eq!(unshadowed(&e, bound).len(), 1, "{:?}", e.local_bindings);
    }
    // `b` is a property key, not a binding: nothing else is affected by it.
    assert_eq!(bare_calls(&e, "b"), 0, "{:?}", e.calls);
}

#[test]
fn the_else_branch_of_an_each_does_not_see_the_item() {
    let e = ex(&format!(
        "{HEAD}{{#each rows as item}}{{item}}{{:else}}{{item}}{{/each}}\n"
    ));
    assert_eq!(bare_calls(&e, "item"), 0, "{:?}", e.calls);
    assert_eq!(
        name_refs(&e, "item"),
        1,
        "only the else branch: {:?}",
        e.references
    );
    assert_eq!(unshadowed(&e, "item").len(), 1, "{:?}", e.local_bindings);
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
        0,
        "a lone identifier is a read: {:?}",
        e.calls
    );
    assert_eq!(
        name_refs(&e, "item"),
        1,
        "only the use after the snippet: {:?}",
        e.references
    );
    assert_eq!(unshadowed(&e, "item").len(), 1, "{:?}", e.local_bindings);
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
        0,
        "a lone identifier is a read: {:?}",
        e.calls
    );
    assert_eq!(
        name_refs(&e, "item"),
        1,
        "only the use after the block: {:?}",
        e.references
    );
    assert_eq!(unshadowed(&e, "item").len(), 1, "{:?}", e.local_bindings);
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
            0,
            "a lone identifier is a read: {:?}",
            e.calls
        );
        assert_eq!(
            name_refs(&e, bound),
            1,
            "{bound}: only the use after the block: {:?}",
            e.references
        );
        assert_eq!(unshadowed(&e, bound).len(), 1, "{:?}", e.local_bindings);
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
            0,
            "a lone identifier is a read: {:?}",
            e.calls
        );
        assert_eq!(
            name_refs(&e, bound),
            1,
            "{bound}: only the use after the element: {:?}",
            e.references
        );
        assert_eq!(unshadowed(&e, bound).len(), 1, "{:?}", e.local_bindings);
    }
    // `foo` is the slot prop's name and `z` is destructured away: only `z` is read.
    assert_eq!(bare_calls(&e, "z"), 0, "{:?}", e.calls);
    assert_eq!(name_refs(&e, "z"), 1, "{:?}", e.references);
    assert_eq!(unshadowed(&e, "z").len(), 1, "{:?}", e.local_bindings);
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
fn a_name_the_template_does_not_bind_is_a_reference_not_a_call() {
    let e = ex(&format!(
        "{HEAD}{{#each rows as item}}{{row}}{{/each}}<button onclick={{row}}>x</button>\n"
    ));
    assert_eq!(bare_calls(&e, "row"), 0, "{:?}", e.calls);
    assert_eq!(call_references(&e, "row"), 0, "{:?}", e.references);
    assert_eq!(name_refs(&e, "row"), 2, "{:?}", e.references);
    assert_eq!(unshadowed(&e, "row").len(), 2, "{:?}", e.local_bindings);
}

/// A lone identifier in Svelte markup is a value read, the way a TSX identifier
/// in value position is: `{size}`, `value={size}`, `onclick={save}` and
/// `on:click={save}` each record one `Name` reference and no call.
#[test]
fn a_lone_markup_identifier_is_a_reference_in_every_position() {
    for (position, markup, name) in [
        ("text", "<p>{size}</p>", "size"),
        ("attribute", "<input value={size} />", "size"),
        (
            "handler attribute",
            "<button onclick={save}>x</button>",
            "save",
        ),
        (
            "handler directive",
            "<button on:click={save}>x</button>",
            "save",
        ),
        ("shorthand attribute", "<input {size} />", "size"),
        ("bind directive", "<input bind:value={x} />", "x"),
        ("class directive", "<i class:active={flag}>x</i>", "flag"),
        ("action argument", "<i use:act={arg}>x</i>", "arg"),
        ("each collection", "{#each list as item}x{/each}", "list"),
        ("await expression", "{#await promise}x{/await}", "promise"),
        ("each key", "{#each rows as item (id)}x{/each}", "id"),
    ] {
        let e = ex(&format!("{HEAD}{markup}\n"));
        assert_eq!(bare_calls(&e, name), 0, "{position}: {:?}", e.calls);
        assert_eq!(
            call_references(&e, name),
            0,
            "{position}: {:?}",
            e.references
        );
        assert_eq!(name_refs(&e, name), 1, "{position}: {:?}", e.references);
        assert_eq!(unshadowed(&e, name).len(), 1, "{:?}", e.local_bindings);
    }
}

/// A call stays a call: `{size()}` runs `size` when the markup renders, and so
/// does `onclick={save()}`.
#[test]
fn a_call_in_markup_is_still_a_call() {
    for (markup, name) in [
        ("<p>{size()}</p>", "size"),
        ("<b onclick={save()}>x</b>", "save"),
    ] {
        let e = ex(&format!("{HEAD}{markup}\n"));
        assert_eq!(bare_calls(&e, name), 1, "{markup}: {:?}", e.calls);
    }
}

/// The six lone markup identifiers of the `accuracy/svelte` fixture that name
/// script variables: each is a value read, so each records one `Name` reference
/// and no call. They were Calls classed `local_binding` before the owner ruled a
/// lone markup identifier a Reference.
#[test]
fn the_fixtures_lone_identifiers_that_name_script_variables_are_references() {
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../testdata/accuracy/svelte/src");
    for (file, names) in [
        (
            "Stores.svelte",
            &["pattern", "template", "member", "renamed", "size"][..],
        ),
        ("Board.svelte", &["cards"][..]),
    ] {
        let source = std::fs::read_to_string(root.join(file)).expect("fixture file");
        let e = extract_file_with_budget(&format!("src/{file}"), &source, Duration::from_secs(600));
        for name in names {
            let markup = e
                .references
                .iter()
                .filter(|r| r.kind == ReferenceKind::Name && r.name == *name)
                .count();
            assert_eq!(
                markup, 1,
                "{file}: {name} is a reference: {:?}",
                e.references
            );
            assert_eq!(
                bare_calls(&e, name),
                0,
                "{file}: {name} is not called: {:?}",
                e.calls
            );
            assert_eq!(
                call_references(&e, name),
                0,
                "{file}: {name}: {:?}",
                e.references
            );
        }
    }
}

/// What one extractor records for a word: `"reference"`, `"call"` or `"nothing"`.
fn outcome(e: &Extraction, word: &str) -> &'static str {
    if bare_calls(e, word) > 0 {
        "call"
    } else if e.references.iter().any(|r| r.name == word) {
        "reference"
    } else {
        "nothing"
    }
}

/// A literal keyword names nothing. `disabled={false}`, `{null}`, `value={this}`
/// and `checked={true}` record no call and no reference in Svelte, and `:disabled="false"`
/// and `{{ null }}` record no call in Vue, where `{{ size }}` is still a call.
#[test]
fn a_literal_keyword_in_markup_records_nothing() {
    for (markup, word) in [
        ("<input disabled={false} />", "false"),
        ("<p>{null}</p>", "null"),
        ("<input value={this} />", "this"),
        ("<input checked={true} />", "true"),
        ("<p>{undefined}</p>", "undefined"),
    ] {
        let e = ex(&format!("{HEAD}{markup}\n"));
        assert_eq!(
            outcome(&e, word),
            "nothing",
            "svelte {markup}: {:?} {:?}",
            e.calls,
            e.references
        );
    }
}

/// The same words in Vue: no call, where `{{ size }}` is still a call.
#[test]
fn a_literal_keyword_in_vue_markup_records_no_call() {
    for (markup, word) in [
        (
            "<template><input :disabled=\"false\" /></template>",
            "false",
        ),
        ("<template><p>{{ null }}</p></template>", "null"),
        ("<template><p>{{ this }}</p></template>", "this"),
    ] {
        let e = extract_file_with_budget("src/V.vue", markup, Duration::from_secs(600));
        assert_eq!(
            outcome(&e, word),
            "nothing",
            "vue {markup}: {:?} {:?}",
            e.calls,
            e.references
        );
    }
    let control = extract_file_with_budget(
        "src/V.vue",
        "<template><p>{{ size }}</p></template>",
        Duration::from_secs(600),
    );
    assert_eq!(outcome(&control, "size"), "call", "{:?}", control.calls);
}

/// The TSX extractor and the template extractors agree on every word that is
/// shaped like an identifier: what TSX records for `W` in value position
/// (`run(W)`, `<A v={W}>{W}</A>`), Svelte records for `{W}` (a reference or
/// nothing) and Vue records as a call or nothing. A word one side records and the
/// other does not is the drift this pins.
#[test]
fn the_template_extractors_agree_with_tsx_on_every_keyword_shaped_word() {
    for word in [
        "true",
        "false",
        "null",
        "undefined",
        "this",
        "NaN",
        "Infinity",
        "arguments",
        "globalThis",
        "other",
    ] {
        let tsx = extract_file_with_budget(
            "src/a.tsx",
            &format!(
                "export function f() {{ run({word}); return <A v={{{word}}}>{{{word}}}</A>; }}\n"
            ),
            Duration::from_secs(600),
        );
        let expected = if tsx.references.iter().any(|r| r.name == word) {
            "reference"
        } else {
            "nothing"
        };
        let svelte = ex(&format!("{HEAD}<p>{{{word}}}</p>\n"));
        assert_eq!(outcome(&svelte, word), expected, "svelte {word}");
        let vue = extract_file_with_budget(
            "src/V.vue",
            &format!("<template><p>{{{{ {word} }}}}</p></template>"),
            Duration::from_secs(600),
        );
        let vue_expected = if expected == "reference" {
            "call"
        } else {
            "nothing"
        };
        assert_eq!(outcome(&vue, word), vue_expected, "vue {word}");
    }
}
