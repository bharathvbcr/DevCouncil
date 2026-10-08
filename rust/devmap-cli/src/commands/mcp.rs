use std::path::PathBuf;

use crate::claude;
use crate::cli::Cli;
use crate::output::emit_json;

#[derive(clap::Args)]
#[group(id = "Mcp")]
pub(crate) struct Args {
    /// Serve MCP 2.0 (protocol 2026-07-28) over HTTP on this address
    /// instead of speaking stdio.
    ///
    /// That revision is not reachable over stdio at all: its requests are
    /// self-contained POSTs carrying their own protocol version, and the
    /// `initialize` handshake every stdio client uses tops out at
    /// 2025-11-25. This flag is the only way to reach it.
    ///
    /// Defaults to loopback when given a bare port. A code index is a map of
    /// a private repository, so binding it to a routable interface publishes
    /// that map — do that deliberately or not at all.
    #[arg(long, value_name = "ADDR")]
    pub(crate) http: Option<String>,

    /// Print the MCP server entry an agent host needs, and exit.
    ///
    /// Creates nothing and starts nothing — the same contract as
    /// `serve --print-socket-path`, and for the same reason: a second
    /// implementation of "how do I reach this server" living in a host's
    /// config generator can disagree with this binary, and the disagreement
    /// is invisible from either side. This is the authority.
    ///
    /// The emitted `command` is this executable's own absolute path, not the
    /// bare name `devmap`. A host config that says `devmap` works only when
    /// something already put it on PATH, which on a fresh machine is exactly
    /// what has not happened.
    #[arg(long)]
    pub(crate) print_config: bool,
}

pub(crate) async fn run(cli: &Cli, args: &Args) -> anyhow::Result<()> {
    let Args { http, print_config } = args;
    // Nothing but JSON-RPC frames may reach stdout on this transport.
    // `main` installs the tracing subscriber before this match, and
    // `FmtSubscriber::builder()` writes to stdout by default — see the
    // `.with_writer(std::io::stderr)` there, which this transport
    // depends on and which `--json` output depended on already.
    if *print_config {
        // Before the store is touched: this must create no file and open
        // no database, so it can be run against a repository that has
        // never been indexed — which is when a user configures a host.
        let executable = std::env::current_exe()?;
        // One owner for "how do I reach this server": the plugin bundle
        // calls the same builder, so a host configured from one cannot
        // point at a different server than a host configured from the
        // other. It also refuses a non-UTF-8 path rather than writing
        // `display()`'s replacement characters into a `command` that
        // then names no file on disk.
        //
        // Global registration: no `--db`. The server resolves the store
        // from MCP roots/list, then cwd. Passing `--db` only when the
        // flag was explicit keeps legacy per-project configs working.
        let entry = claude::mcp_entry(&executable, cli.db.as_deref(), http.as_deref())?;
        emit_json(
            cli,
            &serde_json::json!({"mcpServers": {claude::MCP_SERVER_NAME: entry}}),
        )?;
        return Ok(());
    }

    // Global registration: resolve from MCP roots/list and cwd. An
    // explicit `--root` is a pin (not folded into cwd), so a per-project
    // registration keeps answering that repository when other tabs'
    // roots also have stores.
    let cwd = std::env::current_dir()
        .ok()
        .unwrap_or_else(|| PathBuf::from("."));
    let slot = std::sync::Arc::new(devmap_serve::StoreSlot::resolving(
        cli.db.clone(),
        cwd,
        cli.root.clone(),
    ));
    match http {
        Some(address) => {
            // A bare port means loopback. Spelling the default out here
            // rather than accepting "8080" as 0.0.0.0 is the difference
            // between serving one machine and serving a network.
            let address = claude::normalize_http_address(address);
            let parsed: std::net::SocketAddr = address.parse().map_err(|err| {
                anyhow::anyhow!("could not parse --http address '{address}': {err}")
            })?;
            devmap_serve::serve_http(slot, parsed).await?;
        }
        None => devmap_serve::serve_stdio(slot).await?,
    }
    Ok(())
}
