use std::path::PathBuf;

use clap::Subcommand;

use crate::cli::Cli;
use crate::output::emit_json;

#[derive(clap::Args)]
#[group(id = "Workspace")]
pub(crate) struct Args {
    #[command(subcommand)]
    pub(crate) action: WorkspaceAction,
}

pub(crate) fn run(cli: &Cli, args: &Args) -> anyhow::Result<()> {
    let Args { action } = args;
    // Rooted at the store's repository when `--db` names a store in its
    // standard place, so `devmap --db X workspace` and `dev map workspace`
    // agree on where the registry lives. A store anywhere else — a
    // `$DEVMAP_HOME` layout, a scratch path — names no repository, and
    // the registry belongs to the repository this command ran in. The
    // inverse used to answer the grandparent of *any* path, and an
    // off-layout `--db` put the registry two directories above the
    // store, in a directory that was nobody's repository.
    let root = match &cli.db {
        Some(explicit) => devmap_extract::paths::repo_root_from_store(explicit)
            .unwrap_or_else(|| cli.root_hint().to_path_buf()),
        None => cli.root_hint().to_path_buf(),
    };
    // Absolute in the answer: the registry records absolute roots, and
    // a `registry` of `./.devmap/workspace.json` tells a caller in
    // another directory nothing.
    let root = root.canonicalize().unwrap_or(root);
    // Mutating actions go through `Workspace::update`, which holds an
    // advisory lock across the read and the write. Loading here and
    // saving later — which is what this did — let two concurrent
    // registrations each read the same registry and write their own
    // entry over the other's.
    match action {
        WorkspaceAction::Add { path, name } => {
            let canonical = path
                .canonicalize()
                .map_err(|error| anyhow::anyhow!("cannot resolve {}: {error}", path.display()))?;
            let label = name
                .clone()
                .unwrap_or_else(|| devmap_query::workspace::name_for(&canonical));
            let (added, written) =
                devmap_query::workspace::Workspace::update(&root, |workspace| {
                    workspace.add(label.clone(), canonical.clone())
                })?;
            let replaced = added?;
            if cli.json {
                emit_json(
                    cli,
                    &serde_json::json!({
                        "added": label,
                        "root": canonical,
                        "registry": written,
                        "replaced": replaced,
                    }),
                )?;
            } else {
                outln!(
                    "{} {label} -> {} ({})",
                    if replaced { "replaced" } else { "added" },
                    canonical.display(),
                    written.display()
                );
            }
        }
        WorkspaceAction::Remove { name } => {
            // Distinguished from success: a caller retrying a removal
            // should learn the name was never registered.
            //
            // The registry is rewritten unconditionally rather than
            // only when something was removed: the write is a rename of
            // identical bytes when nothing changed, and skipping it
            // would mean the "nothing to do" path took a different
            // route through the lock than the mutating one.
            let (removed, _) = devmap_query::workspace::Workspace::update(&root, |workspace| {
                workspace.remove(name)
            })?;
            if cli.json {
                emit_json(cli, &serde_json::json!({"removed": removed, "name": name}))?;
            } else if removed {
                outln!("removed {name}");
            } else {
                outln!("{name} is not registered");
            }
        }
        WorkspaceAction::List => {
            let workspace = devmap_query::workspace::Workspace::load(&root)?;
            if cli.json {
                let repos: Vec<serde_json::Value> = workspace
                    .repos
                    .iter()
                    .map(|repo| {
                        let db = repo.db_path();
                        let status = devmap_store::Store::open_existing(&db)
                            .ok()
                            .flatten()
                            .and_then(|store| store.status(&db.display().to_string()).ok());
                        serde_json::json!({
                            "name": repo.name,
                            "root": repo.root,
                            "db": db,
                            // Null rather than zero when there is no
                            // generation: "never indexed" and "indexed
                            // and empty" are different states, and one
                            // of the repositories here is the second.
                            "generation": status.as_ref().and_then(|s| s.latest_generation),
                            "symbols": status.as_ref().map(|s| s.node_count),
                            "edges": status.as_ref().map(|s| s.edge_count),
                        })
                    })
                    .collect();
                emit_json(
                    cli,
                    &serde_json::json!({"repos": repos, "registry_root": root}),
                )?;
                return Ok(());
            }
            if workspace.repos.is_empty() {
                outln!("no repositories registered ({})", root.display());
            }
            for repo in &workspace.repos {
                // The store *file* existing says nothing about whether
                // it holds anything. One of these repositories has two
                // generations whose latest contains zero symbols, and
                // reporting that as "indexed" because the file is on
                // disk is the same error as a check that could not run
                // reporting as one that passed.
                let db = repo.db_path();
                let state = match devmap_store::Store::open_existing(&db) {
                    Ok(None) => "no store".to_string(),
                    Err(error) => format!("unreadable: {error}"),
                    Ok(Some(store)) => match store.status(&db.display().to_string()) {
                        Err(error) => format!("unreadable: {error}"),
                        Ok(status) => match status.latest_generation {
                            None => "no generation".to_string(),
                            Some(gen) => format!(
                                "gen {gen}, {} symbols, {} edges",
                                status.node_count, status.edge_count
                            ),
                        },
                    },
                };
                outln!("{:<16} {:<34} {}", repo.name, state, repo.root.display());
            }
        }
        WorkspaceAction::Search {
            query,
            budget,
            semantic,
        } => {
            let workspace = devmap_query::workspace::Workspace::load(&root)?;
            let result = devmap_query::workspace_search(&workspace, query, *budget, *semantic)?;
            if cli.json {
                emit_json(cli, &serde_json::to_value(&result)?)?;
            } else {
                for entry in &result.items {
                    outln!(
                        "[{}] {}:{}  {}",
                        entry.repo,
                        entry.hit.file_path,
                        entry.hit.span.0,
                        entry.hit.symbol_name
                    );
                }
                outln!(
                    "{} repo(s) queried, {} shown of {}",
                    result.repos_queried,
                    result.shown,
                    result.total
                );
                // Never silent. A workspace answer assembled from a
                // subset is not a workspace answer.
                for missing in &result.unavailable {
                    outln!("  unavailable: {} — {}", missing.repo, missing.reason);
                }
            }
        }
        WorkspaceAction::Links => {
            let workspace = devmap_query::workspace::Workspace::load(&root)?;
            let links = devmap_query::link_candidates(&workspace)?;
            if cli.json {
                // An object, not a bare array: the count and the set of
                // repositories considered are part of the answer, and a
                // reader seeing `[]` should be able to tell "no links"
                // from "no repositories were examined".
                emit_json(
                    cli,
                    &serde_json::json!({
                        "links": links,
                        "count": links.len(),
                        "repos_considered": workspace
                            .repos
                            .iter()
                            .map(|repo| repo.name.as_str())
                            .collect::<Vec<_>>(),
                    }),
                )?;
            } else {
                for link in &links {
                    match link.kind {
                        devmap_query::workspace::LinkKind::Import => outln!(
                            "{} {} -> {}  ({}; {})",
                            link.from_repo,
                            link.module_specifier,
                            link.to_repo,
                            link.from_file,
                            link.evidence
                        ),
                        devmap_query::workspace::LinkKind::EntryName => outln!(
                            "{} {} -> {} {}  (\"{}\"; {})",
                            link.from_repo,
                            link.from_symbol.as_deref().unwrap_or(&link.from_file),
                            link.to_repo,
                            link.to_symbol.as_deref().unwrap_or_default(),
                            link.module_specifier,
                            link.evidence
                        ),
                    }
                }
                outln!("{} candidate link(s)", links.len());
            }
        }
    }
    Ok(())
}

#[derive(Subcommand)]
pub(crate) enum WorkspaceAction {
    /// Register a repository in this workspace.
    Add {
        /// Repository root. Stored canonicalised, so the registry survives a
        /// caller with a different working directory.
        path: PathBuf,
        /// Label for results. Defaults to the directory name.
        #[arg(long)]
        name: Option<String>,
    },
    /// Remove a repository by name.
    Remove { name: String },
    /// List registered repositories and whether each has a readable store.
    List,
    /// Search every registered repository at once.
    Search {
        query: String,
        #[arg(short, long, default_value_t = 2000)]
        budget: u32,
        #[arg(long)]
        semantic: bool,
    },
    /// Imports in one repository that another repository declares the module
    /// for. Candidates with evidence, not resolved edges.
    Links,
}
