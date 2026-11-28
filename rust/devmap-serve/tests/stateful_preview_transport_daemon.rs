//! Differential and stateful test suites for:
//! - Suite A: `preview` vs. save+rebuild differential oracle + cross-transport
//!   parity (`InMemory == Store == IPC == MCP`) across evolving generations.
//! - Suite B: `blast` / `suspects` duality and `unattributed`/`complete` honesty
//!   over synthetic multi-commit git repositories.
//! - Suite C: Virtualized `DebounceBuffer` collapse + `Daemon` drain state
//!   machine (monotonic `(epoch, revision)` watermarks, zero cross-charging of
//!   quarantine attempts across batch-mates, and final indexed store == cold
//!   full build).
//!
//! Configurable via environment variables:
//! - `DEVMAP_STATEFUL_SEED` (u64, decimal or `0x` hex)
//! - `DEVMAP_STATEFUL_ROUNDS` (usize)

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dc_regress::{blast, suspects, Unavailable};
use dc_regress_store::StoreGraph;
use devmap_analyze::analyze_with_discovery;
use devmap_extract::{
    collect_project_manifests, collect_sources_with_report, extract_file, Extraction,
};
use devmap_query::{
    PreviewChange, PreviewReport, QueryEngine, Request, ResolutionAvailability, StoreQueryEngine,
    PREVIEW_CALLER_MIN_CONFIDENCE,
};
use devmap_resolve::model::ResolutionResult;
use devmap_resolve::Resolver;
use devmap_serve::mcp::{handle_line, StoreSlot};
use devmap_serve::{handle_stream, Daemon, PROTOCOL_VERSION};
use devmap_store::{
    current_git_head, discovery_refusals, over_limit_inventory, GenerationWriteOpts, Store,
    MAX_PENDING_ATTEMPTS, UNAVAILABLE_HEAD,
};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[allow(dead_code)]
mod stateful_fuzz {
    include!("../../testsupport/stateful_fuzz.rs");
}

use stateful_fuzz::{
    bail, check, env_u64, env_usize, format_repro_test, generate_micro_repo, lower_damage,
    minimize_sequence, mutate_file_elements, skip_missing, CheckFailure, HealthTracker, MicroLang,
    Source, StepOutcome,
};

const DEFAULT_SEED: u64 = 0x5717_C405_0005_0006;
const ALL_LANGS: &[MicroLang] = &[
    MicroLang::Rust,
    MicroLang::TypeScript,
    MicroLang::Go,
    MicroLang::Python,
];

// ---------------------------------------------------------------------------
// Shared temp-directory RAII guard, build helpers, and transport runners
// ---------------------------------------------------------------------------

struct TempDirGuard {
    path: PathBuf,
}

impl TempDirGuard {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let raw = std::env::temp_dir().join(format!(
            "devmap-stateful-{label}-{}-{stamp}-{seq}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&raw);
        fs::create_dir_all(&raw).expect("create temp dir");
        // Canonicalize so macOS `/var` -> `/private/var` matches daemon root resolution.
        let path = raw.canonicalize().expect("canonicalize temp dir");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn lang_for_path(path: &str) -> MicroLang {
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

fn write_repo_file(root: &Path, rel: &str, content: &str) {
    let full = root.join(rel);
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent).expect("create parent dirs");
    }
    fs::write(&full, content).expect("write source file");
}

fn copy_dir_files(src_root: &Path, dst_root: &Path, files: &BTreeMap<String, String>) {
    for (rel, content) in files {
        write_repo_file(dst_root, rel, content);
    }
    let _ = src_root;
}

/// Build a complete generation in `store` from `root` on disk, returning the
/// in-memory `(extractions, resolution)` corresponding to the saved generation.
fn build_generation_from_disk(
    store: &Store,
    root: &Path,
) -> anyhow::Result<(Vec<Extraction>, ResolutionResult, u32)> {
    let canonical = root.canonicalize()?;
    let (sources, discovery) = collect_sources_with_report(&canonical)?;
    let manifests = collect_project_manifests(&canonical)?;
    let mut extractions: Vec<Extraction> = sources
        .iter()
        .map(|(rel, src)| extract_file(rel, src))
        .collect();
    extractions.sort_by(|a, b| a.file_path.cmp(&b.file_path));

    let mut resolver = Resolver::new();
    resolver.index_project_manifests(&manifests);
    resolver.index_extractions(&extractions);
    resolver.mark_go_dirs_incomplete(devmap_extract::go_dirs_with_unindexed_files(
        &canonical,
        extractions.iter().map(|ext| ext.file_path.as_str()),
    ));
    let resolution = resolver.resolve_all(&extractions)?;

    let refusals = discovery_refusals(&discovery);
    let over_limit = over_limit_inventory(&discovery);
    let inventory = devmap_store::DiscoveryInventory::of(&discovery, &manifests);
    let analysis = analyze_with_discovery(&extractions, &resolution, inventory.coverage());
    let head = current_git_head(&canonical).unwrap_or_else(|_| UNAVAILABLE_HEAD.to_string());

    let gen_id = store.save_generation_with_metadata(
        &extractions,
        &resolution,
        &analysis,
        GenerationWriteOpts {
            repo_root: Some(canonical.to_string_lossy().into_owned()),
            discovery_refusals: Some(refusals),
            over_limit: Some(over_limit),
            ..Default::default()
        },
        &head,
    )?;
    Ok((extractions, resolution, gen_id))
}

async fn ipc_request(store: Arc<Store>, payload: Value) -> Value {
    let (client, server) = tokio::io::duplex(256 * 1024);
    let task = tokio::spawn(handle_stream(server, store));
    let (mut reader, mut writer) = tokio::io::split(client);
    let mut bytes = serde_json::to_vec(&payload).expect("serialize IPC request");
    bytes.push(b'\n');

    let pump = tokio::spawn(async move {
        let _ = writer.write_all(&bytes).await;
        let _ = writer.shutdown().await;
    });
    let mut reply = Vec::new();
    tokio::time::timeout(Duration::from_secs(15), reader.read_to_end(&mut reply))
        .await
        .expect("IPC exchange timed out")
        .expect("IPC read failed");
    let _ = pump.await;
    let _ = task.await;

    assert_eq!(
        reply.last().copied(),
        Some(b'\n'),
        "IPC reply not newline-framed"
    );
    let body = &reply[..reply.len() - 1];
    assert!(
        !body.contains(&b'\n'),
        "IPC reply contained multiple frames"
    );
    let env: Value = serde_json::from_slice(body).expect("IPC reply JSON");
    assert_eq!(env["protocol_version"], PROTOCOL_VERSION);
    assert_eq!(
        env["ok"],
        Value::Bool(true),
        "IPC request failed unexpectedly: {env}"
    );
    env["result"].clone()
}

async fn mcp_call_tool(slot: &Arc<StoreSlot>, tool: &str, arguments: Value) -> Value {
    let frame = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": tool,
            "arguments": arguments,
        }
    });
    let resp = handle_line(slot, &frame.to_string())
        .await
        .expect("MCP tools/call must reply");
    assert_eq!(resp["jsonrpc"], json!("2.0"));
    assert_eq!(resp["id"], json!(1));
    assert!(
        resp.get("error").is_none(),
        "MCP protocol error on {tool}: {resp}"
    );
    let result = &resp["result"];
    assert_ne!(
        result["isError"],
        json!(true),
        "MCP tool error on {tool}: {resp}"
    );
    result["structuredContent"].clone()
}

/// Normalize transport-only metadata (`repository` added by MCP, volatile
/// `verified_age_ms`) so `Store == IPC == MCP` compare exact semantic payloads.
fn normalize_transport_json(mut val: Value) -> Value {
    fn strip_volatile(v: &mut Value) {
        match v {
            Value::Object(map) => {
                if let Some(Value::Object(sf)) = map.get_mut("source_freshness") {
                    sf.remove("verified_age_ms");
                }
                for child in map.values_mut() {
                    strip_volatile(child);
                }
            }
            Value::Array(arr) => {
                for item in arr {
                    strip_volatile(item);
                }
            }
            _ => {}
        }
    }
    if let Some(obj) = val.as_object_mut() {
        obj.remove("repository");
    }
    strip_volatile(&mut val);
    val
}

fn check_envelope_invariants(
    val: &Value,
    action: &'static str,
    label: &str,
) -> Result<(), CheckFailure> {
    let shown = val["shown"].as_u64();
    let hidden = val["hidden"].as_u64();
    let total = val["total"].as_u64();
    let truncated = val["truncated"].as_bool();
    let items_len = val["items"].as_array().map(|a| a.len() as u64);

    if let (Some(s), Some(h), Some(t), Some(tr), Some(len)) =
        (shown, hidden, total, truncated, items_len)
    {
        if s != len {
            return Err(CheckFailure::new(
                "envelope_shown_eq_items_len",
                action,
                format!("{label}: shown={s} != items.len()={len}"),
            ));
        }
        if s + h != t {
            return Err(CheckFailure::new(
                "envelope_shown_plus_hidden_eq_total",
                action,
                format!("{label}: shown({s}) + hidden({h}) != total({t})"),
            ));
        }
        if tr != (h > 0) {
            return Err(CheckFailure::new(
                "envelope_truncated_eq_hidden_gt_zero",
                action,
                format!("{label}: truncated={tr} but hidden={h}"),
            ));
        }
    }
    Ok(())
}

// ===========================================================================
// Suite A: `preview` vs. save+rebuild + Cross-Transport Parity
//          (`InMemory == Store == IPC == MCP`)
// ===========================================================================

#[derive(Clone, Debug)]
enum PreviewKind {
    StructuralMutate(String),
    RemoveCalledSymbol {
        removed_name: String,
        content: String,
    },
    SignatureChange {
        target_name: String,
        content: String,
    },
    BrokenSyntax(String),
    UnindexedNewFile {
        content: String,
    },
}

#[derive(Clone, Debug)]
enum TransportQuery {
    Search {
        query: String,
        budget: u32,
    },
    Impact {
        target: String,
        budget: u32,
        depth: usize,
    },
    Dependencies {
        target: String,
        budget: u32,
    },
    DeadSymbols {
        budget: u32,
    },
}

#[derive(Clone, Debug)]
enum SuiteAAction {
    EditDisk { path: String, content: String },
    PreviewUnsaved { path: String, kind: PreviewKind },
    RebuildStore,
    QueryAllTransports(TransportQuery),
}

impl SuiteAAction {
    fn name(&self) -> &'static str {
        match self {
            Self::EditDisk { .. } => "EditDisk",
            Self::PreviewUnsaved { .. } => "PreviewUnsaved",
            Self::RebuildStore => "RebuildStore",
            Self::QueryAllTransports(_) => "QueryAllTransports",
        }
    }
}

/// Remove the top-level block defining `fn_name` from `content`.
fn remove_block_defining(content: &str, fn_name: &str) -> Option<String> {
    let blocks: Vec<&str> = content.split("\n\n").collect();
    let needle_variants = [
        format!("fn {fn_name}("),
        format!("function {fn_name}("),
        format!("func {fn_name}("),
        format!("def {fn_name}("),
    ];
    let mut kept = Vec::with_capacity(blocks.len());
    let mut removed_any = false;
    for b in blocks {
        if !removed_any && needle_variants.iter().any(|n| b.contains(n)) {
            removed_any = true;
            continue;
        }
        kept.push(b);
    }
    if !removed_any {
        return None;
    }
    let mut out = kept.join("\n\n");
    if !out.ends_with('\n') {
        out.push('\n');
    }
    Some(out)
}

/// Alter the parameter list of `fn_name` so `declaration_hash` moves cleanly
/// while remaining syntactically valid in the file's language.
fn change_signature_of(lang: MicroLang, content: &str, fn_name: &str) -> Option<String> {
    let (old_sig, new_sig) = match lang {
        MicroLang::Rust => (
            format!("fn {fn_name}(x: i32)"),
            format!("fn {fn_name}(x: i32, extra_param: i32)"),
        ),
        MicroLang::TypeScript => (
            format!("function {fn_name}(x: number)"),
            format!("function {fn_name}(x: number, extraParam: number)"),
        ),
        MicroLang::Go => (
            format!("func {fn_name}(x int)"),
            format!("func {fn_name}(x int, extraParam int)"),
        ),
        MicroLang::Python => (
            format!("def {fn_name}(x: int)"),
            format!("def {fn_name}(x: int, extra_param: int)"),
        ),
    };
    if !content.contains(&old_sig) {
        return None;
    }
    Some(content.replacen(&old_sig, &new_sig, 1))
}

fn candidate_api_name(lang: MicroLang, path: &str) -> Option<String> {
    let stem = Path::new(path).file_stem()?.to_str()?;
    let idx = stem.strip_prefix("mod_")?;
    Some(match lang {
        MicroLang::Go => format!("Api{idx}"),
        _ => format!("api_{idx}"),
    })
}

fn candidate_helper_name(lang: MicroLang, path: &str) -> Option<String> {
    let stem = Path::new(path).file_stem()?.to_str()?;
    let idx = stem.strip_prefix("mod_")?;
    Some(match lang {
        MicroLang::Go => format!("helper{idx}"),
        _ => format!("helper_{idx}"),
    })
}

