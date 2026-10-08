//! Output shared by the read commands: the store opener they all use, JSON
//! emission, and the renderers more than one command prints through.

use devmap_query::ResolutionAvailability;
use devmap_store::Store;

use crate::cli::Cli;

pub(crate) fn open_for_read(cli: &Cli) -> anyhow::Result<Store> {
    let db = cli.db();
    if !db.is_file() {
        anyhow::bail!(
            "no devmap store at {} — run `devmap build` first",
            db.display()
        );
    }
    // Navigation must never perform a compatibility upgrade under live clients.
    let store = Store::open_read_only(&db)?;
    if cli.db.is_none() {
        store.validate_repo_root(&cli.root_hint())?;
    }
    Ok(store)
}

pub(crate) fn ensure_parent(path: &std::path::Path) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

pub(crate) fn emit_json(cli: &Cli, payload: &serde_json::Value) -> anyhow::Result<()> {
    if cli.json {
        outln!("{}", serde_json::to_string(payload)?);
    } else {
        outln!("{}", serde_json::to_string_pretty(payload)?);
    }
    Ok(())
}

pub(crate) fn emit_unavailable(reason: &str) {
    outln!("unavailable: {reason}");
}

/// The line a truncated result must carry, if any.
///
/// Split from the printing so the *policy* can be asserted directly. Mutation
/// testing replaced the whole emitter with `()` and flipped every comparison
/// in its condition without a failure, because nothing observed stdout — and
/// this line is the only thing telling a caller their result was capped.
/// PHASE1_CONTRACT.md:17 is explicit: never present a capped sample as
/// complete coverage. A silent emitter does exactly that.
fn truncation_line(shown: u32, hidden: u32, total: u32, truncated: bool) -> Option<String> {
    (truncated || hidden > 0).then(|| format!("shown {shown} of {total} ({hidden} hidden)"))
}

pub(crate) fn emit_truncation(shown: u32, hidden: u32, total: u32, truncated: bool) {
    if let Some(line) = truncation_line(shown, hidden, total, truncated) {
        outln!("{line}");
    }
}

pub(crate) fn emit_search(resp: &devmap_query::Response<devmap_query::SymbolHit>) {
    if let ResolutionAvailability::Unavailable { reason } = &resp.resolution {
        emit_unavailable(reason);
        return;
    }
    emit_scope(resp.scope.as_ref());
    for hit in &resp.items {
        outln!(
            "{}:{}-{}  {}  {}",
            hit.file_path,
            hit.span.0,
            hit.span.1,
            hit.kind,
            hit.symbol_name
        );
    }
    emit_truncation(resp.shown, resp.hidden, resp.total, resp.truncated);
}

/// One line naming what a scoped answer was ranked over, so a short list in
/// the human output is attributable to the scope as it is in `--json`.
pub(crate) fn emit_scope(scope: Option<&devmap_query::ScopeReport>) {
    let Some(scope) = scope else {
        return;
    };
    let mut named: Vec<String> = scope.paths.clone();
    named.extend(
        scope
            .languages
            .iter()
            .map(|language| format!("[{language}]")),
    );
    named.extend(scope.kinds.iter().map(|kind| format!("kind:{kind}")));
    outln!(
        "scope: {} ({} of {} files, {} of {} symbols)",
        named.join(" "),
        scope.files,
        scope.corpus_files,
        scope.symbols,
        scope.corpus_symbols
    );
    if scope.related_tests_outside_scope > 0 {
        outln!(
            "scope: {} related test file(s) outside the scope left out",
            scope.related_tests_outside_scope
        );
    }
}

