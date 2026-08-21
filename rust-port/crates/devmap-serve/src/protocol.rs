use std::sync::Arc;
use std::time::Duration;

use devmap_query::{Request, StoreQueryEngine};
use devmap_store::Store;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const PROTOCOL_VERSION: u32 = 1;
const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(5);
/// Upper bound on how long one query may occupy its connection task.
///
/// Queries share the store connection with the drain loop's generation writes,
/// so a query issued mid-resync otherwise blocks for as long as that write
/// holds the mutex. Left unbounded, such a request pins the connection until
/// the write finishes; bounded, it answers with a structured error the caller
/// can report instead of hanging.
const QUERY_TIMEOUT: Duration = Duration::from_secs(30);
/// Overall ceiling on reading one request frame, per connection. Per-read
/// timeouts bound a silent peer; this bounds a peer that keeps the exchange
/// alive without ever finishing — bytes trickling forever would otherwise
/// hold a connection task open indefinitely.
const REQUEST_DEADLINE: Duration = Duration::from_secs(10);
/// How long the startup liveness probe waits for an existing endpoint to
/// answer a connect before treating it as active-and-unreachable. Bounded so
/// a wedged listener cannot stall a new daemon's bind forever.
const LIVENESS_PROBE_TIMEOUT: Duration = Duration::from_millis(500);
/// Ceiling on concurrently served connections. Each accepted connection
/// spawns a task that may buffer up to MAX_REQUEST_BYTES before any
/// validation runs; without a cap, a flood of connections converts directly
/// into unbounded task memory. Excess connections wait at accept, backing up
/// into the kernel listen backlog instead of daemon heap.
const MAX_CONCURRENT_CONNECTIONS: usize = 64;
/// Consecutive accept failures tolerated before the IPC task gives up: an
/// unrecoverable condition (persistent fd exhaustion) must surface as a loud
/// exit, not a silent spin. Each failure backs off exponentially.
const MAX_CONSECUTIVE_ACCEPT_ERRORS: u32 = 30;
const MAX_QUERY_BYTES: usize = 4 * 1024;
const MAX_TOKEN_BUDGET: u32 = 100_000;
const MAX_TRAVERSAL_DEPTH: usize = 64;

/// Records when the daemon last did anything a consumer asked of it.
///
/// Shared between the IPC handlers and [`crate::daemon::Daemon::run_loop`] so
/// an orphaned daemon whose consumers all died can tell idleness from work and
/// retire itself (bounded by `DEVMAP_MAX_IDLE_SECS`) instead of running — and
/// holding its store open — forever.
#[derive(Default)]
pub struct Activity(std::sync::Mutex<Option<std::time::Instant>>);

impl Activity {
    pub fn touch(&self) {
        let mut slot = self.0.lock().expect("activity mutex poisoned");
        *slot = Some(std::time::Instant::now());
    }

    /// How long since the last touch; `None` when nothing was ever recorded.
    pub fn idle_for(&self) -> Option<Duration> {
        let slot = self.0.lock().expect("activity mutex poisoned");
        slot.map(|at| at.elapsed())
    }
}

fn default_budget() -> u32 {
    2_000
}

fn default_depth() -> usize {
    3
}

#[derive(Debug, Deserialize)]
pub struct IpcRequest {
    pub version: u32,
    #[serde(flatten)]
    pub command: IpcCommand,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum IpcCommand {
    Status,
    Search {
        query: String,
        #[serde(default = "default_budget")]
        budget: u32,
    },
    Deps {
        target: String,
        #[serde(default = "default_budget")]
        budget: u32,
        #[serde(default)]
        min_confidence: f32,
    },
    Impact {
        target: String,
        #[serde(default = "default_budget")]
        budget: u32,
        #[serde(default = "default_depth")]
        depth: usize,
    },
    Trace {
        from: String,
        #[serde(default)]
        to: Option<String>,
        #[serde(default = "default_budget")]
        budget: u32,
        #[serde(default = "default_depth")]
        depth: usize,
    },
    Dead {
        #[serde(default = "default_budget")]
        budget: u32,
    },
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    code: &'static str,
    message: String,
}

#[derive(Debug, Serialize)]
struct Envelope {
    ok: bool,
    protocol_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<ErrorBody>,
}

fn success(result: Value) -> Envelope {
    Envelope {
        ok: true,
        protocol_version: PROTOCOL_VERSION,
        result: Some(result),
        error: None,
    }
}

fn failure(code: &'static str, message: impl Into<String>) -> Envelope {
    Envelope {
        ok: false,
        protocol_version: PROTOCOL_VERSION,
        result: None,
        error: Some(ErrorBody {
            code,
            message: message.into(),
        }),
    }
}

fn validate_request(request: &IpcRequest) -> Result<(), String> {
    let (text, budget, depth, min_confidence) = match &request.command {
        IpcCommand::Status => return Ok(()),
        IpcCommand::Search { query, budget } => (query.as_str(), *budget, 1, None),
        IpcCommand::Deps {
            target,
            budget,
            min_confidence,
        } => (target.as_str(), *budget, 1, Some(*min_confidence)),
        IpcCommand::Impact {
            target,
            budget,
            depth,
        } => (target.as_str(), *budget, *depth, None),
        IpcCommand::Trace {
            from,
            to,
            budget,
            depth,
        } => {
            if to
                .as_ref()
                .is_some_and(|value| value.len() > MAX_QUERY_BYTES)
            {
                return Err(format!("trace destination exceeds {MAX_QUERY_BYTES} bytes"));
            }
            (from.as_str(), *budget, *depth, None)
        }
        IpcCommand::Dead { budget } => ("", *budget, 1, None),
    };
    if text.len() > MAX_QUERY_BYTES {
        return Err(format!("query exceeds {MAX_QUERY_BYTES} bytes"));
    }
    if budget > MAX_TOKEN_BUDGET {
        return Err(format!("token budget exceeds {MAX_TOKEN_BUDGET}"));
    }
    if depth > MAX_TRAVERSAL_DEPTH {
        return Err(format!("traversal depth exceeds {MAX_TRAVERSAL_DEPTH}"));
    }
    if min_confidence.is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value)) {
        return Err("min_confidence must be finite and within [0, 1]".to_string());
    }
    Ok(())
}