struct SuiteAState {
    _guard: TempDirGuard,
    root: PathBuf,
    shadow_root: PathBuf,
    files: BTreeMap<String, String>,
    store: Arc<Store>,
    slot: Arc<StoreSlot>,
    extractions: Vec<Extraction>,
    resolution: ResolutionResult,
    last_gen_id: u32,
    store_in_sync_with_disk: bool,
}

impl SuiteAState {
    fn new(initial_files: BTreeMap<String, String>) -> Self {
        let guard = TempDirGuard::new("suite-a");
        let root = guard.path().join("repo");
        let shadow_root = guard.path().join("shadow");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&shadow_root).unwrap();
        for (rel, content) in &initial_files {
            write_repo_file(&root, rel, content);
        }
        let db_path = guard.path().join("devmap.sqlite");
        let store = Arc::new(Store::open(&db_path).expect("open suite-a store"));
        let (extractions, resolution, gen_id) =
            build_generation_from_disk(&store, &root).expect("initial build");
        let slot = Arc::new(StoreSlot::ready(&db_path, Arc::clone(&store)));
        Self {
            _guard: guard,
            root,
            shadow_root,
            files: initial_files,
            store,
            slot,
            extractions,
            resolution,
            last_gen_id: gen_id,
            store_in_sync_with_disk: true,
        }
    }

    async fn apply(&mut self, action: &SuiteAAction) -> StepOutcome {
        match action {
            SuiteAAction::EditDisk { path, content } => {
                if !self.files.contains_key(path) {
                    return skip_missing("edit_missing_file");
                }
                write_repo_file(&self.root, path, content);
                self.files.insert(path.clone(), content.clone());
                self.store_in_sync_with_disk = false;
                StepOutcome::Ok
            }
            SuiteAAction::RebuildStore => {
                let (extractions, resolution, gen_id) =
                    match build_generation_from_disk(&self.store, &self.root) {
                        Ok(res) => res,
                        Err(err) => {
                            return StepOutcome::CheckFailed(CheckFailure::new(
                                "rebuild_store_succeeds",
                                "RebuildStore",
                                format!("rebuild failed: {err:#}"),
                            ));
                        }
                    };
                let mono = check(
                    gen_id > self.last_gen_id,
                    "generation_id_strictly_increases",
                    "RebuildStore",
                    format!("expected gen_id > {}, got {gen_id}", self.last_gen_id),
                );
                if !mono.is_ok() {
                    return mono;
                }
                self.extractions = extractions;
                self.resolution = resolution;
                self.last_gen_id = gen_id;
                self.store_in_sync_with_disk = true;
                StepOutcome::Ok
            }
            SuiteAAction::PreviewUnsaved { path, kind } => {
                let unsaved = match kind {
                    PreviewKind::StructuralMutate(s)
                    | PreviewKind::BrokenSyntax(s)
                    | PreviewKind::RemoveCalledSymbol { content: s, .. }
                    | PreviewKind::SignatureChange { content: s, .. }
                    | PreviewKind::UnindexedNewFile { content: s } => s.as_str(),
                };
                let budget = 20_000;
                let min_conf = PREVIEW_CALLER_MIN_CONFIDENCE;

                let engine = StoreQueryEngine::new(&self.store);
                let store_preview = match engine.preview(path, unsaved, budget, min_conf) {
                    Ok(report) => report,
                    Err(err) => {
                        return StepOutcome::CheckFailed(CheckFailure::new(
                            "preview_no_error",
                            "PreviewUnsaved",
                            format!("StoreQueryEngine::preview failed on {path}: {err:#}"),
                        ));
                    }
                };

                // 1. Cross-transport parity for preview: Store == IPC == MCP
                let store_json = normalize_transport_json(
                    serde_json::to_value(&store_preview).expect("serialize preview"),
                );
                let ipc_json = normalize_transport_json(
                    ipc_request(
                        Arc::clone(&self.store),
                        json!({
                            "version": PROTOCOL_VERSION,
                            "cmd": "preview",
                            "file": path,
                            "content": unsaved,
                            "budget": budget,
                            "min_confidence": min_conf,
                        }),
                    )
                    .await,
                );
                let mcp_json = normalize_transport_json(
                    mcp_call_tool(
                        &self.slot,
                        "devmap_preview",
                        json!({
                            "file": path,
                            "content": unsaved,
                            "budget": budget,
                            "min_confidence": min_conf,
                        }),
                    )
                    .await,
                );

                if store_json != ipc_json {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "preview_store_eq_ipc",
                        "PreviewUnsaved",
                        format!(
                            "Store != IPC on preview({path}):\nstore={store_json}\nipc={ipc_json}"
                        ),
                    ));
                }
                if store_json != mcp_json {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "preview_store_eq_mcp",
                        "PreviewUnsaved",
                        format!(
                            "Store != MCP on preview({path}):\nstore={store_json}\nmcp={mcp_json}"
                        ),
                    ));
                }

                if let Err(failure) = check_envelope_invariants(
                    &store_json["broken_callers"],
                    "PreviewUnsaved",
                    "preview.broken_callers",
                ) {
                    return StepOutcome::CheckFailed(failure);
                }

                // 2. Differential oracle: preview vs. actual save + cold rebuild in shadow dir
                if let Err(failure) =
                    self.verify_preview_vs_save_rebuild(path, unsaved, &store_preview)
                {
                    return StepOutcome::CheckFailed(failure);
                }

                // 3. Targeted expectations by preview mode
                match kind {
                    PreviewKind::UnindexedNewFile { .. } => {
                        if store_preview.compared_against != "nothing"
                            || store_preview.file_is_indexed
                        {
                            return StepOutcome::CheckFailed(CheckFailure::new(
                                "preview_new_file_metadata",
                                "PreviewUnsaved",
                                format!(
                                    "new file {path}: compared_against={:?}, file_is_indexed={}",
                                    store_preview.compared_against, store_preview.file_is_indexed
                                ),
                            ));
                        }
                    }
                    PreviewKind::RemoveCalledSymbol { removed_name, .. } => {
                        if store_preview.delta_available && store_preview.parse_status == "clean" {
                            let has_removed = store_preview.symbols.iter().any(|s| {
                                s.symbol_name == *removed_name && s.change == PreviewChange::Removed
                            });
                            if !has_removed {
                                return StepOutcome::CheckFailed(CheckFailure::new(
                                    "preview_reports_removed_symbol",
                                    "PreviewUnsaved",
                                    format!(
                                        "expected {removed_name} Removed in {path}, got {:?}",
                                        store_preview.symbols
                                    ),
                                ));
                            }
                        }
                    }
                    PreviewKind::SignatureChange { target_name, .. } => {
                        if store_preview.delta_available && store_preview.parse_status == "clean" {
                            let reported = store_preview.symbols.iter().any(|s| {
                                s.symbol_name == *target_name
                                    && matches!(
                                        s.change,
                                        PreviewChange::SignatureChanged | PreviewChange::Changed
                                    )
                            });
                            if !reported {
                                return StepOutcome::CheckFailed(CheckFailure::new(
                                    "preview_reports_signature_change",
                                    "PreviewUnsaved",
                                    format!(
                                        "expected SignatureChanged/Changed for {target_name} in {path}, got {:?}",
                                        store_preview.symbols
                                    ),
                                ));
                            }
                        }
                    }
                    PreviewKind::BrokenSyntax(_) => {
                        if store_preview.parse_status == "failed" {
                            if store_preview.delta_available
                                || !store_preview.symbols.is_empty()
                                || !store_preview.broken_callers.items.is_empty()
                                || store_preview.degraded_reason.is_none()
                            {
                                return StepOutcome::CheckFailed(CheckFailure::new(
                                    "preview_failed_parse_withholds_delta",
                                    "PreviewUnsaved",
                                    format!("failed parse leaked delta: {store_preview:?}"),
                                ));
                            }
                        } else if store_preview.parse_status == "partial"
                            && store_preview.degraded_reason.is_none()
                        {
                            return StepOutcome::CheckFailed(CheckFailure::new(
                                "preview_partial_parse_sets_degraded_reason",
                                "PreviewUnsaved",
                                format!("partial parse lacked degraded_reason: {store_preview:?}"),
                            ));
                        }
                    }
                    PreviewKind::StructuralMutate(_) => {}
                }

                StepOutcome::Ok
            }
            SuiteAAction::QueryAllTransports(query) => self.verify_transport_parity(query).await,
        }
    }

    fn verify_preview_vs_save_rebuild(
        &self,
        path: &str,
        unsaved: &str,
        preview: &PreviewReport,
    ) -> Result<(), CheckFailure> {
        if !preview.delta_available {
            return Ok(());
        }
        // Check `compared_against` matches disk presence.
        let disk_exists = self.root.join(path).is_file();
        let expected_compared = if disk_exists { "disk" } else { "nothing" };
        if preview.compared_against != expected_compared {
            return Err(CheckFailure::new(
                "preview_compared_against_honest",
                "PreviewUnsaved",
                format!(
                    "{path}: disk_exists={disk_exists} expected compared_against={expected_compared:?}, got {:?}",
                    preview.compared_against
                ),
            ));
        }

        // Only compare exact symbol sets when the candidate parsed cleanly.
        if preview.parse_status != "clean" {
            return Ok(());
        }

        // Materialize shadow copy of current disk repo + unsaved buffer written to disk,
        // then rebuild a fresh cold Store on the shadow directory.
        let _ = fs::remove_dir_all(&self.shadow_root);
        fs::create_dir_all(&self.shadow_root).expect("recreate shadow root");
        copy_dir_files(&self.root, &self.shadow_root, &self.files);
        write_repo_file(&self.shadow_root, path, unsaved);

        let shadow_store = Store::open_in_memory().expect("shadow store");
        if let Err(err) = build_generation_from_disk(&shadow_store, &self.shadow_root) {
            return Err(CheckFailure::new(
                "shadow_rebuild_succeeds",
                "PreviewUnsaved",
                format!("shadow rebuild failed for {path}: {err:#}"),
            ));
        }

        // Before symbols come from disk (what `preview` compares against).
        let before_syms: BTreeSet<String> = self
            .files
            .get(path)
            .map(|disk_src| {
                extract_file(path, disk_src)
                    .symbols
                    .into_iter()
                    .map(|s| s.qualified_name)
                    .collect()
            })
            .unwrap_or_default();

        // After symbols come from the shadow store built after writing `unsaved` to disk.
        let after_syms: BTreeSet<String> = shadow_store
            .latest_extraction_for_path(path)
            .expect("shadow extraction")
            .map(|ext| ext.symbols.into_iter().map(|s| s.qualified_name).collect())
            .unwrap_or_default();

        let preview_added: BTreeSet<String> = preview
            .symbols
            .iter()
            .filter(|s| s.change == PreviewChange::Added)
            .map(|s| s.qualified_name.clone())
            .collect();
        let preview_removed: BTreeSet<String> = preview
            .symbols
            .iter()
            .filter(|s| s.change == PreviewChange::Removed)
            .map(|s| s.qualified_name.clone())
            .collect();

        let expected_added: BTreeSet<String> =
            after_syms.difference(&before_syms).cloned().collect();
        let expected_removed: BTreeSet<String> =
            before_syms.difference(&after_syms).cloned().collect();

        if preview_added != expected_added {
            return Err(CheckFailure::new(
                "preview_added_matches_save_rebuild",
                "PreviewUnsaved",
                format!(
                    "{path}: preview Added={preview_added:?} != save+rebuild added={expected_added:?}"
                ),
            ));
        }
        if preview_removed != expected_removed {
            return Err(CheckFailure::new(
                "preview_removed_matches_save_rebuild",
                "PreviewUnsaved",
                format!(
                    "{path}: preview Removed={preview_removed:?} != save+rebuild removed={expected_removed:?}"
                ),
            ));
        }

        // Cross-file broken callers oracle: compare against persisted store edges
        // and verify removed targets vanish from shadow_store's inbound edges.
        let at_risk: BTreeSet<String> = preview
            .symbols
            .iter()
            .filter(|s| {
                matches!(
                    s.change,
                    PreviewChange::Removed
                        | PreviewChange::SignatureChanged
                        | PreviewChange::Changed
                )
            })
            .map(|s| s.qualified_name.clone())
            .collect();

        let all_store_edges = self.store.latest_edges(0.0).expect("latest_edges");
        let expected_confident: BTreeSet<(String, String, String)> = all_store_edges
            .iter()
            .filter(|e| {
                e.source_file != path
                    && at_risk.contains(&e.target_symbol)
                    && (e.confidence * 1000.0).round() as i64
                        >= (PREVIEW_CALLER_MIN_CONFIDENCE * 1000.0).round() as i64
            })
            .map(|e| {
                (
                    e.source_file.clone(),
                    e.source_symbol.clone(),
                    e.target_symbol.clone(),
                )
            })
            .collect();
        let total_unfiltered = all_store_edges
            .iter()
            .filter(|e| e.source_file != path && at_risk.contains(&e.target_symbol))
            .count();

        if !preview.broken_callers.truncated {
            let got_confident: BTreeSet<(String, String, String)> = preview
                .broken_callers
                .items
                .iter()
                .map(|c| {
                    (
                        c.caller_file.clone(),
                        c.caller_symbol.clone(),
                        c.target_symbol.clone(),
                    )
                })
                .collect();
            if got_confident != expected_confident {
                return Err(CheckFailure::new(
                    "preview_broken_callers_matches_store_edges",
                    "PreviewUnsaved",
                    format!(
                        "{path}: preview broken_callers={got_confident:?} != store expected={expected_confident:?}"
                    ),
                ));
            }
            let expected_ambiguous =
                total_unfiltered.saturating_sub(preview.broken_callers.items.len());
            if preview.ambiguous_callers != expected_ambiguous {
                return Err(CheckFailure::new(
                    "preview_ambiguous_callers_count",
                    "PreviewUnsaved",
                    format!(
                        "{path}: ambiguous_callers={} != expected {expected_ambiguous} (unfiltered={total_unfiltered})",
                        preview.ambiguous_callers
                    ),
                ));
            }
        }

        // For every Removed symbol, after saving `unsaved` and rebuilding `shadow_store`,
        // no cross-file edge in `shadow_store` can still resolve to that removed symbol!
        if self.store_in_sync_with_disk && !preview_removed.is_empty() {
            let shadow_edges = shadow_store.latest_edges(0.0).expect("shadow edges");
            for removed_q in &preview_removed {
                let surviving_callers: Vec<_> = shadow_edges
                    .iter()
                    .filter(|e| e.source_file != path && &e.target_symbol == removed_q)
                    .collect();
                if !surviving_callers.is_empty() {
                    return Err(CheckFailure::new(
                        "removed_symbol_has_no_callers_after_rebuild",
                        "PreviewUnsaved",
                        format!(
                            "{path}: removed symbol {removed_q} still has edges after save+rebuild: {surviving_callers:?}"
                        ),
                    ));
                }
            }
        }

        Ok(())
    }

    async fn verify_transport_parity(&self, query: &TransportQuery) -> StepOutcome {
        let store_engine = StoreQueryEngine::new(&self.store);
        let mem_engine = QueryEngine::new(&self.extractions, &self.resolution);

        match query {
            TransportQuery::Search { query: q, budget } => {
                let req = Request {
                    query: q.clone(),
                    token_budget: *budget,
                    min_confidence: 0.0,
                    max_depth: 1,
                };
                let store_resp = match store_engine.search(req.clone()) {
                    Ok(r) => r,
                    Err(err) => {
                        return StepOutcome::CheckFailed(CheckFailure::new(
                            "store_search_ok",
                            "QueryAllTransports",
                            format!("store search({q:?}) failed: {err:#}"),
                        ));
                    }
                };
                let store_json =
                    normalize_transport_json(serde_json::to_value(&store_resp).unwrap());
                let ipc_json = normalize_transport_json(
                    ipc_request(
                        Arc::clone(&self.store),
                        json!({
                            "version": PROTOCOL_VERSION,
                            "cmd": "search",
                            "query": q,
                            "budget": budget,
                        }),
                    )
                    .await,
                );
                let mcp_json = normalize_transport_json(
                    mcp_call_tool(
                        &self.slot,
                        "devmap_search",
                        json!({
                            "query": q,
                            "budget": budget,
                        }),
                    )
                    .await,
                );

                if let Err(f) =
                    check_envelope_invariants(&store_json, "QueryAllTransports", "search.store")
                {
                    return StepOutcome::CheckFailed(f);
                }
                if store_json != ipc_json {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "search_store_eq_ipc",
                        "QueryAllTransports",
                        format!("search({q:?}, budget={budget}): Store != IPC\nstore={store_json}\nipc={ipc_json}"),
                    ));
                }
                if store_json != mcp_json {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "search_store_eq_mcp",
                        "QueryAllTransports",
                        format!("search({q:?}, budget={budget}): Store != MCP\nstore={store_json}\nmcp={mcp_json}"),
                    ));
                }

                // InMemory parity on search when budget is generous (not truncated on either side).
                // Note: `StoreQueryEngine::search` uses FTS5 `unicode61` prefix tokens (so `_` splits
                // tokens while camelCase `deadHelper_0` stays one token), whereas `QueryEngine::search`
                // (`in_memory.rs`) matches substrings over `name`/`qualified_name`. Therefore:
                // 1) Every Store hit must be found by InMemory (`store_set ⊆ mem_set`), and
                // 2) Exact symbol-name matches (`symbol_name.eq_ignore_ascii_case(q)`) must agree 100%.
                let mem_resp = mem_engine.search(req);
                if !store_resp.truncated && !mem_resp.truncated && *budget >= 10_000 {
                    let store_set: BTreeSet<(String, String, String)> = store_resp
                        .items
                        .iter()
                        .map(|h| (h.file_path.clone(), h.symbol_name.clone(), h.kind.clone()))
                        .collect();
                    let mem_set: BTreeSet<(String, String, String)> = mem_resp
                        .items
                        .iter()
                        .map(|h| (h.file_path.clone(), h.symbol_name.clone(), h.kind.clone()))
                        .collect();
                    let store_only: Vec<_> = store_set.difference(&mem_set).collect();
                    if !store_only.is_empty() {
                        return StepOutcome::CheckFailed(CheckFailure::new(
                            "search_store_subset_of_in_memory",
                            "QueryAllTransports",
                            format!(
                                "search({q:?}): Store hits missing from InMemory: {store_only:?}"
                            ),
                        ));
                    }
                    let q_lower = q.to_lowercase();
                    let store_exact: BTreeSet<_> = store_set
                        .iter()
                        .filter(|(_, name, _)| name.to_lowercase() == q_lower)
                        .cloned()
                        .collect();
                    let mem_exact: BTreeSet<_> = mem_set
                        .iter()
                        .filter(|(_, name, _)| name.to_lowercase() == q_lower)
                        .cloned()
                        .collect();
                    if store_exact != mem_exact {
                        return StepOutcome::CheckFailed(CheckFailure::new(
                            "search_exact_store_eq_in_memory",
                            "QueryAllTransports",
                            format!(
                                "search({q:?}): exact hits Store={store_exact:?} != InMemory={mem_exact:?}"
                            ),
                        ));
                    }
                }
                StepOutcome::Ok
            }
            TransportQuery::Impact {
                target,
                budget,
                depth,
            } => {
                let req = Request {
                    query: target.clone(),
                    token_budget: *budget,
                    min_confidence: 0.0,
                    max_depth: *depth,
                };
                let store_resp = match store_engine.impact(req.clone()) {
                    Ok(r) => r,
                    Err(err) => {
                        return StepOutcome::CheckFailed(CheckFailure::new(
                            "store_impact_ok",
                            "QueryAllTransports",
                            format!("store impact({target:?}) failed: {err:#}"),
                        ));
                    }
                };
                let store_json =
                    normalize_transport_json(serde_json::to_value(&store_resp).unwrap());
                let ipc_json = normalize_transport_json(
                    ipc_request(
                        Arc::clone(&self.store),
                        json!({
                            "version": PROTOCOL_VERSION,
                            "cmd": "impact",
                            "target": target,
                            "budget": budget,
                            "depth": depth,
                        }),
                    )
                    .await,
                );
                let mcp_json = normalize_transport_json(
                    mcp_call_tool(
                        &self.slot,
                        "devmap_impact",
                        json!({
                            "target": target,
                            "budget": budget,
                            "depth": depth,
                        }),
                    )
                    .await,
                );

                if let Err(f) =
                    check_envelope_invariants(&store_json, "QueryAllTransports", "impact.store")
                {
                    return StepOutcome::CheckFailed(f);
                }
                if store_json != ipc_json {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "impact_store_eq_ipc",
                        "QueryAllTransports",
                        format!(
                            "impact({target:?}): Store != IPC\nstore={store_json}\nipc={ipc_json}"
                        ),
                    ));
                }
                if store_json != mcp_json {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "impact_store_eq_mcp",
                        "QueryAllTransports",
                        format!(
                            "impact({target:?}): Store != MCP\nstore={store_json}\nmcp={mcp_json}"
                        ),
                    ));
                }

                // InMemory parity on impact
                let mem_resp = mem_engine.impact(req);
                let store_avail =
                    matches!(store_resp.resolution, ResolutionAvailability::Available);
                let mem_avail = matches!(mem_resp.resolution, ResolutionAvailability::Available);
                if store_avail != mem_avail {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "impact_availability_store_eq_in_memory",
                        "QueryAllTransports",
                        format!(
                            "impact({target:?}): store resolution={:?} != in_memory resolution={:?}",
                            store_resp.resolution, mem_resp.resolution
                        ),
                    ));
                }
                if store_avail && !store_resp.truncated && !mem_resp.truncated {
                    let store_edges: BTreeSet<_> = store_resp
                        .items
                        .iter()
                        .map(|e| {
                            (
                                e.source_file.clone(),
                                e.source_symbol.clone(),
                                e.target_file.clone(),
                                e.target_symbol.clone(),
                            )
                        })
                        .collect();
                    let mem_edges: BTreeSet<_> = mem_resp
                        .items
                        .iter()
                        .map(|e| {
                            (
                                e.source_file.clone(),
                                e.source_symbol.clone(),
                                e.target_file.clone(),
                                e.target_symbol.clone(),
                            )
                        })
                        .collect();
                    if store_edges != mem_edges
                        || store_resp.total != mem_resp.total
                        || store_resp.walk_incomplete.is_some()
                            != mem_resp.walk_incomplete.is_some()
                    {
                        return StepOutcome::CheckFailed(CheckFailure::new(
                            "impact_edges_store_eq_in_memory",
                            "QueryAllTransports",
                            format!(
                                "impact({target:?}, depth={depth}): store(total={}, wi={:?}, edges={store_edges:?}) != mem(total={}, wi={:?}, edges={mem_edges:?})",
                                store_resp.total,
                                store_resp.walk_incomplete,
                                mem_resp.total,
                                mem_resp.walk_incomplete
                            ),
                        ));
                    }
                }
                StepOutcome::Ok
            }
            TransportQuery::Dependencies { target, budget } => {
                let req = Request {
                    query: target.clone(),
                    token_budget: *budget,
                    min_confidence: 0.0,
                    max_depth: 1,
                };
                let store_resp = match store_engine.dependencies(req.clone()) {
                    Ok(r) => r,
                    Err(err) => {
                        return StepOutcome::CheckFailed(CheckFailure::new(
                            "store_deps_ok",
                            "QueryAllTransports",
                            format!("store deps({target:?}) failed: {err:#}"),
                        ));
                    }
                };
                let store_json =
                    normalize_transport_json(serde_json::to_value(&store_resp).unwrap());
                let ipc_json = normalize_transport_json(
                    ipc_request(
                        Arc::clone(&self.store),
                        json!({
                            "version": PROTOCOL_VERSION,
                            "cmd": "deps",
                            "target": target,
                            "budget": budget,
                        }),
                    )
                    .await,
                );
                let mcp_json = normalize_transport_json(
                    mcp_call_tool(
                        &self.slot,
                        "devmap_dependencies",
                        json!({
                            "target": target,
                            "budget": budget,
                        }),
                    )
                    .await,
                );

                if let Err(f) =
                    check_envelope_invariants(&store_json, "QueryAllTransports", "deps.store")
                {
                    return StepOutcome::CheckFailed(f);
                }
                if store_json != ipc_json {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "deps_store_eq_ipc",
                        "QueryAllTransports",
                        format!(
                            "deps({target:?}): Store != IPC\nstore={store_json}\nipc={ipc_json}"
                        ),
                    ));
                }
                if store_json != mcp_json {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "deps_store_eq_mcp",
                        "QueryAllTransports",
                        format!(
                            "deps({target:?}): Store != MCP\nstore={store_json}\nmcp={mcp_json}"
                        ),
                    ));
                }

                // InMemory parity on dependencies
                let mem_resp = mem_engine.dependencies(req);
                let store_avail =
                    matches!(store_resp.resolution, ResolutionAvailability::Available);
                let mem_avail = matches!(mem_resp.resolution, ResolutionAvailability::Available);
                if store_avail != mem_avail {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "deps_availability_store_eq_in_memory",
                        "QueryAllTransports",
                        format!(
                            "deps({target:?}): store resolution={:?} != in_memory resolution={:?}",
                            store_resp.resolution, mem_resp.resolution
                        ),
                    ));
                }
                if store_avail && !store_resp.truncated && !mem_resp.truncated {
                    let store_edges: BTreeSet<_> = store_resp
                        .items
                        .iter()
                        .map(|e| {
                            (
                                e.source_file.clone(),
                                e.source_symbol.clone(),
                                e.target_file.clone(),
                                e.target_symbol.clone(),
                            )
                        })
                        .collect();
                    let mem_edges: BTreeSet<_> = mem_resp
                        .items
                        .iter()
                        .map(|e| {
                            (
                                e.source_file.clone(),
                                e.source_symbol.clone(),
                                e.target_file.clone(),
                                e.target_symbol.clone(),
                            )
                        })
                        .collect();
                    if store_edges != mem_edges
                        || store_resp.total != mem_resp.total
                        || store_resp.truncated != mem_resp.truncated
                    {
                        return StepOutcome::CheckFailed(CheckFailure::new(
                            "deps_edges_store_eq_in_memory",
                            "QueryAllTransports",
                            format!(
                                "deps({target:?}): store(total={}, edges={store_edges:?}) != mem(total={}, edges={mem_edges:?})",
                                store_resp.total, mem_resp.total
                            ),
                        ));
                    }
                }
                StepOutcome::Ok
            }
            TransportQuery::DeadSymbols { budget } => {
                let store_resp = match store_engine.dead_symbols(*budget) {
                    Ok(r) => r,
                    Err(err) => {
                        return StepOutcome::CheckFailed(CheckFailure::new(
                            "store_dead_ok",
                            "QueryAllTransports",
                            format!("store dead_symbols({budget}) failed: {err:#}"),
                        ));
                    }
                };
                let store_json =
                    normalize_transport_json(serde_json::to_value(&store_resp).unwrap());
                let ipc_json = normalize_transport_json(
                    ipc_request(
                        Arc::clone(&self.store),
                        json!({
                            "version": PROTOCOL_VERSION,
                            "cmd": "dead",
                            "budget": budget,
                        }),
                    )
                    .await,
                );
                let mcp_json = normalize_transport_json(
                    mcp_call_tool(
                        &self.slot,
                        "devmap_dead_symbols",
                        json!({ "budget": budget }),
                    )
                    .await,
                );

                if let Err(f) =
                    check_envelope_invariants(&store_json, "QueryAllTransports", "dead.store")
                {
                    return StepOutcome::CheckFailed(f);
                }
                if store_json != ipc_json {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "dead_store_eq_ipc",
                        "QueryAllTransports",
                        format!("dead_symbols({budget}): Store != IPC\nstore={store_json}\nipc={ipc_json}"),
                    ));
                }
                if store_json != mcp_json {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "dead_store_eq_mcp",
                        "QueryAllTransports",
                        format!("dead_symbols({budget}): Store != MCP\nstore={store_json}\nmcp={mcp_json}"),
                    ));
                }
                StepOutcome::Ok
            }
        }
    }
}

