//! Worktree index seeding:
//! - `devmap build --seed-from <main>`: stages a linked worktree's store from its
//!   main checkout's into `<db>.seed-<pid>`, runs the incremental build on the
//!   staged store, and atomically publishes `devmap.sqlite` only once the
//!   worktree's own generation is committed.
//! - `devmap seed [--from <checkout>] [--stacked] [--sync-delta] [--gc-bases] [path]`:
//!   sub-15ms CoW/WAL-healing standalone seeding from any registered checkout of
//!   the same repository, or zero-copy stacked delta provisioning (`stack_lease.json`
//!   + `delta.sqlite` over `.git/devmap/bases/<gen>-<sha8>/`).

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context};
use devmap_extract::paths::{
    code_graph_path, compact_code_graph_path, csr_path_for_store, main_checkout_of,
    registered_common_git_dir, repo_map_path, same_registered_repo, state_dir, store_path,
};
use devmap_query::{
    gc_unused_bases, provision_stacked_worktree, seed_derived_artifacts_and_reuse_caches,
    StackedStore,
};
use devmap_store::seed::{self, SeedMethod};
use devmap_store::Store;

use crate::cli::{default_root_hint, root_parser, Cli};
use crate::output::emit_json;
use crate::reporter::ProgressReporter;

#[derive(clap::Args)]
#[group(id = "Seed")]
pub(crate) struct Args {
    /// Registered checkout of the same repository to seed from (defaults to main checkout).
    #[arg(long, value_parser = root_parser())]
    pub(crate) from: Option<PathBuf>,

    /// Provision a zero-copy stacked worktree (`stack_lease.json` + `delta.sqlite`)
    /// over a shared immutable base snapshot instead of cloning `devmap.sqlite`.
    #[arg(long)]
    pub(crate) stacked: bool,

    /// Synchronize uncommitted/diverged git paths into an existing stacked `delta.sqlite`.
    #[arg(long)]
    pub(crate) sync_delta: bool,

    /// Garbage-collect unreferenced base snapshots under `<common_git_dir>/devmap/bases/`.
    #[arg(long)]
    pub(crate) gc_bases: bool,

    /// Target worktree root.
    #[arg(default_value_os_t = default_root_hint(), value_parser = root_parser())]
    pub(crate) path: PathBuf,
}

