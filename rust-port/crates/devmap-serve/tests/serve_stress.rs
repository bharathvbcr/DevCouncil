//! Load and abuse against the three seams an agent actually reaches: the modern
//! HTTP transport, the stdio transport, and the IPC daemon.
//!
//! Every bound asserted here is a bound on *this* process — a buffer that stops
//! growing, a connection that is let go, a queue that drains — never on RSS.
//! Resident memory measures the allocator's retention policy as much as the
//! program's, so a test that watches it fails for reasons unrelated to the
//! thing under test.
//!
//! The time limits are sized for a debug build on a loaded machine: they are
//! there to catch a hang, not to measure throughput. Nothing here is
//! `#[ignore]`d, because a stress test nobody runs is a stress test that does
//! not exist.

#![cfg(unix)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use devmap_serve::mcp::StoreSlot;
use devmap_store::Store;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn corpus_store() -> Arc<Store> {
    let files = [
        ("core.py", "def helper(rows):\n    return sum(rows)\n"),
        (
            "caller.py",
            "from core import helper\n\n\ndef run(rows):\n    return helper(rows)\n",
        ),
    ];
    let extractions: Vec<_> = files
        .iter()
        .map(|(path, source)| devmap_extract::extract_file(path, source))
        .collect();
    let mut resolver = devmap_resolve::Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let analysis = devmap_analyze::analyze(&extractions, &resolution);
    let store = Store::open_in_memory().expect("in-memory store");
    store
        .save_generation(&extractions, &resolution, &analysis)
        .expect("generation");
    Arc::new(store)
}

fn corpus_slot() -> Arc<StoreSlot> {
    Arc::new(StoreSlot::ready("in-memory", corpus_store()))
}

async fn start_http() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("addr").to_string();
    tokio::spawn(devmap_serve::mcp_http::serve_http_on(
        corpus_slot(),
        listener,
    ));
    address
}

/// The `_meta` fields and mirrored headers the 2026-07-28 revision requires on
/// every request.
///
/// Injected rather than written into each body because they are not what any of
/// these tests is about: the revision replaced the `initialize` handshake with
/// per-request metadata, and the headers must agree with the body they describe,
/// so a request without them is malformed for a reason unrelated to the flood,
/// the ceiling or the stall each case is probing. Derived from the body, the
/// same way `tests/mcp_http_modern.rs` derives them — a fixed `Mcp-Method` would
/// send a mismatched request on every case that is not `tools/list`.
fn with_request_meta(body: &str) -> String {
    let Ok(mut parsed) = serde_json::from_str::<serde_json::Value>(body) else {
        return body.to_string();
    };
    let Some(object) = parsed.as_object_mut() else {
        return body.to_string();
    };
    if let Some(params) = object
        .entry("params")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
    {
        if let Some(meta) = params
            .entry("_meta")
            .or_insert_with(|| serde_json::json!({}))
            .as_object_mut()
        {
            meta.entry("io.modelcontextprotocol/protocolVersion")
                .or_insert_with(|| serde_json::json!("2026-07-28"));
            meta.entry("io.modelcontextprotocol/clientCapabilities")
                .or_insert_with(|| serde_json::json!({}));
        }
    }
    parsed.to_string()
}

fn mirrored_headers(body: &str) -> Vec<(String, String)> {
    let parsed: serde_json::Value = serde_json::from_str(body).unwrap_or(serde_json::Value::Null);
    let version = parsed
        .pointer("/params/_meta/io.modelcontextprotocol~1protocolVersion")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("2026-07-28");
    let mut headers = vec![("MCP-Protocol-Version".to_string(), version.to_string())];
    if let Some(method) = parsed.get("method").and_then(serde_json::Value::as_str) {
        headers.push(("Mcp-Method".to_string(), method.to_string()));
    }
    if let Some(name) = parsed
        .pointer("/params/name")
        .and_then(serde_json::Value::as_str)
    {
        headers.push(("Mcp-Name".to_string(), name.to_string()));
    }
    headers
}