fn replay_suite_a(
    rt: &tokio::runtime::Runtime,
    initial_files: &BTreeMap<String, String>,
    actions: &[SuiteAAction],
) -> Option<CheckFailure> {
    let mut state = SuiteAState::new(initial_files.clone());
    for action in actions {
        let outcome = rt.block_on(state.apply(action));
        if let Some(failure) = outcome.into_failure() {
            return Some(failure);
        }
    }
    None
}

#[test]
fn stateful_suite_a_preview_and_cross_transport_parity() {
    let started = Instant::now();
    let seed = env_u64("DEVMAP_STATEFUL_SEED", DEFAULT_SEED);
    let rounds = env_usize("DEVMAP_STATEFUL_ROUNDS", 28);
    let mut src = Source::from_seed(seed ^ 0xA0A0_0001);

    let initial_files = generate_micro_repo(&mut src, ALL_LANGS, 8);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    let mut state = SuiteAState::new(initial_files.clone());
    let mut tracker = HealthTracker::new();
    let mut history: Vec<SuiteAAction> = Vec::with_capacity(rounds);
    let mut prev_action: Option<&'static str> = None;

    let candidates: &[&'static str] = &[
        "EditDisk",
        "PreviewUnsaved",
        "RebuildStore",
        "QueryAllTransports",
    ];
    let mut query_cycle = 0usize;
    let mut preview_cycle = 0usize;

    for _step in 0..rounds {
        let chosen = tracker.pick_weighted(&mut src, candidates, prev_action);
        let paths: Vec<String> = state.files.keys().cloned().collect();
        let action = match chosen {
            "EditDisk" => {
                let path = src.choose(&paths).expect("non-empty repo").clone();
                let lang = lang_for_path(&path);
                let cur = &state.files[&path];
                let mutated = mutate_file_elements(&mut src, lang, cur);
                SuiteAAction::EditDisk {
                    path,
                    content: mutated,
                }
            }
            "PreviewUnsaved" => {
                let mode = preview_cycle % 5;
                preview_cycle += 1;
                match mode {
                    0 => {
                        // Remove a called symbol (`api_0` / `Api0` or `helper_0`) to exercise broken_callers!
                        let target_path = paths
                            .iter()
                            .find(|p| {
                                p.ends_with("mod_0.rs")
                                    || p.ends_with("mod_0.py")
                                    || p.ends_with("mod_0.ts")
                                    || p.ends_with("mod_0.go")
                            })
                            .cloned()
                            .unwrap_or_else(|| paths[0].clone());
                        let lang = lang_for_path(&target_path);
                        let cur = &state.files[&target_path];
                        let api_name = candidate_api_name(lang, &target_path);
                        if let Some((name, content)) =
                            api_name.and_then(|n| remove_block_defining(cur, &n).map(|c| (n, c)))
                        {
                            SuiteAAction::PreviewUnsaved {
                                path: target_path,
                                kind: PreviewKind::RemoveCalledSymbol {
                                    removed_name: name,
                                    content,
                                },
                            }
                        } else {
                            let mutated = mutate_file_elements(&mut src, lang, cur);
                            SuiteAAction::PreviewUnsaved {
                                path: target_path,
                                kind: PreviewKind::StructuralMutate(mutated),
                            }
                        }
                    }
                    1 => {
                        // Signature change on a symbol
                        let path = src.choose(&paths).unwrap().clone();
                        let lang = lang_for_path(&path);
                        let cur = &state.files[&path];
                        let target_name = candidate_helper_name(lang, &path)
                            .or_else(|| candidate_api_name(lang, &path));
                        if let Some((name, content)) = target_name
                            .and_then(|n| change_signature_of(lang, cur, &n).map(|c| (n, c)))
                        {
                            SuiteAAction::PreviewUnsaved {
                                path,
                                kind: PreviewKind::SignatureChange {
                                    target_name: name,
                                    content,
                                },
                            }
                        } else {
                            let mutated = mutate_file_elements(&mut src, lang, cur);
                            SuiteAAction::PreviewUnsaved {
                                path,
                                kind: PreviewKind::StructuralMutate(mutated),
                            }
                        }
                    }
                    2 => {
                        // Broken syntax via lower_damage
                        let path = src.choose(&paths).unwrap().clone();
                        let cur = &state.files[&path];
                        let damaged = lower_damage(&mut src, cur);
                        SuiteAAction::PreviewUnsaved {
                            path,
                            kind: PreviewKind::BrokenSyntax(damaged),
                        }
                    }
                    3 => {
                        // Brand-new unindexed file preview
                        let id = src.ident("unsaved");
                        let path = format!("src/{id}.rs");
                        let content = format!("pub fn {id}_fn(x: i32) -> i32 {{\n    x + 42\n}}\n");
                        SuiteAAction::PreviewUnsaved {
                            path,
                            kind: PreviewKind::UnindexedNewFile { content },
                        }
                    }
                    _ => {
                        let path = src.choose(&paths).unwrap().clone();
                        let lang = lang_for_path(&path);
                        let cur = &state.files[&path];
                        let mutated = mutate_file_elements(&mut src, lang, cur);
                        SuiteAAction::PreviewUnsaved {
                            path,
                            kind: PreviewKind::StructuralMutate(mutated),
                        }
                    }
                }
            }
            "RebuildStore" => SuiteAAction::RebuildStore,
            _ => {
                // Rotate across all 4 query types so every query surface is thoroughly checked.
                let qmode = query_cycle % 4;
                query_cycle += 1;
                let budget = if src.chance(1, 5) { 80 } else { 20_000 };
                let q = match qmode {
                    0 => {
                        let sym_names: Vec<String> = state
                            .extractions
                            .iter()
                            .flat_map(|e| e.symbols.iter().map(|s| s.name.clone()))
                            .filter(|n| {
                                n.starts_with("api_")
                                    || n.starts_with("helper_")
                                    || n.starts_with("Api")
                                    || n.starts_with("helper")
                            })
                            .collect();
                        let query = src
                            .choose(&sym_names)
                            .cloned()
                            .unwrap_or_else(|| "api_0".to_string());
                        TransportQuery::Search { query, budget }
                    }
                    1 => {
                        let targets: Vec<String> = state
                            .extractions
                            .iter()
                            .flat_map(|e| {
                                e.symbols
                                    .iter()
                                    .filter(|s| {
                                        s.name.starts_with("api_")
                                            || s.name.starts_with("helper_")
                                            || s.name.starts_with("Api")
                                            || s.name.starts_with("helper")
                                    })
                                    .map(|s| s.qualified_name.clone())
                            })
                            .collect();
                        let target = src
                            .choose(&targets)
                            .cloned()
                            .unwrap_or_else(|| paths[0].clone());
                        let depth = 1 + src.usize_below(4);
                        TransportQuery::Impact {
                            target,
                            budget,
                            depth,
                        }
                    }
                    2 => {
                        let target = if src.bool() {
                            src.choose(&paths).unwrap().clone()
                        } else {
                            let syms: Vec<String> = state
                                .extractions
                                .iter()
                                .flat_map(|e| e.symbols.iter().map(|s| s.qualified_name.clone()))
                                .collect();
                            src.choose(&syms)
                                .cloned()
                                .unwrap_or_else(|| paths[0].clone())
                        };
                        TransportQuery::Dependencies { target, budget }
                    }
                    _ => TransportQuery::DeadSymbols { budget },
                };
                SuiteAAction::QueryAllTransports(q)
            }
        };

        let act_name = action.name();
        let outcome = rt.block_on(state.apply(&action));
        tracker.record(act_name, prev_action, &outcome);
        history.push(action);
        prev_action = Some(act_name);

        if let Some(failure) = outcome.into_failure() {
            let sig = failure.signature();
            let minimized = minimize_sequence(&history, &sig, 40, |seq| {
                replay_suite_a(&rt, &initial_files, seq)
            });
            let action_strs: Vec<String> = minimized.iter().map(|a| format!("{a:?}")).collect();
            let repro = format_repro_test(
                "repro_suite_a_preview_transport",
                seed,
                &failure,
                &action_strs,
            );
            panic!(
                "Suite A oracle violation: {failure}\n{}\n\nMinimized reproducer ({} -> {} steps):\n{repro}",
                tracker.probe_report("SuiteA:preview_transport_parity"),
                history.len(),
                minimized.len(),
            );
        }
    }

    println!(
        "{}elapsed={:?}",
        tracker.probe_report("SuiteA:preview_transport_parity"),
        started.elapsed()
    );
    tracker.assert_actions_exercised(candidates);
}

