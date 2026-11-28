//! Stateful model checker for `devmap-store` incremental writes, `ResolveReuse`,
//! coverage-gap carry-forward, retention pruning, and `CsrGraphReader`
//! byte-level fuzzing.
//!
//! Environment knobs:
//! - `DEVMAP_STATEFUL_SEED` (u64, hex or decimal): base seed for deterministic replay.
//! - `DEVMAP_STATEFUL_ROUNDS` (usize): number of independent stateful campaigns.
//! - `DEVMAP_STATEFUL_STEPS` (usize): actions per campaign.
//! - `DEVMAP_CSR_FUZZ_ROUNDS` (usize): CSR reader mutation rounds.

#[allow(dead_code)]
mod stateful_fuzz {
    include!("../../testsupport/stateful_fuzz.rs");
}

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use devmap_analyze::analyze;
use devmap_extract::extract_file;
use devmap_extract::model::{confidence_millis, EdgeKind, Extraction};
use devmap_resolve::model::{Resolution, ResolutionLimits, ResolutionResult, ResolvedEdge};
use devmap_resolve::resolver::ReusedFileResolution;
use devmap_resolve::Resolver;
use devmap_store::csr::{
    CsrGraphReader, CsrGraphWriter, CsrHeader, CsrNodeRecord, DC_GRAPH_MAGIC, DC_GRAPH_VERSION,
};
use devmap_store::{GenerationWriteOpts, Store};
use rusqlite::Connection;

use stateful_fuzz::{
    bail, check, env_u64, env_usize, format_repro_test, generate_micro_repo, lower_damage,
    minimize_sequence, mutate_file_elements, skip_missing, CheckFailure, HealthTracker, MicroLang,
    MicroRepo, Source, StepOutcome,
};

const DEFAULT_SEED: u64 = 0x5717_C400_5704_E001;
const DEFAULT_ROUNDS: usize = 6;
const DEFAULT_STEPS: usize = 48;
const DEFAULT_CSR_ROUNDS: usize = 512;

const ALL_LANGS: [MicroLang; 4] = [
    MicroLang::Rust,
    MicroLang::TypeScript,
    MicroLang::Go,
    MicroLang::Python,
];

const PSD1_PATH: &str = "scripts/manifest.psd1";
const PSD1_BODY: &str = "@{ ModuleVersion = '1.0' }\n";
const PROTO_PATH: &str = "proto/service.proto";
const PROTO_WITH_MSG: &str = "syntax = \"proto3\";\nmessage User {\n  string id = 1;\n}\n";
const PROTO_EMPTY: &str = "// declarations moved elsewhere\n";

const ALL_ACTIONS: [&str; 12] = [
    "CreateFile",
    "EditValid",
    "EditBrokenSyntax",
    "EditGapFile",
    "RenameFile",
    "DeleteFile",
    "SaveGenFullSlice",
    "SaveGenPartialSlice",
    "SaveGenMismatchedAffected",
    "PruneGenerations",
    "ReopenDiskStore",
    "CheckColdEquivalence",
];

// ---------------------------------------------------------------------------
// Tempdir RAII helper
// ---------------------------------------------------------------------------

struct TempDirGuard {
    path: PathBuf,
}

impl TempDirGuard {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "devmap-stateful-store-{label}-{}-{stamp}-{seq}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create stateful store tempdir");
        Self { path }
    }

    fn db_path(&self) -> PathBuf {
        self.path.join("devmap.sqlite")
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

// ---------------------------------------------------------------------------
// Canonical semantic snapshot & cold-build oracle
// ---------------------------------------------------------------------------

type StructuredEdge = (String, String, String, String, String, i64, Option<String>);
type DeadSymbolRow = (String, String, i64, bool, Option<String>);

type UnresolvedRow = (String, String, String);

#[derive(Clone, Debug, PartialEq, Eq)]
struct CanonicalSnapshot {
    symbols_by_file: BTreeMap<String, BTreeSet<String>>,
    edge_strings: Vec<String>,
    structured_edges: Vec<StructuredEdge>,
    unresolved_rows: Vec<UnresolvedRow>,
    analysis_unresolved_calls: Option<usize>,
    coverage_gaps: Vec<(&'static str, String)>,
    dead_symbols: Vec<DeadSymbolRow>,
    resolution_rate_json: String,
    edge_confidence_mismatches: Option<usize>,
    fts_hits: BTreeMap<String, Vec<(String, String)>>,
}

fn collect_coverage_gaps(store: &Store, db_label: &str) -> Vec<(&'static str, String)> {
    let status = store.status(db_label).expect("store status");
    let g = &status.coverage_gaps;
    let mut rows: Vec<(&'static str, String)> = [
        ("discovery_refused", &g.discovery_refused),
        ("parse_failed", &g.parse_failed),
        ("pattern_recovered", &g.pattern_recovered),
        ("call_blind", &g.call_blind),
        ("import_blind", &g.import_blind),
        ("not_parsed", &g.not_parsed),
        ("over_limit", &g.over_limit),
        ("partial_parse", &g.partial_parse),
        ("resolution_budget", &g.resolution_budget),
    ]
    .into_iter()
    .flat_map(|(label, sample)| {
        sample
            .shown
            .iter()
            .map(move |row| (label, row.path.clone()))
    })
    .collect();
    rows.sort();
    rows
}

fn fts_sample_queries(symbols_by_file: &BTreeMap<String, BTreeSet<String>>) -> Vec<String> {
    let mut names = BTreeSet::new();
    for syms in symbols_by_file.values() {
        for name in syms {
            // Keep simple identifier-like names that FTS5 unicode61 accepts cleanly.
            if !name.is_empty()
                && name.len() <= 64
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                names.insert(name.clone());
            }
        }
    }
    names.into_iter().take(12).collect()
}

fn canonical_snapshot(store: &Store, db_label: &str) -> Result<CanonicalSnapshot, String> {
    let symbols_by_file = store
        .latest_symbol_names_by_file()
        .map_err(|e| format!("latest_symbol_names_by_file: {e}"))?;

    let mut edge_strings = store
        .latest_edges_for_test()
        .map_err(|e| format!("latest_edges_for_test: {e}"))?;
    edge_strings.sort();

    let mut structured_edges: Vec<_> = store
        .latest_edges(0.0)
        .map_err(|e| format!("latest_edges: {e}"))?
        .into_iter()
        .map(|e| {
            (
                e.source_file,
                e.target_file,
                e.source_symbol,
                e.target_symbol,
                e.edge_kind,
                confidence_millis(e.confidence),
                e.resolution,
            )
        })
        .collect();
    structured_edges.sort();

    let mut unresolved_rows = store
        .latest_unresolved(100_000)
        .map_err(|e| format!("latest_unresolved: {e}"))?;
    unresolved_rows.sort();

    let analysis_unresolved_calls = store
        .latest_analysis()
        .map_err(|e| format!("latest_analysis: {e}"))?
        .map(|a| a.unresolved_calls);

    let coverage_gaps = collect_coverage_gaps(store, db_label);

    let mut dead_symbols: Vec<_> = store
        .latest_dead_symbols()
        .map_err(|e| format!("latest_dead_symbols: {e}"))?
        .into_iter()
        .map(|d| {
            (
                d.file_path,
                d.symbol_name,
                (f64::from(d.confidence) * 1000.0).round() as i64,
                d.is_exempt,
                d.exemption_reason,
            )
        })
        .collect();
    dead_symbols.sort();

    let rate = store
        .latest_resolution_rate()
        .map_err(|e| format!("latest_resolution_rate: {e}"))?;
    let resolution_rate_json =
        serde_json::to_string(&rate).map_err(|e| format!("serialize resolution_rate: {e}"))?;

    let edge_confidence_mismatches = store
        .edge_confidence_mismatches()
        .map_err(|e| format!("edge_confidence_mismatches: {e}"))?;

    let mut fts_hits = BTreeMap::new();
    for query in fts_sample_queries(&symbols_by_file) {
        let mut hits: Vec<(String, String)> = store
            .search_symbols(&query, 64)
            .map_err(|e| format!("search_symbols({query:?}): {e}"))?
            .into_iter()
            .map(|s| (s.path, s.qualified_name))
            .collect();
        hits.sort();
        fts_hits.insert(query, hits);
    }

    Ok(CanonicalSnapshot {
        symbols_by_file,
        edge_strings,
        structured_edges,
        unresolved_rows,
        analysis_unresolved_calls,
        coverage_gaps,
        dead_symbols,
        resolution_rate_json,
        edge_confidence_mismatches,
        fts_hits,
    })
}

fn diff_snapshots(
    label: &str,
    live: &CanonicalSnapshot,
    cold: &CanonicalSnapshot,
) -> Option<String> {
    if live == cold {
        return None;
    }
    let mut diffs = Vec::new();
    if live.symbols_by_file != cold.symbols_by_file {
        diffs.push(format!(
            "symbols_by_file mismatch:\n  live={:?}\n  cold={:?}",
            live.symbols_by_file, cold.symbols_by_file
        ));
    }
    if live.edge_strings != cold.edge_strings {
        diffs.push(format!(
            "edge_strings mismatch (live={}, cold={}):\n  live={:?}\n  cold={:?}",
            live.edge_strings.len(),
            cold.edge_strings.len(),
            live.edge_strings,
            cold.edge_strings
        ));
    }
    if live.structured_edges != cold.structured_edges {
        diffs.push(format!(
            "structured_edges mismatch (live={}, cold={})",
            live.structured_edges.len(),
            cold.structured_edges.len()
        ));
    }
    if live.unresolved_rows != cold.unresolved_rows {
        diffs.push(format!(
            "unresolved_rows mismatch (live={}, cold={}):\n  live={:?}\n  cold={:?}",
            live.unresolved_rows.len(),
            cold.unresolved_rows.len(),
            live.unresolved_rows,
            cold.unresolved_rows
        ));
    }
    if live.analysis_unresolved_calls != cold.analysis_unresolved_calls {
        diffs.push(format!(
            "analysis_unresolved_calls mismatch: live={:?}, cold={:?}",
            live.analysis_unresolved_calls, cold.analysis_unresolved_calls
        ));
    }
    if live.coverage_gaps != cold.coverage_gaps {
        diffs.push(format!(
            "coverage_gaps mismatch:\n  live={:?}\n  cold={:?}",
            live.coverage_gaps, cold.coverage_gaps
        ));
    }
    if live.dead_symbols != cold.dead_symbols {
        diffs.push(format!(
            "dead_symbols mismatch:\n  live={:?}\n  cold={:?}",
            live.dead_symbols, cold.dead_symbols
        ));
    }
    if live.resolution_rate_json != cold.resolution_rate_json {
        diffs.push(format!(
            "resolution_rate mismatch:\n  live={}\n  cold={}",
            live.resolution_rate_json, cold.resolution_rate_json
        ));
    }
    if live.edge_confidence_mismatches != cold.edge_confidence_mismatches {
        diffs.push(format!(
            "edge_confidence_mismatches mismatch: live={:?}, cold={:?}",
            live.edge_confidence_mismatches, cold.edge_confidence_mismatches
        ));
    }
    if live.fts_hits != cold.fts_hits {
        diffs.push(format!(
            "fts_hits mismatch:\n  live={:?}\n  cold={:?}",
            live.fts_hits, cold.fts_hits
        ));
    }
    Some(format!("{label}: {}", diffs.join("; ")))
}

fn cold_build_snapshot(files: &[(String, String)]) -> Result<CanonicalSnapshot, String> {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, body)| extract_file(path, body))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver
        .resolve_all(&extractions)
        .map_err(|e| format!("cold resolve_all: {e}"))?;
    let analysis = analyze(&extractions, &resolution);

    let store = Store::open_in_memory().map_err(|e| format!("open_in_memory: {e}"))?;
    store
        .save_generation(&extractions, &resolution, &analysis)
        .map_err(|e| format!("cold save_generation: {e}"))?;
    canonical_snapshot(&store, ":memory:")
}

