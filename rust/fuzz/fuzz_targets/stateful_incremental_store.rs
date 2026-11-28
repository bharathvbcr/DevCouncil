#![no_main]

use std::collections::{BTreeMap, BTreeSet};

use devmap_analyze::analyze;
use devmap_extract::{extract_file, Extraction};
use devmap_resolve::resolver::ReusedFileResolution;
use devmap_resolve::{ResolutionLimits, Resolver};
use devmap_store::{GenerationWriteOpts, Store};
use libfuzzer_sys::fuzz_target;

#[allow(dead_code)]
mod stateful_fuzz {
    include!("../../testsupport/stateful_fuzz.rs");
}

use stateful_fuzz::{
    bail, check, generate_micro_repo, lower_damage, mutate_file_elements, CheckFailure,
    HealthTracker, MicroLang, MicroRepo, Source, StepOutcome,
};

const ACTIONS: &[&str] = &[
    "create_file",
    "edit_valid",
    "edit_broken",
    "toggle_gap_file",
    "delete_file",
    "save_full_reuse",
    "save_partial_daemon",
    "prune_generations",
    "check_cold_oracle",
];

const MICRO_LANGS: &[MicroLang] = &[
    MicroLang::Rust,
    MicroLang::TypeScript,
    MicroLang::Go,
    MicroLang::Python,
];

const PSD1_PATH: &str = "scripts/manifest.psd1";
const PSD1_BODY: &str = "@{ ModuleVersion = '1.0' }\n";
const PROTO_PATH: &str = "proto/mod.proto";
const PROTO_WITH_MSG: &str = "message User {\n  string id = 1;\n}\n";
const PROTO_EMPTY: &str = "// empty proto\n";

fn micro_lang_for_path(path: &str) -> Option<MicroLang> {
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

fn symbol_names_of(extractions: &[Extraction]) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for ext in extractions {
        let entry = out.entry(ext.file_path.clone()).or_default();
        for sym in &ext.symbols {
            entry.insert(sym.name.clone());
        }
    }
    out
}

#[derive(Debug, PartialEq, Eq)]
struct CanonicalSnapshot {
    file_hashes: BTreeMap<String, u64>,
    symbols_by_file: BTreeMap<String, BTreeSet<String>>,
    edges: Vec<String>,
    dead_symbols: Vec<(String, String, bool, u32)>,
    gap_counts: (usize, usize, usize, usize, usize),
    edge_confidence_mismatches: usize,
}

fn snapshot_store(store: &Store) -> Result<CanonicalSnapshot, String> {
    let file_hashes = store
        .latest_file_hashes()
        .map_err(|e| format!("latest_file_hashes: {e}"))?;
    let symbols_by_file = store
        .latest_symbol_names_by_file()
        .map_err(|e| format!("latest_symbol_names_by_file: {e}"))?;
    let mut edges = store
        .latest_edges_for_test()
        .map_err(|e| format!("latest_edges_for_test: {e}"))?;
    edges.sort();
    let mut dead_symbols: Vec<(String, String, bool, u32)> = store
        .latest_dead_symbols()
        .map_err(|e| format!("latest_dead_symbols: {e}"))?
        .into_iter()
        .map(|d| {
            (
                d.file_path,
                d.symbol_name,
                d.is_exempt,
                d.confidence.to_bits(),
            )
        })
        .collect();
    dead_symbols.sort();
    let status = store
        .status(":memory:")
        .map_err(|e| format!("status: {e}"))?;
    let gaps = &status.coverage_gaps;
    let gap_counts = (
        gaps.parse_failed.total,
        gaps.pattern_recovered.total,
        gaps.call_blind.total,
        gaps.import_blind.total,
        gaps.not_parsed.total,
    );
    let edge_confidence_mismatches = store
        .edge_confidence_mismatches()
        .map_err(|e| format!("edge_confidence_mismatches: {e}"))?
        .unwrap_or(0);
    Ok(CanonicalSnapshot {
        file_hashes,
        symbols_by_file,
        edges,
        dead_symbols,
        gap_counts,
        edge_confidence_mismatches,
    })
}