// ===========================================================================
// Suite B: `blast` / `suspects` Duality Over Synthetic Multi-Commit Git Repos
// ===========================================================================

fn run_git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Stateful Tester")
        .env("GIT_AUTHOR_EMAIL", "stateful@example.invalid")
        .env("GIT_COMMITTER_NAME", "Stateful Tester")
        .env("GIT_COMMITTER_EMAIL", "stateful@example.invalid")
        .stderr(Stdio::piped())
        .output()
        .unwrap_or_else(|err| panic!("failed to run git {args:?}: {err}"));
    assert!(
        out.status.success(),
        "git {args:?} failed in {repo:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[derive(Clone, Debug)]
enum GitCommitOp {
    EditFunctionBody {
        path: String,
        target_fn: String,
        delta: i32,
    },
    AddFunctionAndCaller {
        path: String,
        new_fn: String,
        caller_fn: String,
        delta: i32,
    },
    EditModuleLevel {
        path: String,
        banner: String,
    },
    DeleteDeadFunction {
        path: String,
        target_fn: String,
    },
    RewireCrossFileCall {
        caller_path: String,
        caller_fn: String,
        callee_fn: String,
        delta: i32,
    },
}

impl GitCommitOp {
    fn name(&self) -> &'static str {
        match self {
            Self::EditFunctionBody { .. } => "EditFunctionBody",
            Self::AddFunctionAndCaller { .. } => "AddFunctionAndCaller",
            Self::EditModuleLevel { .. } => "EditModuleLevel",
            Self::DeleteDeadFunction { .. } => "DeleteDeadFunction",
            Self::RewireCrossFileCall { .. } => "RewireCrossFileCall",
        }
    }
}

/// Insert a statement line inside the body of `target_fn` in `content`.
fn inject_body_line(lang: MicroLang, content: &str, target_fn: &str, stmt: &str) -> Option<String> {
    let mut lines: Vec<String> = content.lines().map(String::from).collect();
    let header_needle = match lang {
        MicroLang::Rust => format!("fn {target_fn}("),
        MicroLang::TypeScript => format!("function {target_fn}("),
        MicroLang::Go => format!("func {target_fn}("),
        MicroLang::Python => format!("def {target_fn}("),
    };
    let pos = lines.iter().position(|l| l.contains(&header_needle))?;
    if pos + 1 >= lines.len() {
        return None;
    }
    lines.insert(pos + 1, stmt.to_string());
    let mut out = lines.join("\n");
    out.push('\n');
    Some(out)
}