/// Mirror `commands/build.rs::affected_closure` so `SaveGenFullSlice` exercises
/// `Resolver::resolve_all_with_limits_reusing` under the production soundness
/// contract.
fn compute_affected_closure(
    store: &Store,
    extractions: &[Extraction],
) -> Result<Option<BTreeSet<String>>, String> {
    let previous_hashes = store
        .latest_file_hashes()
        .map_err(|e| format!("latest_file_hashes: {e}"))?;
    if previous_hashes.is_empty() || previous_hashes.len() != extractions.len() {
        return Ok(None);
    }
    if !store
        .latest_generation_payload_is_current()
        .map_err(|e| format!("latest_generation_payload_is_current: {e}"))?
    {
        return Ok(None);
    }
    let mut changed = BTreeSet::new();
    for ext in extractions {
        match previous_hashes.get(&ext.file_path) {
            Some(hash) if *hash == ext.content_hash => {}
            Some(_) => {
                changed.insert(ext.file_path.clone());
            }
            None => return Ok(None),
        }
    }
    for path in store
        .latest_budget_refused_paths()
        .map_err(|e| format!("latest_budget_refused_paths: {e}"))?
    {
        if previous_hashes.contains_key(&path) {
            changed.insert(path);
        }
    }
    if changed.is_empty() {
        return Ok(Some(BTreeSet::new()));
    }

    let previous_symbols = store
        .latest_symbol_names_by_file()
        .map_err(|e| format!("latest_symbol_names_by_file: {e}"))?;
    let mut changed_names = BTreeSet::new();
    let mut current_symbols: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for ext in extractions {
        current_symbols.insert(
            ext.file_path.as_str(),
            ext.symbols.iter().map(|s| s.name.as_str()).collect(),
        );
    }
    let empty_before = BTreeSet::new();
    for file in &changed {
        let before = previous_symbols.get(file).unwrap_or(&empty_before);
        let after = current_symbols
            .get(file.as_str())
            .cloned()
            .unwrap_or_default();
        for name in before {
            if !after.contains(name.as_str()) {
                changed_names.insert(name.clone());
            }
        }
        for name in after {
            if !before.contains(name) {
                changed_names.insert(name.to_string());
            }
        }
    }

    let mut affected = changed;
    if !changed_names.is_empty() {
        for ext in extractions {
            if affected.contains(&ext.file_path) {
                continue;
            }
            let mentions = ext
                .calls
                .iter()
                .any(|c| changed_names.contains(&c.callee_name))
                || ext
                    .references
                    .iter()
                    .any(|r| changed_names.contains(&r.name))
                || ext
                    .imports
                    .iter()
                    .any(|imp| imp.imported_names.iter().any(|n| changed_names.contains(n)));
            if mentions {
                affected.insert(ext.file_path.clone());
            }
        }
    }
    Ok(Some(affected))
}

// ---------------------------------------------------------------------------
// Typed Stateful Actions & Replay Engine
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum GapToggle {
    AddPsd1,
    RemovePsd1,
    ProtoWithMessage,
    ProtoEmpty,
    RemoveProto,
}

#[derive(Clone, Debug)]
enum MismatchKind {
    /// Pass full extractions after a local edit but omit the moved file from `affected_paths`.
    OmitMovedFromAffected {
        target_path: String,
        decoy_path: String,
        new_body: String,
    },
    /// Hand a pre-deletion edge touching a file named in `deleted_paths`.
    StaleEdgeTouchingDeleted,
}

