//! Output shared by the read commands: the store opener they all use, JSON
//! emission, and the renderers more than one command prints through.

use devmap_query::ResolutionAvailability;
use devmap_store::Store;

use crate::cli::Cli;

/// What a read command opened: the worktree's own store, or a stacked lane
/// (an immutable base plus a delta) that has no store of its own.
pub(crate) enum ReadStore {
    Plain(Box<Store>),
    Stacked(Box<devmap_query::StackedStore>),
}

impl ReadStore {
    /// The query engine over what was opened. A lane answers from its base
    /// merged with its delta.
    pub(crate) fn engine(&self) -> devmap_query::StoreQueryEngine<'_> {
        match self {
            Self::Plain(store) => devmap_query::StoreQueryEngine::new(store),
            Self::Stacked(stacked) => devmap_query::StoreQueryEngine::new(stacked.base_store())
                .with_stacked(Some(stacked)),
        }
    }

    /// Print a JSON answer. A lane's answer also carries what it may be missing
    /// (see [`devmap_query::StackedStore::attach_incompleteness`]), the same
    /// statement the MCP server makes, so the two transports cannot disagree.
    pub(crate) fn emit_answer(
        &self,
        cli: &Cli,
        mut answer: serde_json::Value,
    ) -> anyhow::Result<()> {
        if let Self::Stacked(stacked) = self {
            stacked.attach_incompleteness(&mut answer)?;
        }
        let opened = match self {
            Self::Plain(store) => store,
            Self::Stacked(stacked) => stacked.base_store(),
        };
        emit_store_answer(cli, opened, answer)
    }

    /// After a human-readable answer: the lane's incompleteness warning, if any.
    pub(crate) fn warn_incomplete(&self) -> anyhow::Result<()> {
        if let Self::Stacked(stacked) = self {
            if let Some(reason) = stacked.incompleteness_reason()? {
                outln!("warning: {reason}");
            }
        }
        Ok(())
    }

    /// The store, for a command that reads the whole of it. A lane has none to
    /// give, and answering such a command from its base alone would be silent.
    pub(crate) fn require_plain(self) -> anyhow::Result<Store> {
        match self {
            Self::Plain(store) => Ok(*store),
            Self::Stacked(stacked) => anyhow::bail!(
                "worktree at {} uses a stacked delta store (no standalone devmap.sqlite); \
                 this command reads the whole store, so run `devmap build` first",
                stacked.worktree_root().display()
            ),
        }
    }
}

