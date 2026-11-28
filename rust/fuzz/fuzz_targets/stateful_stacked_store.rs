#![no_main]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

use devmap_analyze::analyze;
use devmap_extract::paths::{stack_delta_path, store_path};
use devmap_extract::{extract_file, Extraction};
use devmap_query::{provision_stacked_worktree, Cancel, StackedStore};
use devmap_resolve::Resolver;
use devmap_store::{DeltaStore, GenerationWriteOpts, Store};
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
    "upsert_new",
    "edit_existing",
    "tombstone_file",
    "revert_base_file",
    "reopen_stacked",
    "check_flat_oracle",
];

const MICRO_LANGS: &[MicroLang] = &[
    MicroLang::Rust,
    MicroLang::TypeScript,
    MicroLang::Go,
    MicroLang::Python,
];

struct StackedFixture {
    _root: PathBuf,
    lane: PathBuf,
    base_files: MicroRepo,
}

static FIXTURE: OnceLock<Mutex<StackedFixture>> = OnceLock::new();

fn git(cwd: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Fuzz")
        .env("GIT_AUTHOR_EMAIL", "fuzz@example.com")
        .env("GIT_COMMITTER_NAME", "Fuzz")
        .env("GIT_COMMITTER_EMAIL", "fuzz@example.com")
        .output()
        .expect("git must be available to initialize stacked fuzz fixture");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout)
        .expect("utf8 git output")
        .trim()
        .to_string()
}

fn init_fixture() -> StackedFixture {
    let root = std::env::temp_dir().join(format!("devmap-fuzz-stacked-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let anchor = root.join("anchor");
    fs::create_dir_all(&anchor).expect("create anchor dir");

    let mut seed_src = Source::from_seed(0x57AC_ED00_0000_0001);
    let base_files = generate_micro_repo(&mut seed_src, MICRO_LANGS, 4);
    for (rel, body) in &base_files {
        let full = anchor.join(rel);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).expect("create parent dir");
        }
        fs::write(&full, body).expect("write base file");
    }
    fs::write(anchor.join(".gitignore"), ".devmap/\n.devcouncil/\n").expect("write gitignore");

    git(&anchor, &["init", "-b", "main"]);
    git(&anchor, &["add", "."]);
    git(&anchor, &["commit", "-m", "base"]);
    let head = git(&anchor, &["rev-parse", "HEAD"]);

    let db_path = store_path(&anchor);
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent).expect("create store parent");
    }
    let store = Store::open(&db_path).expect("open anchor store");
    store
        .bind_repo_root(&anchor.canonicalize().expect("canonicalize anchor"))
        .expect("bind anchor repo root");
    let exts: Vec<Extraction> = base_files
        .iter()
        .map(|(rel, body)| extract_file(rel, body))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&exts);
    let res = resolver.resolve_all(&exts).expect("resolve anchor base");
    let analysis = analyze(&exts, &res);
    store
        .save_generation_with_metadata(
            &exts,
            &res,
            &analysis,
            GenerationWriteOpts::default(),
            &head,
        )
        .expect("save anchor base generation");
    let _ = store.write_latest_csr_sidecar().expect("write anchor CSR");
    drop(store);

    let lane = root.join("lane");
    git(
        &anchor,
        &["worktree", "add", "-b", "lane", lane.to_str().unwrap()],
    );
    provision_stacked_worktree(&anchor, &lane).expect("provision stacked worktree");

    StackedFixture {
        _root: root,
        lane,
        base_files,
    }
}

fn reset_lane_delta(lane: &Path) {
    let delta = stack_delta_path(lane);
    let _ = fs::remove_file(&delta);
    for suffix in ["-journal", "-wal", "-shm"] {
        let sidecar = PathBuf::from(format!("{}{suffix}", delta.display()));
        let _ = fs::remove_file(sidecar);
    }
}

fn micro_lang_for_path(path: &str) -> MicroLang {
    if path.ends_with(".rs") {
        MicroLang::Rust
    } else if path.ends_with(".ts") {
        MicroLang::TypeScript
    } else if path.ends_with(".go") {
        MicroLang::Go
    } else {
        MicroLang::Python
    }
}