fn dispatch(store: &Store, request: IpcRequest) -> anyhow::Result<Value> {
    if request.version != PROTOCOL_VERSION {
        anyhow::bail!(
            "unsupported protocol version {}; server requires {}",
            request.version,
            PROTOCOL_VERSION
        );
    }
    let engine = StoreQueryEngine::new(store);
    match request.command {
        IpcCommand::Status => {
            let status = store.status("daemon")?;
            Ok(json!({
                "generation_id": status.latest_generation,
                "pending_count": status.pending_count,
                "node_count": status.node_count,
                "edge_count": status.edge_count,
                "is_fresh": status.pending_count == 0,
                "degraded_reason": status.degraded_reason,
                "quarantined_count": status.quarantined_count,
            }))
        }
        IpcCommand::Search { query, budget } => {
            Ok(serde_json::to_value(engine.search(Request {
                query,
                token_budget: budget,
                min_confidence: 0.0,
                max_depth: 1,
            })?)?)
        }
        IpcCommand::Deps {
            target,
            budget,
            min_confidence,
        } => Ok(serde_json::to_value(engine.dependencies(Request {
            query: target,
            token_budget: budget,
            min_confidence,
            max_depth: 1,
        })?)?),
        IpcCommand::Impact {
            target,
            budget,
            depth,
        } => Ok(serde_json::to_value(engine.impact(Request {
            query: target,
            token_budget: budget,
            min_confidence: 0.0,
            max_depth: depth,
        })?)?),
        IpcCommand::Trace {
            from,
            to,
            budget,
            depth,
        } => {
            let response = if let Some(destination) = to {
                engine.trace_between(Request {
                    query: (from, destination),
                    token_budget: budget,
                    min_confidence: 0.0,
                    max_depth: depth,
                })?
            } else {
                engine.trace(Request {
                    query: from,
                    token_budget: budget,
                    min_confidence: 0.0,
                    max_depth: depth,
                })?
            };
            Ok(serde_json::to_value(response)?)
        }
        IpcCommand::Dead { budget } => Ok(serde_json::to_value(engine.dead_symbols(budget)?)?),
    }
}

async fn write_envelope<S>(stream: &mut S, envelope: &Envelope) -> anyhow::Result<()>
where
    S: AsyncWrite + Unpin,
{
    let mut payload = serde_json::to_vec(envelope)?;
    payload.push(b'\n');
    tokio::time::timeout(IO_TIMEOUT, stream.write_all(&payload))
        .await
        .map_err(|_| anyhow::anyhow!("IPC response write timed out"))??;
    tokio::time::timeout(IO_TIMEOUT, stream.shutdown())
        .await
        .map_err(|_| anyhow::anyhow!("IPC shutdown timed out"))??;
    Ok(())
}

pub async fn handle_stream<S>(mut stream: S, store: Arc<Store>) -> anyhow::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    handle_stream_with_activity(&mut stream, store, &Activity::default()).await
}

/// Why a request frame could not be read.
#[derive(Debug)]
enum FrameReadError {
    /// The overall request deadline passed before a complete frame arrived,
    /// cutting off peers that dribble bytes to hold the connection open.
    DeadlineExceeded,
    /// The peer closed the connection without sending anything.
    ClosedBeforeNewline,
    /// The frame grew past [`MAX_REQUEST_BYTES`] before terminating.
    TooLarge(usize),
    /// The transport itself failed or one read exceeded `io_timeout`.
    Io(std::io::Error),
}

impl std::fmt::Display for FrameReadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DeadlineExceeded => {
                write!(formatter, "request deadline exceeded before newline")
            }
            Self::ClosedBeforeNewline => {
                write!(formatter, "connection closed before newline")
            }
            Self::TooLarge(limit) => write!(formatter, "request exceeds {limit} bytes"),
            Self::Io(error) => write!(formatter, "IPC request read failed: {error}"),
        }
    }
}

impl std::error::Error for FrameReadError {}

impl From<std::io::Error> for FrameReadError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// Read one newline-terminated request frame under two bounds: each read gets
/// `io_timeout`, and the whole frame must complete by `deadline`.
///
/// A clean close after bytes were buffered ends the frame there, so clients
/// that half-close instead of writing a trailing newline still work; a clean
/// close with nothing buffered is an incomplete request, not silence.
async fn read_frame<S>(
    mut stream: S,
    io_timeout: Duration,
    deadline: std::time::Instant,
) -> Result<Vec<u8>, FrameReadError>
where
    S: AsyncRead + Unpin,
{
    let mut payload = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if std::time::Instant::now() >= deadline {
            return Err(FrameReadError::DeadlineExceeded);
        }
        let read = tokio::time::timeout(io_timeout, stream.read(&mut chunk))
            .await
            .map_err(|_| {
                FrameReadError::Io(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "IPC request read timed out",
                ))
            })??;
        if read == 0 {
            return if payload.is_empty() {
                Err(FrameReadError::ClosedBeforeNewline)
            } else {
                Ok(payload)
            };
        }
        let newline = chunk[..read].iter().position(|byte| *byte == b'\n');
        let take = newline.unwrap_or(read);
        if payload.len().saturating_add(take) > MAX_REQUEST_BYTES {
            return Err(FrameReadError::TooLarge(MAX_REQUEST_BYTES));
        }
        payload.extend_from_slice(&chunk[..take]);
        if newline.is_some() {
            return Ok(payload);
        }
    }
}