fn post(body: &str) -> String {
    let body = with_request_meta(body);
    let mut head = String::from(
        "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n\
Accept: application/json, text/event-stream\r\n",
    );
    for (name, value) in mirrored_headers(&body) {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    format!(
        "{head}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// One HTTP exchange, retried while the transport refuses to carry it.
///
/// A burst of 256 simultaneous connections overruns the kernel's listen
/// backlog, and an overrun backlog resets connections — on the handshake, or
/// after they were established but never accepted. That is admission control
/// below the server, not the server's answer, and every real HTTP client
/// retries it. `tools/list` is idempotent, so a retry is free.
///
/// A `503` is retried too, which is the whole point of answering with one: the
/// server has a ceiling on concurrent connections, and `Retry-After` is the
/// instruction a compliant client follows. Before that ceiling existed this
/// helper only retried transport errors, because the server had no other way to
/// say "busy" — it simply served everyone, or reset them.
///
/// What the caller asserts is what the retries cannot paper over: every id
/// answered, each exactly once, with its own answer, inside the time bound, and
/// the server still serving afterwards. A server that stopped accepting, or one
/// that shed and never recovered, fails here on the deadline.
async fn raw_request(address: &str, raw: &[u8]) -> (u16, String) {
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        match try_exchange(address, raw).await {
            Ok((503, body)) => {
                assert!(
                    body.contains("ceiling"),
                    "a 503 must name the bound it hit: {body}"
                );
                assert!(
                    Instant::now() < deadline,
                    "the server shed this request and never recovered"
                );
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Ok(answer) => return answer,
            Err(error) => {
                assert!(
                    Instant::now() < deadline,
                    "the server stopped answering: {error}"
                );
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
    }
}

/// Send raw bytes, read to EOF, return the status line's code and the body.
async fn try_exchange(address: &str, raw: &[u8]) -> std::io::Result<(u16, String)> {
    let mut stream = tokio::net::TcpStream::connect(address).await?;
    stream.write_all(raw).await?;
    stream.flush().await?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await?;
    let text = String::from_utf8_lossy(&response).to_string();
    let status: u16 = text
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or_else(|| panic!("no status line in response: {text}"));
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_string())
        .unwrap_or_default();
    Ok((status, body))
}

fn tools_list(id: u64) -> String {
    format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/list"}}"#)
}

/// 256 clients at once, each with its own connection.
///
/// Four times `MAX_CONCURRENT_HTTP_CONNECTIONS`, so most of them meet the
/// ceiling and are shed with a `503` and a `Retry-After`. That is the answer
/// under test as much as the `200` is: a client that obeys it must complete,
/// every id must come back exactly once with its own answer, and the server
/// must still be serving afterwards.
///
/// This asserted a flat `200` per client before the ceiling existed, which was
/// only correct because the server had no admission control — it served all 256
/// at once, each free to buffer a megabyte before any validation ran, and the
/// only thing bounding the burst was the client's willingness to open sockets.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_hundred_and_fifty_six_concurrent_posts_are_each_answered_once() {
    let address = start_http().await;
    let started = Instant::now();

    let mut clients = Vec::new();
    for id in 0..256u64 {
        let address = address.clone();
        clients.push(tokio::spawn(async move {
            let (status, body) = raw_request(&address, post(&tools_list(id)).as_bytes()).await;
            (id, status, body)
        }));
    }

    let mut answered = std::collections::BTreeSet::new();
    for client in clients {
        let (id, status, body) = client.await.expect("no client task may panic");
        assert_eq!(status, 200, "request {id} was answered {status}: {body}");
        let value: Value = serde_json::from_str(&body).expect("a JSON body");
        assert_eq!(
            value["id"], id,
            "an answer was matched to the wrong request"
        );
        assert!(
            value["result"]["tools"].is_array(),
            "request {id} got no tool list: {body}"
        );
        assert!(answered.insert(id), "request {id} was answered twice");
    }
    assert_eq!(answered.len(), 256);
    assert!(
        started.elapsed() < Duration::from_secs(60),
        "256 concurrent exchanges took {:?}",
        started.elapsed()
    );

    // Still serving, which is the half a load test usually forgets.
    let (status, _) = raw_request(&address, post(&tools_list(9_999)).as_bytes()).await;
    assert_eq!(status, 200, "the server stopped serving after the burst");
}

/// A client that announces a body and then stops writing must be let go, and
/// must not take the server's availability with it.
///
/// `header_read_timeout` bounds the *headers* and nothing else, so once the
/// request line and headers were in, `read_body` awaited the next frame with no
/// deadline at all. A peer that sent `Content-Length: 4096` and four bytes held
/// its connection, its task and its file descriptor for as long as it liked —
/// free, repeatable, and invisible. Bounding the body read is what makes the
/// slowloris cost the client something.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_client_that_stops_writing_its_body_is_let_go() {
    let address = start_http().await;

    // Headers claiming a body, then four bytes of it, then silence.
    let mut slow = tokio::net::TcpStream::connect(&address)
        .await
        .expect("connect");
    slow.write_all(
        b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n\
Content-Length: 4096\r\nConnection: close\r\n\r\n{\"js",
    )
    .await
    .expect("write");
    slow.flush().await.expect("flush");

    // The server keeps answering everyone else while that connection hangs.
    let (status, _) = raw_request(&address, post(&tools_list(1)).as_bytes()).await;
    assert_eq!(status, 200, "a stalled peer must not block other clients");

    // And the stalled connection is closed by the server rather than held for
    // as long as the client cares to hold it. Thirty seconds is the window the
    // brief names; the bound itself is smaller, and the assertion is only that
    // it exists.
    let mut trailing = Vec::new();
    let closed = tokio::time::timeout(
        Duration::from_secs(60),
        tokio::io::AsyncReadExt::read_to_end(&mut slow, &mut trailing),
    )
    .await;
    assert!(
        closed.is_ok(),
        "the server held a half-written request open past its own deadline; \
         a peer that never finishes writing must not own a connection forever"
    );

    let (status, _) = raw_request(&address, post(&tools_list(2)).as_bytes()).await;
    assert_eq!(status, 200, "the server must still serve after the timeout");
}

/// Ten megabytes, sent chunked so no `Content-Length` announces it.
///
/// The declared-length check is the easy half and is already pinned; this is
/// the other one — a body that claims nothing and sends everything must be cut
/// while it streams, not after it has been assembled.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_ten_megabyte_chunked_body_is_cut_while_it_streams() {
    let address = start_http().await;
    let started = Instant::now();

    let mut stream = tokio::net::TcpStream::connect(&address)
        .await
        .expect("connect");
    stream
        .write_all(
            b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n\
Transfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
        )
        .await
        .expect("write headers");

    // 10 MB in 64 KiB chunks. The server is expected to stop reading partway,
    // so a broken pipe here is the success case, not a failure.
    let payload = vec![b'x'; 64 * 1024];
    let chunk_header = format!("{:x}\r\n", payload.len());
    let mut sent = 0usize;
    while sent < 10 * 1024 * 1024 {
        if stream.write_all(chunk_header.as_bytes()).await.is_err()
            || stream.write_all(&payload).await.is_err()
            || stream.write_all(b"\r\n").await.is_err()
        {
            break;
        }
        sent += payload.len();
    }
    let _ = stream.write_all(b"0\r\n\r\n").await;

    let mut response = Vec::new();
    let read =
        tokio::time::timeout(Duration::from_secs(60), stream.read_to_end(&mut response)).await;
    assert!(read.is_ok(), "the oversized body was never answered or cut");
    let text = String::from_utf8_lossy(&response);
    if let Some(code) = text.split_whitespace().nth(1) {
        assert_ne!(
            code, "200",
            "a body far past the limit must not be accepted: {text}"
        );
    }
    assert!(
        started.elapsed() < Duration::from_secs(60),
        "streaming refusal took {:?}",
        started.elapsed()
    );

    let (status, _) = raw_request(&address, post(&tools_list(3)).as_bytes()).await;
    assert_eq!(status, 200, "the server must still serve after the flood");
}

/// A five-thousand-member JSON-RPC batch.
///
/// This transport does not do batches — the `2026-07-28` revision removed them
/// — so the correct answer is one refusal, arrived at without walking the array.
/// What must not happen is a partial answer, a hang, or five thousand
/// responses on a transport whose contract is one exchange per POST.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_five_thousand_member_batch_is_refused_once_and_promptly() {
    let address = start_http().await;
    let members: Vec<String> = (0..5_000u64).map(tools_list).collect();
    let batch = format!("[{}]", members.join(","));
    assert!(
        batch.len() < 1024 * 1024,
        "the fixture must be a legal-sized body, so what is under test is the \
         batch and not the size limit: {} bytes",
        batch.len()
    );

    let started = Instant::now();
    let (status, body) = raw_request(&address, post(&batch).as_bytes()).await;
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "a batch took {:?} to refuse",
        started.elapsed()
    );
    assert_eq!(
        status, 400,
        "a batch is not a request this transport can answer: {body}"
    );
    let value: Value = serde_json::from_str(&body).expect("a JSON refusal");
    assert!(
        value.get("error").is_some(),
        "the refusal must be a JSON-RPC error frame: {body}"
    );

    let (status, _) = raw_request(&address, post(&tools_list(4)).as_bytes()).await;
    assert_eq!(status, 200, "the server must still serve after the batch");
}