fn build_cold_store(repo: &MicroRepo) -> Result<Store, String> {
    let extractions: Vec<Extraction> = repo
        .iter()
        .map(|(path, body)| extract_file(path, body))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver
        .resolve_all(&extractions)
        .map_err(|e| format!("cold resolve_all: {e}"))?;
    let analysis = analyze(&extractions, &resolution);
    let cold = Store::open_in_memory().map_err(|e| format!("open cold store: {e}"))?;
    cold.save_generation(&extractions, &resolution, &analysis)
        .map_err(|e| format!("save cold generation: {e}"))?;
    Ok(cold)
}

fn check_equivalence(action: &'static str, store: &Store, repo: &MicroRepo) -> StepOutcome {
    let cold = match build_cold_store(repo) {
        Ok(c) => c,
        Err(detail) => {
            return StepOutcome::CheckFailed(CheckFailure::new("cold_oracle_build", action, detail))
        }
    };
    let inc_snap = match snapshot_store(store) {
        Ok(s) => s,
        Err(detail) => {
            return StepOutcome::CheckFailed(CheckFailure::new("inc_snapshot", action, detail))
        }
    };
    let cold_snap = match snapshot_store(&cold) {
        Ok(s) => s,
        Err(detail) => {
            return StepOutcome::CheckFailed(CheckFailure::new("cold_snapshot", action, detail))
        }
    };

    let c0 = check(
        inc_snap.edge_confidence_mismatches == 0,
        "zero_edge_confidence_mismatches",
        action,
        format!(
            "incremental store reported {} edge_confidence_mismatches",
            inc_snap.edge_confidence_mismatches
        ),
    );
    if !c0.is_ok() {
        return c0;
    }

    let c1 = check(
        inc_snap.file_hashes == cold_snap.file_hashes,
        "cold_file_hashes_match",
        action,
        format!(
            "file_hashes mismatch:\ninc={:?}\ncold={:?}",
            inc_snap.file_hashes, cold_snap.file_hashes
        ),
    );
    if !c1.is_ok() {
        return c1;
    }

    let c2 = check(
        inc_snap.symbols_by_file == cold_snap.symbols_by_file,
        "cold_symbols_match",
        action,
        format!(
            "symbols_by_file mismatch:\ninc={:?}\ncold={:?}",
            inc_snap.symbols_by_file, cold_snap.symbols_by_file
        ),
    );
    if !c2.is_ok() {
        return c2;
    }

    let c3 = check(
        inc_snap.edges == cold_snap.edges,
        "cold_edges_match",
        action,
        format!(
            "edges mismatch (inc={} vs cold={})",
            inc_snap.edges.len(),
            cold_snap.edges.len()
        ),
    );
    if !c3.is_ok() {
        return c3;
    }

    let c4 = check(
        inc_snap.gap_counts == cold_snap.gap_counts,
        "cold_coverage_gaps_match",
        action,
        format!(
            "gap_counts mismatch: inc={:?} vs cold={:?}",
            inc_snap.gap_counts, cold_snap.gap_counts
        ),
    );
    if !c4.is_ok() {
        return c4;
    }

    let c5 = check(
        inc_snap.dead_symbols == cold_snap.dead_symbols,
        "cold_dead_symbols_match",
        action,
        format!(
            "dead_symbols mismatch:\ninc={:?}\ncold={:?}",
            inc_snap.dead_symbols, cold_snap.dead_symbols
        ),
    );
    if !c5.is_ok() {
        return c5;
    }

    // Spot-check FTS search parity on a live simple-identifier symbol name.
    if let Some(sample_name) = cold_snap
        .symbols_by_file
        .values()
        .flat_map(|names| names.iter())
        .find(|n| {
            !n.is_empty()
                && n.len() <= 64
                && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
    {
        let inc_hits: BTreeSet<(String, String)> = match store.search_symbols(sample_name, 64) {
            Ok(rows) => rows
                .into_iter()
                .map(|r| (r.path, r.qualified_name))
                .collect(),
            Err(e) => {
                return StepOutcome::CheckFailed(CheckFailure::new(
                    "fts_search_ok",
                    action,
                    format!("incremental search_symbols({sample_name:?}): {e}"),
                ))
            }
        };
        let cold_hits: BTreeSet<(String, String)> = match cold.search_symbols(sample_name, 64) {
            Ok(rows) => rows
                .into_iter()
                .map(|r| (r.path, r.qualified_name))
                .collect(),
            Err(e) => {
                return StepOutcome::CheckFailed(CheckFailure::new(
                    "fts_search_ok",
                    action,
                    format!("cold search_symbols({sample_name:?}): {e}"),
                ))
            }
        };
        let c6 = check(
            inc_hits == cold_hits,
            "cold_fts_search_match",
            action,
            format!("FTS search({sample_name:?}) mismatch:\ninc={inc_hits:?}\ncold={cold_hits:?}"),
        );
        if !c6.is_ok() {
            return c6;
        }
    }

    StepOutcome::Ok
}

struct FuzzState {
    store: Store,
    repo: MicroRepo,
    pending_affected: BTreeSet<String>,
    pending_deleted: BTreeSet<String>,
    reuse_cache: BTreeMap<String, ReusedFileResolution>,
    last_symbol_names: BTreeMap<String, BTreeSet<String>>,
    saved_once: bool,
    next_file_id: usize,
    trace: Vec<String>,
}

impl FuzzState {
    fn save_generation(&mut self, partial_daemon_shape: bool, action: &'static str) -> StepOutcome {
        let all_extractions: Vec<Extraction> = self
            .repo
            .iter()
            .map(|(path, body)| extract_file(path, body))
            .collect();
        let current_names = symbol_names_of(&all_extractions);
        let decls_unchanged = self.saved_once && current_names == self.last_symbol_names;

        let mut resolver = Resolver::new();
        resolver.index_extractions(&all_extractions);

        let resolution = if decls_unchanged && !self.reuse_cache.is_empty() {
            match resolver.resolve_all_with_limits_reusing(
                &all_extractions,
                ResolutionLimits::default(),
                &self.reuse_cache,
            ) {
                Ok(reused) => {
                    for (path, entry) in reused.produced {
                        self.reuse_cache.insert(path, entry);
                    }
                    self.reuse_cache.retain(|p, _| self.repo.contains_key(p));
                    reused.result
                }
                Err(e) => {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "resolve_reuse_ok",
                        action,
                        format!("resolve_all_with_limits_reusing failed: {e}"),
                    ))
                }
            }
        } else {
            match resolver.resolve_all_with_limits_reusing(
                &all_extractions,
                ResolutionLimits::default(),
                &BTreeMap::new(),
            ) {
                Ok(fresh) => {
                    self.reuse_cache = fresh.produced;
                    fresh.result
                }
                Err(e) => {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "resolve_full_ok",
                        action,
                        format!("resolve_all failed: {e}"),
                    ))
                }
            }
        };

        let analysis = analyze(&all_extractions, &resolution);

        let save_res = if !self.saved_once {
            self.store
                .save_generation(&all_extractions, &resolution, &analysis)
        } else {
            let affected: Vec<String> = self.pending_affected.iter().cloned().collect();
            let deleted: Vec<String> = self.pending_deleted.iter().cloned().collect();
            let opts = GenerationWriteOpts {
                affected_paths: affected.clone(),
                deleted_paths: deleted,
                ..Default::default()
            };
            if partial_daemon_shape {
                let fresh: Vec<Extraction> = all_extractions
                    .iter()
                    .filter(|ext| self.pending_affected.contains(&ext.file_path))
                    .cloned()
                    .collect();
                self.store
                    .save_generation_with_opts(&fresh, &resolution, &analysis, opts)
            } else {
                self.store
                    .save_generation_with_opts(&all_extractions, &resolution, &analysis, opts)
            }
        };

        if let Err(e) = save_res {
            return StepOutcome::CheckFailed(CheckFailure::new(
                "save_generation_ok",
                action,
                format!("save_generation failed: {e}"),
            ));
        }

        self.saved_once = true;
        self.last_symbol_names = current_names;
        self.pending_affected.clear();
        self.pending_deleted.clear();
        StepOutcome::Ok
    }
}