fn stmt_local(lang: MicroLang, delta: i32) -> String {
    match lang {
        MicroLang::Rust => format!("    let _stateful_v = {delta};"),
        MicroLang::TypeScript => format!("  const _stateful_v = {delta};"),
        MicroLang::Go => format!("\t_stateful_v := {delta}; _ = _stateful_v"),
        MicroLang::Python => format!("    _stateful_v = {delta}"),
    }
}

fn stmt_call(lang: MicroLang, callee: &str, delta: i32) -> String {
    match lang {
        MicroLang::Rust => format!("    let _stateful_c = {callee}({delta});"),
        MicroLang::TypeScript => format!("  const _stateful_c = {callee}({delta});"),
        MicroLang::Go => format!("\t_ = {callee}({delta})"),
        MicroLang::Python => format!("    _stateful_c = {callee}({delta})"),
    }
}

fn new_fn_decl(lang: MicroLang, name: &str, delta: i32) -> String {
    match lang {
        MicroLang::Rust => format!("\npub fn {name}(x: i32) -> i32 {{\n    x + {delta}\n}}\n"),
        MicroLang::TypeScript => {
            format!("\nexport function {name}(x: number): number {{\n  return x + {delta};\n}}\n")
        }
        MicroLang::Go => format!("\nfunc {name}(x int) int {{\n\treturn x + {delta}\n}}\n"),
        MicroLang::Python => format!("\ndef {name}(x: int) -> int:\n    return x + {delta}\n"),
    }
}

fn module_level_line(lang: MicroLang, banner: &str) -> String {
    match lang {
        MicroLang::Rust => format!("// {banner}\n"),
        MicroLang::TypeScript => format!("// {banner}\n"),
        MicroLang::Go => format!("// {banner}\n"),
        MicroLang::Python => format!("# {banner}\n"),
    }
}

fn apply_git_op(files: &mut BTreeMap<String, String>, op: &GitCommitOp) -> bool {
    match op {
        GitCommitOp::EditFunctionBody {
            path,
            target_fn,
            delta,
        } => {
            let Some(cur) = files.get(path) else {
                return false;
            };
            let lang = lang_for_path(path);
            let Some(updated) = inject_body_line(lang, cur, target_fn, &stmt_local(lang, *delta))
            else {
                return false;
            };
            files.insert(path.clone(), updated);
            true
        }
        GitCommitOp::AddFunctionAndCaller {
            path,
            new_fn,
            caller_fn,
            delta,
        } => {
            let Some(cur) = files.get(path) else {
                return false;
            };
            let lang = lang_for_path(path);
            let mut updated = format!("{cur}{}", new_fn_decl(lang, new_fn, *delta));
            if let Some(with_call) =
                inject_body_line(lang, &updated, caller_fn, &stmt_call(lang, new_fn, *delta))
            {
                updated = with_call;
            }
            files.insert(path.clone(), updated);
            true
        }
        GitCommitOp::EditModuleLevel { path, banner } => {
            let Some(cur) = files.get(path) else {
                return false;
            };
            let lang = lang_for_path(path);
            let line = module_level_line(lang, banner);
            let updated = if lang == MicroLang::Go {
                // Keep `package micro` at top; place comment right after package line
                if let Some((first, rest)) = cur.split_once('\n') {
                    format!("{first}\n{line}{rest}")
                } else {
                    format!("{cur}\n{line}")
                }
            } else {
                format!("{line}{cur}")
            };
            files.insert(path.clone(), updated);
            true
        }
        GitCommitOp::DeleteDeadFunction { path, target_fn } => {
            let Some(cur) = files.get(path) else {
                return false;
            };
            let Some(updated) = remove_block_defining(cur, target_fn) else {
                return false;
            };
            files.insert(path.clone(), updated);
            true
        }
        GitCommitOp::RewireCrossFileCall {
            caller_path,
            caller_fn,
            callee_fn,
            delta,
        } => {
            let Some(cur) = files.get(caller_path) else {
                return false;
            };
            let lang = lang_for_path(caller_path);
            let Some(updated) =
                inject_body_line(lang, cur, caller_fn, &stmt_call(lang, callee_fn, *delta))
            else {
                return false;
            };
            files.insert(caller_path.clone(), updated);
            true
        }
    }
}

/// Replay a full synthetic git commit sequence and run all `blast` <-> `suspects`
/// duality, honesty, and transport-parity oracles.
fn run_suite_b_scenario(
    rt: &tokio::runtime::Runtime,
    initial_files: &BTreeMap<String, String>,
    ops: &[GitCommitOp],
    depth: u32,
    tracker: Option<&mut HealthTracker>,
) -> Option<CheckFailure> {
    let guard = TempDirGuard::new("suite-b");
    let root = guard.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    run_git(&root, &["init", "-q"]);
    run_git(&root, &["config", "user.email", "stateful@example.invalid"]);
    run_git(&root, &["config", "user.name", "Stateful Tester"]);

    let mut files = initial_files.clone();
    for (rel, content) in &files {
        write_repo_file(&root, rel, content);
    }
    run_git(&root, &["add", "-A"]);
    run_git(&root, &["commit", "-q", "-m", "base"]);
    let base_sha = run_git(&root, &["rev-parse", "HEAD"]);

    let mut commit_shas = vec![base_sha.clone()];
    let mut applied_ops: Vec<(GitCommitOp, String, String)> = Vec::new();
    let mut tracker_opt = tracker;
    let mut prev_act: Option<&'static str> = None;

    for (idx, op) in ops.iter().enumerate() {
        let act_name = op.name();
        if !apply_git_op(&mut files, op) {
            if let Some(t) = tracker_opt.as_deref_mut() {
                t.record(act_name, prev_act, &bail("op_target_absent"));
            }
            continue;
        }
        for (rel, content) in &files {
            write_repo_file(&root, rel, content);
        }
        run_git(&root, &["add", "-A"]);
        let diff_check = Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["diff", "--cached", "--quiet"])
            .status()
            .expect("git diff --cached");
        if diff_check.success() {
            if let Some(t) = tracker_opt.as_deref_mut() {
                t.record(act_name, prev_act, &bail("no_effective_diff"));
            }
            continue;
        }
        let parent_sha = commit_shas.last().unwrap().clone();
        run_git(
            &root,
            &["commit", "-q", "-m", &format!("step-{idx}-{act_name}")],
        );
        let sha = run_git(&root, &["rev-parse", "HEAD"]);
        commit_shas.push(sha.clone());
        applied_ops.push((op.clone(), parent_sha, sha));
        if let Some(t) = tracker_opt.as_deref_mut() {
            t.record(act_name, prev_act, &StepOutcome::Ok);
        }
        prev_act = Some(act_name);
    }

    if applied_ops.is_empty() {
        return None;
    }

    let head_sha = commit_shas.last().unwrap().clone();
    let parent_of_head = commit_shas[commit_shas.len() - 2].clone();
    let last_op = &applied_ops.last().unwrap().0;

    let db_path = guard.path().join("devmap.sqlite");
    let store = Arc::new(Store::open(&db_path).expect("open suite-b store"));
    if let Err(err) = build_generation_from_disk(&store, &root) {
        return Some(CheckFailure::new(
            "suite_b_build_store",
            "DualityCheck",
            format!("store build failed: {err:#}"),
        ));
    }
    let graph = StoreGraph::new(&store, &root).expect("StoreGraph::new");
    if graph.indexed_head() != Some(head_sha.as_str()) {
        return Some(CheckFailure::new(
            "store_graph_head_matches_git_head",
            "DualityCheck",
            format!(
                "StoreGraph head {:?} != git HEAD {head_sha}",
                graph.indexed_head()
            ),
        ));
    }

    // 1. Blast for the latest commit `parent_of_head..head_sha`
    let blast_last = blast(&root, &graph, &parent_of_head, &head_sha, depth);
    if blast_last.complete != blast_last.unavailable.is_empty() {
        return Some(CheckFailure::new(
            "blast_complete_matches_unavailable",
            "DualityCheck",
            format!(
                "blast.complete={} != unavailable.is_empty() (unavailable={:?})",
                blast_last.complete, blast_last.unavailable
            ),
        ));
    }

    // If the latest commit edited module-level lines outside symbols, blast MUST
    // honestly report `unattributed` and `complete == false`!
    if matches!(last_op, GitCommitOp::EditModuleLevel { .. }) {
        let has_unattr_unavail = blast_last
            .unavailable
            .iter()
            .any(|u| matches!(u, Unavailable::ChangeUnattributed { .. }));
        if blast_last.unattributed.is_empty() || !has_unattr_unavail || blast_last.complete {
            return Some(CheckFailure::new(
                "blast_unattributed_module_level_honest",
                "EditModuleLevel",
                format!(
                    "module-level edit was not flagged unattributed: unattributed={:?}, unavailable={:?}, complete={}",
                    blast_last.unattributed, blast_last.unavailable, blast_last.complete
                ),
            ));
        }
    }

    let blast_capped = blast_last.unavailable.iter().any(|u| {
        matches!(
            u,
            Unavailable::ImpactIncomplete { .. }
                | Unavailable::SeedsCapped { .. }
                | Unavailable::ChangedFilesCapped { .. }
                | Unavailable::BudgetExhausted { .. }
        )
    });

    // 2. Duality Direction 1 (`blast => suspects`) on `parent_of_head..head_sha`:
    // Every non-deletion-only seed S, and every impacted target T (when all seeds
    // are non-deletion-only), must reach `head_sha` in `suspects(T, parent_of_head, head_sha, depth)`.
    let all_seeds_non_deletion =
        !blast_last.seeds.is_empty() && blast_last.seeds.iter().all(|s| !s.deletion_only);

    let mut targets_to_check: Vec<String> = blast_last
        .seeds
        .iter()
        .filter(|s| !s.deletion_only)
        .map(|s| s.qualified_name.clone())
        .collect();
    if all_seeds_non_deletion {
        for imp in &blast_last.impacted {
            targets_to_check.push(imp.qualified_name.clone());
        }
    }

    for target in &targets_to_check {
        let susp = suspects(&root, &graph, target, &parent_of_head, &head_sha, depth);
        if susp.complete != susp.unavailable.is_empty() {
            return Some(CheckFailure::new(
                "suspects_complete_matches_unavailable",
                "DualityCheck",
                format!(
                    "suspects({target}).complete={} != unavailable.is_empty() ({:?})",
                    susp.complete, susp.unavailable
                ),
            ));
        }
        let susp_capped = susp.unavailable.iter().any(|u| {
            matches!(
                u,
                Unavailable::ConeIncomplete { .. }
                    | Unavailable::FilesCapped { .. }
                    | Unavailable::WindowCapped { .. }
                    | Unavailable::BudgetExhausted { .. }
            )
        });
        if !blast_capped && !susp_capped {
            let found_head = susp.suspects.iter().any(|s| s.commit == head_sha);
            if !found_head {
                return Some(CheckFailure::new(
                    "duality_blast_implies_suspects",
                    "DualityCheck",
                    format!(
                        "target {target:?} was in blast({parent_of_head}..{head_sha}, depth={depth}) \
                         but suspects({target:?}) did not contain {head_sha}: suspects={:?}, unavailable={:?}",
                        susp.suspects, susp.unavailable
                    ),
                ));
            }
        }
    }

    // 3. Duality Direction 2 (`suspects => blast`) on `parent_of_head..head_sha`:
    // For every symbol in the repo, if `suspects(sym, parent_of_head, head_sha, depth)`
    // blames `head_sha`, then either `sym` itself is in `blast_last.seeds ∪ impacted`,
    // OR `sym` is a container symbol that reached `blast_reach` via forward `Contains`/`Defines`
    // (which `traverse_graph_indexed` intentionally excludes in reverse/impact direction).
    if !blast_capped {
        let blast_reach: BTreeSet<String> = blast_last
            .seeds
            .iter()
            .map(|s| s.qualified_name.clone())
            .chain(blast_last.impacted.iter().map(|i| i.qualified_name.clone()))
            .collect();

        // Precompute containers that have an outgoing `Contains`/`Defines` edge to a
        // symbol in `blast_reach` (or transitively via containment).
        let containment_edges: Vec<(String, String)> = store
            .latest_edges(0.0)
            .expect("latest_edges")
            .into_iter()
            .filter(|e| e.edge_kind == "Contains" || e.edge_kind == "Defines")
            .map(|e| (e.source_symbol, e.target_symbol))
            .collect();
        let mut container_of_reached = blast_reach.clone();
        let mut grew = true;
        while grew {
            grew = false;
            for (parent, child) in &containment_edges {
                if container_of_reached.contains(child)
                    && container_of_reached.insert(parent.clone())
                {
                    grew = true;
                }
            }
        }

        let all_syms: Vec<String> = store
            .all_symbols()
            .expect("all_symbols")
            .into_iter()
            .filter(|s| s.qualified_name.contains("::"))
            .map(|s| s.qualified_name)
            .collect();
        for sym in all_syms.iter().take(8) {
            let susp = suspects(&root, &graph, sym, &parent_of_head, &head_sha, depth);
            let blames_head = susp.suspects.iter().any(|s| s.commit == head_sha);
            if blames_head && !container_of_reached.contains(sym) {
                return Some(CheckFailure::new(
                    "duality_suspects_implies_blast",
                    "DualityCheck",
                    format!(
                        "suspects({sym:?}, {parent_of_head}..{head_sha}, depth={depth}) blamed {head_sha}, \
                         but {sym:?} was NOT in blast(seeds U impacted)={blast_reach:?} or its containers"
                    ),
                ));
            }
        }
    }

    // 4. Full-window `base_sha..head_sha` check: every commit reported by `suspects`
    // must belong to `(base_sha..head_sha]`, and if `head_sha` was found in the single-step
    // suspects for a target, it must also appear in the full-window suspects!
    let window_set: BTreeSet<&str> = commit_shas[1..].iter().map(String::as_str).collect();
    if let Some(sample_target) = targets_to_check.first() {
        let susp_full = suspects(&root, &graph, sample_target, &base_sha, &head_sha, depth);
        if susp_full.complete != susp_full.unavailable.is_empty() {
            return Some(CheckFailure::new(
                "suspects_full_window_complete_flag",
                "DualityCheck",
                format!("full window complete mismatch: {susp_full:?}"),
            ));
        }
        for s in &susp_full.suspects {
            if !window_set.contains(s.commit.as_str()) {
                return Some(CheckFailure::new(
                    "suspects_commit_in_window",
                    "DualityCheck",
                    format!("suspect commit {} outside window {window_set:?}", s.commit),
                ));
            }
        }
        if !blast_capped
            && susp_full.complete
            && !susp_full.suspects.iter().any(|s| s.commit == head_sha)
        {
            return Some(CheckFailure::new(
                "full_window_preserves_head_suspect",
                "DualityCheck",
                format!(
                    "target {sample_target:?} blamed {head_sha} in 1-step window but not in {base_sha}..{head_sha}: {:?}",
                    susp_full.suspects
                ),
            ));
        }

        // 5. Cross-transport parity for `blast` and `suspects` (Direct == IPC == MCP)
        let slot = Arc::new(StoreSlot::ready(&db_path, Arc::clone(&store)));
        let direct_blast = normalize_transport_json(serde_json::to_value(&blast_last).unwrap());
        let ipc_blast = normalize_transport_json(rt.block_on(ipc_request(
            Arc::clone(&store),
            json!({
                "version": PROTOCOL_VERSION,
                "cmd": "blast",
                "since": parent_of_head,
                "depth": depth,
            }),
        )));
        let mcp_blast = normalize_transport_json(rt.block_on(mcp_call_tool(
            &slot,
            "devmap_blast",
            json!({
                "since": parent_of_head,
                "depth": depth,
            }),
        )));
        if direct_blast != ipc_blast || direct_blast != mcp_blast {
            return Some(CheckFailure::new(
                "blast_transport_parity",
                "DualityCheck",
                format!("blast transport drift:\ndirect={direct_blast}\nipc={ipc_blast}\nmcp={mcp_blast}"),
            ));
        }

        let direct_susp = normalize_transport_json(serde_json::to_value(&susp_full).unwrap());
        let ipc_susp = normalize_transport_json(rt.block_on(ipc_request(
            Arc::clone(&store),
            json!({
                "version": PROTOCOL_VERSION,
                "cmd": "suspects",
                "symptom": sample_target,
                "since": base_sha,
                "depth": depth,
            }),
        )));
        let mcp_susp = normalize_transport_json(rt.block_on(mcp_call_tool(
            &slot,
            "devmap_suspects",
            json!({
                "symptom": sample_target,
                "since": base_sha,
                "depth": depth,
            }),
        )));
        if direct_susp != ipc_susp || direct_susp != mcp_susp {
            return Some(CheckFailure::new(
                "suspects_transport_parity",
                "DualityCheck",
                format!("suspects transport drift:\ndirect={direct_susp}\nipc={ipc_susp}\nmcp={mcp_susp}"),
            ));
        }
    }

    // 6. Honesty on basis mismatch: running `blast` against an older `until` commit
    // whose content differs from the indexed `head_sha` must NOT return a clean report.
    if commit_shas.len() >= 3 {
        let older_until = &commit_shas[commit_shas.len() - 2];
        let mismatch_blast = blast(&root, &graph, &base_sha, older_until, depth);
        if mismatch_blast.complete && !mismatch_blast.changed_files.is_empty() {
            // Check whether any changed file actually differs between older_until and head_sha.
            let changed_since_older =
                run_git(&root, &["diff", "--name-only", older_until, &head_sha]);
            let overlap = mismatch_blast
                .changed_files
                .iter()
                .any(|cf| changed_since_older.lines().any(|l| l == cf.path));
            if overlap {
                return Some(CheckFailure::new(
                    "blast_basis_mismatch_honest",
                    "DualityCheck",
                    format!(
                        "blast against mismatched until={older_until} (indexed head={head_sha}) reported complete=true: {mismatch_blast:?}"
                    ),
                ));
            }
        }
    }

    if let Some(t) = tracker_opt {
        t.record("DualityCheck", prev_act, &StepOutcome::Ok);
    }
    None
}

