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