pub(crate) fn run(cli: &Cli, args: &Args) -> anyhow::Result<()> {
    let root = cli
        .root_hint()
        .canonicalize()
        .with_context(|| format!("cannot resolve worktree root {}", cli.root_hint().display()))?;

    if args.gc_bases {
        let common = registered_common_git_dir(&root).ok_or_else(|| {
            anyhow!(
                "{} is not a registered git checkout; cannot locate common git dir",
                root.display()
            )
        })?;
        let report = gc_unused_bases(&common)?;
        if cli.json {
            emit_json(cli, &serde_json::to_value(&report)?)?;
        } else {
            outln!(
                "removed={} retained={}",
                report.removed_generations.len(),
                report.retained_generations.len(),
            );
        }
        return Ok(());
    }

    if args.sync_delta {
        let mut stacked = StackedStore::open(&root)?;
        let report = stacked.sync_worktree()?;
        if cli.json {
            emit_json(cli, &serde_json::to_value(&report)?)?;
        } else {
            outln!(
                "synced={} deleted={} cleaned={} rebased={}",
                report.synced_files,
                report.deleted_files,
                report.cleaned_files,
                report.rebased_to_standalone.is_some(),
            );
        }
        return Ok(());
    }

    let source = match &args.from {
        Some(explicit) => {
            let resolved = explicit
                .canonicalize()
                .with_context(|| format!("cannot resolve --from {}", explicit.display()))?;
            if resolved == root {
                bail!(
                    "devmap seed: source checkout {} is the target checkout itself",
                    resolved.display()
                );
            }
            if !same_registered_repo(&resolved, &root) {
                bail!(
                    "devmap seed: {} and {} are not registered checkouts of the same repository",
                    resolved.display(),
                    root.display()
                );
            }
            resolved
        }
        None => main_checkout_of(&root).ok_or_else(|| {
            anyhow!(
                "devmap seed: {} is not a registered linked worktree (or pass --from <checkout>)",
                root.display()
            )
        })?,
    };

    let started = std::time::Instant::now();
    if args.stacked {
        let lease = provision_stacked_worktree(&source, &root)?;
        let mut stacked = StackedStore::open(&root)?;
        let sync = stacked.sync_worktree()?;
        let elapsed_ms = started.elapsed().as_millis();
        let payload = serde_json::json!({
            "mode": "stacked",
            "root": root,
            "source": source,
            "generation_id": lease.generation_id,
            "base_dir": lease.base_dir,
            "base_sqlite_path": lease.base_sqlite_path(),
            "synced_files": sync.synced_files,
            "deleted_files": sync.deleted_files,
            "rebased_to_standalone": sync.rebased_to_standalone,
            "elapsed_ms": elapsed_ms,
        });
        if cli.json {
            emit_json(cli, &payload)?;
        } else {
            outln!(
                "stacked {} from {} (gen #{}, {} ms)",
                root.display(),
                source.display(),
                lease.generation_id,
                elapsed_ms
            );
        }
        return Ok(());
    }

    let source_store = store_path(&source);
    let target_db = cli.db();
    if source_store == target_db {
        bail!(
            "devmap seed: source and target resolve to the same store ({})",
            source_store.display()
        );
    }
    if target_db.exists() {
        bail!(
            "devmap seed: target store {} already exists",
            target_db.display()
        );
    }
    if let Some(parent) = target_db.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }

    let _writer = Store::lock_writer_at(&target_db, Store::WRITER_LOCK_WAIT)?;
    let staged = seed::sidecar(&target_db, &format!(".seed-{}", std::process::id()));
    let staged_csr = csr_path_for_store(&staged);
    seed::discard_staged(&staged);
    let staged_outcome = (|| -> anyhow::Result<(SeedMethod, bool)> {
        let m = seed::stage_seed_healing_wal(&source_store, &staged)
            .with_context(|| format!("cannot seed from {}", source_store.display()))?;
        seed::rebind_staged(&staged, &root)?;
        let staged_store = Store::open(&staged)
            .with_context(|| "the seeded store cannot be opened by this devmap".to_string())?;
        if let Some(damage) = staged_store.search_index_unreadable()? {
            bail!(
                "the donor devmap store {} has a damaged search index ({damage}); run `devmap build --full` on {} before seeding",
                source_store.display(),
                source.display(),
            );
        }
        let tree_matches_donor = staged_store.latest_generation_matches_working_tree()?
            && staged_store
                .latest_budget_refusals()?
                .is_none_or(|refused| refused.total == 0);
        if tree_matches_donor {
            if let Ok(lane_head) = devmap_store::current_git_head(&root) {
                if staged_store.latest_generation_head_sha()?.as_deref() != Some(lane_head.as_str())
                {
                    staged_store.restamp_latest_head(&lane_head)?;
                }
            }
            let src_csr = csr_path_for_store(&source_store);
            if src_csr.is_file() {
                drop(staged_store);
                seed::seed_artifact(&src_csr, &staged_csr).with_context(|| {
                    format!("cannot stage CSR sidecar from {}", src_csr.display())
                })?;
            } else {
                let _ = staged_store.write_latest_csr_sidecar()?;
                drop(staged_store);
            }
        } else {
            drop(staged_store);
            seed_resolution_reuse_dirs(&source, &root);
            crate::commands::build::build_staged_seed_store(&staged, &root)?;
            let _ = std::fs::remove_file(seed::sidecar(&staged, ".lock"));
        }
        seed::settle_staged(&staged)?;
        if staged_csr.is_file() {
            let final_csr = csr_path_for_store(&target_db);
            std::fs::rename(&staged_csr, &final_csr)
                .with_context(|| format!("cannot place CSR sidecar {}", final_csr.display()))?;
        }
        std::fs::rename(&staged, &target_db)
            .with_context(|| format!("cannot place {}", target_db.display()))?;
        Ok((m, tree_matches_donor))
    })();
    let (method, tree_matches_donor) = match staged_outcome {
        Ok(pair) => pair,
        Err(err) => {
            seed::discard_staged(&staged);
            return Err(err);
        }
    };

    // The store is placed: nothing after this point may fail the command. When
    // the lane's tree matched the donor's, seed consumer artifacts and reuse
    // caches as a best-effort note; when the lane diverged, `resolution-reuse-*`
    // was already seeded and updated during the staged incremental build, and
    // donor artifacts would describe the wrong tree.
    if tree_matches_donor {
        if let Err(error) = seed_derived_artifacts_and_reuse_caches(&source, &root) {
            eprintln!(
                "devmap seed: store placed at {}, but derived artifacts were not seeded (the next manifest is written in full): {error:#}",
                target_db.display()
            );
        }
    }

    let elapsed_ms = started.elapsed().as_millis();
    let payload = serde_json::json!({
        "mode": "cow",
        "method": method.label(),
        "root": root,
        "source": source,
        "db_path": target_db,
        "elapsed_ms": elapsed_ms,
    });
    if cli.json {
        emit_json(cli, &payload)?;
    } else {
        outln!(
            "seeded {} from {} by {} in {} ms",
            target_db.display(),
            source_store.display(),
            method.label(),
            elapsed_ms
        );
    }
    Ok(())
}

