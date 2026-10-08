use std::path::PathBuf;

use anyhow::Context;
use devmap_store::Store;

use crate::cli::Cli;
use crate::output::emit_json;

#[derive(clap::Args)]
#[group(id = "Repair")]
pub(crate) struct Args {
    /// Explicitly upgrade the store schema after coordinating all readers and writers.
    #[arg(long, conflicts_with_all = ["fts", "pending", "page_size"])]
    pub(crate) schema: bool,
    #[arg(long)]
    pub(crate) fts: bool,
    /// Drop pending-queue rows that no drain can ever process: quarantined
    /// rows, paths outside the repository, directories and files that are
    /// gone, oversized sources, and non-source files.
    ///
    /// K1(f): the queue had no operator-facing repair at all. A store whose
    /// checkout had moved carried 64 rows naming the old location, every
    /// one of them quarantined, and the only way out was to open the
    /// database by hand or delete it.
    #[arg(long)]
    pub(crate) pending: bool,
    /// Rewrite the store at the current default page size.
    ///
    /// Page size is fixed when a database first gets content, so a store
    /// built before the default changed keeps its old one for life: the
    /// pragma is accepted and ignored on an existing database, and the
    /// daemon reopens whatever it finds. Nothing in the normal course of
    /// running converts one, which is why this is an explicit action —
    /// the rewrite takes an exclusive lock and leaves WAL for its duration.
    #[arg(long = "page-size")]
    pub(crate) page_size: bool,
}

pub(crate) fn run(cli: &Cli, args: &Args) -> anyhow::Result<()> {
    let Args {
        schema,
        fts,
        pending,
        page_size,
    } = args;
    if !*schema && !*fts && !*pending && !*page_size {
        anyhow::bail!("specify a repair target: --schema, --fts, --pending or --page-size");
    }
    let db = cli.db();
    if !db.is_file() {
        anyhow::bail!(
            "no devmap store at {} — run `devmap build` first",
            db.display()
        );
    }
    let _writer = Store::lock_writer_at(&db, Store::WRITER_LOCK_WAIT)?;
    let before = Store::stored_schema_version(&db)?;
    let store = Store::open(&db)?;
    drop(_writer); // Page-size conversion acquires its own writer guard.
    if cli.db.is_none() {
        store.validate_repo_root(&cli.root_hint())?;
    }
    let mut receipt = serde_json::Map::new();
    if *schema {
        receipt.insert("schema_before".into(), serde_json::json!(before));
        receipt.insert(
            "schema_version".into(),
            serde_json::json!(devmap_store::CURRENT_SCHEMA_VERSION),
        );
        receipt.insert(
            "upgraded".into(),
            serde_json::json!(before != Some(devmap_store::CURRENT_SCHEMA_VERSION)),
        );
        if !cli.json {
            outln!(
                "Store schema {} (previous: {:?}).",
                devmap_store::CURRENT_SCHEMA_VERSION,
                before
            );
        }
    }
    if *page_size {
        let outcome = store.convert_page_size().with_context(|| {
            format!(
                "page-size repair failed; completed results: {}",
                serde_json::json!(receipt)
            )
        })?;
        receipt.insert("page_size_before".into(), serde_json::json!(outcome.before));
        receipt.insert("page_size_after".into(), serde_json::json!(outcome.after));
        receipt.insert("converted".into(), serde_json::json!(outcome.converted));
        if !cli.json {
            if outcome.converted {
                outln!(
                    "Store rewritten at {} byte pages (was {}).",
                    outcome.after,
                    outcome.before
                );
            } else {
                outln!("Store already uses {} byte pages.", outcome.after);
            }
        }
    }
    if *fts {
        store.repair_fts().with_context(|| {
            format!(
                "FTS repair failed; completed results: {}",
                serde_json::json!(receipt)
            )
        })?;
        receipt.insert("fts_repaired".into(), serde_json::json!(true));
        if !cli.json {
            outln!("FTS search index repaired.");
        }
    }
    if *pending {
        // K1(f): report what was dropped, per row, with the reason.
        // A repair that says "done" is indistinguishable from one that
        // found nothing, and the whole point of this command is that
        // the operator could not see what was stuck.
        //
        // The structural pass needs a root. The store records the one
        // the latest generation was built from; without a generation
        // there is nothing to compare a path against, so only the
        // quarantined rows are dropped and the payload says so.
        let root = store.latest_repo_root()?.map(PathBuf::from);
        let structural = match &root {
            Some(root) => store.reconcile_pending_paths(root)?,
            None => devmap_store::PendingReconcile::default(),
        };
        let quarantined = store.drop_quarantined_pending_paths()?;

        if cli.json {
            let pending_receipt = serde_json::json!({
                "repo_root": root.as_ref().map(|root| root.display().to_string()),
                "structural_pass_ran": root.is_some(),
                "dropped_unprocessable": structural.dropped
                    .iter()
                    .map(|(path, reason)| serde_json::json!({
                        "path": path, "reason": reason,
                    }))
                    .collect::<Vec<_>>(),
                "normalized": structural.rewritten
                    .iter()
                    .map(|(from, to)| serde_json::json!({ "from": from, "to": to }))
                    .collect::<Vec<_>>(),
                "dropped_quarantined": quarantined,
                "retained": structural.retained.saturating_sub(quarantined.len()),
            });
            if let serde_json::Value::Object(fields) = pending_receipt {
                receipt.extend(fields);
            }
        } else {
            if root.is_none() {
                outln!(
                    "No generation yet, so no repository root to check paths against; \
                     dropping quarantined rows only."
                );
            }
            for (dropped, reason) in &structural.dropped {
                outln!("dropped {dropped}: {reason}");
            }
            for (from, to) in &structural.rewritten {
                outln!("normalized {from} -> {to}");
            }
            for dropped in &quarantined {
                outln!(
                    "dropped {dropped}: exceeded {} retry attempts",
                    devmap_store::MAX_PENDING_ATTEMPTS
                );
            }
            outln!(
                "Pending queue repaired: {} unprocessable, {} quarantined, \
                 {} normalized.",
                structural.dropped.len(),
                quarantined.len(),
                structural.rewritten.len(),
            );
        }
    }
    if cli.json {
        emit_json(cli, &serde_json::Value::Object(receipt))?;
    }
    Ok(())
}