#[test]
fn stateful_suite_b_blast_suspects_duality() {
    let started = Instant::now();
    let seed = env_u64("DEVMAP_STATEFUL_SEED", DEFAULT_SEED);
    let rounds = env_usize("DEVMAP_STATEFUL_ROUNDS", 3);
    let mut src = Source::from_seed(seed ^ 0xB0B0_0002);

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    let mut tracker = HealthTracker::new();
    let op_candidates: &[&'static str] = &[
        "EditFunctionBody",
        "AddFunctionAndCaller",
        "EditModuleLevel",
        "DeleteDeadFunction",
        "RewireCrossFileCall",
    ];

    for round in 0..rounds {
        let initial_files = generate_micro_repo(&mut src, ALL_LANGS, 8);
        let paths: Vec<String> = initial_files.keys().cloned().collect();
        let num_commits = 4 + src.usize_below(4); // 4..=7 commits
        let depth = 1 + (src.usize_below(4) as u32); // 1..=4

        let mut ops: Vec<GitCommitOp> = Vec::with_capacity(num_commits);
        let mut prev_op: Option<&'static str> = None;

        for c in 0..num_commits {
            // Ensure at least one EditModuleLevel lands as a final commit on one round,
            // and at least one EditFunctionBody lands as a final commit on other rounds!
            let chosen = if c == num_commits - 1 && round % 2 == 0 {
                "EditFunctionBody"
            } else if c == num_commits - 1 && round % 2 == 1 {
                "EditModuleLevel"
            } else {
                tracker.pick_weighted(&mut src, op_candidates, prev_op)
            };
            prev_op = Some(chosen);

            let op = match chosen {
                "EditFunctionBody" => {
                    // Prefer mod_0 (which has cross-file callers in mod_1!) so blast.impacted is non-empty.
                    let mod0_paths: Vec<String> = paths
                        .iter()
                        .filter(|p| p.contains("mod_0."))
                        .cloned()
                        .collect();
                    let path = src
                        .choose(&mod0_paths)
                        .or_else(|| src.choose(&paths))
                        .unwrap()
                        .clone();
                    let lang = lang_for_path(&path);
                    let target_fn = if src.bool() {
                        candidate_api_name(lang, &path)
                    } else {
                        candidate_helper_name(lang, &path)
                    }
                    .unwrap_or_else(|| "api_0".to_string());
                    GitCommitOp::EditFunctionBody {
                        path,
                        target_fn,
                        delta: (src.usize_below(500) + 1) as i32,
                    }
                }
                "AddFunctionAndCaller" => {
                    let path = src.choose(&paths).unwrap().clone();
                    let lang = lang_for_path(&path);
                    let new_fn = if lang == MicroLang::Go {
                        format!("AddedFn_{round}_{c}")
                    } else {
                        format!("added_fn_{round}_{c}")
                    };
                    let caller_fn =
                        candidate_api_name(lang, &path).unwrap_or_else(|| "api_0".to_string());
                    GitCommitOp::AddFunctionAndCaller {
                        path,
                        new_fn,
                        caller_fn,
                        delta: (src.usize_below(200) + 1) as i32,
                    }
                }
                "EditModuleLevel" => {
                    let path = src.choose(&paths).unwrap().clone();
                    GitCommitOp::EditModuleLevel {
                        path,
                        banner: format!("stateful module banner r{round}_c{c}"),
                    }
                }
                "DeleteDeadFunction" => {
                    let path = src.choose(&paths).unwrap().clone();
                    let lang = lang_for_path(&path);
                    let stem = Path::new(&path)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("mod_0");
                    let idx = stem.strip_prefix("mod_").unwrap_or("0");
                    let target_fn = match lang {
                        MicroLang::Rust => format!("dead_helper_{idx}"),
                        MicroLang::TypeScript => format!("deadHelper_{idx}"),
                        MicroLang::Go => format!("deadHelper{idx}"),
                        MicroLang::Python => format!("_dead_helper_{idx}"),
                    };
                    GitCommitOp::DeleteDeadFunction { path, target_fn }
                }
                _ => {
                    // Rewire a call inside mod_1 to also call helper or api in mod_0 / mod_1.
                    let mod1_paths: Vec<String> = paths
                        .iter()
                        .filter(|p| p.contains("mod_1."))
                        .cloned()
                        .collect();
                    let caller_path = src
                        .choose(&mod1_paths)
                        .or_else(|| src.choose(&paths))
                        .unwrap()
                        .clone();
                    let lang = lang_for_path(&caller_path);
                    let caller_fn = candidate_api_name(lang, &caller_path)
                        .unwrap_or_else(|| "api_1".to_string());
                    let callee_fn = match lang {
                        MicroLang::Go => "Api0".to_string(),
                        _ => "api_0".to_string(),
                    };
                    GitCommitOp::RewireCrossFileCall {
                        caller_path,
                        caller_fn,
                        callee_fn,
                        delta: (src.usize_below(100) + 1) as i32,
                    }
                }
            };
            ops.push(op);
        }

        if let Some(failure) =
            run_suite_b_scenario(&rt, &initial_files, &ops, depth, Some(&mut tracker))
        {
            let sig = failure.signature();
            let minimized = minimize_sequence(&ops, &sig, 30, |seq| {
                run_suite_b_scenario(&rt, &initial_files, seq, depth, None)
            });
            let action_strs: Vec<String> = minimized.iter().map(|a| format!("{a:?}")).collect();
            let repro = format_repro_test(
                "repro_suite_b_blast_suspects_duality",
                seed,
                &failure,
                &action_strs,
            );
            panic!(
                "Suite B duality violation (round {round}, depth={depth}): {failure}\n{}\n\nMinimized reproducer ({} -> {} commits):\n{repro}",
                tracker.probe_report("SuiteB:blast_suspects_duality"),
                ops.len(),
                minimized.len(),
            );
        }
    }

    println!(
        "{}elapsed={:?}",
        tracker.probe_report("SuiteB:blast_suspects_duality"),
        started.elapsed()
    );
    tracker.assert_actions_exercised(&[
        "EditFunctionBody",
        "AddFunctionAndCaller",
        "EditModuleLevel",
        "DeleteDeadFunction",
        "RewireCrossFileCall",
        "DualityCheck",
    ]);
}

// ===========================================================================
// Suite C: Virtualized Debounce Collapse + Daemon Drain State Machine
// ===========================================================================

/// Virtualized model of `watcher.rs::DebounceBuffer` (`MAX_DEBOUNCE_PATHS = 4096`)
/// for synthesizing bursts over 4,096 paths without OS sleeps or `notify` timing.
const VIRTUAL_DEBOUNCE_CAP: usize = 4_096;

struct VirtualDebounce {
    pending: BTreeSet<String>,
    collapse_to: String,
    collapsed: bool,
}

impl VirtualDebounce {
    fn new(root: &Path) -> Self {
        Self {
            pending: BTreeSet::new(),
            collapse_to: root.to_string_lossy().into_owned(),
            collapsed: false,
        }
    }

    fn push_burst<I: IntoIterator<Item = String>>(&mut self, paths: I) {
        if self.collapsed {
            return;
        }
        self.pending.extend(paths);
        if self.pending.len() > VIRTUAL_DEBOUNCE_CAP {
            self.pending.clear();
            self.pending.insert(self.collapse_to.clone());
            self.collapsed = true;
        }
    }

    fn flush(&mut self) -> Vec<String> {
        self.collapsed = false;
        std::mem::take(&mut self.pending).into_iter().collect()
    }
}

#[derive(Clone, Debug)]
enum SuiteCAction {
    Create {
        path: String,
        content: String,
    },
    Edit {
        path: String,
        content: String,
    },
    Delete {
        path: String,
    },
    Rename {
        old_path: String,
        new_path: String,
    },
    CollapseOver4096 {
        edited_path: String,
        edited_content: String,
        extra_synthetic_count: usize,
    },
    UnreadablePath {
        poison_path: String,
        healthy_mate_path: String,
        healthy_mate_content: String,
    },
    VanishedFile {
        vanish_path: String,
        live_mate_path: String,
        live_mate_content: String,
    },
    DrainAndVerifyColdParity,
}

