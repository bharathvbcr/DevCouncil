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
const MAX_QUERY_BYTES: usize = 4 * 1024;
const MAX_TOKEN_BUDGET: u32 = 100_000;
const MAX_TRAVERSAL_DEPTH: usize = 64;

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
    let mut payload = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let read = tokio::time::timeout(IO_TIMEOUT, stream.read(&mut chunk))
            .await
            .map_err(|_| anyhow::anyhow!("IPC request read timed out"))??;
        if read == 0 {
            let envelope = failure("incomplete_request", "connection closed before newline");
            return write_envelope(&mut stream, &envelope).await;
        }
        let newline = chunk[..read].iter().position(|byte| *byte == b'\n');
        let take = newline.unwrap_or(read);
        if payload.len().saturating_add(take) > MAX_REQUEST_BYTES {
            let envelope = failure(
                "request_too_large",
                format!("request exceeds {MAX_REQUEST_BYTES} bytes"),
            );
            return write_envelope(&mut stream, &envelope).await;
        }
        payload.extend_from_slice(&chunk[..take]);
        if newline.is_some() {
            break;
        }
    }

    let envelope = match serde_json::from_slice::<IpcRequest>(&payload) {
        Ok(request) => match validate_request(&request) {
            Err(error) => failure("invalid_parameters", error),
            Ok(()) => {
                let query_store = Arc::clone(&store);
                match tokio::task::spawn_blocking(move || dispatch(&query_store, request)).await {
                    Ok(Ok(result)) => success(result),
                    Ok(Err(error)) => failure("request_failed", error.to_string()),
                    Err(error) => failure("internal_error", format!("query task failed: {error}")),
                }
            }
        },
        Err(error) => failure("invalid_request", error.to_string()),
    };
    write_envelope(&mut stream, &envelope).await
}

#[cfg(unix)]
pub struct UnixIpcServer {
    listener: tokio::net::UnixListener,
    path: std::path::PathBuf,
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
            match std::os::unix::net::UnixStream::connect(path) {
                Ok(_) => anyhow::bail!("devmap IPC endpoint is already active at {path:?}"),
                Err(_) => std::fs::remove_file(path)?,
            }
        }
        let listener = tokio::net::UnixListener::bind(path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        Ok(Self {
            listener,
            path: path.to_path_buf(),
        })
    }

    pub async fn run(self, store: Arc<Store>) -> anyhow::Result<()> {
        loop {
            let (stream, _) = self.listener.accept().await?;
            let store = Arc::clone(&store);
            tokio::spawn(async move {
                if let Err(error) = handle_stream(stream, store).await {
                    tracing::warn!("IPC connection failed: {error}");
                }
            });
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
pub async fn run_named_pipe(store: Arc<Store>, name: &str) -> anyhow::Result<()> {
    use tokio::net::windows::named_pipe::ServerOptions;

    let mut server = ServerOptions::new()
        .first_pipe_instance(true)
        .create(name)?;
    loop {
        server.connect().await?;
        let connected = server;
        server = ServerOptions::new().create(name)?;
        let store = Arc::clone(&store);
        tokio::spawn(async move {
            if let Err(error) = handle_stream(connected, store).await {
                tracing::warn!("named-pipe IPC connection failed: {error}");
            }
        });
    }
}

#[cfg(test)]
mod tests {
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
        let task = tokio::spawn(server.run(Arc::new(Store::open_in_memory().unwrap())));

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