/// A gigabyte on one line, and a line of NULs, over the stdio transport.
///
/// The reader is fed from a generator rather than a buffer: allocating the
/// gigabyte in the test would prove nothing about the server and would be the
/// only part of the run that used a gigabyte. `read_frame` must count and
/// discard past its limit, answer one refusal, and keep the session alive for
/// the request that follows.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_gigabyte_line_and_a_nul_line_cost_their_own_frames_and_no_more() {
    /// A reader that produces `total` bytes of `fill` without holding them.
    struct Endless {
        remaining: usize,
        fill: u8,
        tail: std::collections::VecDeque<u8>,
    }

    impl tokio::io::AsyncRead for Endless {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            let this = &mut *self;
            if this.remaining > 0 {
                let take = this.remaining.min(buf.remaining()).min(64 * 1024);
                buf.put_slice(&vec![this.fill; take]);
                this.remaining -= take;
                return std::task::Poll::Ready(Ok(()));
            }
            let take = this.tail.len().min(buf.remaining());
            for _ in 0..take {
                buf.put_slice(&[this.tail.pop_front().unwrap()]);
            }
            std::task::Poll::Ready(Ok(()))
        }
    }

    // A gigabyte of 'x' with no newline, then a newline, then a line of NULs,
    // then one well-formed request, then EOF.
    let mut tail: Vec<u8> = Vec::new();
    tail.extend_from_slice(b"\n");
    tail.extend_from_slice(&[0u8; 512]);
    tail.extend_from_slice(b"\n");
    tail.extend_from_slice(tools_list(7).as_bytes());
    tail.extend_from_slice(b"\n");
    let reader = Endless {
        remaining: 1024 * 1024 * 1024,
        fill: b'x',
        tail: tail.into_iter().collect(),
    };

    let started = Instant::now();
    let output = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    let sink = SharedSink(Arc::clone(&output));
    let served = tokio::time::timeout(
        Duration::from_secs(120),
        devmap_serve::mcp::serve_streams(
            corpus_slot(),
            tokio::io::BufReader::with_capacity(64 * 1024, reader),
            sink,
        ),
    )
    .await
    .expect("a gigabyte frame must be discarded, not buffered until it is answered");
    served.expect("the session must end cleanly at EOF");

    let written = output.lock().expect("sink").clone();
    let text = String::from_utf8_lossy(&written);
    let frames: Vec<Value> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("every frame must be JSON"))
        .collect();
    assert_eq!(
        frames.len(),
        3,
        "one refusal per bad frame and one answer for the good one: {text}"
    );
    assert_eq!(frames[0]["error"]["code"], -32600, "the oversized frame");
    assert_eq!(frames[1]["error"]["code"], -32700, "the NUL frame");
    assert_eq!(
        frames[2]["id"], 7,
        "the session must survive both and answer the request that follows"
    );
    assert!(
        started.elapsed() < Duration::from_secs(120),
        "the gigabyte frame took {:?}",
        started.elapsed()
    );
}

