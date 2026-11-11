//! Svelte snippets are lexically scoped, and a render reaches the snippet of its
//! own block.
//!
//! `{#if c}{#snippet r()}…{/snippet}{@render r()}{:else}{#snippet r()}…{/snippet}
//! {@render r()}{/if}` declares two snippets of one name. The graph names them
//! `r` and `r#2`; each `{@render r()}` has to reach the one in its own branch,
//! or the other is reported dead.

use devmap_analyze::analyze;
use devmap_extract::extract_file;
use devmap_extract::model::EdgeKind;
use devmap_resolve::Resolver;
use std::collections::BTreeSet;

const FILE: &str = "src/Dup.svelte";

fn reached_and_dead(source: &str) -> (BTreeSet<String>, BTreeSet<String>, Vec<String>) {
    let extractions = vec![extract_file(FILE, source)];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    let reached = resolution
        .edges
        .iter()
        .filter(|edge| {
            edge.source_file == FILE
                && !matches!(
                    edge.edge_kind,
                    EdgeKind::Contains | EdgeKind::MemberOf | EdgeKind::Imports | EdgeKind::Defines
                )
        })
        .map(|edge| edge.target_symbol.clone())
        .collect();
    let unresolved = resolution
        .unresolved
        .iter()
        .map(|row| {
            format!(
                "{} -> {} ({})",
                row.source_symbol,
                row.callee_name,
                row.class.label()
            )
        })
        .collect();
    let analysis = analyze(&extractions, &resolution);
    let dead = analysis
        .dead_symbols
        .iter()
        .filter(|report| !report.is_exempt && report.file_path == FILE)
        .map(|report| report.symbol_name.clone())
        .collect();
    (reached, dead, unresolved)
}

#[test]
fn each_render_of_a_duplicate_snippet_reaches_the_snippet_in_its_own_branch() {
    let (reached, dead, unresolved) = reached_and_dead(
        "<script lang=\"ts\">\n  let c = true;\n</script>\n\
         {#if c}{#snippet r()}x{/snippet}{@render r()}{:else}{#snippet r()}y{/snippet}{@render r()}{/if}\n",
    );
    assert!(
        reached.contains("src/Dup.svelte::r") && reached.contains("src/Dup.svelte::r#2"),
        "both snippets are rendered: reached {reached:?}, unresolved {unresolved:?}"
    );
    assert!(
        !dead.iter().any(|name| name.starts_with('r')),
        "neither snippet is dead: {dead:?}"
    );
}

/// Every edge from `FILE`, as `(kind, target symbol)`, for a source.
fn edges_of(source: &str) -> Vec<(EdgeKind, String)> {
    let extractions = vec![extract_file(FILE, source)];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    resolution
        .edges
        .iter()
        .filter(|edge| {
            edge.source_file == FILE
                && !matches!(
                    edge.edge_kind,
                    EdgeKind::Contains | EdgeKind::MemberOf | EdgeKind::Imports | EdgeKind::Defines
                )
        })
        .map(|edge| (edge.edge_kind, edge.target_symbol.clone()))
        .collect()
}

/// `onclick={save}` and `on:click={save}` hand `save` over; nothing calls it. The
/// use is a `References` edge to the script's `save` (a Calls edge would be a
/// claim that the template runs it), and the function is not dead.
#[test]
fn a_function_used_only_as_a_handler_is_referenced_not_called_and_is_live() {
    let source = "<script lang=\"ts\">\n  function save() {}\n  function drop() {}\n</script>\n\
                  <button onclick={save}>a</button><button on:click={drop}>b</button>\n";
    let edges = edges_of(source);
    for target in ["src/Dup.svelte::save", "src/Dup.svelte::drop"] {
        assert!(
            edges
                .iter()
                .any(|(kind, to)| *kind == EdgeKind::References && to == target),
            "{target} is referenced: {edges:?}"
        );
        assert!(
            !edges
                .iter()
                .any(|(kind, to)| *kind == EdgeKind::Calls && to == target),
            "{target} is not called: {edges:?}"
        );
    }
    let (_, dead, _) = reached_and_dead(source);
    assert!(
        !dead.iter().any(|name| name == "save" || name == "drop"),
        "a handler is live: {dead:?}"
    );
}

/// A call stays a call: `{save()}` is a Calls edge.
#[test]
fn a_called_function_in_markup_is_still_a_calls_edge() {
    let edges =
        edges_of("<script lang=\"ts\">\n  function save() {}\n</script>\n<p>{save()}</p>\n");
    assert!(
        edges
            .iter()
            .any(|(kind, to)| *kind == EdgeKind::Calls && to == "src/Dup.svelte::save"),
        "{edges:?}"
    );
}
