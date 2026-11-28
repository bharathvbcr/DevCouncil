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
    "delete_file",
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

const FIXTURE_DIR_PREFIX: &str = "devmap-fuzz-stacked-";

struct StackedFixture {
    _root: PathBuf,
    lane: PathBuf,
    base_files: MicroRepo,
}

static FIXTURE: OnceLock<Mutex<StackedFixture>> = OnceLock::new();

fn make_tree_writable(path: &Path) {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return;
    };
    if meta.file_type().is_symlink() {
        return;
    }
    if meta.is_dir() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o755));
        }
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                make_tree_writable(&entry.path());
            }
        }
    } else {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o644));
        }
        #[cfg(not(unix))]
        {
            if let Ok(mut perms) = fs::metadata(path).map(|m| m.permissions()) {
                perms.set_readonly(false);
                let _ = fs::set_permissions(path, perms);
            }
        }
    }
}

fn remove_readonly_tree(path: &Path) {
    if fs::symlink_metadata(path).is_err() {
        return;
    }
    make_tree_writable(path);
    let _ = fs::remove_dir_all(path);
}

#[cfg(unix)]
fn pid_is_dead(pid: u32) -> bool {
    if pid == std::process::id() {
        return false;
    }
    if Path::new("/proc").is_dir() {
        return !Path::new(&format!("/proc/{pid}")).exists();
    }
    match Command::new("kill").args(["-0", &pid.to_string()]).output() {
        Ok(out) => {
            if out.status.success() {
                return false;
            }
            let stderr = String::from_utf8_lossy(&out.stderr).to_ascii_lowercase();
            !stderr.contains("operation not permitted") && !stderr.contains("permission denied")
        }
        Err(_) => false,
    }
}

#[cfg(not(unix))]
fn pid_is_dead(_pid: u32) -> bool {
    false
}

/// Reclaim leftover `devmap-fuzz-stacked-<pid>` fixture trees from exited
/// fuzzer processes (including `-jobs`/`-fork` workers) without using `unsafe`.
fn cleanup_stale_fixture_dirs() {
    let Ok(entries) = fs::read_dir(std::env::temp_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else {
            continue;
        };
        let Some(pid_str) = name_str.strip_prefix(FIXTURE_DIR_PREFIX) else {
            continue;
        };
        let Ok(pid) = pid_str.parse::<u32>() else {
            continue;
        };
        if pid_is_dead(pid) {
            remove_readonly_tree(&entry.path());
        }
    }
}

fn write_rel(root: &Path, rel: &str, body: &str) {
    let full = root.join(rel);
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent).expect("create parent dir");
    }
    fs::write(&full, body).expect("write source file");
}

