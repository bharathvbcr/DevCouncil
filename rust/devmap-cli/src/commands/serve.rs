use std::path::PathBuf;

use devmap_serve::{default_ipc_path_for, Daemon};
use devmap_store::Store;

use crate::cli::{default_root_hint, Cli};
use crate::output::{emit_json, ensure_parent};

#[derive(clap::Args)]
#[group(id = "Serve")]
pub(crate) struct Args {
    #[arg(default_value_os_t = default_root_hint())]
    pub(crate) path: PathBuf,
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
    let root = path.canonicalize()?;
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
    let store = Store::open(cli.db())?;
    let daemon = Daemon::new(store, root)
        // So the daemon can notice its own store being deleted and
        // exit, instead of serving a removed inode until its idle bound
        // expires half an hour later.
        .with_store_path(cli.db())
        .with_ipc_path(ipc_path);
    daemon.run_loop().await?;
    Ok(())
}