/// The one opener for read commands. It never writes, never syncs a delta and
/// never upgrades: a worktree with a store gets that store; one with only a
/// stack lease gets the lane; a retired `base_store_ref.json` pointer is refused
/// by name (`StackedStore::open_if_leased` runs `refuse_legacy_base_store_ref`
/// for a lane with neither a store nor a lease).
pub(crate) fn open_for_read(cli: &Cli) -> anyhow::Result<ReadStore> {
    let db = cli.db();
    if !db.is_file() {
        if cli.db.is_none() {
            if let Some(stacked) = devmap_query::StackedStore::open_if_leased(&cli.root_hint())? {
                return Ok(ReadStore::Stacked(Box::new(stacked)));
            }
        }
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
    Ok(ReadStore::Plain(Box::new(store)))
}

pub(crate) fn ensure_parent(path: &std::path::Path) -> anyhow::Result<()> {
    // Refuses a linked ancestor before anything is created: `create_dir_all`
    // follows links, so a build the store was about to refuse still left an
    // empty state directory inside the link's target.
    devmap_extract::safe_fs::preflight_write(path)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

/// Print a JSON answer read from `store`, naming the repository it came from
/// as the MCP envelope does, through the same builder. Additive: an answer that
/// already carries a `repository` key keeps it, and only an object can carry
/// one. The store is the path this command opened, never a second resolution.
/// Every read command that prints a store-backed answer goes through here,
/// including the ones that need the whole store (`require_plain`).
pub(crate) fn emit_store_answer(
    cli: &Cli,
    store: &Store,
    mut answer: serde_json::Value,
) -> anyhow::Result<()> {
    if let (Some(object), Some(path)) = (answer.as_object_mut(), store.path()) {
        object.entry("repository").or_insert_with(|| {
            devmap_serve::repo_scope::RepositoryRef::for_cli_read(
                path,
                &cli.root_hint(),
                cli.root.is_some(),
                cli.db.is_some(),
            )
            .to_json()
        });
    }
    emit_json(cli, &answer)
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
///
/// `widen` is the flag text the *caller* knows widens its own answer: most
/// commands are sized by `--budget` alone, `explore` also by `--limit`. A
/// caller with nothing that widens the answer passes `None` and the line says
/// nothing about it, rather than naming a flag that does not help.
fn truncation_line(
    shown: u32,
    hidden: u32,
    total: u32,
    truncated: bool,
    widen: Option<&str>,
) -> Option<String> {
    (truncated || hidden > 0).then(|| {
        let mut line = format!("shown {shown} of {total} ({hidden} hidden)");
        if let Some(flag) = widen {
            line.push_str(&format!("; raise {flag} to show more"));
        }
        line
    })
}

/// The flag text for a command whose answer is sized by `--budget` alone.
pub(crate) const WIDEN_BUDGET: &str = "--budget";

/// `explore` sizes its definitions list by `--limit` (how many to consider)
/// and `--budget` (how many of those fit), so either can be the one that binds.
pub(crate) const WIDEN_LIMIT_OR_BUDGET: &str = "--limit or --budget";

pub(crate) fn emit_truncation(
    shown: u32,
    hidden: u32,
    total: u32,
    truncated: bool,
    widen: Option<&str>,
) {
    if let Some(line) = truncation_line(shown, hidden, total, truncated, widen) {
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
    emit_truncation(
        resp.shown,
        resp.hidden,
        resp.total,
        resp.truncated,
        Some(WIDEN_BUDGET),
    );
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

/// One edge as text mode prints it.
fn edge_line(edge: &devmap_resolve::ResolvedEdge) -> String {
    format!(
        "{}  --{:?}-->  {}",
        symbol_id(&edge.source_file, &edge.source_symbol),
        edge.edge_kind,
        symbol_id(&edge.target_file, &edge.target_symbol)
    )
}

/// `<file>::<name>` once. The extractors store a symbol already qualified by
/// its file, so prefixing the file again printed every path twice; a symbol
/// that is a bare name (no file qualifier) still gets its file.
fn symbol_id(file: &str, symbol: &str) -> String {
    match symbol.strip_prefix(file) {
        Some(rest) if rest.starts_with("::") => symbol.to_string(),
        _ => format!("{file}::{symbol}"),
    }
}

/// Characters of a `walk_incomplete` reason kept in the one-line text form.
const WALK_INCOMPLETE_TEXT_CHARS: usize = 160;

/// The text-mode form of a `walk_incomplete` reason: its first clause on one
/// line, with the flags that carry the whole of it. The reason is a paragraph
/// of several coverage clauses (up to a thousand characters) and every
/// command repeated it; `--json` and `--verbose` keep it whole.
fn walk_incomplete_line(reason: &str, verbose: bool) -> String {
    if verbose {
        return format!("warning: {reason}");
    }
    let flat = reason.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut clauses = flat.split(';').map(str::trim).filter(|c| !c.is_empty());
    let first = clauses.next().unwrap_or("");
    let dropped = clauses.count();
    let clipped = if first.chars().count() > WALK_INCOMPLETE_TEXT_CHARS {
        let head: String = first.chars().take(WALK_INCOMPLETE_TEXT_CHARS).collect();
        format!("{}…", head.trim_end())
    } else {
        first.to_string()
    };
    let more = if dropped > 0 {
        format!("+{dropped} more caveats; ")
    } else {
        String::new()
    };
    format!("warning: {clipped} ({more}full detail: --verbose or --json)")
}

/// Print the text-mode `walk_incomplete` line; `verbose` is the CLI's
/// `--verbose`, which keeps the whole reason.
pub(crate) fn emit_walk_incomplete(reason: &str, verbose: bool) {
    outln!("{}", walk_incomplete_line(reason, verbose));
}

pub(crate) fn emit_edges(
    resp: &devmap_query::Response<devmap_resolve::ResolvedEdge>,
    verbose: bool,
) {
    if let ResolutionAvailability::Unavailable { reason } = &resp.resolution {
        emit_unavailable(reason);
        return;
    }
    for edge in &resp.items {
        outln!("{}", edge_line(edge));
    }
    emit_truncation(
        resp.shown,
        resp.hidden,
        resp.total,
        resp.truncated,
        Some(WIDEN_BUDGET),
    );
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
        emit_walk_incomplete(reason, verbose);
    }
    // Information about a complete answer, not a defect in it: no `warning:`.
    if let Some(note) = &resp.note {
        outln!("note: {note}");
    }
}

pub(crate) fn emit_blast_radius(radius: &devmap_query::BlastRadius, verbose: bool) {
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
        Some(WIDEN_BUDGET),
    );
    if let Some(reason) = &radius.layers.walk_incomplete {
        emit_walk_incomplete(reason, verbose);
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
        assert_eq!(truncation_line(10, 0, 10, false, None), None);

        // Flagged by the budget even though nothing is hidden.
        assert!(
            truncation_line(10, 0, 10, true, None).is_some(),
            "a budget-truncated result must say so even with nothing hidden"
        );

        // Rows dropped without the flag.
        let line = truncation_line(3, 7, 10, false, None)
            .expect("hidden rows must be reported even when not flagged");
        assert!(
            line.contains('3') && line.contains("10") && line.contains('7'),
            "{line}"
        );

        // Both.
        assert!(truncation_line(3, 7, 10, true, None).is_some());
    }

    fn edge(
        source_file: &str,
        source_symbol: &str,
        target_file: &str,
        target_symbol: &str,
    ) -> devmap_resolve::ResolvedEdge {
        devmap_resolve::ResolvedEdge {
            source_file: source_file.into(),
            target_file: target_file.into(),
            source_symbol: source_symbol.into(),
            target_symbol: target_symbol.into(),
            edge_kind: devmap_extract::EdgeKind::Calls,
            confidence: devmap_extract::Confidence(1.0),
            resolution: None,
            evidence: None,
            details: None,
        }
    }

    /// A stored symbol id is already `<file>::<name>`; the edge line must not
    /// prepend the file a second time, on either end.
    #[test]
    fn an_edge_line_prints_a_file_path_once_per_symbol() {
        let line = edge_line(&edge(
            "crates/core/flags/hiargs.rs",
            "crates/core/flags/hiargs.rs::HiArgs.searcher",
            "crates/core/main.rs",
            "crates/core/main.rs::search",
        ));
        assert_eq!(
            line,
            "crates/core/flags/hiargs.rs::HiArgs.searcher  --Calls-->  crates/core/main.rs::search"
        );
        assert_eq!(line.matches("hiargs.rs").count(), 1, "{line}");
        assert_eq!(line.matches("main.rs").count(), 1, "{line}");
    }

    /// An id that is not file-qualified (a bare name from an unresolved or
    /// external endpoint) still gets its file, so nothing loses its location.
    #[test]
    fn an_edge_line_qualifies_a_bare_symbol_with_its_file() {
        let line = edge_line(&edge("a.rs", "run", "b.rs", "b.rs::go"));
        assert_eq!(line, "a.rs::run  --Calls-->  b.rs::go");
    }

    /// The truncation line names the flag its caller says widens the answer.
    #[test]
    fn the_truncation_line_names_the_flag_that_shows_the_rest() {
        let line = truncation_line(80, 159, 239, true, Some(WIDEN_BUDGET)).expect("capped");
        assert!(line.starts_with("shown 80 of 239 (159 hidden)"), "{line}");
        assert!(line.contains("raise --budget to show more"), "{line}");
        assert_eq!(truncation_line(10, 0, 10, false, Some(WIDEN_BUDGET)), None);
    }

    /// `explore` is sized by `--limit` as well as `--budget`: with `--limit 5` the
    /// list stays at five at any budget, so naming `--budget` alone would be
    /// false. A caller that has no flag that widens its answer says nothing.
    #[test]
    fn the_truncation_line_names_each_commands_own_widening_flag() {
        let line = truncation_line(20, 97, 117, true, Some(WIDEN_LIMIT_OR_BUDGET)).expect("capped");
        assert!(line.contains("raise --limit or --budget"), "{line}");
        let bare = truncation_line(20, 97, 117, true, None).expect("capped");
        assert_eq!(bare, "shown 20 of 117 (97 hidden)");
    }

    /// A multi-line, multi-clause reason prints as exactly one line in text
    /// mode and points at the flags that show the rest; `--verbose` keeps all.
    #[test]
    fn a_walk_incomplete_reason_prints_as_one_line_in_text_mode() {
        let reason = "the walk did not complete: stopped at depth 3; the result is a lower bound, \
                      not the full blast radius; the analysis is partial: call extraction did not \
                      cover the whole corpus:\n1 file(s) failed to parse,\n6 recovered by pattern";
        let line = walk_incomplete_line(reason, false);
        assert!(!line.contains('\n'), "{line:?}");
        assert!(
            line.starts_with("warning: the walk did not complete"),
            "{line}"
        );
        assert!(
            line.contains("--verbose") && line.contains("--json"),
            "{line}"
        );
        assert!(line.len() < reason.len(), "{line}");
        // Two clauses were dropped; the line says so rather than hiding them.
        assert!(line.contains("+2 more caveats"), "{line}");
        // A reason with a single clause drops nothing and claims nothing.
        let single = walk_incomplete_line("the walk did not complete: stopped at depth 3", false);
        assert!(!single.contains("more caveat"), "{single}");
    }

    /// `--verbose` reaches the renderer as a parameter and keeps the whole
    /// reason: every clause, in order, with its original line breaks.
    #[test]
    fn a_verbose_walk_incomplete_prints_the_whole_reason() {
        let reason = "first clause; second clause;\nthird clause";
        assert_eq!(
            walk_incomplete_line(reason, true),
            format!("warning: {reason}")
        );
        assert_ne!(
            walk_incomplete_line(reason, false),
            walk_incomplete_line(reason, true)
        );
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