#[derive(Clone, Debug)]
enum Action {
    CreateFile { path: String, content: String },
    EditValid { path: String, new_content: String },
    EditBrokenSyntax { path: String, new_content: String },
    EditGapFile { toggle: GapToggle },
    RenameFile { from: String, to: String },
    DeleteFile { path: String },
    SaveGenFullSlice { verify_every_row: bool },
    SaveGenPartialSlice,
    SaveGenMismatchedAffected { kind: MismatchKind },
    PruneGenerations { keep: usize },
    ReopenDiskStore,
    CheckColdEquivalence { reverse_scan_order: bool },
}

impl Action {
    fn name(&self) -> &'static str {
        match self {
            Self::CreateFile { .. } => "CreateFile",
            Self::EditValid { .. } => "EditValid",
            Self::EditBrokenSyntax { .. } => "EditBrokenSyntax",
            Self::EditGapFile { .. } => "EditGapFile",
            Self::RenameFile { .. } => "RenameFile",
            Self::DeleteFile { .. } => "DeleteFile",
            Self::SaveGenFullSlice { .. } => "SaveGenFullSlice",
            Self::SaveGenPartialSlice => "SaveGenPartialSlice",
            Self::SaveGenMismatchedAffected { .. } => "SaveGenMismatchedAffected",
            Self::PruneGenerations { .. } => "PruneGenerations",
            Self::ReopenDiskStore => "ReopenDiskStore",
            Self::CheckColdEquivalence { .. } => "CheckColdEquivalence",
        }
    }

    fn summary(&self) -> String {
        match self {
            Self::CreateFile { path, content } => {
                format!("CreateFile({path:?}, {} bytes)", content.len())
            }
            Self::EditValid { path, new_content } => {
                format!("EditValid({path:?}, {} bytes)", new_content.len())
            }
            Self::EditBrokenSyntax { path, new_content } => {
                format!("EditBrokenSyntax({path:?}, {} bytes)", new_content.len())
            }
            Self::EditGapFile { toggle } => format!("EditGapFile({toggle:?})"),
            Self::RenameFile { from, to } => format!("RenameFile({from:?} -> {to:?})"),
            Self::DeleteFile { path } => format!("DeleteFile({path:?})"),
            Self::SaveGenFullSlice { verify_every_row } => {
                format!("SaveGenFullSlice(verify_every_row={verify_every_row})")
            }
            Self::SaveGenPartialSlice => "SaveGenPartialSlice".to_string(),
            Self::SaveGenMismatchedAffected { kind } => match kind {
                MismatchKind::OmitMovedFromAffected {
                    target_path,
                    decoy_path,
                    ..
                } => format!(
                    "SaveGenMismatchedAffected::OmitMoved(target={target_path:?}, decoy={decoy_path:?})"
                ),
                MismatchKind::StaleEdgeTouchingDeleted => {
                    "SaveGenMismatchedAffected::StaleEdgeTouchingDeleted".to_string()
                }
            },
            Self::PruneGenerations { keep } => format!("PruneGenerations(keep={keep})"),
            Self::ReopenDiskStore => "ReopenDiskStore".to_string(),
            Self::CheckColdEquivalence { reverse_scan_order } => {
                format!("CheckColdEquivalence(reverse={reverse_scan_order})")
            }
        }
    }
}

fn lang_for_path(path: &str) -> Option<MicroLang> {
    if path.ends_with(".rs") {
        Some(MicroLang::Rust)
    } else if path.ends_with(".ts") {
        Some(MicroLang::TypeScript)
    } else if path.ends_with(".go") {
        Some(MicroLang::Go)
    } else if path.ends_with(".py") {
        Some(MicroLang::Python)
    } else {
        None
    }
}

struct HarnessState {
    _dir: TempDirGuard,
    db_path: PathBuf,
    store: Store,
    working: MicroRepo,
    committed: MicroRepo,
    dirty_paths: BTreeSet<String>,
    deleted_paths: BTreeSet<String>,
    reuse_cache: BTreeMap<String, ReusedFileResolution>,
    reuse_bound_generation: Option<u32>,
    generations_written: usize,
}

impl HarnessState {
    fn new(label: &str, initial_repo: MicroRepo) -> Self {
        let dir = TempDirGuard::new(label);
        let db_path = dir.db_path();
        let store = Store::open(&db_path).expect("open disk store");
        let dirty_paths: BTreeSet<String> = initial_repo.keys().cloned().collect();
        Self {
            _dir: dir,
            db_path,
            store,
            working: initial_repo,
            committed: BTreeMap::new(),
            dirty_paths,
            deleted_paths: BTreeSet::new(),
            reuse_cache: BTreeMap::new(),
            reuse_bound_generation: None,
            generations_written: 0,
        }
    }

    fn db_label(&self) -> String {
        self.db_path.to_string_lossy().into_owned()
    }

    fn code_paths(&self) -> Vec<String> {
        self.working
            .keys()
            .filter(|p| lang_for_path(p).is_some())
            .cloned()
            .collect()
    }