pub(crate) struct Plan {
    root: PathBuf,
    main: PathBuf,
    source_store: PathBuf,
}

/// RAII guard for a staged `--seed-from` database (`<db>.seed-<pid>`).
///
/// Keeping the seeded store off `cli.db()` until the worktree's own generation
/// is committed prevents concurrent `SessionStart` hooks and `status` readers
/// from seeing the main checkout's symbol counts as this worktree's.
pub(crate) struct StagedSeed<'a> {
    plan: &'a Plan,
    final_db: PathBuf,
    staged_db: PathBuf,
    committed: bool,
}

impl<'a> StagedSeed<'a> {
    pub(crate) fn staged_path(&self) -> &Path {
        &self.staged_db
    }

    pub(crate) fn commit(mut self, progress: &ProgressReporter) -> anyhow::Result<Store> {
        seed::settle_staged(&self.staged_db)?;
        std::fs::rename(&self.staged_db, &self.final_db)
            .with_context(|| format!("cannot place {}", self.final_db.display()))?;
        self.committed = true;
        let src_csr = csr_path_for_store(&self.plan.source_store);
        if src_csr.is_file() {
            let _ = seed::seed_artifact(&src_csr, &csr_path_for_store(&self.final_db));
        }
        self.plan.seed_artifacts(progress);
        Store::open(&self.final_db)
            .with_context(|| format!("cannot reopen placed store {}", self.final_db.display()))
    }
}

impl Drop for StagedSeed<'_> {
    fn drop(&mut self) {
        if !self.committed {
            seed::discard_staged(&self.staged_db);
        }
    }
}

/// Validate `seed_from` against the worktree this build is for. Nothing is
/// created or written.
///
/// The one accepted source for `build --seed-from` is what [`crate::hook::main_checkout_of`]
/// resolves for `root`: the worktree's registered main checkout.
pub(crate) fn plan(cli: &Cli, seed_from: &Path) -> anyhow::Result<Plan> {
    let root = cli
        .root_hint()
        .canonicalize()
        .context("--seed-from: cannot resolve the worktree being built")?;
    let Some(main) = crate::hook::main_checkout_of(&root) else {
        bail!(
            "--seed-from: {} is not a registered linked worktree, so it has no main checkout \
             to seed from",
            root.display()
        );
    };
    let named = seed_from
        .canonicalize()
        .with_context(|| format!("--seed-from: cannot resolve {}", seed_from.display()))?;
    if named != main {
        bail!(
            "--seed-from: {} is not the main checkout of {} (that is {}); a store is only \
             seeded from the checkout its worktree was created from",
            named.display(),
            root.display(),
            main.display()
        );
    }
    let source_store = store_path(&main);
    if source_store == cli.db() {
        bail!(
            "--seed-from: the main checkout and this worktree resolve to the same store ({}); \
             a store shared through DEVMAP_HOME cannot be seeded",
            source_store.display()
        );
    }
    Ok(Plan {
        root,
        main,
        source_store,
    })
}