/// A writer the test can read back once the server has let go of it.
struct SharedSink(Arc<std::sync::Mutex<Vec<u8>>>);

impl tokio::io::AsyncWrite for SharedSink {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        self.0
            .lock()
            .expect("the sink mutex is only taken here")
            .extend_from_slice(buf);
        std::task::Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
}

/// A scratch repository under `/tmp`, short enough for a Unix socket path.
///
/// `std::env::temp_dir()` on macOS is a per-user path under `/var/folders`,
/// which alone is most of the portable 100-byte `sockaddr_un` budget.
fn scratch_repo(tag: &str) -> std::path::PathBuf {
    let dir =
        std::path::PathBuf::from("/tmp").join(format!("devmap-st-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch repo");
    std::fs::write(
        dir.join("core.py"),
        "def helper(rows):\n    return sum(rows)\n",
    )
    .expect("source");
    std::fs::write(
        dir.join("caller.py"),
        "from core import helper\n\n\ndef run(rows):\n    return helper(rows)\n",
    )
    .expect("source");
    dir
}

/// One framed request over the daemon's Unix socket, answered on one line.
async fn ipc_exchange(socket: &std::path::Path, frame: &str) -> std::io::Result<Value> {
    let mut stream = tokio::net::UnixStream::connect(socket).await?;
    stream.write_all(frame.as_bytes()).await?;
    stream.write_all(b"\n").await?;
    stream.flush().await?;
    let mut reply = Vec::new();
    stream.read_to_end(&mut reply).await?;
    let text = String::from_utf8_lossy(&reply).trim().to_string();
    serde_json::from_str(&text).map_err(|error| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("reply was not one JSON envelope ({error}): {text}"),
        )
    })
}