    fn apply(&mut self, action: &Action) -> StepOutcome {
        let act_name = action.name();
        match action {
            Action::CreateFile { path, content } => {
                if self.working.contains_key(path) {
                    return bail("create_already_exists");
                }
                self.working.insert(path.clone(), content.clone());
                self.dirty_paths.insert(path.clone());
                self.deleted_paths.remove(path);
                StepOutcome::Ok
            }
            Action::EditValid { path, new_content } => {
                if !self.working.contains_key(path) {
                    return skip_missing("edit_missing_path");
                }
                self.working.insert(path.clone(), new_content.clone());
                self.dirty_paths.insert(path.clone());
                self.deleted_paths.remove(path);
                StepOutcome::Ok
            }
            Action::EditBrokenSyntax { path, new_content } => {
                if !self.working.contains_key(path) {
                    return skip_missing("edit_broken_missing_path");
                }
                self.working.insert(path.clone(), new_content.clone());
                self.dirty_paths.insert(path.clone());
                self.deleted_paths.remove(path);
                StepOutcome::Ok
            }
            Action::EditGapFile { toggle } => {
                match toggle {
                    GapToggle::AddPsd1 => {
                        self.working
                            .insert(PSD1_PATH.to_string(), PSD1_BODY.to_string());
                        self.dirty_paths.insert(PSD1_PATH.to_string());
                        self.deleted_paths.remove(PSD1_PATH);
                    }
                    GapToggle::RemovePsd1 => {
                        if self.working.remove(PSD1_PATH).is_none() {
                            return bail("psd1_not_present");
                        }
                        self.dirty_paths.remove(PSD1_PATH);
                        if self.committed.contains_key(PSD1_PATH) {
                            self.deleted_paths.insert(PSD1_PATH.to_string());
                        }
                    }
                    GapToggle::ProtoWithMessage => {
                        self.working
                            .insert(PROTO_PATH.to_string(), PROTO_WITH_MSG.to_string());
                        self.dirty_paths.insert(PROTO_PATH.to_string());
                        self.deleted_paths.remove(PROTO_PATH);
                    }
                    GapToggle::ProtoEmpty => {
                        self.working
                            .insert(PROTO_PATH.to_string(), PROTO_EMPTY.to_string());
                        self.dirty_paths.insert(PROTO_PATH.to_string());
                        self.deleted_paths.remove(PROTO_PATH);
                    }
                    GapToggle::RemoveProto => {
                        if self.working.remove(PROTO_PATH).is_none() {
                            return bail("proto_not_present");
                        }
                        self.dirty_paths.remove(PROTO_PATH);
                        if self.committed.contains_key(PROTO_PATH) {
                            self.deleted_paths.insert(PROTO_PATH.to_string());
                        }
                    }
                }
                StepOutcome::Ok
            }
            Action::RenameFile { from, to } => {
                if from == to || self.working.contains_key(to) {
                    return bail("rename_collision");
                }
                let Some(body) = self.working.remove(from) else {
                    return skip_missing("rename_missing_source");
                };
                self.working.insert(to.clone(), body);
                self.dirty_paths.remove(from);
                if self.committed.contains_key(from) {
                    self.deleted_paths.insert(from.clone());
                }
                self.dirty_paths.insert(to.clone());
                self.deleted_paths.remove(to);
                StepOutcome::Ok
            }
            Action::DeleteFile { path } => {
                if self.working.len() <= 1 {
                    return bail("cannot_delete_last_file");
                }
                if self.working.remove(path).is_none() {
                    return skip_missing("delete_missing_path");
                }
                self.dirty_paths.remove(path);
                if self.committed.contains_key(path) {
                    self.deleted_paths.insert(path.clone());
                }
                StepOutcome::Ok
            }
            Action::SaveGenFullSlice { verify_every_row } => {
                if self.working.is_empty() {
                    return bail("empty_repo");
                }
                let extractions: Vec<Extraction> = self
                    .working
                    .iter()
                    .map(|(p, body)| extract_file(p, body))
                    .collect();

                let closure = match compute_affected_closure(&self.store, &extractions) {
                    Ok(c) => c,
                    Err(e) => {
                        return StepOutcome::CheckFailed(CheckFailure::new(
                            "affected_closure",
                            act_name,
                            e,
                        ));
                    }
                };

                let latest_gen = self.store.latest_generation_id().ok().flatten();
                let mut reusable: BTreeMap<String, ReusedFileResolution> = BTreeMap::new();
                if let Some(ref affected_set) = closure {
                    if self.reuse_bound_generation == latest_gen && latest_gen.is_some() {
                        for ext in &extractions {
                            if affected_set.contains(&ext.file_path) {
                                continue;
                            }
                            if let Some(entry) = self.reuse_cache.get(&ext.file_path) {
                                if entry.content_hash == ext.content_hash {
                                    reusable.insert(ext.file_path.clone(), entry.clone());
                                }
                            }
                        }
                    }
                }

                let mut resolver = Resolver::new();
                resolver.index_extractions(&extractions);
                let outcome = match resolver.resolve_all_with_limits_reusing(
                    &extractions,
                    ResolutionLimits::default(),
                    &reusable,
                ) {
                    Ok(o) => o,
                    Err(e) => {
                        return StepOutcome::CheckFailed(CheckFailure::new(
                            "resolve_reuse",
                            act_name,
                            format!("resolve_all_with_limits_reusing failed: {e}"),
                        ));
                    }
                };

                let analysis = analyze(&extractions, &outcome.result);
                let (affected_paths, deleted_paths) = match closure {
                    Some(ref set) if !set.is_empty() => (set.iter().cloned().collect(), Vec::new()),
                    _ => (Vec::new(), self.deleted_paths.iter().cloned().collect()),
                };

                let opts = GenerationWriteOpts {
                    affected_paths,
                    deleted_paths,
                    verify_every_row: *verify_every_row,
                    ..Default::default()
                };

                let gen_id = match self.store.save_generation_with_opts(
                    &extractions,
                    &outcome.result,
                    &analysis,
                    opts,
                ) {
                    Ok(id) => id,
                    Err(e) => {
                        return StepOutcome::CheckFailed(CheckFailure::new(
                            "save_generation_full",
                            act_name,
                            format!("save_generation_with_opts failed: {e}"),
                        ));
                    }
                };

                // Update reuse cache to reflect the newly committed generation.
                if reusable.is_empty() {
                    self.reuse_cache = outcome.produced;
                } else {
                    self.reuse_cache
                        .retain(|path, _| self.working.contains_key(path));
                    for (path, res) in outcome.produced {
                        self.reuse_cache.insert(path, res);
                    }
                }
                self.reuse_bound_generation = Some(gen_id);
                self.committed = self.working.clone();
                self.dirty_paths.clear();
                self.deleted_paths.clear();
                self.generations_written += 1;

                let mismatches = self.store.edge_confidence_mismatches().ok().flatten();
                check(
                    mismatches == Some(0),
                    "edge_confidence_mismatches",
                    act_name,
                    format!("expected Some(0), got {mismatches:?}"),
                )
            }
            Action::SaveGenPartialSlice => {
                if self.committed.is_empty() || self.generations_written == 0 {
                    return skip_missing("no_prior_generation_for_partial");
                }
                if self.dirty_paths.is_empty() && self.deleted_paths.is_empty() {
                    return bail("no_pending_diff_for_partial");
                }
                // Daemon shape: extract ONLY the dirty files into `fresh`, load
                // unchanged extractions from the store, merge for resolve+analyze,
                // and persist ONLY `&fresh` with `affected_paths` + `deleted_paths`.
                let fresh: Vec<Extraction> = self
                    .dirty_paths
                    .iter()
                    .filter_map(|p| self.working.get(p).map(|body| extract_file(p, body)))
                    .collect();

                let stored_prev = match self.store.latest_extractions() {
                    Ok(rows) => rows,
                    Err(e) => {
                        return StepOutcome::CheckFailed(CheckFailure::new(
                            "partial_load_prev",
                            act_name,
                            format!("latest_extractions failed: {e}"),
                        ));
                    }
                };

                let mut merged: BTreeMap<String, Extraction> = BTreeMap::new();
                for ext in stored_prev {
                    if self.deleted_paths.contains(&ext.file_path)
                        || self.dirty_paths.contains(&ext.file_path)
                    {
                        continue;
                    }
                    merged.insert(ext.file_path.clone(), ext);
                }
                for ext in &fresh {
                    merged.insert(ext.file_path.clone(), ext.clone());
                }
                let all_extractions: Vec<Extraction> = merged.into_values().collect();

                let mut resolver = Resolver::new();
                resolver.index_extractions(&all_extractions);
                let resolution = match resolver.resolve_all(&all_extractions) {
                    Ok(r) => r,
                    Err(e) => {
                        return StepOutcome::CheckFailed(CheckFailure::new(
                            "partial_resolve",
                            act_name,
                            format!("resolve_all failed: {e}"),
                        ));
                    }
                };
                let analysis = analyze(&all_extractions, &resolution);

                let opts = GenerationWriteOpts {
                    affected_paths: self.dirty_paths.iter().cloned().collect(),
                    deleted_paths: self.deleted_paths.iter().cloned().collect(),
                    ..Default::default()
                };

                if let Err(e) =
                    self.store
                        .save_generation_with_opts(&fresh, &resolution, &analysis, opts)
                {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "save_generation_partial",
                        act_name,
                        format!("partial-slice save_generation_with_opts failed: {e}"),
                    ));
                }

                // Daemon drain unbinds the CLI reuse cache (same as production).
                self.reuse_bound_generation = None;
                self.committed = self.working.clone();
                self.dirty_paths.clear();
                self.deleted_paths.clear();
                self.generations_written += 1;

                let mismatches = self.store.edge_confidence_mismatches().ok().flatten();
                check(
                    mismatches == Some(0),
                    "edge_confidence_mismatches",
                    act_name,
                    format!("expected Some(0), got {mismatches:?}"),
                )
            }
            Action::SaveGenMismatchedAffected { kind } => {
                if self.committed.is_empty() || self.generations_written == 0 {
                    return skip_missing("no_prior_generation_for_mismatch");
                }
                match kind {
                    MismatchKind::OmitMovedFromAffected {
                        target_path,
                        decoy_path,
                        new_body,
                    } => {
                        if !self.committed.contains_key(target_path)
                            || !self.committed.contains_key(decoy_path)
                            || target_path == decoy_path
                        {
                            return bail("mismatch_paths_unavailable");
                        }
                        if !self.dirty_paths.is_empty() || !self.deleted_paths.is_empty() {
                            // Apply against clean committed state so the assertion isolates `content_matches`.
                            return bail("working_tree_has_pending_edits");
                        }
                        let mut next_repo = self.committed.clone();
                        next_repo.insert(target_path.clone(), new_body.clone());

                        let extractions: Vec<Extraction> = next_repo
                            .iter()
                            .map(|(p, body)| extract_file(p, body))
                            .collect();
                        let expected_hash = extractions
                            .iter()
                            .find(|e| e.file_path == *target_path)
                            .map(|e| e.content_hash)
                            .expect("target extraction present");

                        let mut resolver = Resolver::new();
                        resolver.index_extractions(&extractions);
                        let resolution = match resolver.resolve_all(&extractions) {
                            Ok(r) => r,
                            Err(e) => {
                                return StepOutcome::CheckFailed(CheckFailure::new(
                                    "mismatch_resolve",
                                    act_name,
                                    format!("resolve_all failed: {e}"),
                                ));
                            }
                        };
                        let analysis = analyze(&extractions, &resolution);

                        // Adversarially omit `target_path` from `affected_paths`!
                        let opts = GenerationWriteOpts {
                            affected_paths: vec![decoy_path.clone()],
                            ..Default::default()
                        };
                        if let Err(e) = self.store.save_generation_with_opts(
                            &extractions,
                            &resolution,
                            &analysis,
                            opts,
                        ) {
                            return StepOutcome::CheckFailed(CheckFailure::new(
                                "mismatch_save",
                                act_name,
                                format!("save_generation_with_opts failed: {e}"),
                            ));
                        }

                        self.working = next_repo.clone();
                        self.committed = next_repo;
                        self.dirty_paths.clear();
                        self.deleted_paths.clear();
                        self.reuse_bound_generation = None;
                        self.generations_written += 1;

                        let hashes = match self.store.latest_file_hashes() {
                            Ok(h) => h,
                            Err(e) => {
                                return StepOutcome::CheckFailed(CheckFailure::new(
                                    "content_matches_guard",
                                    act_name,
                                    format!("latest_file_hashes failed: {e}"),
                                ));
                            }
                        };
                        check(
                            hashes.get(target_path) == Some(&expected_hash),
                            "content_matches_guard",
                            act_name,
                            format!(
                                "moved hash for {target_path:?} was not updated when omitted from affected_paths: stored={:?}, expected={expected_hash}",
                                hashes.get(target_path)
                            ),
                        )
                    }
                    MismatchKind::StaleEdgeTouchingDeleted => {
                        if self.committed.len() < 2 {
                            return bail("need_two_files_for_deleted_edge");
                        }
                        let before_gen = self.store.latest_generation_id().ok().flatten();
                        let mut iter = self.committed.keys();
                        let gone_path = iter.next().unwrap().clone();
                        let survivor_path = iter.next().unwrap().clone();

                        let all_extractions: Vec<Extraction> = self
                            .committed
                            .iter()
                            .map(|(p, body)| extract_file(p, body))
                            .collect();
                        let structural = Arc::new(Resolution::Structural {
                            target_symbol: "survivor_sym".to_string(),
                            target_file: survivor_path.clone(),
                        });
                        let stale = ResolutionResult {
                            edges: vec![ResolvedEdge::resolved(
                                gone_path.clone(),
                                survivor_path.clone(),
                                "gone_sym".to_string(),
                                "survivor_sym".to_string(),
                                EdgeKind::Calls,
                                structural,
                                None,
                            )],
                            receiver_types: BTreeMap::new(),
                            reexport_chains: BTreeMap::new(),
                            unresolved: Vec::new(),
                        };
                        let stale_analysis = analyze(&all_extractions, &stale);
                        let survivors: Vec<Extraction> = all_extractions
                            .into_iter()
                            .filter(|e| e.file_path != gone_path)
                            .collect();

                        let res = self.store.save_generation_with_opts(
                            &survivors,
                            &stale,
                            &stale_analysis,
                            GenerationWriteOpts {
                                deleted_paths: vec![gone_path.clone()],
                                ..Default::default()
                            },
                        );
                        let after_gen = self.store.latest_generation_id().ok().flatten();
                        if res.is_ok() {
                            return StepOutcome::CheckFailed(CheckFailure::new(
                                "deleted_edge_guard",
                                act_name,
                                format!(
                                    "store admitted a phantom edge from deleted file {gone_path:?} -> {survivor_path:?}"
                                ),
                            ));
                        }
                        check(
                            before_gen == after_gen,
                            "deleted_edge_rollback",
                            act_name,
                            format!(
                                "refused write moved latest_generation_id from {before_gen:?} to {after_gen:?}"
                            ),
                        )
                    }
                }
            }
            Action::PruneGenerations { keep } => {
                if self.generations_written == 0 {
                    return skip_missing("no_generation_to_prune");
                }
                if let Err(e) = self.store.prune_generations_except_latest(*keep) {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "prune_generations",
                        act_name,
                        format!("prune_generations_except_latest({keep}) failed: {e}"),
                    ));
                }
                if let Err(e) = self.store.prune_extraction_cache() {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "prune_extraction_cache",
                        act_name,
                        format!("prune_extraction_cache failed: {e}"),
                    ));
                }
                // Verify zero orphaned rows across every generation-scoped relation.
                let conn = match Connection::open(&self.db_path) {
                    Ok(c) => c,
                    Err(e) => {
                        return StepOutcome::CheckFailed(CheckFailure::new(
                            "prune_orphan_open",
                            act_name,
                            format!("raw SQLite open failed: {e}"),
                        ));
                    }
                };
                for table in [
                    "generation_files",
                    "generation_nodes",
                    "generation_edges",
                    "generation_dead_symbols",
                    "nodes_fts_map",
                    "generation_coverage_gaps",
                ] {
                    let orphans: i64 = match conn.query_row(
                        &format!(
                            "SELECT COUNT(*) FROM {table} \
                             WHERE generation_id NOT IN (SELECT id FROM generations)"
                        ),
                        [],
                        |r| r.get(0),
                    ) {
                        Ok(n) => n,
                        Err(e) => {
                            return StepOutcome::CheckFailed(CheckFailure::new(
                                "prune_orphan_query",
                                act_name,
                                format!("orphan check on {table} failed: {e}"),
                            ));
                        }
                    };
                    if orphans != 0 {
                        return StepOutcome::CheckFailed(CheckFailure::new(
                            "prune_no_orphans",
                            act_name,
                            format!("{table} holds {orphans} orphaned row(s) after prune({keep})"),
                        ));
                    }
                }
                StepOutcome::Ok
            }
            Action::ReopenDiskStore => {
                if self.generations_written == 0 {
                    return skip_missing("no_generation_before_reopen");
                }
                let before_gen = self.store.latest_generation_id().ok().flatten();
                let reopened = match Store::open(&self.db_path) {
                    Ok(s) => s,
                    Err(e) => {
                        return StepOutcome::CheckFailed(CheckFailure::new(
                            "reopen_disk_store",
                            act_name,
                            format!("Store::open after reopen failed: {e}"),
                        ));
                    }
                };
                self.store = reopened;
                let after_gen = self.store.latest_generation_id().ok().flatten();
                if before_gen != after_gen {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "reopen_generation_preserved",
                        act_name,
                        format!("generation moved across reopen: {before_gen:?} -> {after_gen:?}"),
                    ));
                }
                match self.store.search_index_damage() {
                    Ok(None) => {}
                    Ok(Some(damage)) => {
                        return StepOutcome::CheckFailed(CheckFailure::new(
                            "reopen_fts_healthy",
                            act_name,
                            format!("unexpected FTS damage after reopen: {damage}"),
                        ));
                    }
                    Err(e) => {
                        return StepOutcome::CheckFailed(CheckFailure::new(
                            "reopen_fts_healthy",
                            act_name,
                            format!("search_index_damage check failed after reopen: {e}"),
                        ));
                    }
                }
                let mismatches = self.store.edge_confidence_mismatches().ok().flatten();
                check(
                    mismatches == Some(0),
                    "reopen_edge_confidence",
                    act_name,
                    format!("expected Some(0), got {mismatches:?}"),
                )
            }
            Action::CheckColdEquivalence { reverse_scan_order } => {
                if self.committed.is_empty() || self.generations_written == 0 {
                    return skip_missing("no_committed_generation");
                }
                let live = match canonical_snapshot(&self.store, &self.db_label()) {
                    Ok(s) => s,
                    Err(e) => {
                        return StepOutcome::CheckFailed(CheckFailure::new(
                            "live_snapshot",
                            act_name,
                            e,
                        ));
                    }
                };
                if live.edge_confidence_mismatches != Some(0) {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "edge_confidence_mismatches",
                        act_name,
                        format!(
                            "live store reported edge_confidence_mismatches={:?}",
                            live.edge_confidence_mismatches
                        ),
                    ));
                }

                let mut files: Vec<(String, String)> = self
                    .committed
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                let cold = match cold_build_snapshot(&files) {
                    Ok(s) => s,
                    Err(e) => {
                        return StepOutcome::CheckFailed(CheckFailure::new(
                            "cold_snapshot",
                            act_name,
                            e,
                        ));
                    }
                };
                if let Some(diff) = diff_snapshots("live vs cold", &live, &cold) {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "cold_equivalence",
                        act_name,
                        diff,
                    ));
                }

                if *reverse_scan_order && files.len() >= 2 {
                    files.reverse();
                    let permuted = match cold_build_snapshot(&files) {
                        Ok(s) => s,
                        Err(e) => {
                            return StepOutcome::CheckFailed(CheckFailure::new(
                                "permuted_cold_snapshot",
                                act_name,
                                e,
                            ));
                        }
                    };
                    if let Some(diff) = diff_snapshots("scan-order permutation", &cold, &permuted) {
                        return StepOutcome::CheckFailed(CheckFailure::new(
                            "scan_order_invariance",
                            act_name,
                            diff,
                        ));
                    }
                }
                StepOutcome::Ok
            }
        }
    }
}

