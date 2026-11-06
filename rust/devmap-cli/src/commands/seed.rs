//! `devmap build --seed-from <main>`: start a linked worktree's store from its
//! main checkout's instead of from nothing.
//!
//! Two phases, so a refusal writes nothing. [`plan`] decides, from the
//! filesystem alone, that the named path is the main checkout of this
//! worktree and nothing else. [`Plan::apply`] runs under the build's writer
//! lock, once the state directory exists, and produces the store; the ordinary
//! incremental build then runs on top of it, so everything the build normally
//! does (`--manifest`, `--full`, ...) still applies.
//!
//! A refused source is an error. A seed that merely could not be made (no
//! store at the source, a full disk, a store this binary cannot open) is not:
//! it is named on stderr and the build carries on cold, because a
//! missing seed costs time, never correctness.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use devmap_store::seed::{self, SeedMethod};
use devmap_store::Store;

use crate::cli::Cli;
use crate::reporter::ProgressReporter;

pub(crate) struct Plan {
    root: PathBuf,
    source_store: PathBuf,
}

/// Validate `seed_from` against the worktree this build is for. Nothing is
/// created or written.
///
/// The one accepted source is what [`crate::hook::main_checkout_of`] resolves
/// for `root`: the worktree's registered main checkout. A sibling worktree, an
/// unrelated repository, a forged gitlink and the worktree itself all fail
/// there, and a path that only *names* the main checkout through a symlink is
/// compared after canonicalization, so it is the same directory or it is not.
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
    let source_store = devmap_extract::paths::store_path(&main);
    if source_store == cli.db() {
        bail!(
            "--seed-from: the main checkout and this worktree resolve to the same store ({}); \
             a store shared through DEVMAP_HOME cannot be seeded",
            source_store.display()
        );
    }
    Ok(Plan { root, source_store })
}

impl Plan {
    /// Place a seeded store at `cli.db()`, or say why not and leave the build
    /// to start cold. Call with the writer lock held.
    pub(crate) fn apply(&self, cli: &Cli, progress: &ProgressReporter) {
        let db = cli.db();
        match std::fs::symlink_metadata(&db) {
            Ok(_) => {
                eprintln!(
                    "devmap: --seed-from ignored, {} already exists",
                    db.display()
                );
                return;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                eprintln!(
                    "devmap: --seed-from not used, cold build instead: cannot inspect {}: {error}",
                    db.display()
                );
                return;
            }
        }
        let started = std::time::Instant::now();
        let staged = seed::sidecar(&db, &format!(".seed-{}", std::process::id()));
        seed::discard_staged(&staged);
        match self.place(&db, &staged) {
            Ok(method) => progress.note(format_args!(
                "seeded {} from {} by {} in {} ms",
                db.display(),
                self.source_store.display(),
                method.label(),
                started.elapsed().as_millis()
            )),
            Err(error) => {
                seed::discard_staged(&staged);
                eprintln!("devmap: --seed-from not used, cold build instead: {error:#}");
            }
        }
    }

    fn place(&self, db: &Path, staged: &Path) -> anyhow::Result<SeedMethod> {
        let method = seed::stage_seed(&self.source_store, staged)
            .with_context(|| format!("cannot copy {}", self.source_store.display()))?;
        seed::rebind_staged(staged, &self.root)?;
        // This binary has to be able to open what it is about to place: a store
        // from a newer schema is refused here, not after the rename.
        Store::open(staged)
            .with_context(|| "the seed store cannot be opened by this devmap".to_string())?;
        seed::settle_staged(staged)?;
        std::fs::rename(staged, db).with_context(|| format!("cannot place {}", db.display()))?;
        Ok(method)
    }
}