fn git(cwd: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(cwd)
        .args(["-c", "commit.gpgsign=false", "-c", "core.autocrlf=false"])
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
    cleanup_stale_fixture_dirs();
    let root = std::env::temp_dir().join(format!("{FIXTURE_DIR_PREFIX}{}", std::process::id()));
    remove_readonly_tree(&root);
    let anchor = root.join("anchor");
    fs::create_dir_all(&anchor).expect("create anchor dir");

    let mut seed_src = Source::from_seed(0x57AC_ED00_0000_0001);
    let base_files = generate_micro_repo(&mut seed_src, MICRO_LANGS, 4);
    for (rel, body) in &base_files {
        write_rel(&anchor, rel, body);
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

fn reset_lane(fixture: &StackedFixture) {
    let delta = stack_delta_path(&fixture.lane);
    let _ = fs::remove_file(&delta);
    for suffix in ["-journal", "-wal", "-shm"] {
        let sidecar = PathBuf::from(format!("{}{suffix}", delta.display()));
        let _ = fs::remove_file(sidecar);
    }
    let _ = fs::remove_dir_all(fixture.lane.join("delta"));
    for (rel, body) in &fixture.base_files {
        let full = fixture.lane.join(rel);
        if fs::read_to_string(&full).ok().as_deref() != Some(body.as_str()) {
            write_rel(&fixture.lane, rel, body);
        }
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

fn is_simple_fts_ident(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
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
    ever_seen_files: &BTreeSet<String>,
    ever_seen_symbols: &BTreeSet<String>,
    sample_path: Option<&str>,
) -> StepOutcome {
    let cold = match build_cold_flat_store(live_files) {
        Ok(c) => c,
        Err(detail) => {
            return StepOutcome::CheckFailed(CheckFailure::new("cold_flat_build", action, detail))
        }
    };

    let deleted_files: BTreeSet<&str> = ever_seen_files
        .iter()
        .filter(|p| !live_files.contains_key(*p))
        .map(String::as_str)
        .collect();

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

    // 3. Per-file symbols parity + deleted/tombstoned files return zero symbols.
    let tombstoned: BTreeSet<&str> = base_files
        .keys()
        .filter(|p| !live_files.contains_key(*p))
        .map(String::as_str)
        .collect();
    for &gone in &deleted_files {
        match stacked.symbols_for_file(gone) {
            Ok(rows) => {
                let c = check(
                    rows.is_empty(),
                    "deleted_file_empty",
                    action,
                    format!(
                        "deleted file {gone} still returned {} symbol(s)",
                        rows.len()
                    ),
                );
                if !c.is_ok() {
                    return c;
                }
            }
            Err(e) => {
                return StepOutcome::CheckFailed(CheckFailure::new(
                    "deleted_lookup_ok",
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

    // 4. `keyword_page` FTS5 parity: NUL refusal, empty/whitespace queries,
    //    and exact parity with `cold.search_page` across live and deleted names.
    let c_nul = check(
        stacked.keyword_page("api\0bad", 128, None).is_err(),
        "stacked_keyword_nul_refused",
        action,
        "keyword_page accepted a query containing a NUL byte".to_string(),
    );
    if !c_nul.is_ok() {
        return c_nul;
    }

    let mut keyword_queries: Vec<String> = vec![
        String::new(),
        "   ".to_string(),
        "api".to_string(),
        "helper".to_string(),
        "mut_fn".to_string(),
        "dead".to_string(),
    ];
    let mut seen_queries: BTreeSet<String> = keyword_queries.iter().cloned().collect();
    for sym in &cold_syms {
        if is_simple_fts_ident(&sym.2) && seen_queries.insert(sym.2.clone()) {
            keyword_queries.push(sym.2.clone());
            if keyword_queries.len() >= 10 {
                break;
            }
        }
    }
    for sym in ever_seen_symbols {
        if is_simple_fts_ident(sym) && seen_queries.insert(sym.clone()) {
            keyword_queries.push(sym.clone());
            if keyword_queries.len() >= 12 {
                break;
            }
        }
    }

    for q in &keyword_queries {
        let st_page = match stacked.keyword_page(q, 256, None) {
            Ok(Some(page)) => page,
            Ok(None) => {
                return StepOutcome::CheckFailed(CheckFailure::new(
                    "stacked_keyword_page_some",
                    action,
                    format!("keyword_page({q:?}) returned None on live stacked store"),
                ))
            }
            Err(e) => {
                return StepOutcome::CheckFailed(CheckFailure::new(
                    "stacked_keyword_page_ok",
                    action,
                    format!("keyword_page({q:?}) failed: {e}"),
                ))
            }
        };
        let cd_page = match cold.search_page(q, 256) {
            Ok(Some(page)) => page,
            Ok(None) => {
                return StepOutcome::CheckFailed(CheckFailure::new(
                    "cold_search_page_some",
                    action,
                    format!("cold search_page({q:?}) returned None on live store"),
                ))
            }
            Err(e) => {
                return StepOutcome::CheckFailed(CheckFailure::new(
                    "cold_search_page_ok",
                    action,
                    format!("cold search_page({q:?}) failed: {e}"),
                ))
            }
        };
        let st_set: BTreeSet<(String, String, String, String)> = st_page
            .rows
            .iter()
            .map(|r| {
                (
                    r.path.clone(),
                    r.qualified_name.clone(),
                    r.name.clone(),
                    r.kind.clone(),
                )
            })
            .collect();
        let cd_set: BTreeSet<(String, String, String, String)> = cd_page
            .rows
            .iter()
            .map(|r| {
                (
                    r.path.clone(),
                    r.qualified_name.clone(),
                    r.name.clone(),
                    r.kind.clone(),
                )
            })
            .collect();
        for hit in &st_page.rows {
            let c_ghost = check(
                !deleted_files.contains(hit.path.as_str())
                    && !tombstoned.contains(hit.path.as_str()),
                "stacked_keyword_no_ghost_symbols",
                action,
                format!(
                    "keyword_page({q:?}) returned deleted/tombstoned symbol {} in {}",
                    hit.qualified_name, hit.path
                ),
            );
            if !c_ghost.is_ok() {
                return c_ghost;
            }
        }
        let c_kw = check(
            st_page.total == cd_page.total && st_set == cd_set,
            "stacked_keyword_page_parity",
            action,
            format!(
                "keyword_page({q:?}) mismatch (totals stacked={} vs cold={}):\nstacked={st_set:?}\ncold={cd_set:?}",
                st_page.total, cd_page.total
            ),
        );
        if !c_kw.is_ok() {
            return c_kw;
        }
    }

    // 5. Merged `file_edges`: check `Contains`, `Calls`, and `Imports` edges
    //    (zero tombstone leaks, exact `Contains` + same-file `Calls` parity,
    //    live endpoints on `Calls` & `Imports`, and full edge parity when clean).
    let cold_sym_set: BTreeSet<(String, String)> = cold_syms
        .iter()
        .map(|s| (s.0.clone(), s.1.clone()))
        .collect();
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
        let cd_edges = match cold.file_edges(path, 0.0) {
            Ok(Some(page)) => page.edges,
            Ok(None) => Vec::new(),
            Err(e) => {
                return StepOutcome::CheckFailed(CheckFailure::new(
                    "cold_file_edges_ok",
                    action,
                    format!("cold file_edges({path}) failed: {e}"),
                ))
            }
        };

        for edge in &st_edges {
            let c = check(
                !deleted_files.contains(edge.source_file.as_str())
                    && !deleted_files.contains(edge.target_file.as_str()),
                "stacked_edges_no_tombstone_leak",
                action,
                format!(
                    "file_edges({path}) leaked deleted/tombstoned endpoint ({}): {} -> {}",
                    edge.edge_kind, edge.source_file, edge.target_file
                ),
            );
            if !c.is_ok() {
                return c;
            }
            if edge.edge_kind == "Calls" {
                let c_target = check(
                    live_files.contains_key(&edge.target_file)
                        && cold_sym_set
                            .contains(&(edge.target_file.clone(), edge.target_symbol.clone())),
                    "stacked_calls_target_live",
                    action,
                    format!(
                        "file_edges({path}) emitted Calls edge to missing target {}:{}",
                        edge.target_file, edge.target_symbol
                    ),
                );
                if !c_target.is_ok() {
                    return c_target;
                }
            } else if edge.edge_kind == "Imports" && !edge.target_file.starts_with("package:") {
                let c_imp = check(
                    live_files.contains_key(&edge.target_file),
                    "stacked_imports_target_live",
                    action,
                    format!(
                        "file_edges({path}) emitted Imports edge to missing file {}",
                        edge.target_file
                    ),
                );
                if !c_imp.is_ok() {
                    return c_imp;
                }
            }
        }

        let canonical_edge_set = |edges: &[devmap_store::StoredEdge]| {
            edges
                .iter()
                .map(|e| {
                    (
                        e.source_file.clone(),
                        e.target_file.clone(),
                        e.source_symbol.clone(),
                        e.target_symbol.clone(),
                        e.edge_kind.clone(),
                    )
                })
                .collect::<BTreeSet<_>>()
        };

        let st_all = canonical_edge_set(&st_edges);
        let cd_all = canonical_edge_set(&cd_edges);

        if !stacked.has_delta_changes() {
            let c_clean = check(
                st_all == cd_all,
                "stacked_clean_edges_parity",
                action,
                format!("clean file_edges({path}) mismatch:\nstacked={st_all:?}\ncold={cd_all:?}"),
            );
            if !c_clean.is_ok() {
                return c_clean;
            }
        } else {
            let st_contains: BTreeSet<_> = st_all
                .iter()
                .filter(|(_, _, _, _, k)| k == "Contains")
                .cloned()
                .collect();
            let cd_contains: BTreeSet<_> = cd_all
                .iter()
                .filter(|(_, _, _, _, k)| k == "Contains")
                .cloned()
                .collect();
            let c_contains = check(
                st_contains == cd_contains,
                "stacked_contains_edges_parity",
                action,
                format!(
                    "Contains edges mismatch for {path}:\nstacked={st_contains:?}\ncold={cd_contains:?}"
                ),
            );
            if !c_contains.is_ok() {
                return c_contains;
            }

            let st_same_calls: BTreeSet<_> = st_all
                .iter()
                .filter(|(sf, tf, _, _, k)| k == "Calls" && sf == tf)
                .cloned()
                .collect();
            let cd_same_calls: BTreeSet<_> = cd_all
                .iter()
                .filter(|(sf, tf, _, _, k)| k == "Calls" && sf == tf)
                .cloned()
                .collect();
            let c_calls = check(
                st_same_calls == cd_same_calls,
                "stacked_same_file_calls_parity",
                action,
                format!(
                    "same-file Calls edges mismatch for {path}:\nstacked={st_same_calls:?}\ncold={cd_same_calls:?}"
                ),
            );
            if !c_calls.is_ok() {
                return c_calls;
            }
        }
    }

    StepOutcome::Ok
}

fn record_seen_symbols(ever_seen_symbols: &mut BTreeSet<String>, path: &str, body: &str) {
    let ext = extract_file(path, body);
    for sym in ext.symbols {
        ever_seen_symbols.insert(sym.name);
    }
}

fuzz_target!(|data: &[u8]| {
    if data.len() < 4 {
        return;
    }

    let fixture_mutex = FIXTURE.get_or_init(|| Mutex::new(init_fixture()));
    let fixture = fixture_mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    reset_lane(&fixture);

    let mut stacked = match StackedStore::open(&fixture.lane) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("DEVMAP_FUZZ_SIG=stacked_open_ok|init");
            panic!(
                "StackedStore::open({}) failed: {e:#}",
                fixture.lane.display()
            );
        }
    };

    let mut src = Source::from_bytes(data);
    let mut live_files = fixture.base_files.clone();
    let mut ever_seen_files: BTreeSet<String> = fixture.base_files.keys().cloned().collect();
    let mut ever_seen_symbols: BTreeSet<String> = BTreeSet::new();
    for (p, body) in &fixture.base_files {
        record_seen_symbols(&mut ever_seen_symbols, p, body);
    }
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
                        write_rel(&fixture.lane, &path, &body);
                        let res = if src.bool() {
                            stacked.sync_file_from_disk(&path)
                        } else {
                            let ext = extract_file(&path, &body);
                            stacked.record_extracted_file(&path, body.as_bytes(), &ext)
                        };
                        match res {
                            Ok(()) => {
                                ever_seen_files.insert(path.clone());
                                record_seen_symbols(&mut ever_seen_symbols, &path, &body);
                                live_files.insert(path.clone(), body);
                                trace.push(format!("upsert_new({path})"));
                                StepOutcome::Ok
                            }
                            Err(e) => StepOutcome::CheckFailed(CheckFailure::new(
                                "record_extracted_ok",
                                "upsert_new",
                                format!("upsert({path}) failed: {e}"),
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
                    write_rel(&fixture.lane, &path, &next);
                    let res = if fixture.base_files.get(&path) == Some(&next) {
                        if src.bool() {
                            stacked.sync_file_from_disk(&path)
                        } else {
                            stacked.clear_reverted_file(&path)
                        }
                    } else if src.bool() {
                        stacked.sync_file_from_disk(&path)
                    } else {
                        let ext = extract_file(&path, &next);
                        stacked.record_extracted_file(&path, next.as_bytes(), &ext)
                    };
                    match res {
                        Ok(()) => {
                            record_seen_symbols(&mut ever_seen_symbols, &path, &next);
                            live_files.insert(path.clone(), next);
                            trace.push(format!("edit_existing({path})"));
                            StepOutcome::Ok
                        }
                        Err(e) => StepOutcome::CheckFailed(CheckFailure::new(
                            "record_extracted_ok",
                            "edit_existing",
                            format!("edit({path}) failed: {e}"),
                        )),
                    }
                } else {
                    bail("no_live_files")
                }
            }
            "delete_file" => {
                if live_files.len() <= 1 {
                    bail("keep_last_file")
                } else {
                    let keys: Vec<String> = live_files.keys().cloned().collect();
                    let path = src.choose(&keys).cloned().unwrap();
                    let _ = fs::remove_file(fixture.lane.join(&path));
                    let is_base = fixture.base_files.contains_key(&path);
                    let res = if is_base {
                        if src.bool() {
                            stacked.sync_file_from_disk(&path)
                        } else {
                            stacked.record_deleted_file(&path)
                        }
                    } else if src.bool() {
                        stacked.sync_file_from_disk(&path)
                    } else {
                        stacked.clear_reverted_file(&path)
                    };
                    match res {
                        Ok(()) => {
                            live_files.remove(&path);
                            trace.push(format!("delete_file({path}, base={is_base})"));
                            StepOutcome::Ok
                        }
                        Err(e) => StepOutcome::CheckFailed(CheckFailure::new(
                            "delete_file_ok",
                            "delete_file",
                            format!("delete({path}, base={is_base}) failed: {e}"),
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
                    let orig = fixture.base_files.get(&path).cloned().unwrap();
                    write_rel(&fixture.lane, &path, &orig);
                    let res = if src.bool() {
                        stacked.sync_file_from_disk(&path)
                    } else {
                        stacked.clear_reverted_file(&path)
                    };
                    match res {
                        Ok(()) => {
                            live_files.insert(path.clone(), orig);
                            trace.push(format!("revert_base_file({path})"));
                            StepOutcome::Ok
                        }
                        Err(e) => StepOutcome::CheckFailed(CheckFailure::new(
                            "clear_reverted_ok",
                            "revert_base_file",
                            format!("revert({path}) failed: {e}"),
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
                    &ever_seen_files,
                    &ever_seen_symbols,
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
        &ever_seen_files,
        &ever_seen_symbols,
        first_live,
    )
    .into_failure()
    {
        eprintln!("DEVMAP_FUZZ_SIG={}", failure.signature());
        eprintln!("trace={trace:?}");
        panic!("{failure}");
    }
});