pub async fn handle_stream_with_activity<S>(
    mut stream: S,
    store: Arc<Store>,
    activity: &Activity,
) -> anyhow::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let payload = match read_frame(
        &mut stream,
        IO_TIMEOUT,
        std::time::Instant::now() + REQUEST_DEADLINE,
    )
    .await
    {
        Ok(payload) => payload,
        // Answer a closed-before-newline peer with the same structured
        // refusal as before; every other read failure means the peer is gone
        // or hostile, and there is nothing useful to say to it.
        Err(FrameReadError::ClosedBeforeNewline) => {
            let envelope = failure("incomplete_request", "connection closed before newline");
            return write_envelope(&mut stream, &envelope).await;
        }
        Err(FrameReadError::TooLarge(limit)) => {
            let envelope = failure("request_too_large", format!("request exceeds {limit} bytes"));
            return write_envelope(&mut stream, &envelope).await;
        }
        Err(_) => return Ok(()),
    };

    let envelope = match serde_json::from_slice::<IpcRequest>(&payload) {
        Ok(request) => match validate_request(&request) {
            Err(error) => failure("invalid_parameters", error),
            Ok(()) => {
                activity.touch();
                let query_store = Arc::clone(&store);
                // Bounded, so a query wedged behind a long generation write
                // answers with a structured error rather than occupying the
                // connection for as long as the write holds the store mutex.
                let dispatched = tokio::time::timeout(
                    QUERY_TIMEOUT,
                    tokio::task::spawn_blocking(move || dispatch(&query_store, request)),
                )
                .await;
                match dispatched {
                    Err(_) => failure(
                        "query_timeout",
                        format!("query exceeded {QUERY_TIMEOUT:?}"),
                    ),
                    Ok(Err(error)) => {
                        failure("internal_error", format!("query task failed: {error}"))
                    }
                    Ok(Ok(Ok(result))) => success(result),
                    Ok(Ok(Err(error))) => failure("request_failed", error.to_string()),
                }
            }
        },
        Err(error) => failure("invalid_request", error.to_string()),
    };
    write_envelope(&mut stream, &envelope).await
}

/// Delay before the next accept attempt after `consecutive` failures.
///
/// Zero errors cost nothing; each additional failure doubles a 10 ms base
/// delay, capped at one second. The curve keeps a daemon under fd exhaustion
/// or platform EPROTO storms from spinning hot while still retrying promptly
/// once the condition clears.
fn accept_error_backoff(consecutive: u32) -> Duration {
    if consecutive == 0 {
        return Duration::ZERO;
    }
    let shift = consecutive.saturating_sub(1).min(63);
    let millis = 10u64.saturating_mul(1u64 << shift);
    Duration::from_millis(millis).min(Duration::from_secs(1))
}

#[cfg(unix)]
pub struct UnixIpcServer {
    listener: tokio::net::UnixListener,
    path: std::path::PathBuf,
    /// Held exclusively for the server's lifetime. Two daemons racing to serve
    /// one endpoint previously interleaved the exists→probe→remove→bind
    /// sequence: the loser unlinked the winner's live socket and bound its own,
    /// leaving an orphaned listener that answered nothing while still holding
    /// the store open and running its watcher and drain loops.
    _lock: std::fs::File,
}

#[cfg(unix)]
fn ipc_lock_path(path: &std::path::Path) -> std::path::PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("devmap-ipc");
    match path.parent() {
        Some(parent) => parent.join(format!("{file_name}.lock")),
        None => std::path::PathBuf::from(format!("{file_name}.lock")),
    }
}

/// Acquire the exclusive advisory lock guarding `path`'s bind sequence.
///
/// `File::try_lock` is an flock, so the kernel releases it if the holder dies
/// — no stale-lock cleanup is ever needed, unlike an O_EXCL marker file. The
/// caller must keep the returned file alive for as long as it owns the
/// endpoint.
#[cfg(unix)]
fn lock_ipc_endpoint(path: &std::path::Path) -> anyhow::Result<std::fs::File> {
    use std::io::Write;

    let lock_path = ipc_lock_path(path);
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)?;
    match file.try_lock() {
        Ok(()) => {
            let mut file = file;
            // Best-effort ownership record for diagnostics; failure to write
            // does not weaken the lock itself.
            let _ = writeln!(file, "{}", std::process::id());
            let _ = file.flush();
            Ok(file)
        }
        Err(_busy) => anyhow::bail!(
            "devmap IPC endpoint {path:?} is owned by another live daemon (lock {:?} held)",
            lock_path
        ),
    }
}