type CanonicalSymbol = (String, String, String, String, usize, usize, bool);

fn canonical_symbols_from_rows(rows: Vec<devmap_store::StoredSymbol>) -> Vec<CanonicalSymbol> {
    let mut out: Vec<CanonicalSymbol> = rows
        .into_iter()
        .map(|s| {
            (
                s.path,
                s.qualified_name,
                s.name,
                s.kind,
                s.span_start,
                s.span_end,
                s.is_exported,
            )
        })
        .collect();
    out.sort();
    out
}

fn build_cold_flat_store(live_files: &MicroRepo) -> Result<Store, String> {
    let exts: Vec<Extraction> = live_files
        .iter()
        .map(|(path, body)| extract_file(path, body))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&exts);
    let res = resolver
        .resolve_all(&exts)
        .map_err(|e| format!("cold flat resolve_all: {e}"))?;
    let analysis = analyze(&exts, &res);
    let cold = Store::open_in_memory().map_err(|e| format!("open cold flat store: {e}"))?;
    cold.save_generation(&exts, &res, &analysis)
        .map_err(|e| format!("save cold flat generation: {e}"))?;
    Ok(cold)
}

fn check_flat_oracle(
    action: &'static str,
    stacked: &StackedStore,
    base_files: &MicroRepo,
    live_files: &MicroRepo,
    sample_path: Option<&str>,
) -> StepOutcome {
    let cold = match build_cold_flat_store(live_files) {
        Ok(c) => c,
        Err(detail) => {
            return StepOutcome::CheckFailed(CheckFailure::new("cold_flat_build", action, detail))
        }
    };

    // 1. Indexed file inventory parity.
    let stacked_files: BTreeMap<String, String> = match stacked.all_indexed_files() {
        Ok(Some((_, _, files))) => files.into_iter().collect(),
        Ok(None) => BTreeMap::new(),
        Err(e) => {
            return StepOutcome::CheckFailed(CheckFailure::new(
                "stacked_indexed_files_ok",
                action,
                format!("all_indexed_files failed: {e}"),
            ))
        }
    };
    let cold_files: BTreeMap<String, String> = match cold.latest_scope_inputs() {
        Ok(Some((_, _, files))) => files.into_iter().collect(),
        Ok(None) => BTreeMap::new(),
        Err(e) => {
            return StepOutcome::CheckFailed(CheckFailure::new(
                "cold_indexed_files_ok",
                action,
                format!("latest_scope_inputs failed: {e}"),
            ))
        }
    };
    let c1 = check(
        stacked_files == cold_files,
        "stacked_indexed_files_parity",
        action,
        format!("indexed files mismatch:\nstacked={stacked_files:?}\ncold={cold_files:?}"),
    );
    if !c1.is_ok() {
        return c1;
    }

    // 2. Merged symbol table parity.
    let stacked_syms = match stacked.all_merged_symbols() {
        Ok(Some(page)) => canonical_symbols_from_rows(page.rows),
        Ok(None) => Vec::new(),
        Err(e) => {
            return StepOutcome::CheckFailed(CheckFailure::new(
                "stacked_merged_symbols_ok",
                action,
                format!("all_merged_symbols failed: {e}"),
            ))
        }
    };
    let cold_syms = match cold.all_symbols_page() {
        Ok(Some(page)) => canonical_symbols_from_rows(page.rows),
        Ok(None) => Vec::new(),
        Err(e) => {
            return StepOutcome::CheckFailed(CheckFailure::new(
                "cold_symbols_ok",
                action,
                format!("all_symbols_page failed: {e}"),
            ))
        }
    };
    let c2 = check(
        stacked_syms == cold_syms,
        "stacked_merged_symbols_parity",
        action,
        format!(
            "merged symbols mismatch (stacked={} vs cold={})",
            stacked_syms.len(),
            cold_syms.len()
        ),
    );
    if !c2.is_ok() {
        return c2;
    }

    // 3. Per-file symbols parity + tombstoned base files return zero symbols.
    let tombstoned: BTreeSet<&str> = base_files
        .keys()
        .filter(|p| !live_files.contains_key(*p))
        .map(String::as_str)
        .collect();
    for &gone in &tombstoned {
        match stacked.symbols_for_file(gone) {
            Ok(rows) => {
                let c = check(
                    rows.is_empty(),
                    "tombstoned_file_empty",
                    action,
                    format!(
                        "tombstoned file {gone} still returned {} symbol(s)",
                        rows.len()
                    ),
                );
                if !c.is_ok() {
                    return c;
                }
            }
            Err(e) => {
                return StepOutcome::CheckFailed(CheckFailure::new(
                    "tombstoned_lookup_ok",
                    action,
                    format!("symbols_for_file({gone}) failed: {e}"),
                ))
            }
        }
    }

    for path in live_files.keys() {
        let st_rows = match stacked.symbols_for_file(path) {
            Ok(rows) => canonical_symbols_from_rows(rows),
            Err(e) => {
                return StepOutcome::CheckFailed(CheckFailure::new(
                    "stacked_file_symbols_ok",
                    action,
                    format!("symbols_for_file({path}) failed: {e}"),
                ))
            }
        };
        let cd_rows = match cold.latest_symbols_for_file(path) {
            Ok(Some(page)) => canonical_symbols_from_rows(page.rows),
            Ok(None) => Vec::new(),
            Err(e) => {
                return StepOutcome::CheckFailed(CheckFailure::new(
                    "cold_file_symbols_ok",
                    action,
                    format!("latest_symbols_for_file({path}) failed: {e}"),
                ))
            }
        };
        let c = check(
            st_rows == cd_rows,
            "stacked_per_file_symbols_parity",
            action,
            format!("symbols_for_file({path}) mismatch:\nstacked={st_rows:?}\ncold={cd_rows:?}"),
        );
        if !c.is_ok() {
            return c;
        }
    }

    // 4. Search returns no tombstoned/ghost symbols and every hit exists in cold store.
    let cold_sym_set: BTreeSet<(String, String)> = cold_syms
        .iter()
        .map(|s| (s.0.clone(), s.1.clone()))
        .collect();
    let search_hits = match stacked.search_symbols("api", 128) {
        Ok(rows) => rows,
        Err(e) => {
            return StepOutcome::CheckFailed(CheckFailure::new(
                "stacked_search_ok",
                action,
                format!("search_symbols failed: {e}"),
            ))
        }
    };
    for hit in search_hits {
        let c = check(
            !tombstoned.contains(hit.path.as_str())
                && cold_sym_set.contains(&(hit.path.clone(), hit.qualified_name.clone())),
            "stacked_search_no_ghost_symbols",
            action,
            format!(
                "search_symbols returned ghost/tombstoned symbol {} in {}",
                hit.qualified_name, hit.path
            ),
        );
        if !c.is_ok() {
            return c;
        }
    }

    // 5. Merged file_edges must never leak tombstoned endpoints and must agree
    //    with cold Contains edges for any live file.
    if let Some(path) = sample_path.filter(|p| live_files.contains_key(*p)) {
        let cancel = Cancel::new();
        let st_edges = match stacked.file_edges(path, 0.0, &cancel) {
            Ok(Some(page)) => page.edges,
            Ok(None) => Vec::new(),
            Err(e) => {
                return StepOutcome::CheckFailed(CheckFailure::new(
                    "stacked_file_edges_ok",
                    action,
                    format!("file_edges({path}) failed: {e}"),
                ))
            }
        };
        for edge in &st_edges {
            let c = check(
                !tombstoned.contains(edge.source_file.as_str())
                    && !tombstoned.contains(edge.target_file.as_str()),
                "stacked_edges_no_tombstone_leak",
                action,
                format!(
                    "file_edges({path}) leaked tombstoned endpoint: {} -> {}",
                    edge.source_file, edge.target_file
                ),
            );
            if !c.is_ok() {
                return c;
            }
        }

        let st_contains: BTreeSet<(String, String)> = st_edges
            .into_iter()
            .filter(|e| e.edge_kind == "Contains")
            .map(|e| (e.source_symbol, e.target_symbol))
            .collect();
        let cd_contains: BTreeSet<(String, String)> = match cold.file_edges(path, 0.0) {
            Ok(Some(page)) => page
                .edges
                .into_iter()
                .filter(|e| e.edge_kind == "Contains")
                .map(|e| (e.source_symbol, e.target_symbol))
                .collect(),
            Ok(None) => BTreeSet::new(),
            Err(e) => {
                return StepOutcome::CheckFailed(CheckFailure::new(
                    "cold_file_edges_ok",
                    action,
                    format!("cold file_edges({path}) failed: {e}"),
                ))
            }
        };
        let c = check(
            st_contains == cd_contains,
            "stacked_contains_edges_parity",
            action,
            format!(
                "Contains edges mismatch for {path}:\nstacked={st_contains:?}\ncold={cd_contains:?}"
            ),
        );
        if !c.is_ok() {
            return c;
        }
    }

    StepOutcome::Ok
}