async fn wait_for_socket(socket: &std::path::Path) {
    for _ in 0..500 {
        if socket.exists() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the daemon never bound {}", socket.display());
}

/// 128 clients against the daemon's socket while its queue holds ten thousand
/// paths.
///
/// `MAX_CONCURRENT_CONNECTIONS` is 64, so half of these are held at the
/// semaphore rather than served immediately — which is the point. Backpressure
/// means *queued*, not *dropped*: every client must still get its own envelope.
/// The pending queue is loaded first so the drain is competing for the store's
/// mutex the whole time, which is the condition under which a query that takes
/// the same lock could deadlock instead of waiting.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_ipc_daemon_answers_every_client_while_its_queue_drains() {
    let root = scratch_repo("ipcload");
    let db = root.join("index.sqlite");
    let socket = root.join("d.sock");

    {
        let store = Store::open(&db).expect("store");
        // Ten thousand watcher events' worth of work, queued before the daemon
        // starts so the drain has something to do from its first tick.
        let paths: Vec<String> = (0..10_000)
            .map(|index| format!("generated/file_{index}.py"))
            .collect();
        store.enqueue_pending_paths(&paths).expect("enqueue");
    }

    let store = Store::open(&db).expect("store");
    let daemon = devmap_serve::Daemon::new(store, root.clone())
        .with_ipc_path(socket.clone())
        .with_store_path(db.clone())
        .with_idle_poll(Duration::from_millis(20))
        .with_max_idle(None);
    let serving = tokio::spawn(async move { daemon.run_loop().await });
    wait_for_socket(&socket).await;

    let started = Instant::now();
    let clients: Vec<_> = (0..128u32)
        .map(|index| {
            let socket = socket.clone();
            tokio::spawn(async move {
                let frame = format!(
                    r#"{{"version":1,"cmd":"impact","target":"helper","budget":2000,"depth":{}}}"#,
                    1 + (index % 3)
                );
                // The socket is answered under a semaphore; a client that loses
                // the accept race retries the way a real one does.
                let deadline = Instant::now() + Duration::from_secs(90);
                loop {
                    match ipc_exchange(&socket, &frame).await {
                        Ok(value) => return (index, value),
                        Err(error) => {
                            assert!(
                                Instant::now() < deadline,
                                "client {index} never got an answer: {error}"
                            );
                            tokio::time::sleep(Duration::from_millis(20)).await;
                        }
                    }
                }
            })
        })
        .collect();

    let mut answered = std::collections::BTreeSet::new();
    for client in clients {
        let (index, value) = tokio::time::timeout(Duration::from_secs(120), client)
            .await
            .expect("a client never finished: the daemon deadlocked under load")
            .expect("no client task may panic");
        assert!(
            value["ok"].is_boolean(),
            "client {index} got something that is not an envelope: {value}"
        );
        assert_eq!(
            value["protocol_version"],
            devmap_serve::PROTOCOL_VERSION,
            "client {index} got an unversioned reply"
        );
        assert!(answered.insert(index), "client {index} was answered twice");
    }
    assert_eq!(answered.len(), 128, "clients went missing under load");
    assert!(
        started.elapsed() < Duration::from_secs(120),
        "128 concurrent impact queries took {:?}",
        started.elapsed()
    );

    // Still answering after the burst, and the drain made progress rather than
    // being starved by it.
    let status = ipc_exchange(&socket, r#"{"version":1,"cmd":"status"}"#)
        .await
        .expect("the daemon must still answer after the burst");
    assert_eq!(status["ok"], true, "post-burst status: {status}");

    serving.abort();
    let _ = serving.await;
    let _ = std::fs::remove_dir_all(&root);
}

/// Fifty spawn/retire cycles against one store on disk, then a daemon killed
/// mid-drain and restarted.
///
/// Each cycle binds the endpoint, sweeps, retires on its idle bound and
/// releases. A leak anywhere in that sequence — a socket left behind, a lock
/// still held, a store handle not closed — shows up as the next cycle refusing
/// to bind, which is exactly the failure K-A5 describes.
///
/// The kill is an abort rather than a `SIGKILL`: a real signal would take the
/// test binary with it, and what has to survive is the *store*, which is a
/// separate process's worth of state either way. Aborting the task drops the
/// daemon mid-drain with its SQLite connection open, which is the state a
/// `kill -9` leaves behind; the assertion is that the store reopens, the work
/// is still queued, and the next drain completes it — once, not twice.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fifty_spawn_retire_cycles_and_a_kill_mid_drain_leave_the_store_usable() {
    let root = scratch_repo("lifecycle");
    let db = root.join("index.sqlite");
    let socket = root.join("d.sock");

    let started = Instant::now();
    for cycle in 0..50u32 {
        let store = Store::open(&db).expect("store");
        let daemon = devmap_serve::Daemon::new(store, root.clone())
            .with_ipc_path(socket.clone())
            .with_store_path(db.clone())
            .with_idle_poll(Duration::from_millis(10))
            .with_max_idle(Some(Duration::from_millis(20)));
        let outcome = tokio::time::timeout(Duration::from_secs(60), daemon.run_loop())
            .await
            .unwrap_or_else(|_| panic!("cycle {cycle} never retired"));
        assert!(outcome.is_ok(), "cycle {cycle} reported: {outcome:?}");
        assert!(
            !socket.exists(),
            "cycle {cycle} returned with its socket still on disk; the next \
             client would be told the endpoint is already active"
        );
    }
    assert!(
        started.elapsed() < Duration::from_secs(180),
        "50 spawn/retire cycles took {:?}",
        started.elapsed()
    );

    // Queue real work, then kill a daemon while it is draining it.
    std::fs::write(
        root.join("late.py"),
        "def added_after_the_crash():\n    return 1\n",
    )
    .expect("source");
    {
        let store = Store::open(&db).expect("store");
        store
            .enqueue_pending_paths(&["late.py".to_string()])
            .expect("enqueue");
        assert_eq!(store.get_pending_paths().unwrap().len(), 1);
    }

    let killed = {
        let store = Store::open(&db).expect("store");
        let daemon = devmap_serve::Daemon::new(store, root.clone())
            .with_ipc_path(socket.clone())
            .with_store_path(db.clone())
            .with_idle_poll(Duration::from_millis(10))
            .with_max_idle(None);
        tokio::spawn(async move { daemon.run_loop().await })
    };
    wait_for_socket(&socket).await;
    killed.abort();
    let _ = killed.await;
    // The abort drops the daemon's store handle; the socket may survive it,
    // which is precisely the stale-endpoint state a `kill -9` leaves and which
    // `UnixIpcServer::bind` is expected to clear on the next start.
    let _ = std::fs::remove_file(&socket);

    // The store must reopen, and the work must be accounted for.
    //
    // Whether the killed daemon got through its drain first is a race — its
    // poll interval is 10 ms and the kill lands as soon as the socket appears —
    // so the assertion is on the property that does not depend on who won:
    // after an abrupt exit the work is either still queued or already in the
    // generation. Neither is the shape "lost" takes, and both is the shape
    // "double-charged" takes, which the queue check at the end rules out.
    let store = Store::open(&db).expect("the store must reopen after an abrupt exit");
    let still_queued = store
        .get_pending_paths()
        .unwrap()
        .contains(&"late.py".to_string());
    let already_indexed = store
        .latest_extractions()
        .unwrap()
        .iter()
        .any(|extraction| extraction.file_path == "late.py");
    assert!(
        still_queued || already_indexed,
        "the work queued before the kill was neither replayable nor done"
    );

    let daemon = devmap_serve::Daemon::new(store, root.clone()).with_store_path(db.clone());
    let drained = tokio::task::spawn_blocking(move || daemon.drain_pending_batch())
        .await
        .expect("drain task")
        .expect("the next drain must complete the interrupted work");
    if still_queued {
        assert!(
            drained >= 1,
            "work left queued by the kill must be claimable, not stranded"
        );
    }

    let store = Store::open(&db).expect("store");
    assert!(
        store
            .latest_extractions()
            .unwrap()
            .iter()
            .any(|extraction| extraction.file_path == "late.py"),
        "the file queued before the kill must be in the generation after recovery"
    );
    assert!(
        store.get_pending_paths().unwrap().is_empty(),
        "and it must not still be queued: work completed once is not work to redo"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// Start a server under a caller-supplied ceiling, returning its address and
/// the pool so the test can read the high-water mark back.
async fn start_http_with(admission: devmap_serve::Admission) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("addr").to_string();
    tokio::spawn(devmap_serve::mcp_http::serve_http_on_with_admission(
        corpus_slot(),
        listener,
        admission,
    ));
    address
}

/// Open a connection, send headers announcing a body, send four bytes of it,
/// and hold. Returns the stream so the caller decides when it dies.
async fn slow_post(address: &str) -> Option<tokio::net::TcpStream> {
    let mut stream = tokio::net::TcpStream::connect(address).await.ok()?;
    stream
        .write_all(
            b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n\
Content-Length: 4096\r\nConnection: close\r\n\r\n{\"js",
        )
        .await
        .ok()?;
    stream.flush().await.ok()?;
    Some(stream)
}

/// Twice the ceiling of connections that will not finish, at once.
///
/// Before this, `serve_http_on` spawned a task per accept with no ceiling: the
/// peak was whatever the clients chose, and each of those tasks may buffer up
/// to `MAX_BODY_BYTES` before any validation runs. The socket transport has had
/// `MAX_CONCURRENT_CONNECTIONS` since it was written; this transport had
/// nothing.
///
/// Two properties, and the second is why shedding was chosen over pausing the
/// accept loop. The ceiling must hold — the pool's high-water mark must not
/// pass its limit — and the overflow must be *told*. A paused accept loop pushes
/// the excess into the kernel backlog, an overrun backlog resets it, and a reset
/// on connect is indistinguishable from a dead server: nothing in it a client
/// can act on. A `503` with `Retry-After` is.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_connections_past_the_ceiling_are_shed_with_a_retry_after() {
    const CEILING: usize = 8;

    // Baseline first, through the same harness: what the peak reaches when the
    // pool does not bound it. This is the number the fix moves.
    let unbounded = devmap_serve::Admission::new(usize::MAX);
    let baseline_address = start_http_with(unbounded.clone()).await;
    let mut baseline_clients = Vec::new();
    for _ in 0..CEILING * 2 {
        if let Some(stream) = slow_post(&baseline_address).await {
            baseline_clients.push(stream);
        }
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    let baseline_peak = unbounded.peak();
    drop(baseline_clients);
    assert!(
        baseline_peak > CEILING,
        "the baseline must actually exceed the ceiling, or the comparison below \
         proves nothing: peak was {baseline_peak}"
    );
    assert_eq!(
        unbounded.shed(),
        0,
        "an unbounded pool sheds nothing — that is the problem, not a bug in the fixture"
    );

    let admission = devmap_serve::Admission::new(CEILING);
    let address = start_http_with(admission.clone()).await;
    assert_eq!(admission.limit(), CEILING);

    let mut held = Vec::new();
    for _ in 0..CEILING * 2 {
        if let Some(stream) = slow_post(&address).await {
            held.push(stream);
        }
    }
    assert_eq!(held.len(), CEILING * 2, "every connection must be accepted");

    // Whatever else is true, the excess is answered rather than left to rot:
    // read what each connection got with a short deadline, and require that at
    // least the overflow was told, in a form a client can obey.
    let mut shed_answers = 0usize;
    let mut retry_after_seen = false;
    for mut stream in held {
        let mut response = Vec::new();
        let read =
            tokio::time::timeout(Duration::from_secs(2), stream.read_to_end(&mut response)).await;
        if read.is_err() {
            // Still being served — it holds a permit and is waiting out the
            // body deadline. That is the admitted half.
            continue;
        }
        let text = String::from_utf8_lossy(&response);
        if text.starts_with("HTTP/1.1 503") {
            shed_answers += 1;
            retry_after_seen |= text.to_ascii_lowercase().contains("retry-after:");
            assert!(
                text.contains("ceiling"),
                "a shed connection must be told why, naming the bound: {text}"
            );
        }
    }

    assert!(
        admission.peak() <= CEILING,
        "the pool served {} connections at once, over its ceiling of {CEILING}",
        admission.peak()
    );
    assert!(
        admission.shed() >= CEILING,
        "half of {} connections must have been shed; the pool counted {}",
        CEILING * 2,
        admission.shed()
    );
    assert!(
        shed_answers >= 1,
        "the overflow must be reported to the clients it refused, not only counted"
    );
    assert!(
        retry_after_seen,
        "a 503 without `Retry-After` tells a client it failed but not what to do"
    );
    println!(
        "http in-flight peak: unbounded={baseline_peak} bounded={} (ceiling {CEILING}, \
         shed {})",
        admission.peak(),
        admission.shed()
    );

    // And the ceiling is recoverable: once the body deadline releases the
    // permits the admitted half was holding, an ordinary request is served
    // again. A ceiling that never lets go is an outage with a status code.
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        // `try_exchange`, not `raw_request`: this loop is the one place that
        // has to observe the 503 rather than obey it.
        match try_exchange(&address, post(&tools_list(1)).as_bytes()).await {
            Ok((200, _)) => break,
            Ok((503, body)) => assert!(
                body.contains("ceiling"),
                "while at the ceiling the answer must name it: {body}"
            ),
            Ok((status, body)) => panic!(
                "while at the ceiling the only honest answers are 200 and 503, \
                 not {status}: {body}"
            ),
            Err(error) => assert!(
                Instant::now() < deadline,
                "the server stopped answering entirely: {error}"
            ),
        }
        assert!(
            Instant::now() < deadline,
            "the server never recovered its ceiling after the slow peers were cut"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// A reader that hands over a fixed script of bytes as fast as it is asked.
///
/// Models a client that pipelines: every request is already in the pipe before
/// the first answer is written.
struct Scripted {
    bytes: Vec<u8>,
    offset: usize,
}

impl tokio::io::AsyncRead for Scripted {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let this = &mut *self;
        let take = (this.bytes.len() - this.offset).min(buf.remaining());
        buf.put_slice(&this.bytes[this.offset..this.offset + take]);
        this.offset += take;
        std::task::Poll::Ready(Ok(()))
    }
}

/// Ten thousand requests pipelined into one stdio session.
///
/// The loop spawned a task per line and reaped the finished handles, which
/// bounds the handle vector and nothing that matters: the client decided the
/// fan-out, and every in-flight request holds a frame, a blocking-pool slot and
/// a claim on the store mutex.
///
/// Bounding it must not turn into losing requests, so both halves are asserted:
/// the in-flight peak stays inside the ceiling, and every one of the ten
/// thousand ids comes back exactly once, as a result. Not "as a result or a
/// refusal" — the first version of this ceiling shed past a bounded wait, and
/// a bound that discards a well-formed request from a client doing nothing
/// wrong is data loss wearing a good error message. The policy is to wait, so
/// `shed()` must be zero however deep the pipeline goes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ten_thousand_pipelined_stdio_requests_stay_inside_the_ceiling() {
    const REQUESTS: u64 = 10_000;
    const CEILING: usize = 32;

    let script: Vec<u8> = (0..REQUESTS)
        .flat_map(|id| {
            let mut line = tools_list(id).into_bytes();
            line.push(b'\n');
            line
        })
        .collect();

    async fn run(script: Vec<u8>, admission: devmap_serve::Admission) -> Vec<Value> {
        let output = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
        let served = tokio::time::timeout(
            Duration::from_secs(300),
            devmap_serve::mcp::serve_streams_with_admission(
                corpus_slot(),
                tokio::io::BufReader::with_capacity(
                    64 * 1024,
                    Scripted {
                        bytes: script,
                        offset: 0,
                    },
                ),
                SharedSink(Arc::clone(&output)),
                admission,
            ),
        )
        .await
        .expect("the session must finish rather than hang under a pipelined flood");
        served.expect("the session must end cleanly at EOF");
        let written = output.lock().expect("sink").clone();
        String::from_utf8_lossy(&written)
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).expect("every frame must be JSON"))
            .collect()
    }

    // Baseline through the same harness: the peak with nothing bounding it.
    let unbounded = devmap_serve::Admission::new(usize::MAX);
    let baseline = run(script.clone(), unbounded.clone()).await;
    assert_eq!(baseline.len() as u64, REQUESTS);
    let baseline_peak = unbounded.peak();

    let admission = devmap_serve::Admission::new(CEILING);
    let started = Instant::now();
    let frames = run(script, admission.clone()).await;

    assert!(
        admission.peak() <= CEILING,
        "the session ran {} requests at once, over its ceiling of {CEILING}",
        admission.peak()
    );
    assert!(
        baseline_peak > admission.peak(),
        "the baseline must actually exceed the bounded peak, or this proves \
         nothing: unbounded={baseline_peak} bounded={}",
        admission.peak()
    );
    println!(
        "stdio in-flight peak: unbounded={baseline_peak} bounded={} (ceiling {CEILING}, \
         shed {})",
        admission.peak(),
        admission.shed()
    );

    let mut answered = std::collections::BTreeSet::new();
    for frame in &frames {
        let id = frame["id"].as_u64().unwrap_or_else(|| {
            panic!("every answer must carry the id it was asked against: {frame}")
        });
        assert!(answered.insert(id), "request {id} was answered twice");
        assert!(
            frame.get("error").is_none(),
            "the stdio ceiling is backpressure, not shedding: a well-formed \
             request must be delayed, never refused: {frame}"
        );
        assert!(
            frame["result"]["tools"].is_array(),
            "an accepted request must be answered, not acknowledged: {frame}"
        );
    }
    assert_eq!(
        admission.shed(),
        0,
        "nothing may be shed on stdio; the ceiling is held by waiting"
    );
    assert_eq!(
        answered.len() as u64,
        REQUESTS,
        "every pipelined request must be answered exactly once: a bound that \
         drops work silently is worse than no bound"
    );
    assert!(
        started.elapsed() < Duration::from_secs(300),
        "the bounded session took {:?}",
        started.elapsed()
    );
}