/// Probe whether the endpoint at `path` is live, bounded by
/// [`LIVENESS_PROBE_TIMEOUT`].
///
/// - `Some(true)` — a peer accepted within the window; the endpoint is active.
/// - `Some(false)` — connect failed definitively (refused, or the path is not
///   a socket); whatever sits there is stale and may be replaced.
/// - `None` — no definitive answer inside the window. The caller must treat
///   this as *active*: deleting a possibly-live endpoint under a daemon whose
///   listen backlog is momentarily full would orphan every future client,
///   while refusing to start is always recoverable by retrying.
///
/// The connect runs on a helper thread because a Unix-domain connect to a
/// listener with a full backlog can block for an unbounded time; the probe
/// must stay bounded even when the endpoint is hostile.
#[cfg(unix)]
fn probe_endpoint_liveness(path: &std::path::Path) -> Option<bool> {
    use std::sync::mpsc;

    let probe_path = path.to_path_buf();
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let connected = std::os::unix::net::UnixStream::connect(&probe_path).is_ok();
        let _ = sender.send(connected);
    });
    receiver
        .recv_timeout(LIVENESS_PROBE_TIMEOUT)
        .ok()
}

#[cfg(unix)]
impl UnixIpcServer {
    pub fn bind(path: &std::path::Path) -> anyhow::Result<Self> {
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::PermissionsExt;

        // 100 bytes is portable across the common 104-byte macOS/BSD and
        // 108-byte Linux sockaddr_un limits, including the trailing NUL.
        if path.as_os_str().as_bytes().len() > 100 {
            anyhow::bail!(
                "devmap IPC path is {} bytes; portable Unix limit is 100: {:?}",
                path.as_os_str().as_bytes().len(),
                path
            );
        }

        // Serialize concurrent starters before any of them touches the socket
        // file. Losing means a live daemon already owns this endpoint.
        let lock = lock_ipc_endpoint(path)?;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
            let is_managed_runtime_dir = parent.parent() == Some(std::env::temp_dir().as_path())
                && parent
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("devmap-"));
            if is_managed_runtime_dir {
                std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
            }
        }
        if path.exists() {
            match probe_endpoint_liveness(path) {
                Some(true) => {
                    anyhow::bail!("devmap IPC endpoint is already active at {path:?}")
                }
                Some(false) => std::fs::remove_file(path)?,
                None => anyhow::bail!(
                    "devmap IPC endpoint {path:?} did not answer its liveness \
                     probe within {LIVENESS_PROBE_TIMEOUT:?}; leaving it untouched"
                ),
            }
        }
        let listener = tokio::net::UnixListener::bind(path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        Ok(Self {
            listener,
            path: path.to_path_buf(),
            _lock: lock,
        })
    }

    pub async fn run(self, store: Arc<Store>, activity: Arc<Activity>) -> anyhow::Result<()> {
        let permits = Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_CONNECTIONS));
        let mut consecutive_accept_errors: u32 = 0;
        loop {
            match self.listener.accept().await {
                Ok((stream, _)) => {
                    consecutive_accept_errors = 0;
                    // Saturated pool => accept pauses here: backpressure lands
                    // in the kernel backlog rather than unbounded task memory.
                    let permit = Arc::clone(&permits)
                        .acquire_owned()
                        .await
                        .map_err(|_| anyhow::anyhow!("connection semaphore closed"))?;
                    let store = Arc::clone(&store);
                    let activity = Arc::clone(&activity);
                    tokio::spawn(async move {
                        let _permit = permit;
                        if let Err(error) =
                            handle_stream_with_activity(stream, store, &activity).await
                        {
                            tracing::warn!("IPC connection failed: {error}");
                        }
                    });
                }
                Err(error) => {
                    consecutive_accept_errors = consecutive_accept_errors.saturating_add(1);
                    if consecutive_accept_errors >= MAX_CONSECUTIVE_ACCEPT_ERRORS {
                        return Err(anyhow::anyhow!(
                            "IPC accept failed {consecutive_accept_errors} times \
                             consecutively; giving up: {error}"
                        ));
                    }
                    let delay = accept_error_backoff(consecutive_accept_errors);
                    tracing::warn!(
                        "IPC accept failed ({} consecutive; retry in {delay:?}): {error}",
                        consecutive_accept_errors
                    );
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }
}

#[cfg(unix)]
impl Drop for UnixIpcServer {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(windows)]
pub async fn run_named_pipe(
    store: Arc<Store>,
    name: &str,
    activity: Arc<Activity>,
) -> anyhow::Result<()> {
    use tokio::net::windows::named_pipe::ServerOptions;

    let mut server = ServerOptions::new()
        .first_pipe_instance(true)
        .create(name)?;
    let permits = Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_CONNECTIONS));
    let mut consecutive_connect_errors: u32 = 0;
    loop {
        match server.connect().await {
            Ok(()) => consecutive_connect_errors = 0,
            Err(error) => {
                consecutive_connect_errors = consecutive_connect_errors.saturating_add(1);
                if consecutive_connect_errors >= MAX_CONSECUTIVE_ACCEPT_ERRORS {
                    return Err(anyhow::anyhow!(
                        "named-pipe connect failed {consecutive_connect_errors} \
                         times consecutively; giving up: {error}"
                    ));
                }
                let delay = accept_error_backoff(consecutive_connect_errors);
                tracing::warn!(
                    "named-pipe connect failed ({} consecutive; retry in {delay:?}): {error}",
                    consecutive_connect_errors
                );
                tokio::time::sleep(delay).await;
                continue;
            }
        }
        // Same bound as the Unix transport: saturated pool => stop creating
        // pipe instances until a slot frees, instead of fanning out without
        // limit.
        let permit = Arc::clone(&permits)
            .acquire_owned()
            .await
            .map_err(|_| anyhow::anyhow!("connection semaphore closed"))?;
        let connected = server;
        server = ServerOptions::new().create(name)?;
        let store = Arc::clone(&store);
        let activity = Arc::clone(&activity);
        tokio::spawn(async move {
            let _permit = permit;
            if let Err(error) =
                handle_stream_with_activity(connected, store, &activity).await
            {
                tracing::warn!("named-pipe IPC connection failed: {error}");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    /// A peer dribbling one byte at a time must be cut by an *overall* request
    /// deadline, not just the per-read timeout.
    ///
    /// `handle_stream` bounded each `read` individually: a client that never
    /// stops sending could hold a connection task and its buffer open forever
    /// by keeping each byte inside the per-chunk window. The overall deadline
    /// is what actually bounds a hostile or wedged peer.
    #[tokio::test]
    async fn a_dribbling_peer_is_cut_at_the_overall_deadline() {
        let (mut client, server) = tokio::io::duplex(64);
        let writer = tokio::spawn(async move {
            for _ in 0..200 {
                if client.write_all(b"x").await.is_err() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        });

        let started = std::time::Instant::now();
        let deadline = started + Duration::from_millis(300);
        let result =
            tokio::time::timeout(Duration::from_secs(3), read_frame(server, IO_TIMEOUT, deadline))
                .await
                .expect("read_frame must return instead of hanging past the deadline");
        let elapsed = started.elapsed();

        let error = result.expect_err("a frame still open at the deadline must be refused");
        assert!(
            error.to_string().contains("deadline"),
            "the refusal must name the overall deadline, got: {error}"
        );
        assert!(
            elapsed < Duration::from_secs(2),
            "the deadline must cut the exchange near 300 ms, took {elapsed:?}"
        );
        writer.abort();
    }

    /// The deadline bounds only unfinished frames: a complete request that
    /// arrives normally is unaffected, so this cannot pass by refusing
    /// everything.
    #[tokio::test]
    async fn a_complete_frame_within_the_deadline_reads_normally() {
        let (mut client, server) = tokio::io::duplex(4096);
        client
            .write_all(br#"{"version":1,"cmd":"status"}"#)
            .await
            .unwrap();
        drop(client); // half-close: no more bytes are coming

        let payload = read_frame(
            server,
            IO_TIMEOUT,
            std::time::Instant::now() + Duration::from_secs(5),
        )
        .await
        .expect("a complete frame must read cleanly");
        assert_eq!(payload, br#"{"version":1,"cmd":"status"}"#.to_vec());
    }

    /// Accept-error backoff grows and then caps, and zero errors cost nothing.
    ///
    /// The accept loop must survive transient errors (fd exhaustion, EPROTO on
    /// some platforms) without spinning hot: unbounded retry at full speed is
    /// its own outage. The delay curve is policy, so it is pinned here.
    #[test]
    fn accept_error_backoff_grows_and_caps() {
        assert_eq!(accept_error_backoff(0), Duration::ZERO);
        assert_eq!(accept_error_backoff(1), Duration::from_millis(10));
        assert_eq!(accept_error_backoff(2), Duration::from_millis(20));
        assert_eq!(accept_error_backoff(3), Duration::from_millis(40));
        // Monotone growth, hard cap.
        let mut previous = accept_error_backoff(1);
        for consecutive in 2..=12u32 {
            let delay = accept_error_backoff(consecutive);
            assert!(delay >= previous, "backoff must not shrink");
            assert!(delay <= Duration::from_secs(1), "backoff must stay capped");
            previous = delay;
        }
        assert_eq!(accept_error_backoff(12), Duration::from_secs(1));
    }

    /// `min_confidence` must be finite and inside [0, 1].
    ///
    /// Both negations and the disjunction were mutable without a failure. A
    /// NaN threshold makes every `confidence >= min` comparison false, so the
    /// query returns nothing and looks like an empty graph rather than a bad
    /// request; a threshold above 1 does the same, and one below 0 silently
    /// disables the filter.
    #[test]
    fn min_confidence_must_be_finite_and_within_the_unit_interval() {
        let deps = |min_confidence: f32| IpcRequest {
            version: 1,
            command: IpcCommand::Deps {
                target: "a.go".to_string(),
                budget: 10,
                min_confidence,
            },
        };

        // Both endpoints are inclusive and valid.
        assert!(validate_request(&deps(0.0)).is_ok());
        assert!(validate_request(&deps(1.0)).is_ok());
        assert!(validate_request(&deps(0.4)).is_ok());

        // Outside the interval in either direction is rejected.
        assert!(
            validate_request(&deps(-0.1)).is_err(),
            "a negative threshold would disable the filter"
        );
        assert!(
            validate_request(&deps(1.1)).is_err(),
            "a threshold above 1 matches nothing and reads as an empty graph"
        );

        // Non-finite is rejected: NaN makes every comparison false.
        assert!(
            validate_request(&deps(f32::NAN)).is_err(),
            "NaN must be refused"
        );
        assert!(validate_request(&deps(f32::INFINITY)).is_err());
        assert!(validate_request(&deps(f32::NEG_INFINITY)).is_err());
    }

    /// The declared limits are a contract, not just relative bounds.
    ///
    /// The bound tests below compare against these constants, so they cannot
    /// notice the constants themselves moving — mutation testing changed
    /// `4 * 1024` to `4 + 1024` and `1024 * 1024` likewise without a failure.
    /// A limit that silently shrinks by three orders of magnitude rejects
    /// legitimate work; one that grows removes the bound. Pinned by value.
    #[test]
    fn protocol_limits_are_their_declared_values() {
        assert_eq!(MAX_REQUEST_BYTES, 1_048_576, "1 MiB request ceiling");
        assert_eq!(MAX_QUERY_BYTES, 4_096, "4 KiB query ceiling");
        assert_eq!(MAX_TOKEN_BUDGET, 100_000);
        assert_eq!(MAX_TRAVERSAL_DEPTH, 64);
    }

    /// Omitted budget and depth default to usable values.
    ///
    /// Both defaults were replaceable with 0 and 1. A zero budget returns
    /// nothing for every request that omits one, and a depth of 1 silently
    /// truncates impact to direct neighbours — both look like an empty graph
    /// rather than a misconfigured default.
    #[test]
    fn omitted_budget_and_depth_have_usable_defaults() {
        assert_eq!(
            default_budget(),
            2_000,
            "a default budget must return results"
        );
        assert!(
            default_depth() > 1,
            "a default depth of {} truncates impact to direct neighbours",
            default_depth()
        );

        // And they must actually be what deserialization uses.
        let request: IpcRequest =
            serde_json::from_str(r#"{"version":1,"cmd":"impact","target":"a.go::T.m"}"#)
                .expect("an impact request may omit budget and depth");
        match request.command {
            IpcCommand::Impact { budget, depth, .. } => {
                assert_eq!(budget, default_budget());
                assert_eq!(depth, default_depth());
            }
            other => panic!("expected an impact command, got {other:?}"),
        }
    }

    /// Every request bound is exclusive, and each is load-bearing.
    ///
    /// Mutation testing flipped `>` to `>=` and `==` on all three limits
    /// without a failure. These are the only thing bounding work an IPC caller
    /// can ask for: an off-by-one is minor, but a comparison that never fires
    /// lets a single request pin the daemon with an unbounded budget or depth.
    /// Each case sits exactly on the boundary and one step past it.
    #[test]
    fn request_bounds_are_exclusive_and_each_limit_is_enforced() {
        let search = |query: String, budget: u32| IpcRequest {
            version: 1,
            command: IpcCommand::Search { query, budget },
        };

        // Query length: at the limit is fine, one byte over is not.
        assert!(validate_request(&search("q".repeat(MAX_QUERY_BYTES), 10)).is_ok());
        assert!(validate_request(&search("q".repeat(MAX_QUERY_BYTES + 1), 10)).is_err());

        // Token budget.
        assert!(validate_request(&search("q".to_string(), MAX_TOKEN_BUDGET)).is_ok());
        assert!(validate_request(&search("q".to_string(), MAX_TOKEN_BUDGET + 1)).is_err());

        // Traversal depth, which only an Impact/Trace request carries.
        let impact = |depth: usize| IpcRequest {
            version: 1,
            command: IpcCommand::Impact {
                target: "a.go::T.m".to_string(),
                budget: 10,
                depth,
            },
        };
        assert!(validate_request(&impact(MAX_TRAVERSAL_DEPTH)).is_ok());
        assert!(validate_request(&impact(MAX_TRAVERSAL_DEPTH + 1)).is_err());

        // A trace *destination* is bounded independently of the source.
        let trace = |to: String| IpcRequest {
            version: 1,
            command: IpcCommand::Trace {
                from: "a.go::T.m".to_string(),
                to: Some(to),
                budget: 10,
                depth: 1,
            },
        };
        assert!(validate_request(&trace("t".repeat(MAX_QUERY_BYTES))).is_ok());
        assert!(
            validate_request(&trace("t".repeat(MAX_QUERY_BYTES + 1))).is_err(),
            "an oversized trace destination must be rejected even when the \
             source is small"
        );
    }

    /// A second binder must be refused while the first holds the endpoint,
    /// and allowed once it lets go.
    ///
    /// Without the lock, two racing daemons interleaved
    /// exists→probe→remove→bind: the loser unlinked the winner's live socket
    /// and bound its own, stranding an orphaned listener that answered
    /// nothing while still running its watcher and drain loops against the
    /// store. The lock is what makes the cleanup sequence exclusive.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_second_binder_is_refused_while_the_first_holds_the_endpoint() {
        let path =
            std::env::temp_dir().join(format!("devmap-lock-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let first = UnixIpcServer::bind(&path).expect("first bind must succeed");
        assert!(
            UnixIpcServer::bind(&path).is_err(),
            "a second bind while the first holds the lock must be refused"
        );
        drop(first);
        // The lock releases on drop (flock semantics), so a fresh daemon can
        // take over immediately — including after a crash of the previous
        // holder, which is exactly why an flock is used rather than a marker
        // file that would need stale-lock cleanup.
        let second = UnixIpcServer::bind(&path).expect("bind after release must succeed");
        drop(second);
        let _ = std::fs::remove_file(ipc_lock_path(&path));
    }

    /// The query bound is pinned by value: a constant that silently shrank
    /// would fail slow-but-legitimate queries on large stores, and one that
    /// grew would stop bounding queries wedged behind long generation writes.
    #[test]
    fn query_timeout_is_thirty_seconds() {
        assert_eq!(QUERY_TIMEOUT, Duration::from_secs(30));
        assert_eq!(REQUEST_DEADLINE, Duration::from_secs(10));
    }

    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[cfg(unix)]
    #[test]
    fn unix_ipc_rejects_overlong_paths_before_bind() {
        let path = std::env::temp_dir().join("x".repeat(200));
        let error = match UnixIpcServer::bind(&path) {
            Ok(_) => panic!("overlong path must fail"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("portable Unix limit"));
    }

    /// The 100-byte socket-path bound is exclusive, and a legal path binds.
    ///
    /// The comparison was mutable to `>=` without a failure, because the only
    /// case tested was 200 bytes — far past the boundary, where every variant
    /// of the comparison rejects. `sockaddr_un` truncates silently rather than
    /// erroring, so a path one byte over the real limit produces a socket at a
    /// *different* path than requested and the daemon appears to start while
    /// nothing can reach it.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_socket_path_bound_is_exclusive_and_a_legal_path_binds() {
        use std::os::unix::ffi::OsStrExt;

        let dir = std::env::temp_dir().join(format!("devmap-sockbound-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        // Build a path of exactly 101 bytes: one past the portable limit.
        let base_len = dir.as_os_str().as_bytes().len() + 1; // + separator
        if base_len < 100 {
            let over = dir.join("y".repeat(101 - base_len));
            assert_eq!(over.as_os_str().as_bytes().len(), 101);
            assert!(
                UnixIpcServer::bind(&over).is_err(),
                "a path one byte past the limit must be refused"
            );

            // And a path exactly at the limit must bind, or the bound is off by
            // one in the other direction and legitimate paths are rejected.
            let at = dir.join("z".repeat(100 - base_len));
            assert_eq!(at.as_os_str().as_bytes().len(), 100);
            assert!(
                UnixIpcServer::bind(&at).is_ok(),
                "a path exactly at the limit must bind: {} bytes",
                at.as_os_str().as_bytes().len()
            );
            let _ = std::fs::remove_file(&at);
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A managed runtime directory is hardened to owner-only; an arbitrary
    /// parent is left alone.
    ///
    /// Both halves of the `is_managed_runtime_dir` test were mutable. Dropping
    /// the `devmap-` prefix check makes the daemon chmod 0700 on any directory
    /// it is pointed at — including a shared one it does not own; dropping the
    /// temp-dir parent check does the same. Widening a permission change beyond
    /// the directory we created is the failure that matters here.
    #[cfg(unix)]
    #[tokio::test]
    async fn only_the_managed_runtime_directory_is_hardened() {
        use std::os::unix::fs::PermissionsExt;

        // A managed dir: directly under temp_dir() and named `devmap-*`.
        let managed = std::env::temp_dir().join(format!("devmap-managed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&managed);
        let socket = managed.join("s.sock");
        UnixIpcServer::bind(&socket).expect("managed runtime dir binds");
        let mode = std::fs::metadata(&managed).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "a managed runtime dir must be owner-only");
        let _ = std::fs::remove_dir_all(&managed);

        // An unmanaged parent keeps whatever permissions it had; the daemon
        // must not widen or narrow a directory it did not create.
        let unmanaged = std::env::temp_dir().join(format!("plain-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&unmanaged);
        std::fs::create_dir_all(&unmanaged).unwrap();
        std::fs::set_permissions(&unmanaged, std::fs::Permissions::from_mode(0o755)).unwrap();
        let other = unmanaged.join("s.sock");
        UnixIpcServer::bind(&other).expect("an unmanaged parent still binds");
        let kept = std::fs::metadata(&unmanaged).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            kept, 0o755,
            "the daemon must not chmod a directory it did not create"
        );
        let _ = std::fs::remove_dir_all(&unmanaged);
    }

    /// A request past the byte ceiling is refused with a structured error.
    ///
    /// `handle_stream` accumulates until a newline, checking
    /// `payload.len() + take > MAX_REQUEST_BYTES` as it goes. That comparison
    /// was mutable to `==` and `>=` without a failure, and it is the only
    /// bound on how much a client can make the daemon buffer: a comparison
    /// that never fires lets one connection grow the buffer without limit.
    /// Asserted on the refusal being *structured* rather than a dropped
    /// connection, so the caller learns why.
    #[tokio::test]
    async fn an_oversized_request_is_refused_before_it_is_buffered() {
        let (mut client, server) = tokio::io::duplex(64 * 1024);
        let store = Arc::new(Store::open_in_memory().unwrap());
        let task = tokio::spawn(handle_stream(server, store));

        // A single logical request (no newline) well past the ceiling.
        let oversized = format!(
            "{{\"version\":1,\"cmd\":\"search\",\"query\":\"{}\"}}\n",
            "x".repeat(MAX_REQUEST_BYTES + 4096)
        );
        let _ = client.write_all(oversized.as_bytes()).await;
        let mut response = String::new();
        let _ = client.read_to_string(&mut response).await;
        let _ = task.await;

        assert!(
            response.contains("request_too_large"),
            "an oversized request must be refused with a structured error, got: {}",
            response.chars().take(200).collect::<String>()
        );
    }

    #[tokio::test]
    async fn protocol_rejects_missing_version_with_structured_error() {
        let (mut client, server) = tokio::io::duplex(4096);
        let store = Arc::new(Store::open_in_memory().unwrap());
        let task = tokio::spawn(handle_stream(server, store));
        client.write_all(b"{\"cmd\":\"status\"}\n").await.unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).await.unwrap();
        task.await.unwrap().unwrap();
        let value: Value = serde_json::from_str(response.trim()).unwrap();
        assert_eq!(value["ok"], false);
        assert_eq!(value["error"]["code"], "invalid_request");
    }

    #[tokio::test]
    async fn protocol_status_round_trip_is_versioned() {
        let (mut client, server) = tokio::io::duplex(4096);
        let store = Arc::new(Store::open_in_memory().unwrap());
        let task = tokio::spawn(handle_stream(server, store));
        client
            .write_all(b"{\"version\":1,\"cmd\":\"status\"}\n")
            .await
            .unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).await.unwrap();
        task.await.unwrap().unwrap();
        let value: Value = serde_json::from_str(response.trim()).unwrap();
        assert_eq!(value["ok"], true);
        assert_eq!(value["protocol_version"], PROTOCOL_VERSION);
        assert_eq!(value["result"]["pending_count"], 0);
    }

    #[tokio::test]
    async fn protocol_rejects_unbounded_query_work_before_dispatch() {
        let (mut client, server) = tokio::io::duplex(16 * 1024);
        let store = Arc::new(Store::open_in_memory().unwrap());
        let task = tokio::spawn(handle_stream(server, store));
        client
            .write_all(
                b"{\"version\":1,\"cmd\":\"impact\",\"target\":\"x\",\"budget\":4294967295,\"depth\":1000000}\n",
            )
            .await
            .unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).await.unwrap();
        task.await.unwrap().unwrap();
        let value: Value = serde_json::from_str(response.trim()).unwrap();
        assert_eq!(value["ok"], false);
        assert_eq!(value["error"]["code"], "invalid_parameters");
    }

    #[test]
    fn trace_protocol_is_backward_compatible_and_bounds_both_endpoints() {
        let legacy: IpcRequest = serde_json::from_str(
            r#"{"version":1,"cmd":"trace","from":"caller","budget":2000,"depth":3}"#,
        )
        .unwrap();
        assert!(validate_request(&legacy).is_ok());

        let destination = "x".repeat(MAX_QUERY_BYTES + 1);
        let request = IpcRequest {
            version: PROTOCOL_VERSION,
            command: IpcCommand::Trace {
                from: "caller".to_string(),
                to: Some(destination),
                budget: 2_000,
                depth: 3,
            },
        };
        assert!(validate_request(&request)
            .unwrap_err()
            .contains("trace destination"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unix_socket_round_trip_is_owner_only_and_bounded() {
        use std::os::unix::fs::PermissionsExt;
        use std::time::{SystemTime, UNIX_EPOCH};

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("devmap-ipc-{stamp}.sock"));
        let server = UnixIpcServer::bind(&path).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let task = tokio::spawn(server.run(
            Arc::new(Store::open_in_memory().unwrap()),
            Arc::new(Activity::default()),
        ));

        let mut client = tokio::net::UnixStream::connect(&path).await.unwrap();
        client
            .write_all(b"{\"version\":1,\"cmd\":\"status\"}\n")
            .await
            .unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).await.unwrap();
        let value: Value = serde_json::from_str(response.trim()).unwrap();
        assert_eq!(value["ok"], true);

        task.abort();
        let _ = task.await;
        tokio::task::yield_now().await;
        assert!(
            !path.exists(),
            "socket path must be cleaned when server stops"
        );
    }
}

#[cfg(test)]
mod hardening_limit_tests {
    use super::*;

    /// The fan-out cap and probe window are contracts, not vibes: both were
    /// introduced because an unbounded value had a concrete failure mode
    /// (flood => unbounded task memory; wedged listener => bind hanging
    /// forever). Pinned by value so silent drift fails here.
    #[test]
    fn connection_and_probe_bounds_are_pinned() {
        assert_eq!(MAX_CONCURRENT_CONNECTIONS, 64);
        assert_eq!(LIVENESS_PROBE_TIMEOUT, Duration::from_millis(500));
        assert_eq!(MAX_CONSECUTIVE_ACCEPT_ERRORS, 30);
    }

    /// A live-but-foreign endpoint (no lock of ours) must be refused by the
    /// liveness probe rather than clobbered, and a stale non-socket file must
    /// still be replaced cleanly.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_probe_refuses_live_endpoints_and_replaces_stale_files() {
        let path = std::env::temp_dir().join(format!(
            "devmap-probe-{}.sock",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(ipc_lock_path(&path));

        // A live listener we do not own: bind must refuse, naming activity.
        let foreign = std::os::unix::net::UnixListener::bind(&path).unwrap();
        match UnixIpcServer::bind(&path) {
            Ok(_) => panic!("a live foreign endpoint must not be replaced"),
            Err(error) => assert!(
                error.to_string().contains("already active"),
                "refusal must name the live endpoint: {error}"
            ),
        }
        drop(foreign);
        std::fs::remove_file(&path).unwrap();

        // A stale non-socket file answers the probe negatively and is replaced.
        std::fs::write(&path, b"junk").unwrap();
        let server = UnixIpcServer::bind(&path)
            .expect("a stale file must be replaceable");
        drop(server);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(ipc_lock_path(&path));
    }

    /// Accept-failure backoff must stay hot early (retry transient errors
    /// promptly) and cold late (never spin during sustained failure).
    #[test]
    fn accept_backoff_grows_and_caps() {
        assert_eq!(accept_error_backoff(0), Duration::ZERO);
        assert_eq!(accept_error_backoff(1), Duration::from_millis(10));
        assert_eq!(accept_error_backoff(2), Duration::from_millis(20));
        assert_eq!(accept_error_backoff(20), Duration::from_secs(1));
        assert_eq!(
            accept_error_backoff(u32::MAX),
            Duration::from_secs(1),
            "the curve must saturate, not overflow"
        );
    }
}