impl SuiteCAction {
    fn name(&self) -> &'static str {
        match self {
            Self::Create { .. } => "Create",
            Self::Edit { .. } => "Edit",
            Self::Delete { .. } => "Delete",
            Self::Rename { .. } => "Rename",
            Self::CollapseOver4096 { .. } => "CollapseOver4096",
            Self::UnreadablePath { .. } => "UnreadablePath",
            Self::VanishedFile { .. } => "VanishedFile",
            Self::DrainAndVerifyColdParity => "DrainAndVerifyColdParity",
        }
    }
}

fn read_pending_state(db_path: &Path) -> (String, i64) {
    let conn = rusqlite::Connection::open(db_path).expect("open sqlite for pending_state");
    conn.query_row(
        "SELECT epoch, revision FROM pending_state WHERE singleton = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .expect("read pending_state")
}

#[derive(Debug, PartialEq, Eq)]
struct CanonicalStoreSnapshot {
    file_hashes: BTreeMap<String, u64>,
    symbols: BTreeSet<(String, String, String, usize, usize)>,
    edges: BTreeSet<(String, String, String, String, String, i64)>,
    dead_symbols: BTreeSet<(String, String, i64)>,
}

fn snapshot_store(store: &Store) -> CanonicalStoreSnapshot {
    let file_hashes = store.latest_file_hashes().expect("latest_file_hashes");
    let symbols = store
        .all_symbols()
        .expect("all_symbols")
        .into_iter()
        .map(|s| (s.path, s.qualified_name, s.kind, s.span_start, s.span_end))
        .collect();
    let edges = store
        .latest_edges(0.0)
        .expect("latest_edges")
        .into_iter()
        .map(|e| {
            (
                e.source_file,
                e.source_symbol,
                e.target_file,
                e.target_symbol,
                e.edge_kind,
                (e.confidence * 1000.0).round() as i64,
            )
        })
        .collect();
    let engine = StoreQueryEngine::new(store);
    let dead_resp = engine.dead_symbols(50_000).expect("dead_symbols");
    let dead_symbols = dead_resp
        .items
        .into_iter()
        .map(|d| {
            (
                d.file_path,
                d.symbol_name,
                (d.confidence * 1000.0).round() as i64,
            )
        })
        .collect();
    CanonicalStoreSnapshot {
        file_hashes,
        symbols,
        edges,
        dead_symbols,
    }
}

struct SuiteCState {
    _guard: TempDirGuard,
    root: PathBuf,
    db_path: PathBuf,
    daemon: Daemon,
    reader: Store,
    files: BTreeMap<String, String>,
    debounce: VirtualDebounce,
    expected_epoch: String,
    last_revision: i64,
    active_poisons: BTreeSet<String>,
}

impl SuiteCState {
    fn new(initial_files: BTreeMap<String, String>) -> Self {
        let guard = TempDirGuard::new("suite-c");
        let root = guard.path().join("repo");
        fs::create_dir_all(&root).unwrap();
        for (rel, content) in &initial_files {
            write_repo_file(&root, rel, content);
        }
        let db_path = guard.path().join("devmap.sqlite");
        let daemon = Daemon::new(Store::open(&db_path).expect("daemon store"), root.clone());
        let reader = Store::open(&db_path).expect("reader store");

        let enqueued = daemon
            .reconcile_connect_time()
            .expect("initial reconcile_connect_time");
        assert_eq!(enqueued, initial_files.len());
        let drained = daemon.drain_pending_batch().expect("initial drain");
        assert_eq!(drained, initial_files.len());

        let (epoch, revision) = read_pending_state(&db_path);
        let wm = reader.pending_watermark().expect("pending_watermark");
        assert_eq!(wm.epoch(), epoch);

        let debounce = VirtualDebounce::new(&root);
        Self {
            _guard: guard,
            root,
            db_path,
            daemon,
            reader,
            files: initial_files,
            debounce,
            expected_epoch: epoch,
            last_revision: revision,
            active_poisons: BTreeSet::new(),
        }
    }

    fn check_watermark_monotonic(
        &mut self,
        action: &'static str,
        must_strictly_advance: bool,
    ) -> Result<(), CheckFailure> {
        let wm = self.reader.pending_watermark().expect("pending_watermark");
        let (epoch, revision) = read_pending_state(&self.db_path);
        if wm.epoch() != self.expected_epoch || epoch != self.expected_epoch {
            return Err(CheckFailure::new(
                "watermark_epoch_stable",
                action,
                format!(
                    "store epoch changed unexpectedly: expected={}, wm={}, sql={epoch}",
                    self.expected_epoch,
                    wm.epoch()
                ),
            ));
        }
        if revision < self.last_revision {
            return Err(CheckFailure::new(
                "watermark_revision_monotonic",
                action,
                format!(
                    "pending revision went backwards: {} -> {revision}",
                    self.last_revision
                ),
            ));
        }
        if must_strictly_advance && revision <= self.last_revision {
            return Err(CheckFailure::new(
                "watermark_revision_advances_on_enqueue",
                action,
                format!(
                    "expected revision > {} after enqueue, got {revision}",
                    self.last_revision
                ),
            ));
        }
        self.last_revision = revision;
        Ok(())
    }

    fn flush_debounce_to_store(&mut self, action: &'static str) -> Result<usize, CheckFailure> {
        let batch = self.debounce.flush();
        if batch.is_empty() {
            return Ok(0);
        }
        let report = self
            .reader
            .enqueue_pending_paths_under_root(&self.root, &batch)
            .map_err(|err| {
                CheckFailure::new(
                    "enqueue_under_root_ok",
                    action,
                    format!("enqueue_pending_paths_under_root failed: {err:#}"),
                )
            })?;
        self.check_watermark_monotonic(action, !report.enqueued.is_empty())?;
        Ok(report.enqueued.len())
    }

    fn apply(&mut self, action: &SuiteCAction) -> StepOutcome {
        let act_name = action.name();
        match action {
            SuiteCAction::Create { path, content } => {
                if self.files.contains_key(path) {
                    return bail("create_already_exists");
                }
                write_repo_file(&self.root, path, content);
                self.files.insert(path.clone(), content.clone());
                self.debounce
                    .push_burst([self.root.join(path).to_string_lossy().into_owned()]);
                if let Err(f) = self.flush_debounce_to_store(act_name) {
                    return StepOutcome::CheckFailed(f);
                }
                StepOutcome::Ok
            }
            SuiteCAction::Edit { path, content } => {
                if !self.files.contains_key(path) {
                    return skip_missing("edit_missing_file");
                }
                write_repo_file(&self.root, path, content);
                self.files.insert(path.clone(), content.clone());
                self.debounce
                    .push_burst([self.root.join(path).to_string_lossy().into_owned()]);
                if let Err(f) = self.flush_debounce_to_store(act_name) {
                    return StepOutcome::CheckFailed(f);
                }
                StepOutcome::Ok
            }
            SuiteCAction::Delete { path } => {
                if !self.files.contains_key(path) || self.files.len() <= 2 {
                    return bail("delete_would_empty_repo");
                }
                let full = self.root.join(path);
                if full.exists() {
                    fs::remove_file(&full).expect("remove source file");
                }
                self.files.remove(path);
                self.debounce
                    .push_burst([full.to_string_lossy().into_owned()]);
                if let Err(f) = self.flush_debounce_to_store(act_name) {
                    return StepOutcome::CheckFailed(f);
                }
                StepOutcome::Ok
            }
            SuiteCAction::Rename { old_path, new_path } => {
                if !self.files.contains_key(old_path) || self.files.contains_key(new_path) {
                    return bail("rename_precondition");
                }
                let content = self.files.remove(old_path).unwrap();
                let old_full = self.root.join(old_path);
                let new_full = self.root.join(new_path);
                if let Some(parent) = new_full.parent() {
                    fs::create_dir_all(parent).expect("create rename target parent");
                }
                fs::rename(&old_full, &new_full).expect("rename file");
                self.files.insert(new_path.clone(), content);
                // Watcher emits both old and new paths in the same burst.
                self.debounce.push_burst([
                    old_full.to_string_lossy().into_owned(),
                    new_full.to_string_lossy().into_owned(),
                ]);
                if let Err(f) = self.flush_debounce_to_store(act_name) {
                    return StepOutcome::CheckFailed(f);
                }
                StepOutcome::Ok
            }
            SuiteCAction::CollapseOver4096 {
                edited_path,
                edited_content,
                extra_synthetic_count,
            } => {
                if !self.files.contains_key(edited_path) {
                    return skip_missing("collapse_missing_target");
                }
                write_repo_file(&self.root, edited_path, edited_content);
                self.files
                    .insert(edited_path.clone(), edited_content.clone());

                let mut burst =
                    Vec::with_capacity(VIRTUAL_DEBOUNCE_CAP + extra_synthetic_count + 2);
                burst.push(self.root.join(edited_path).to_string_lossy().into_owned());
                for i in 0..=(VIRTUAL_DEBOUNCE_CAP + *extra_synthetic_count) {
                    burst.push(
                        self.root
                            .join(format!("synthetic_storm/f_{i}.py"))
                            .to_string_lossy()
                            .into_owned(),
                    );
                }
                self.debounce.push_burst(burst);
                if !self.debounce.collapsed {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "debounce_collapses_over_4096",
                        act_name,
                        "expected VirtualDebounce to collapse when > 4096 paths pushed",
                    ));
                }
                let flushed = self.debounce.flush();
                if flushed.len() != 1 {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "collapsed_batch_has_single_root_entry",
                        act_name,
                        format!("expected 1 collapsed root entry, got {}", flushed.len()),
                    ));
                }
                let report = self
                    .reader
                    .enqueue_pending_paths_under_root(&self.root, &flushed)
                    .expect("enqueue collapsed root");
                if report.enqueued != vec![".".to_string()] {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "collapsed_root_canonicalizes_to_dot",
                        act_name,
                        format!("expected canonical [\".\"], got {:?}", report.enqueued),
                    ));
                }
                if let Err(f) = self.check_watermark_monotonic(act_name, true) {
                    return StepOutcome::CheckFailed(f);
                }
                // Drain the collapsed whole-tree rescan immediately and confirm the edit landed.
                if let Err(err) = self.daemon.drain_pending_batch() {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "collapsed_drain_succeeds",
                        act_name,
                        format!("drain after >4096 collapse failed: {err:#}"),
                    ));
                }
                if let Err(f) = self.check_watermark_monotonic(act_name, false) {
                    return StepOutcome::CheckFailed(f);
                }
                StepOutcome::Ok
            }
            SuiteCAction::UnreadablePath {
                poison_path,
                healthy_mate_path,
                healthy_mate_content,
            } => {
                if !self.files.contains_key(healthy_mate_path) {
                    return skip_missing("unreadable_missing_mate");
                }
                // Mutate a real healthy batch-mate on disk and enqueue it alongside a
                // poison path that fails in isolation (`collect_pending_path` refuses `..`).
                write_repo_file(&self.root, healthy_mate_path, healthy_mate_content);
                self.files
                    .insert(healthy_mate_path.clone(), healthy_mate_content.clone());

                let prev_poison_attempts = self
                    .reader
                    .pending_attempts(poison_path)
                    .expect("pending_attempts poison")
                    .unwrap_or(0);

                // Raw enqueue for poison (simulating a pre-existing / uncanonicalized row)
                // + canonical enqueue for the healthy mate.
                self.reader
                    .enqueue_pending_paths(&[poison_path.clone(), healthy_mate_path.clone()])
                    .expect("enqueue poison + healthy mate");
                self.active_poisons.insert(poison_path.clone());
                if let Err(f) = self.check_watermark_monotonic(act_name, true) {
                    return StepOutcome::CheckFailed(f);
                }

                // Drain: healthy batch-mate MUST succeed and be cleared (attempts = None),
                // while ONLY the poison path is charged an attempt (+1, up to quarantine).
                let drained = match self.daemon.drain_pending_batch() {
                    Ok(n) => n,
                    Err(err) => {
                        return StepOutcome::CheckFailed(CheckFailure::new(
                            "healthy_batch_mate_succeeds_alongside_poison",
                            act_name,
                            format!(
                                "drain failed even though healthy mate {healthy_mate_path} was in batch: {err:#}"
                            ),
                        ));
                    }
                };
                if drained == 0 {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "healthy_batch_mate_counted_as_drained",
                        act_name,
                        "expected at least 1 drained healthy path alongside poison".to_string(),
                    ));
                }

                let mate_attempts = self
                    .reader
                    .pending_attempts(healthy_mate_path)
                    .expect("pending_attempts mate");
                if mate_attempts.is_some() {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "zero_cross_charging_of_batch_mates",
                        act_name,
                        format!(
                            "healthy batch-mate {healthy_mate_path} was NOT cleared / was cross-charged: attempts={mate_attempts:?}"
                        ),
                    ));
                }

                let new_poison_attempts = self
                    .reader
                    .pending_attempts(poison_path)
                    .expect("pending_attempts poison after drain");
                let expected_attempts = if prev_poison_attempts >= MAX_PENDING_ATTEMPTS {
                    // Already quarantined before this drain — re-enqueueing reset attempts to 0
                    // so draining it once charges 1.
                    1
                } else {
                    prev_poison_attempts + 1
                };
                if new_poison_attempts != Some(expected_attempts) {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "poison_charged_exact_attempt",
                        act_name,
                        format!(
                            "poison {poison_path} expected attempts={expected_attempts}, got {new_poison_attempts:?}"
                        ),
                    ));
                }
                if let Err(f) = self.check_watermark_monotonic(act_name, false) {
                    return StepOutcome::CheckFailed(f);
                }
                StepOutcome::Ok
            }
            SuiteCAction::VanishedFile {
                vanish_path,
                live_mate_path,
                live_mate_content,
            } => {
                if !self.files.contains_key(live_mate_path) || self.files.contains_key(vanish_path)
                {
                    return bail("vanish_precondition");
                }
                // Create a scratch file, enqueue it with a live mate, then remove it BEFORE drain.
                let vanish_full = self.root.join(vanish_path);
                write_repo_file(&self.root, vanish_path, "def ephemeral():\n    return 0\n");
                write_repo_file(&self.root, live_mate_path, live_mate_content);
                self.files
                    .insert(live_mate_path.clone(), live_mate_content.clone());

                self.debounce.push_burst([
                    vanish_full.to_string_lossy().into_owned(),
                    self.root
                        .join(live_mate_path)
                        .to_string_lossy()
                        .into_owned(),
                ]);
                if let Err(f) = self.flush_debounce_to_store(act_name) {
                    return StepOutcome::CheckFailed(f);
                }

                // File vanishes before drain reads it!
                fs::remove_file(&vanish_full).expect("remove vanished scratch file");

                if let Err(err) = self.daemon.drain_pending_batch() {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "vanished_file_does_not_fail_drain",
                        act_name,
                        format!("drain failed when {vanish_path} vanished: {err:#}"),
                    ));
                }
                // Vanished file must be reconciled cleanly as a deletion — NEVER charged a retry!
                let vanish_attempts = self
                    .reader
                    .pending_attempts(vanish_path)
                    .expect("pending_attempts vanish");
                if vanish_attempts.is_some() {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "vanished_file_not_charged_retry",
                        act_name,
                        format!(
                            "vanished file {vanish_path} was charged retry/left pending: {vanish_attempts:?}"
                        ),
                    ));
                }
                if self
                    .reader
                    .latest_extraction_for_path(vanish_path)
                    .expect("check vanished extraction")
                    .is_some()
                {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "vanished_file_no_ghost_extraction",
                        act_name,
                        format!("vanished file {vanish_path} left ghost extraction in store"),
                    ));
                }
                if let Err(f) = self.check_watermark_monotonic(act_name, false) {
                    return StepOutcome::CheckFailed(f);
                }
                StepOutcome::Ok
            }
            SuiteCAction::DrainAndVerifyColdParity => {
                // Flush any remaining debounced paths, then drain all non-poison pending work.
                if let Err(f) = self.flush_debounce_to_store(act_name) {
                    return StepOutcome::CheckFailed(f);
                }

                // Drive any active poison rows to quarantine or clear them after verifying
                // zero cross-charging, so the final store can be compared cleanly against a
                // cold full build.
                if !self.active_poisons.is_empty() {
                    let poisons: Vec<String> = self.active_poisons.iter().cloned().collect();
                    // Drive one poison to full quarantine (MAX_PENDING_ATTEMPTS) if any remain
                    // queued below threshold, checking that no healthy path is ever touched.
                    for _ in 0..MAX_PENDING_ATTEMPTS {
                        let _ = self.daemon.drain_pending_batch();
                    }
                    let status_with_q = self.reader.status("test").expect("status");
                    if status_with_q.quarantined_count == 0 {
                        return StepOutcome::CheckFailed(CheckFailure::new(
                            "poison_reaches_quarantine",
                            act_name,
                            format!(
                                "expected active poisons {poisons:?} to reach quarantine after {MAX_PENDING_ATTEMPTS} drains, got status={status_with_q:?}"
                            ),
                        ));
                    }
                    let dropped = self
                        .reader
                        .drop_quarantined_pending_paths()
                        .expect("drop quarantined");
                    for p in &dropped {
                        self.active_poisons.remove(p);
                    }
                }

                // Drain any remaining live pending work.
                for _ in 0..4 {
                    match self.daemon.drain_pending_batch() {
                        Ok(0) => break,
                        Ok(_) => {}
                        Err(err) => {
                            return StepOutcome::CheckFailed(CheckFailure::new(
                                "drain_remaining_work_succeeds",
                                act_name,
                                format!("drain failed: {err:#}"),
                            ));
                        }
                    }
                }
                if let Err(f) = self.check_watermark_monotonic(act_name, false) {
                    return StepOutcome::CheckFailed(f);
                }

                let status = self.reader.status("test").expect("status after drain");
                if status.pending_count != 0 || status.quarantined_count != 0 {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "drained_queue_is_clean",
                        act_name,
                        format!(
                            "expected pending=0 quarantined=0, got pending={} quarantined={}",
                            status.pending_count, status.quarantined_count
                        ),
                    ));
                }

                // Build a completely fresh cold Store on the same directory and assert
                // full canonical equivalence!
                let cold_store = Store::open_in_memory().expect("cold store");
                if let Err(err) = build_generation_from_disk(&cold_store, &self.root) {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "cold_build_succeeds",
                        act_name,
                        format!("cold build failed: {err:#}"),
                    ));
                }
                let inc_snap = snapshot_store(&self.reader);
                let cold_snap = snapshot_store(&cold_store);
                if inc_snap != cold_snap {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "daemon_incremental_eq_cold_full_build",
                        act_name,
                        format!(
                            "incremental daemon store != cold full build!\n\
                             hashes_eq={}, symbols_eq={}, edges_eq={}, dead_eq={}\n\
                             inc_hashes={:?}\ncold_hashes={:?}\n\
                             inc_syms_only={:?}\ncold_syms_only={:?}\n\
                             inc_edges_only={:?}\ncold_edges_only={:?}",
                            inc_snap.file_hashes == cold_snap.file_hashes,
                            inc_snap.symbols == cold_snap.symbols,
                            inc_snap.edges == cold_snap.edges,
                            inc_snap.dead_symbols == cold_snap.dead_symbols,
                            inc_snap.file_hashes,
                            cold_snap.file_hashes,
                            inc_snap
                                .symbols
                                .difference(&cold_snap.symbols)
                                .collect::<Vec<_>>(),
                            cold_snap
                                .symbols
                                .difference(&inc_snap.symbols)
                                .collect::<Vec<_>>(),
                            inc_snap
                                .edges
                                .difference(&cold_snap.edges)
                                .collect::<Vec<_>>(),
                            cold_snap
                                .edges
                                .difference(&inc_snap.edges)
                                .collect::<Vec<_>>(),
                        ),
                    ));
                }

                StepOutcome::Ok
            }
        }
    }
}