fn synth_new_file(src: &mut Source, existing: &MicroRepo) -> (String, String) {
    let lang = *src.choose(&ALL_LANGS).unwrap_or(&MicroLang::Python);
    let idx = src.usize_below(900) + 10;
    let ext = lang.ext();
    let dir = match lang {
        MicroLang::Rust => "src",
        MicroLang::TypeScript => "ts",
        MicroLang::Go => "go",
        MicroLang::Python => "py",
    };
    let mut path = format!("{dir}/dyn_{idx}.{ext}");
    let mut bump = 0usize;
    while existing.contains_key(&path) {
        bump += 1;
        path = format!("{dir}/dyn_{idx}_{bump}.{ext}");
    }
    let seed_repo = generate_micro_repo(src, &[lang], 1);
    let body = seed_repo
        .into_values()
        .next()
        .unwrap_or_else(|| "def fallback():\n    return 1\n".to_string());
    (path, body)
}

fn sample_action(
    src: &mut Source,
    state: &HarnessState,
    health: &HealthTracker,
    prev: Option<&'static str>,
    step_idx: usize,
) -> Action {
    // Ensure initial generation exists early so downstream stateful actions
    // operate on a real populated store.
    let chosen = if state.generations_written == 0 && step_idx == 0 {
        "SaveGenFullSlice"
    } else {
        health.pick_weighted(src, &ALL_ACTIONS, prev)
    };

    let code_paths = state.code_paths();

    match chosen {
        "CreateFile" => {
            let (path, content) = synth_new_file(src, &state.working);
            Action::CreateFile { path, content }
        }
        "EditValid" => {
            if let Some(path) = src.choose(&code_paths).cloned() {
                let lang = lang_for_path(&path).unwrap_or(MicroLang::Python);
                let old = state.working.get(&path).map(String::as_str).unwrap_or("");
                let new_content = mutate_file_elements(src, lang, old);
                Action::EditValid { path, new_content }
            } else {
                let (path, content) = synth_new_file(src, &state.working);
                Action::CreateFile { path, content }
            }
        }
        "EditBrokenSyntax" => {
            if let Some(path) = src.choose(&code_paths).cloned() {
                let old = state.working.get(&path).map(String::as_str).unwrap_or("");
                let new_content = lower_damage(src, old);
                Action::EditBrokenSyntax { path, new_content }
            } else {
                let (path, content) = synth_new_file(src, &state.working);
                Action::CreateFile { path, content }
            }
        }
        "EditGapFile" => {
            let toggles = [
                GapToggle::AddPsd1,
                GapToggle::RemovePsd1,
                GapToggle::ProtoWithMessage,
                GapToggle::ProtoEmpty,
                GapToggle::RemoveProto,
            ];
            let toggle = src.choose(&toggles).cloned().unwrap_or(GapToggle::AddPsd1);
            Action::EditGapFile { toggle }
        }
        "RenameFile" => {
            if let Some(from) = src.choose(&code_paths).cloned() {
                let lang = lang_for_path(&from).unwrap_or(MicroLang::Python);
                let ext = lang.ext();
                let dir = from.rsplit_once('/').map_or("src", |(d, _)| d);
                let tag = src.usize_below(9_000) + 100;
                let to = format!("{dir}/renamed_{tag}.{ext}");
                Action::RenameFile { from, to }
            } else {
                Action::RenameFile {
                    from: "missing.py".to_string(),
                    to: "missing_2.py".to_string(),
                }
            }
        }
        "DeleteFile" => {
            let all_paths: Vec<String> = state.working.keys().cloned().collect();
            let path = src
                .choose(&all_paths)
                .cloned()
                .unwrap_or_else(|| "missing.py".to_string());
            Action::DeleteFile { path }
        }
        "SaveGenFullSlice" => Action::SaveGenFullSlice {
            verify_every_row: src.bool(),
        },
        "SaveGenPartialSlice" => Action::SaveGenPartialSlice,
        "SaveGenMismatchedAffected" => {
            let committed_code: Vec<String> = state
                .committed
                .keys()
                .filter(|p| lang_for_path(p).is_some())
                .cloned()
                .collect();
            let use_omit = src.bool()
                && committed_code.len() >= 2
                && state.dirty_paths.is_empty()
                && state.deleted_paths.is_empty();
            if use_omit {
                let target_idx = src.usize_below(committed_code.len());
                let decoy_idx = (target_idx + 1) % committed_code.len();
                let target_path = committed_code[target_idx].clone();
                let decoy_path = committed_code[decoy_idx].clone();
                let lang = lang_for_path(&target_path).unwrap_or(MicroLang::Python);
                let old = state
                    .committed
                    .get(&target_path)
                    .map(String::as_str)
                    .unwrap_or("");
                let new_body = mutate_file_elements(src, lang, old);
                Action::SaveGenMismatchedAffected {
                    kind: MismatchKind::OmitMovedFromAffected {
                        target_path,
                        decoy_path,
                        new_body,
                    },
                }
            } else {
                Action::SaveGenMismatchedAffected {
                    kind: MismatchKind::StaleEdgeTouchingDeleted,
                }
            }
        }
        "PruneGenerations" => Action::PruneGenerations {
            keep: 1 + src.usize_below(3),
        },
        "ReopenDiskStore" => Action::ReopenDiskStore,
        _ => Action::CheckColdEquivalence {
            reverse_scan_order: src.bool(),
        },
    }
}