fuzz_target!(|data: &[u8]| {
    if data.len() < 4 {
        return;
    }

    let fixture_mutex = FIXTURE.get_or_init(|| Mutex::new(init_fixture()));
    let fixture = fixture_mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    reset_lane_delta(&fixture.lane);

    let Ok(mut stacked) = StackedStore::open(&fixture.lane) else {
        return;
    };

    let mut src = Source::from_bytes(data);
    let mut live_files = fixture.base_files.clone();
    let mut next_delta_id = 0usize;
    let mut tracker = HealthTracker::new();
    let mut prev: Option<&'static str> = None;
    let mut trace = Vec::new();

    for _ in 0..12 {
        if src.remaining_bytes() == 0 {
            break;
        }
        let action = tracker.pick_weighted(&mut src, ACTIONS, prev);
        let outcome = match action {
            "upsert_new" => {
                if live_files.len() >= 10 {
                    bail("live_file_cap")
                } else {
                    let lang = *src.choose(MICRO_LANGS).unwrap_or(&MicroLang::Rust);
                    let id = next_delta_id;
                    next_delta_id += 1;
                    let generated = generate_micro_repo(&mut src, &[lang], 1);
                    if let Some((_, body)) = generated.into_iter().next() {
                        let path = format!("delta/d_{id}.{}", lang.ext());
                        let ext = extract_file(&path, &body);
                        match stacked.record_extracted_file(&path, body.as_bytes(), &ext) {
                            Ok(()) => {
                                live_files.insert(path.clone(), body);
                                trace.push(format!("upsert_new({path})"));
                                StepOutcome::Ok
                            }
                            Err(e) => StepOutcome::CheckFailed(CheckFailure::new(
                                "record_extracted_ok",
                                "upsert_new",
                                format!("record_extracted_file({path}) failed: {e}"),
                            )),
                        }
                    } else {
                        bail("empty_gen")
                    }
                }
            }
            "edit_existing" => {
                let keys: Vec<String> = live_files.keys().cloned().collect();
                if let Some(path) = src.choose(&keys).cloned() {
                    let lang = micro_lang_for_path(&path);
                    let cur = live_files.get(&path).cloned().unwrap_or_default();
                    let next = if src.chance(1, 5) {
                        lower_damage(&mut src, &cur)
                    } else {
                        mutate_file_elements(&mut src, lang, &cur)
                    };
                    let ext = extract_file(&path, &next);
                    match stacked.record_extracted_file(&path, next.as_bytes(), &ext) {
                        Ok(()) => {
                            live_files.insert(path.clone(), next);
                            trace.push(format!("edit_existing({path})"));
                            StepOutcome::Ok
                        }
                        Err(e) => StepOutcome::CheckFailed(CheckFailure::new(
                            "record_extracted_ok",
                            "edit_existing",
                            format!("record_extracted_file({path}) failed: {e}"),
                        )),
                    }
                } else {
                    bail("no_live_files")
                }
            }
            "tombstone_file" => {
                if live_files.len() <= 1 {
                    bail("keep_last_file")
                } else {
                    let keys: Vec<String> = live_files.keys().cloned().collect();
                    let path = src.choose(&keys).cloned().unwrap();
                    match stacked.record_deleted_file(&path) {
                        Ok(()) => {
                            live_files.remove(&path);
                            trace.push(format!("tombstone_file({path})"));
                            StepOutcome::Ok
                        }
                        Err(e) => StepOutcome::CheckFailed(CheckFailure::new(
                            "record_deleted_ok",
                            "tombstone_file",
                            format!("record_deleted_file({path}) failed: {e}"),
                        )),
                    }
                }
            }
            "revert_base_file" => {
                let dirty_base: Vec<String> = fixture
                    .base_files
                    .iter()
                    .filter(|(p, orig)| live_files.get(*p) != Some(*orig))
                    .map(|(p, _)| p.clone())
                    .collect();
                if let Some(path) = src.choose(&dirty_base).cloned() {
                    match stacked.clear_reverted_file(&path) {
                        Ok(()) => {
                            let orig = fixture.base_files.get(&path).cloned().unwrap();
                            live_files.insert(path.clone(), orig);
                            trace.push(format!("revert_base_file({path})"));
                            StepOutcome::Ok
                        }
                        Err(e) => StepOutcome::CheckFailed(CheckFailure::new(
                            "clear_reverted_ok",
                            "revert_base_file",
                            format!("clear_reverted_file({path}) failed: {e}"),
                        )),
                    }
                } else {
                    bail("no_dirty_base_file")
                }
            }
            "reopen_stacked" => {
                trace.push("reopen_stacked".to_string());
                let delta_path = stack_delta_path(&fixture.lane);
                let ro_check = DeltaStore::open_read_only(&delta_path).and_then(|opt| {
                    if let Some(delta) = opt {
                        let _ = delta.load_snapshot()?;
                    }
                    Ok(())
                });
                if let Err(e) = ro_check {
                    StepOutcome::CheckFailed(CheckFailure::new(
                        "delta_readonly_reopen_ok",
                        "reopen_stacked",
                        format!("DeltaStore::open_read_only failed: {e}"),
                    ))
                } else {
                    match StackedStore::open(&fixture.lane) {
                        Ok(reopened) => {
                            stacked = reopened;
                            StepOutcome::Ok
                        }
                        Err(e) => StepOutcome::CheckFailed(CheckFailure::new(
                            "stacked_reopen_ok",
                            "reopen_stacked",
                            format!("StackedStore::open failed: {e}"),
                        )),
                    }
                }
            }
            "check_flat_oracle" => {
                let keys: Vec<String> = live_files.keys().cloned().collect();
                let sample = src.choose(&keys).map(String::as_str);
                trace.push("check_flat_oracle".to_string());
                check_flat_oracle(
                    "check_flat_oracle",
                    &stacked,
                    &fixture.base_files,
                    &live_files,
                    sample,
                )
            }
            _ => bail("unknown_action"),
        };

        tracker.record(action, prev, &outcome);
        if outcome.is_ok() {
            prev = Some(action);
        }
        if let Some(failure) = outcome.into_failure() {
            eprintln!("DEVMAP_FUZZ_SIG={}", failure.signature());
            eprintln!("trace={trace:?}");
            panic!("{failure}");
        }
    }

    let first_live = live_files.keys().next().map(String::as_str);
    if let Some(failure) = check_flat_oracle(
        "final_flat_oracle",
        &stacked,
        &fixture.base_files,
        &live_files,
        first_live,
    )
    .into_failure()
    {
        eprintln!("DEVMAP_FUZZ_SIG={}", failure.signature());
        eprintln!("trace={trace:?}");
        panic!("{failure}");
    }
});