fn replay_suite_c(
    initial_files: &BTreeMap<String, String>,
    actions: &[SuiteCAction],
) -> Option<CheckFailure> {
    let mut state = SuiteCState::new(initial_files.clone());
    for action in actions {
        let outcome = state.apply(action);
        if let Some(failure) = outcome.into_failure() {
            return Some(failure);
        }
    }
    None
}

#[test]
fn stateful_suite_c_virtualized_debounce_and_daemon_drain() {
    let started = Instant::now();
    let seed = env_u64("DEVMAP_STATEFUL_SEED", DEFAULT_SEED);
    let rounds = env_usize("DEVMAP_STATEFUL_ROUNDS", 32);
    let mut src = Source::from_seed(seed ^ 0xC0C0_0003);

    let initial_files = generate_micro_repo(&mut src, ALL_LANGS, 8);
    let mut state = SuiteCState::new(initial_files.clone());
    let mut tracker = HealthTracker::new();
    let mut history: Vec<SuiteCAction> = Vec::with_capacity(rounds + 1);
    let mut prev_action: Option<&'static str> = None;

    let candidates: &[&'static str] = &[
        "Create",
        "Edit",
        "Delete",
        "Rename",
        "CollapseOver4096",
        "UnreadablePath",
        "VanishedFile",
        "DrainAndVerifyColdParity",
    ];

    for step in 0..rounds {
        let chosen = if step + 1 == rounds {
            // Always finish with a full drain + cold-build equivalence check!
            "DrainAndVerifyColdParity"
        } else {
            tracker.pick_weighted(&mut src, candidates, prev_action)
        };

        let paths: Vec<String> = state.files.keys().cloned().collect();
        let action = match chosen {
            "Create" => {
                let lang = *src.choose(ALL_LANGS).unwrap();
                let id = src.ident("created");
                let path = match lang {
                    MicroLang::Rust => format!("src/{id}.rs"),
                    MicroLang::TypeScript => format!("ts/{id}.ts"),
                    MicroLang::Go => format!("go/{id}.go"),
                    MicroLang::Python => format!("py/{id}.py"),
                };
                let content = match lang {
                    MicroLang::Rust => format!("pub fn {id}_fn(x: i32) -> i32 {{\n    x + 7\n}}\n"),
                    MicroLang::TypeScript => {
                        format!(
                            "export function {id}_fn(x: number): number {{\n  return x + 7;\n}}\n"
                        )
                    }
                    MicroLang::Go => {
                        format!("package micro\n\nfunc Created_{step}(x int) int {{\n\treturn x + 7\n}}\n")
                    }
                    MicroLang::Python => {
                        format!("def {id}_fn(x: int) -> int:\n    return x + 7\n")
                    }
                };
                SuiteCAction::Create { path, content }
            }
            "Edit" => {
                let path = src.choose(&paths).unwrap().clone();
                let lang = lang_for_path(&path);
                let content = mutate_file_elements(&mut src, lang, &state.files[&path]);
                SuiteCAction::Edit { path, content }
            }
            "Delete" => {
                let path = src.choose(&paths).unwrap().clone();
                SuiteCAction::Delete { path }
            }
            "Rename" => {
                let old_path = src.choose(&paths).unwrap().clone();
                let lang = lang_for_path(&old_path);
                let id = src.ident("renamed");
                let new_path = match lang {
                    MicroLang::Rust => format!("src/{id}.rs"),
                    MicroLang::TypeScript => format!("ts/{id}.ts"),
                    MicroLang::Go => format!("go/{id}.go"),
                    MicroLang::Python => format!("py/{id}.py"),
                };
                SuiteCAction::Rename { old_path, new_path }
            }
            "CollapseOver4096" => {
                let edited_path = src.choose(&paths).unwrap().clone();
                let lang = lang_for_path(&edited_path);
                let edited_content =
                    mutate_file_elements(&mut src, lang, &state.files[&edited_path]);
                SuiteCAction::CollapseOver4096 {
                    edited_path,
                    edited_content,
                    extra_synthetic_count: 1 + src.usize_below(64),
                }
            }
            "UnreadablePath" => {
                let healthy_mate_path = src.choose(&paths).unwrap().clone();
                let lang = lang_for_path(&healthy_mate_path);
                let healthy_mate_content =
                    mutate_file_elements(&mut src, lang, &state.files[&healthy_mate_path]);
                SuiteCAction::UnreadablePath {
                    poison_path: format!("../poison_escape_{step}.py"),
                    healthy_mate_path,
                    healthy_mate_content,
                }
            }
            "VanishedFile" => {
                let live_mate_path = src.choose(&paths).unwrap().clone();
                let lang = lang_for_path(&live_mate_path);
                let live_mate_content =
                    mutate_file_elements(&mut src, lang, &state.files[&live_mate_path]);
                SuiteCAction::VanishedFile {
                    vanish_path: format!("py/vanish_scratch_{step}.py"),
                    live_mate_path,
                    live_mate_content,
                }
            }
            _ => SuiteCAction::DrainAndVerifyColdParity,
        };

        let act_name = action.name();
        let outcome = state.apply(&action);
        tracker.record(act_name, prev_action, &outcome);
        history.push(action);
        prev_action = Some(act_name);

        if let Some(failure) = outcome.into_failure() {
            let sig = failure.signature();
            let minimized = minimize_sequence(&history, &sig, 35, |seq| {
                replay_suite_c(&initial_files, seq)
            });
            let action_strs: Vec<String> = minimized.iter().map(|a| format!("{a:?}")).collect();
            let repro = format_repro_test(
                "repro_suite_c_debounce_daemon_drain",
                seed,
                &failure,
                &action_strs,
            );
            panic!(
                "Suite C state-machine violation: {failure}\n{}\n\nMinimized reproducer ({} -> {} steps):\n{repro}",
                tracker.probe_report("SuiteC:debounce_daemon_drain"),
                history.len(),
                minimized.len(),
            );
        }
    }

    println!(
        "{}elapsed={:?}",
        tracker.probe_report("SuiteC:debounce_daemon_drain"),
        started.elapsed()
    );
    tracker.assert_actions_exercised(candidates);
}