fn replay_sequence(initial_repo: &MicroRepo, actions: &[Action]) -> Option<CheckFailure> {
    let mut state = HarnessState::new("replay", initial_repo.clone());
    for action in actions {
        if let Some(failure) = state.apply(action).into_failure() {
            return Some(failure);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Test 1: Stateful Incremental Store, ResolveReuse, Carry-Forward & Retention
// ---------------------------------------------------------------------------

#[test]
fn stateful_incremental_store_model_checker() {
    let started = Instant::now();
    let base_seed = env_u64("DEVMAP_STATEFUL_SEED", DEFAULT_SEED);
    let rounds = env_usize("DEVMAP_STATEFUL_ROUNDS", DEFAULT_ROUNDS);
    let steps_per_round = env_usize("DEVMAP_STATEFUL_STEPS", DEFAULT_STEPS);

    let mut health = HealthTracker::new();

    for round in 0..rounds {
        let round_seed = base_seed.wrapping_add((round as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15));
        let mut src = Source::from_seed(round_seed);
        let initial_repo = generate_micro_repo(&mut src, &ALL_LANGS, 6);
        let mut state = HarnessState::new(&format!("r{round}"), initial_repo.clone());
        let mut history: Vec<Action> = Vec::with_capacity(steps_per_round + 2);
        let mut prev: Option<&'static str> = None;

        for step in 0..steps_per_round {
            let action = sample_action(&mut src, &state, &health, prev, step);
            let act_name = action.name();
            let outcome = state.apply(&action);
            health.record(act_name, prev, &outcome);
            history.push(action);
            prev = Some(act_name);

            if let Some(failure) = outcome.into_failure() {
                let sig = failure.signature();
                let minimal = minimize_sequence(&history, &sig, 120, |cand| {
                    replay_sequence(&initial_repo, cand)
                });
                let rendered: Vec<String> = minimal.iter().map(Action::summary).collect();
                let repro = format_repro_test(
                    &format!("repro_incremental_store_round_{round}"),
                    round_seed,
                    &failure,
                    &rendered,
                );
                panic!(
                    "stateful incremental store oracle violation (round={round}, seed={round_seed:#x}, \
                     shrunk {} -> {} steps):\n{failure}\n\n{repro}\n{}",
                    history.len(),
                    minimal.len(),
                    health.probe_report("stateful_incremental_store")
                );
            }
        }

        // End each round with a forced flush + cold equivalence + permutation check.
        let tail_actions = [
            Action::SaveGenFullSlice {
                verify_every_row: false,
            },
            Action::CheckColdEquivalence {
                reverse_scan_order: true,
            },
        ];
        for action in tail_actions {
            let act_name = action.name();
            let outcome = state.apply(&action);
            health.record(act_name, prev, &outcome);
            history.push(action);
            prev = Some(act_name);

            if let Some(failure) = outcome.into_failure() {
                let sig = failure.signature();
                let minimal = minimize_sequence(&history, &sig, 120, |cand| {
                    replay_sequence(&initial_repo, cand)
                });
                let rendered: Vec<String> = minimal.iter().map(Action::summary).collect();
                let repro = format_repro_test(
                    &format!("repro_incremental_store_tail_{round}"),
                    round_seed,
                    &failure,
                    &rendered,
                );
                panic!(
                    "stateful incremental store tail check failed (round={round}, seed={round_seed:#x}, \
                     shrunk {} -> {} steps):\n{failure}\n\n{repro}\n{}",
                    history.len(),
                    minimal.len(),
                    health.probe_report("stateful_incremental_store")
                );
            }
        }
    }

    eprintln!(
        "{}elapsed={:.2}s (seed={base_seed:#x}, rounds={rounds}, steps/round={steps_per_round})",
        health.probe_report("stateful_incremental_store"),
        started.elapsed().as_secs_f64()
    );

    health.assert_actions_exercised(&ALL_ACTIONS);
    assert_eq!(
        health.total_failed(),
        0,
        "unexpected recorded oracle failures"
    );
}

// ---------------------------------------------------------------------------
// Test 2: Minimizer Self-Check on Injected Synthetic Bug
// ---------------------------------------------------------------------------

#[test]
fn minimizer_shrinks_synthetic_bug_to_minimal_sequence() {
    #[derive(Clone, Debug, PartialEq, Eq)]
    enum SynthAction {
        Noise(usize),
        TriggerA,
        TriggerB,
    }

    let mut seq = Vec::new();
    for i in 0..8 {
        seq.push(SynthAction::Noise(i));
    }
    seq.push(SynthAction::TriggerA);
    for i in 8..16 {
        seq.push(SynthAction::Noise(i));
    }
    seq.push(SynthAction::TriggerB);
    for i in 16..22 {
        seq.push(SynthAction::Noise(i));
    }

    let run = |actions: &[SynthAction]| -> Option<CheckFailure> {
        let mut saw_a = false;
        for a in actions {
            match a {
                SynthAction::TriggerA => saw_a = true,
                SynthAction::TriggerB if saw_a => {
                    return Some(CheckFailure::new(
                        "synthetic_pair",
                        "TriggerB",
                        "TriggerB after TriggerA triggered synthetic fault",
                    ));
                }
                _ => {}
            }
        }
        None
    };

    let failure = run(&seq).expect("full sequence must reproduce synthetic bug");
    let shrunk = minimize_sequence(&seq, &failure.signature(), 200, run);
    assert_eq!(
        shrunk,
        vec![SynthAction::TriggerA, SynthAction::TriggerB],
        "minimizer must shrink 24-step sequence to the exact 2-step root cause"
    );

    let rendered: Vec<String> = shrunk.iter().map(|a| format!("{a:?}")).collect();
    let repro = format_repro_test("repro_synthetic_pair", 42, &failure, &rendered);
    assert!(repro.contains("#[test]"));
    assert!(repro.contains("fn repro_synthetic_pair()"));
    assert!(repro.contains("synthetic_pair|TriggerB"));
    assert!(repro.contains("TriggerA"));
    assert!(repro.contains("TriggerB"));
}

// ---------------------------------------------------------------------------
// Test 3: CsrGraphReader Byte-Level & Structured Mutation Fuzzer
// ---------------------------------------------------------------------------

fn put_u32(bytes: &mut [u8], at: usize, value: u32) {
    if at + 4 <= bytes.len() {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
}

fn put_u64(bytes: &mut [u8], at: usize, value: u64) {
    if at + 8 <= bytes.len() {
        bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
    }
}

fn exercise_csr_reader(reader: &CsrGraphReader) {
    let n = reader.node_count();
    let m = reader.edge_count();
    let _ = reader.generation();
    assert!(
        reader.ensure_unchanged().is_ok(),
        "in-memory or untouched mmap reader must report unchanged"
    );
    let sample_nodes = n.min(256) as u32;
    for u in 0..sample_nodes {
        let node = reader.node(u);
        assert!(node.is_some(), "node({u}) missing for node_count={n}");
        let edges = reader.outgoing_edges(u);
        let attrs = reader.edge_attributes(u);
        assert_eq!(
            edges.len(),
            attrs.len(),
            "outgoing_edges and edge_attributes length mismatch for node {u}"
        );
        assert!(
            edges.len() <= m,
            "node {u} outgoing degree {} exceeds edge_count {m}",
            edges.len()
        );
    }
    assert!(reader.node(u32::MAX).is_none());
    assert!(reader.outgoing_edges(u32::MAX).is_empty());
    assert!(reader.edge_attributes(u32::MAX).is_empty());
    if n > 0 {
        let _ = reader.bfs_reachability(0, 4);
    }
}

struct RandomCsr {
    bytes: Vec<u8>,
    generation: u64,
    node_count: usize,
    expected_edges: Vec<Vec<(u32, u16)>>,
}

fn build_random_csr(src: &mut Source) -> RandomCsr {
    let generation = src.u64();
    let n = 1 + src.usize_below(24);
    let mut writer = CsrGraphWriter::new(generation);
    let mut expected_edges: Vec<Vec<(u32, u16)>> = vec![Vec::new(); n];

    for i in 0..n {
        let flags = if i == 0 {
            CsrNodeRecord::FLAG_ENTRYPOINT
        } else {
            (src.u8() as u16) & 0x0f
        };
        writer.add_node(CsrNodeRecord {
            node_id: i as u32,
            kind: (src.u8() as u16) % 8,
            flags,
            out_degree: 0,
            _reserved: 0,
            name_hash: src.u64(),
            file_path_hash: src.u64(),
        });
    }

    let edge_draws = src.usize_below(n * 3 + 1);
    for _ in 0..edge_draws {
        let u = src.usize_below(n) as u32;
        let v = src.usize_below(n) as u32;
        let attr = (src.u32() & 0xffff) as u16;
        writer.add_edge(u, v, attr);
        expected_edges[u as usize].push((v, attr));
    }
    for adj in &mut expected_edges {
        adj.sort_by_key(|(target, _)| *target);
    }

    RandomCsr {
        bytes: writer.build(),
        generation,
        node_count: n,
        expected_edges,
    }
}

#[test]
fn csr_reader_byte_and_structured_fuzz() {
    let started = Instant::now();
    let seed = env_u64("DEVMAP_STATEFUL_SEED", DEFAULT_SEED ^ 0xC5A0_F022);
    let rounds = env_usize("DEVMAP_CSR_FUZZ_ROUNDS", DEFAULT_CSR_ROUNDS);
    let mut src = Source::from_seed(seed);
    let mut health = HealthTracker::new();

    // Header field byte offsets for `#[repr(C)] CsrHeader`.
    const OFF_VERSION: usize = 8;
    const OFF_NODE_COUNT: usize = 24;
    const OFF_EDGE_COUNT: usize = 32;
    const OFF_NODES: usize = 40;
    const OFF_ROWS: usize = 48;
    const OFF_COLS: usize = 56;
    const OFF_ATTRS: usize = 64;
    let header_len = std::mem::size_of::<CsrHeader>();

    let mmap_dir = TempDirGuard::new("csr-mmap");

    for round in 0..rounds {
        let RandomCsr {
            bytes: valid_bytes,
            generation,
            node_count: n,
            expected_edges,
        } = build_random_csr(&mut src);

        // 1. Valid round-trip equivalence (in-memory + occasional mmap check).
        let reader = CsrGraphReader::from_bytes(valid_bytes.clone())
            .expect("CsrGraphWriter output must always parse cleanly");
        assert_eq!(reader.generation(), generation);
        assert_eq!(reader.node_count(), n);
        let expected_edge_total: usize = expected_edges.iter().map(Vec::len).sum();
        assert_eq!(reader.edge_count(), expected_edge_total);
        for (u, adj) in expected_edges.iter().enumerate() {
            let got_targets = reader.outgoing_edges(u as u32);
            let got_attrs = reader.edge_attributes(u as u32);
            let exp_targets: Vec<u32> = adj.iter().map(|(t, _)| *t).collect();
            let exp_attrs: Vec<u16> = adj.iter().map(|(_, a)| *a).collect();
            assert_eq!(got_targets, exp_targets.as_slice());
            assert_eq!(got_attrs, exp_attrs.as_slice());
        }
        exercise_csr_reader(&reader);
        health.record("CsrValidRoundTrip", None, &StepOutcome::Ok);

        if round % 32 == 0 {
            let path = mmap_dir.path.join(format!("g_{round}.csr"));
            fs::write(&path, &valid_bytes).expect("write csr fixture");
            let mapped = CsrGraphReader::open_mmap(&path).expect("open_mmap valid csr");
            assert_eq!(mapped.node_count(), n);
            assert_eq!(mapped.edge_count(), expected_edge_total);
            exercise_csr_reader(&mapped);
            health.record("CsrValidMmap", None, &StepOutcome::Ok);
        }

        // 2. Structured & unstructured hostile mutations.
        let mut mutated = valid_bytes;
        let kind = src.usize_below(9);
        let action_name = match kind {
            // Corrupt magic bytes.
            0 => {
                mutated[0] ^= 0xff;
                "CsrMutateMagic"
            }
            // Corrupt version.
            1 => {
                put_u32(
                    &mut mutated,
                    OFF_VERSION,
                    DC_GRAPH_VERSION.wrapping_add(1 + src.u32() % 7),
                );
                "CsrMutateVersion"
            }
            // Wrapping / overflowing node_count or edge_count.
            2 => {
                let extreme = [1u64 << 61, 1u64 << 63, u64::MAX, u64::MAX - 1];
                let val = *src.choose(&extreme).unwrap_or(&u64::MAX);
                if src.bool() {
                    put_u64(&mut mutated, OFF_NODE_COUNT, val);
                } else {
                    put_u64(&mut mutated, OFF_EDGE_COUNT, val);
                }
                "CsrMutateOverflowCount"
            }
            // Overlapping header, misaligned, or out-of-bounds section offsets.
            3 => {
                let fields = [OFF_NODES, OFF_ROWS, OFF_COLS, OFF_ATTRS];
                let field = *src.choose(&fields).unwrap_or(&OFF_ROWS);
                let bad_offset = match src.usize_below(4) {
                    0 => src.usize_below(header_len) as u64, // overlaps header
                    1 => (header_len + 1 + (src.usize_below(3) * 2)) as u64, // misaligned
                    2 => (mutated.len() + 8) as u64,         // past EOF
                    _ => u64::MAX - (src.usize_below(16) as u64), // wrapping add
                };
                put_u64(&mut mutated, field, bad_offset);
                "CsrMutateSectionOffset"
            }
            // Corrupt row_offsets boundaries (non-monotonic / past edge_count / bad end).
            4 => {
                if mutated.len() >= OFF_ROWS + 8 {
                    let rows_off =
                        u64::from_le_bytes(mutated[OFF_ROWS..OFF_ROWS + 8].try_into().unwrap())
                            as usize;
                    let slot = src.usize_below(n + 1);
                    let at = rows_off + slot * 8;
                    let bogus = match src.usize_below(3) {
                        0 => (expected_edge_total as u64).saturating_add(1),
                        1 => u64::MAX,
                        _ => 1,
                    };
                    put_u64(&mut mutated, at, bogus);
                }
                "CsrMutateRowBoundary"
            }
            // Truncate to arbitrary prefix.
            5 => {
                let cut = src.usize_below(mutated.len());
                mutated.truncate(cut);
                "CsrMutateTruncate"
            }
            // Random multi-byte bit flips.
            6 => {
                let flips = 1 + src.usize_below(6);
                for _ in 0..flips {
                    if !mutated.is_empty() {
                        let at = src.usize_below(mutated.len());
                        mutated[at] ^= 1u8 << (src.usize_below(8));
                    }
                }
                "CsrMutateBitFlip"
            }
            // Pure random bytes (including empty and short headers).
            7 => {
                mutated = src.bytes(256);
                if src.chance(1, 4) && mutated.len() >= DC_GRAPH_MAGIC.len() {
                    mutated[..8].copy_from_slice(DC_GRAPH_MAGIC);
                    put_u32(&mut mutated, OFF_VERSION, DC_GRAPH_VERSION);
                }
                "CsrMutateRandomBytes"
            }
            // Append trailing garbage or splice extra bytes.
            _ => {
                let extra = src.bytes(64);
                mutated.extend_from_slice(&extra);
                "CsrMutateExtend"
            }
        };

        match CsrGraphReader::from_bytes(mutated) {
            Ok(accepted) => {
                // Any accepted buffer MUST satisfy all safety & accessor invariants.
                exercise_csr_reader(&accepted);
            }
            Err(err) => {
                assert!(
                    !err.to_string().is_empty(),
                    "{action_name} returned empty error message"
                );
            }
        }
        health.record(action_name, None, &StepOutcome::Ok);
    }

    eprintln!(
        "{}elapsed={:.2}s (seed={seed:#x}, rounds={rounds})",
        health.probe_report("csr_reader_fuzz"),
        started.elapsed().as_secs_f64()
    );

    health.assert_actions_exercised(&[
        "CsrValidRoundTrip",
        "CsrValidMmap",
        "CsrMutateMagic",
        "CsrMutateVersion",
        "CsrMutateOverflowCount",
        "CsrMutateSectionOffset",
        "CsrMutateRowBoundary",
        "CsrMutateTruncate",
        "CsrMutateBitFlip",
        "CsrMutateRandomBytes",
        "CsrMutateExtend",
    ]);
}