pub(crate) fn emit_edges(resp: &devmap_query::Response<devmap_resolve::ResolvedEdge>) {
    if let ResolutionAvailability::Unavailable { reason } = &resp.resolution {
        emit_unavailable(reason);
        return;
    }
    for edge in &resp.items {
        outln!(
            "{}::{}  --{:?}-->  {}::{}",
            edge.source_file,
            edge.source_symbol,
            edge.edge_kind,
            edge.target_file,
            edge.target_symbol
        );
    }
    emit_truncation(resp.shown, resp.hidden, resp.total, resp.truncated);
    // What a `--min-rung` floor cost, printed only when it cost something.
    //
    // Without it a narrowed answer is indistinguishable at the terminal from a
    // sparse graph, and the second reading is the one that gets a live symbol
    // deleted. Silent when nothing was filtered, so an unfiltered query reads
    // exactly as it did before the flag existed.
    if let Some(rungs) = &resp.rungs {
        if rungs.filtered_out > 0 {
            outln!(
                "note: --min-rung hid {} of {} edges (deterministic {}, high {}, speculative {})",
                rungs.filtered_out,
                rungs.total(),
                rungs.deterministic,
                rungs.high,
                rungs.speculative
            );
        }
    }
    // Distinct from the truncation line, which describes the token budget. This
    // one says the walk that produced `items` stopped before the graph ran out,
    // so `total` is the size of a partial answer.
    if let Some(reason) = &resp.walk_incomplete {
        outln!("warning: {reason}");
    }
}

pub(crate) fn emit_blast_radius(radius: &devmap_query::BlastRadius) {
    if !radius.unmatched_targets.is_empty() {
        outln!(
            "warning: no indexed traversal start for: {}",
            radius.unmatched_targets.join(", ")
        );
    }
    if let ResolutionAvailability::Unavailable { reason } = &radius.layers.resolution {
        emit_unavailable(reason);
        return;
    }
    outln!("blast radius: {} impacted", radius.total_impacted);
    for layer in &radius.layers.items {
        let confidence = layer
            .lowest_confidence
            .map(|value| format!("{value:.2}"))
            .unwrap_or_else(|| "-".to_string());
        outln!(
            "  depth {}: {} nodes (lowest confidence {confidence}){}",
            layer.depth,
            layer.node_count,
            if layer.nodes_omitted > 0 {
                format!(", {} not listed", layer.nodes_omitted)
            } else {
                String::new()
            }
        );
        for node in &layer.nodes {
            outln!("    {node}");
        }
    }
    emit_truncation(
        radius.layers.shown,
        radius.layers.hidden,
        radius.layers.total,
        radius.layers.truncated,
    );
    if let Some(reason) = &radius.layers.walk_incomplete {
        outln!("warning: {reason}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A capped result always says so; a complete one stays quiet.
    ///
    /// Both halves of the condition matter. `truncated` alone covers a budget
    /// stop that happened to hide nothing; `hidden > 0` covers a result the
    /// budget did not flag but which dropped rows anyway. Collapsing them to
    /// `&&` silences the first, and any comparison that never fires silences
    /// both — leaving a partial answer indistinguishable from a full one.
    #[test]
    fn a_capped_result_is_always_reported_as_capped() {
        // Complete: nothing hidden, not flagged.
        assert_eq!(truncation_line(10, 0, 10, false), None);

        // Flagged by the budget even though nothing is hidden.
        assert!(
            truncation_line(10, 0, 10, true).is_some(),
            "a budget-truncated result must say so even with nothing hidden"
        );

        // Rows dropped without the flag.
        let line = truncation_line(3, 7, 10, false)
            .expect("hidden rows must be reported even when not flagged");
        assert!(
            line.contains('3') && line.contains("10") && line.contains('7'),
            "{line}"
        );

        // Both.
        assert!(truncation_line(3, 7, 10, true).is_some());
    }

    /// `ensure_parent` actually creates the directory it promises.
    ///
    /// It was replaceable with `Ok(())`, which defers the failure to whatever
    /// tries to write the file — reporting a path error instead of a missing
    /// directory.
    #[test]
    fn ensure_parent_creates_the_directory() {
        let dir = std::env::temp_dir().join(format!(
            "devmap-ensure-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let target = dir.join("nested/deeper/out.json");
        assert!(!dir.exists());
        ensure_parent(&target).unwrap();
        assert!(
            target.parent().unwrap().is_dir(),
            "ensure_parent must create the full parent chain"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