fuzz_target!(|data: &[u8]| {
    if data.len() < 4 {
        return;
    }

    let mut src = Source::from_bytes(data);
    let initial_files = 2 + src.usize_below(4);
    let repo = generate_micro_repo(&mut src, MICRO_LANGS, initial_files);
    if repo.is_empty() {
        return;
    }

    let store = match Store::open_in_memory() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("DEVMAP_FUZZ_SIG=store_open_ok|init");
            panic!("Store::open_in_memory() failed: {e:#}");
        }
    };

    let mut state = FuzzState {
        store,
        repo,
        pending_affected: BTreeSet::new(),
        pending_deleted: BTreeSet::new(),
        reuse_cache: BTreeMap::new(),
        last_symbol_names: BTreeMap::new(),
        saved_once: false,
        next_file_id: initial_files,
        trace: Vec::new(),
    };

    // Establish initial committed generation.
    if let Some(failure) = state.save_generation(false, "initial_save").into_failure() {
        eprintln!("DEVMAP_FUZZ_SIG={}", failure.signature());
        panic!("{failure}");
    }
    state.trace.push("initial_save".to_string());

    let mut tracker = HealthTracker::new();
    let mut prev: Option<&'static str> = None;

    for _ in 0..14 {
        if src.remaining_bytes() == 0 {
            break;
        }
        let action = tracker.pick_weighted(&mut src, ACTIONS, prev);
        let outcome = match action {
            "create_file" => {
                if state.repo.len() >= 10 {
                    bail("repo_cap")
                } else {
                    let lang = *src.choose(MICRO_LANGS).unwrap_or(&MicroLang::Rust);
                    let id = state.next_file_id;
                    state.next_file_id += 1;
                    let generated = generate_micro_repo(&mut src, &[lang], 1);
                    if let Some((_, body)) = generated.into_iter().next() {
                        let path = format!("extra/f_{id}.{}", lang.ext());
                        state.repo.insert(path.clone(), body);
                        state.pending_deleted.remove(&path);
                        state.pending_affected.insert(path.clone());
                        state.trace.push(format!("create_file({path})"));
                        StepOutcome::Ok
                    } else {
                        bail("empty_gen")
                    }
                }
            }
            "edit_valid" => {
                let candidates: Vec<String> = state
                    .repo
                    .keys()
                    .filter(|p| micro_lang_for_path(p).is_some())
                    .cloned()
                    .collect();
                if let Some(path) = src.choose(&candidates).cloned() {
                    let lang = micro_lang_for_path(&path).unwrap();
                    let cur = state.repo.get(&path).cloned().unwrap_or_default();
                    let next = mutate_file_elements(&mut src, lang, &cur);
                    state.repo.insert(path.clone(), next);
                    state.pending_deleted.remove(&path);
                    state.pending_affected.insert(path.clone());
                    state.trace.push(format!("edit_valid({path})"));
                    StepOutcome::Ok
                } else {
                    bail("no_micro_file")
                }
            }
            "edit_broken" => {
                let candidates: Vec<String> = state
                    .repo
                    .keys()
                    .filter(|p| micro_lang_for_path(p).is_some())
                    .cloned()
                    .collect();
                if let Some(path) = src.choose(&candidates).cloned() {
                    let cur = state.repo.get(&path).cloned().unwrap_or_default();
                    let next = lower_damage(&mut src, &cur);
                    state.repo.insert(path.clone(), next);
                    state.pending_deleted.remove(&path);
                    state.pending_affected.insert(path.clone());
                    state.trace.push(format!("edit_broken({path})"));
                    StepOutcome::Ok
                } else {
                    bail("no_micro_file")
                }
            }
            "toggle_gap_file" => {
                if src.bool() {
                    if state.repo.contains_key(PSD1_PATH) {
                        state.repo.remove(PSD1_PATH);
                        state.pending_affected.remove(PSD1_PATH);
                        state.pending_deleted.insert(PSD1_PATH.to_string());
                        state.trace.push("delete_gap(psd1)".to_string());
                    } else {
                        state
                            .repo
                            .insert(PSD1_PATH.to_string(), PSD1_BODY.to_string());
                        state.pending_deleted.remove(PSD1_PATH);
                        state.pending_affected.insert(PSD1_PATH.to_string());
                        state.trace.push("add_gap(psd1)".to_string());
                    }
                } else {
                    let next =
                        if state.repo.get(PROTO_PATH).map(String::as_str) == Some(PROTO_WITH_MSG) {
                            PROTO_EMPTY
                        } else {
                            PROTO_WITH_MSG
                        };
                    state.repo.insert(PROTO_PATH.to_string(), next.to_string());
                    state.pending_deleted.remove(PROTO_PATH);
                    state.pending_affected.insert(PROTO_PATH.to_string());
                    state.trace.push("toggle_gap(proto)".to_string());
                }
                StepOutcome::Ok
            }
            "delete_file" => {
                if state.repo.len() <= 1 {
                    bail("keep_last_file")
                } else {
                    let keys: Vec<String> = state.repo.keys().cloned().collect();
                    let path = src.choose(&keys).cloned().unwrap();
                    state.repo.remove(&path);
                    state.pending_affected.remove(&path);
                    state.pending_deleted.insert(path.clone());
                    state.trace.push(format!("delete_file({path})"));
                    StepOutcome::Ok
                }
            }
            "save_full_reuse" => {
                state.trace.push("save_full_reuse".to_string());
                state.save_generation(false, "save_full_reuse")
            }
            "save_partial_daemon" => {
                if state.pending_affected.is_empty() && state.pending_deleted.is_empty() {
                    bail("no_pending_delta")
                } else {
                    state.trace.push("save_partial_daemon".to_string());
                    state.save_generation(true, "save_partial_daemon")
                }
            }
            "prune_generations" => {
                let keep = 1 + src.usize_below(3);
                state.trace.push(format!("prune_generations({keep})"));
                match state.store.prune_generations_except_latest(keep) {
                    Ok(_) => StepOutcome::Ok,
                    Err(e) => StepOutcome::CheckFailed(CheckFailure::new(
                        "prune_ok",
                        "prune_generations",
                        format!("prune_generations_except_latest({keep}) failed: {e}"),
                    )),
                }
            }
            "check_cold_oracle" => {
                if !state.pending_affected.is_empty() || !state.pending_deleted.is_empty() {
                    let saved = state.save_generation(src.bool(), "check_cold_oracle_flush");
                    if !saved.is_ok() {
                        saved
                    } else {
                        state.trace.push("check_cold_oracle".to_string());
                        check_equivalence("check_cold_oracle", &state.store, &state.repo)
                    }
                } else {
                    state.trace.push("check_cold_oracle".to_string());
                    check_equivalence("check_cold_oracle", &state.store, &state.repo)
                }
            }
            _ => bail("unknown_action"),
        };

        tracker.record(action, prev, &outcome);
        if outcome.is_ok() {
            prev = Some(action);
        }
        if let Some(failure) = outcome.into_failure() {
            eprintln!("DEVMAP_FUZZ_SIG={}", failure.signature());
            eprintln!("trace={:?}", state.trace);
            panic!("{failure}");
        }
    }

    // Always flush any remaining dirty edits and verify final cold equivalence.
    if !state.pending_affected.is_empty() || !state.pending_deleted.is_empty() {
        if let Some(failure) = state.save_generation(false, "final_flush").into_failure() {
            eprintln!("DEVMAP_FUZZ_SIG={}", failure.signature());
            eprintln!("trace={:?}", state.trace);
            panic!("{failure}");
        }
    }
    if let Some(failure) =
        check_equivalence("final_cold_oracle", &state.store, &state.repo).into_failure()
    {
        eprintln!("DEVMAP_FUZZ_SIG={}", failure.signature());
        eprintln!("trace={:?}", state.trace);
        panic!("{failure}");
    }
});
