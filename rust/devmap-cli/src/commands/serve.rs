use std::path::PathBuf;

use devmap_serve::{default_ipc_path_for, Daemon};
use devmap_store::Store;

use crate::cli::{root_parser, Cli};
use crate::output::{emit_json, ensure_parent};

#[derive(clap::Args)]
#[group(id = "Serve")]
pub(crate) struct Args {
    #[arg(value_parser = root_parser())]
    pub(crate) path: Option<PathBuf>,
    #[arg(long)]
    pub(crate) socket: Option<PathBuf>,
    /// Print the IPC endpoint this repository would be served on and exit,
    /// without starting a daemon, opening a store, or creating any file.
    ///
    /// Exists so a second implementation of the socket-path formula can be
    /// checked against this one. The Python client derives the same path
    /// without spawning anything, and when the two disagree each side
    /// starts its own daemon against one store — a divergence that is
    /// invisible from either side.
    #[arg(long)]
    pub(crate) print_socket_path: bool,
}

/// How often a start that is waiting on a seed's writer lock says so.
const WAIT_REPORT_EVERY: std::time::Duration = std::time::Duration::from_secs(5);

/// Open the store, creating it if it is absent.
///
/// Creating a store waits for the writer lock a seed holds while it places its
/// sidecars and store (`Store::open` does it for any creator). Done in the
/// open, that wait is silent for up to `Store::WRITER_LOCK_WAIT`, and a host
/// whose start timeout is shorter sees a hang it cannot name. So a creating
/// start takes the lock itself, on a blocking thread, naming the wait on stderr
/// when it begins and every few seconds after, and abandoning it when the
/// process is told to stop. `Store::open` on the same thread then recognises
/// the lock as held and does not wait again.
async fn open_store(cli: &Cli) -> anyhow::Result<Store> {
    let db = cli.db();
    let absent = matches!(
        std::fs::symlink_metadata(&db),
        Err(ref error) if error.kind() == std::io::ErrorKind::NotFound
    );
    if !absent {
        return Ok(Store::open(db)?);
    }
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let waiter = {
        let cancel = std::sync::Arc::clone(&cancel);
        tokio::task::spawn_blocking(move || create_store_under_lock(&db, &cancel))
    };
    tokio::pin!(waiter);
    let mut stop = std::pin::pin!(stop_requested());
    loop {
        tokio::select! {
            outcome = &mut waiter => return outcome?,
            () = &mut stop, if !cancel.load(std::sync::atomic::Ordering::Relaxed) => {
                cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }
}

fn create_store_under_lock(
    db: &std::path::Path,
    cancel: &std::sync::atomic::AtomicBool,
) -> anyhow::Result<Store> {
    let mut reported = None::<std::time::Duration>;
    let guard = Store::lock_writer_at_watched(db, Store::WRITER_LOCK_WAIT, &mut |wait| {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return std::ops::ControlFlow::Break(());
        }
        if reported.is_none_or(|at| wait.waited >= at + WAIT_REPORT_EVERY) {
            reported = Some(wait.waited);
            eprintln!(
                "devmap serve: state=waiting_for_seed_writer_lock store={} pid={} waited_s={} \
                 bound_s={}",
                db.display(),
                wait.holder_pid(),
                wait.waited.as_secs(),
                wait.bound.as_secs()
            );
        }
        std::ops::ControlFlow::Continue(())
    })?;
    if let Some(waited) = reported {
        eprintln!(
            "devmap serve: state=writer_lock_acquired store={} waited_s={}",
            db.display(),
            waited.as_secs()
        );
    }
    let store = Store::open(db)?;
    drop(guard);
    Ok(store)
}

/// Resolves when the process is asked to stop (SIGTERM or SIGINT).
#[cfg(unix)]
async fn stop_requested() {
    use tokio::signal::unix::{signal, SignalKind};
    match (signal(SignalKind::terminate()), signal(SignalKind::interrupt())) {
        (Ok(mut term), Ok(mut int)) => {
            tokio::select! {
                _ = term.recv() => {}
                _ = int.recv() => {}
            }
        }
        _ => std::future::pending::<()>().await,
    }
}

#[cfg(not(unix))]
async fn stop_requested() {
    if tokio::signal::ctrl_c().await.is_err() {
        std::future::pending::<()>().await;
    }
}

pub(crate) async fn run(cli: &Cli, args: &Args) -> anyhow::Result<()> {
    let Args {
        path,
        socket,
        print_socket_path,
    } = args;
    // Canonicalize once, here: the daemon's watcher, reconcile sweep
    // and path-containment guards each canonicalized independently
    // before, and a non-canonical root (a symlinked tmpdir, `.`) made
    // the IPC identity hash — and therefore the socket path — differ
    // between invocations of the same repository.
    //
    // `default_ipc_path_for` canonicalizes too, so the two agree; this
    // one is what the daemon is *rooted* at.
    if let (Some(flag), Some(positional)) = (&cli.root, path) {
        let same = flag == positional
            || matches!(
                (flag.canonicalize(), positional.canonicalize()),
                (Ok(a), Ok(b)) if a == b
            );
        if !same {
            anyhow::bail!(
                "--root {} and the serve path {} name different repositories; the store comes \
                 from --root and the endpoint from the path. Pass one of them",
                flag.display(),
                positional.display()
            );
        }
    }
    let root = cli.root_hint().canonicalize()?;
    let ipc_path = socket
        .clone()
        .unwrap_or_else(|| default_ipc_path_for(&root));

    // Before the store is touched: `--print-socket-path` must create
    // nothing, and `Store::open` creates the database file.
    if *print_socket_path {
        if cli.json {
            emit_json(cli, &serde_json::json!({"socket": ipc_path}))?;
        } else {
            outln!("{}", ipc_path.display());
        }
        return Ok(());
    }

    ensure_parent(&cli.db())?;
    // Whose store this is comes before the open, which migrates and heals. The
    // daemon is the longest-lived writer of the store, so it proves ownership
    // the way `build` and `repair` do, from a read that writes nothing. Every
    // other store open of the MCP server is read-only and cannot migrate.
    Store::refuse_a_live_foreign_owner_before_open(&cli.db(), &root)?;
    let store = open_store(cli).await?;
    let daemon = Daemon::new(store, root)
        // So the daemon can notice its own store being deleted or
        // replaced at the same path and exit, instead of serving a
        // removed inode until its idle bound expires half an hour later.
        .with_store_path(cli.db())
        .with_ipc_path(ipc_path);
    daemon.run_loop().await?;
    Ok(())
}