impl Plan {
    /// Stage a seeded store at `<cli.db()>.seed-<pid>` and clone `resolution-reuse-*`
    /// directories, or say why not and leave the build to start cold. Call with the
    /// writer lock held.
    pub(crate) fn stage<'a>(
        &'a self,
        cli: &Cli,
        progress: &ProgressReporter,
    ) -> Option<StagedSeed<'a>> {
        let db = cli.db();
        match std::fs::symlink_metadata(&db) {
            Ok(_) => {
                eprintln!(
                    "devmap: --seed-from ignored, {} already exists",
                    db.display()
                );
                return None;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                eprintln!(
                    "devmap: --seed-from not used, cold build instead: cannot inspect {}: {error}",
                    db.display()
                );
                return None;
            }
        }
        let started = std::time::Instant::now();
        let staged = seed::sidecar(&db, &format!(".seed-{}", std::process::id()));
        seed::discard_staged(&staged);
        match self.stage_into(&staged) {
            Ok(method) => {
                self.seed_resolution_reuse();
                progress.note(format_args!(
                    "seeded {} from {} by {} in {} ms",
                    db.display(),
                    self.source_store.display(),
                    method.label(),
                    started.elapsed().as_millis()
                ));
                Some(StagedSeed {
                    plan: self,
                    final_db: db,
                    staged_db: staged,
                    committed: false,
                })
            }
            Err(error) => {
                seed::discard_staged(&staged);
                eprintln!("devmap: --seed-from not used, cold build instead: {error:#}");
                None
            }
        }
    }

    fn seed_resolution_reuse(&self) {
        seed_resolution_reuse_dirs(&self.main, &self.root);
    }

    /// Clone the main checkout's manifest artifacts beside the placed store so
    /// this worktree's manifest writer patches them block by block instead of
    /// writing them afresh.
    fn seed_artifacts(&self, progress: &ProgressReporter) -> bool {
        let mut placed = false;
        let pairs = [
            (repo_map_path(&self.main), repo_map_path(&self.root)),
            (code_graph_path(&self.main), code_graph_path(&self.root)),
            (
                compact_code_graph_path(&self.main),
                compact_code_graph_path(&self.root),
            ),
        ];
        for (source, target) in pairs {
            match seed::seed_artifact(&source, &target) {
                Ok(Some(method)) => {
                    placed = true;
                    progress.note(format_args!(
                        "seeded {} by {}",
                        target.display(),
                        method.label()
                    ));
                }
                Ok(None) => {}
                Err(error) => eprintln!(
                    "devmap: --seed-from did not seed {} (its next manifest is written in full): {error:#}",
                    target.display()
                ),
            }
        }
        placed
    }

    fn stage_into(&self, staged: &Path) -> anyhow::Result<SeedMethod> {
        let method = seed::stage_seed(&self.source_store, staged)
            .with_context(|| format!("cannot copy {}", self.source_store.display()))?;
        seed::rebind_staged(staged, &self.root)?;
        Store::open(staged)
            .with_context(|| "the seed store cannot be opened by this devmap".to_string())?;
        seed::settle_staged(staged)?;
        Ok(method)
    }
}

fn seed_resolution_reuse_dirs(source_root: &Path, target_root: &Path) {
    let src_state = state_dir(source_root);
    let dst_state = state_dir(target_root);
    if let Ok(entries) = std::fs::read_dir(&src_state) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            if let Some(name_str) = name.to_str() {
                if name_str.starts_with("resolution-reuse-") && entry.path().is_dir() {
                    let _ = seed::seed_directory_cow(&entry.path(), &dst_state.join(name_str));
                }
            }
        }
    }
}
