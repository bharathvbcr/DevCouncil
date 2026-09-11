//! The MCP server, driven as the binary an agent actually spawns.
//!
//! `mcp_spec_conformance.rs` proves the handler correct, but it calls
//! `handle_line` and `serve_streams` in-process. An agent host does not do
//! that: it spawns `devmap mcp` and speaks newline-delimited JSON-RPC over the
//! child's stdin and stdout. Everything between those two — argv parsing, which
//! stream the frames go to, line buffering, flushing, whether the process stays
//! up and whether it comes down when stdin closes — is untested by an
//! in-process suite, and every one of them is load-bearing for a client that
//! sees only the pipe.
//!
//! What this actually guards was established by mutation, not by assertion:
//!
//! * **Publishing `outputSchema`.** Replacing it with `Null` fails this file
//!   ("devmap_status published no outputSchema") while
//!   `mcp_spec_conformance.rs` stays green — the in-process suite reads the
//!   tool table, this reads what came out of the pipe.
//! * The subcommand's own contract — that the server starts *before* an index
//!   exists, so an agent sees the tools and a message naming the build command
//!   rather than no server at all. Asserted against a store path that does not
//!   exist.
//!
//! One thing it does **not** guard, checked rather than assumed: removing the
//! per-frame `writer.flush()` in `mcp.rs` leaves both this file and the
//! in-process suite green. The comment there describes a real deadlock, but
//! `tokio::io::Stdout` writes through on this platform, so the flush is
//! defence rather than load-bearing and nothing here would catch its loss.
//! Said plainly so a later reader does not credit this file with a guard it
//! has not got.
//!
//! Every read is bounded. A protocol test that can hang is a test that will one
//! day hang CI instead of failing it.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

/// Long enough for a debug-build cold start on a loaded machine, short enough
/// that a wedged server fails the run instead of hanging it.
const READ_TIMEOUT: Duration = Duration::from_secs(20);

fn devmap() -> PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap")
}

fn scratch(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "devmap-mcp-e2e-{label}-{}-{stamp}-{seq}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A live `devmap mcp` child, with its stdout drained on a thread so a read
/// can be given a deadline.
struct Server {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
}

impl Server {
    fn start(db: &PathBuf) -> Self {
        let mut child = Command::new(devmap())
            .arg("--db")
            .arg(db)
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("devmap mcp spawns; the binary is built by the test harness");
        let stdout = child.stdout.take().expect("piped stdout");
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(line) => {
                        if tx.send(line).is_err() {
                            return;
                        }
                    }
                    Err(_) => return,
                }
            }
        });
        let stdin = child.stdin.take().expect("piped stdin");
        Self {
            child,
            stdin: Some(stdin),
            lines,
        }
    }

    fn send(&mut self, frame: &Value) {
        let stdin = self.stdin.as_mut().expect("stdin still open");
        writeln!(stdin, "{frame}").expect("write request frame");
        stdin.flush().expect("flush request frame");
    }

    /// One response frame, or a failure naming what was being waited for.
    fn recv(&self, what: &str) -> Value {
        match self.lines.recv_timeout(READ_TIMEOUT) {
            Ok(line) => serde_json::from_str(&line)
                .unwrap_or_else(|e| panic!("{what}: response was not JSON ({e}): {line}")),
            Err(RecvTimeoutError::Timeout) => {
                panic!("{what}: no frame within {READ_TIMEOUT:?} — the server answered nothing")
            }
            Err(RecvTimeoutError::Disconnected) => {
                panic!("{what}: the server's stdout closed before it answered")
            }
        }
    }

    fn request(&mut self, id: Value, method: &str, params: Value, what: &str) -> Value {
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        self.recv(what)
    }

    /// Close stdin and require the child to exit on its own.
    fn shutdown(mut self) {
        drop(self.stdin.take());
        let deadline = std::time::Instant::now() + READ_TIMEOUT;
        loop {
            match self.child.try_wait().expect("try_wait") {
                Some(status) => {
                    assert!(
                        status.success(),
                        "closing stdin must end the server cleanly, not with {status}"
                    );
                    return;
                }
                None if std::time::Instant::now() >= deadline => {
                    let _ = self.child.kill();
                    panic!(
                        "the server did not exit within {READ_TIMEOUT:?} of stdin closing — an \
                         agent host that ends a session would leak this process"
                    );
                }
                None => std::thread::sleep(Duration::from_millis(20)),
            }
        }
    }
}

#[test]
fn an_agent_host_can_speak_mcp_to_the_binary_over_a_pipe() {
    let dir = scratch("handshake");
    // Deliberately absent: the subcommand's contract is that the server starts
    // before any index exists.
    let db = dir.join("devmap.sqlite");
    let mut server = Server::start(&db);

    // --- initialize -------------------------------------------------------
    let reply = server.request(
        json!(1),
        "initialize",
        json!({
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {"name": "devmap-e2e", "version": "0"}
        }),
        "initialize",
    );
    assert_eq!(
        reply["id"],
        json!(1),
        "the reply must carry the request's id"
    );
    let negotiated = reply["result"]["protocolVersion"]
        .as_str()
        .unwrap_or_else(|| panic!("initialize returned no protocolVersion: {reply}"));
    assert!(
        devmap_serve::mcp::HANDSHAKE_PROTOCOL_VERSIONS.contains(&negotiated),
        "negotiated {negotiated}, which is not a version this server declares: {:?}",
        devmap_serve::mcp::HANDSHAKE_PROTOCOL_VERSIONS
    );

    // --- tools/list -------------------------------------------------------
    let reply = server.request(json!(2), "tools/list", json!({}), "tools/list");
    let tools = reply["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("tools/list returned no tools array: {reply}"));
    // Measured against the list the server publishes, not against a number
    // typed here — otherwise "all tools were checked" is only a claim.
    assert_eq!(
        tools.len(),
        devmap_serve::mcp::TOOL_NAMES.len(),
        "the pipe delivered {} tools against {} published",
        tools.len(),
        devmap_serve::mcp::TOOL_NAMES.len()
    );
    for tool in tools {
        let name = tool["name"].as_str().expect("every tool has a name");
        assert!(
            tool["inputSchema"].is_object(),
            "{name} published no inputSchema over the pipe"
        );
        assert!(
            tool["outputSchema"].is_object(),
            "{name} published no outputSchema. Every success carries \
             `structuredContent`, and an undeclared schema leaves a client unable to \
             validate the very fields that say an answer is incomplete."
        );
    }

    // --- tools/call, with no index behind it ------------------------------
    //
    // The subcommand's contract is that this answers rather than refusing to
    // start. It is a *tool* error, not a protocol one: the call succeeded and
    // the answer is "there is nothing to read yet". `structuredContent` is
    // rightly absent, because the declared `outputSchema` describes a success.
    let reply = server.request(
        json!(3),
        "tools/call",
        json!({"name": "devmap_status", "arguments": {}}),
        "tools/call devmap_status",
    );
    assert!(
        reply.get("error").is_none(),
        "an unbuilt index is a tool-level answer, not a JSON-RPC error: {reply}"
    );
    assert_eq!(
        reply["result"]["isError"],
        json!(true),
        "an unbuilt index must be flagged as an error result, or an agent reads \
         'no index' as an answer about the code: {reply}"
    );
    let text = reply["result"]["content"][0]["text"].as_str().unwrap_or("");
    assert!(
        text.contains("devmap build"),
        "the refusal must name the command that fixes it: {text}"
    );
    assert!(
        reply["result"]["structuredContent"].is_null(),
        "an error result must not carry structuredContent, which is what the \
         declared outputSchema describes: {reply}"
    );

    // --- a null id is refused, over the pipe ------------------------------
    server.send(&json!({"jsonrpc": "2.0", "id": null, "method": "tools/list", "params": {}}));
    let reply = server.recv("null id");
    assert!(
        reply.get("error").is_some() && reply.get("result").is_none(),
        "MCP forbids a null id in both eras; answering one with a result hands the \
         client something it is obliged to reject: {reply}"
    );

    // --- and it comes down when the host goes away ------------------------
    server.shutdown();
}

/// The success path, over the same pipe, against a real index.
///
/// Separated from the handshake test because it needs a built store, and
/// because the two failures mean different things: the first says the pipe or
/// the framing is broken, this says the answer that reaches an agent does not
/// match the schema the server published for it.
#[test]
fn a_tool_answer_that_reaches_an_agent_matches_its_declared_schema() {
    let dir = scratch("built");
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("lib.py"),
        "def helper(rows):\n    return rows\n\n\ndef caller(rows):\n    return helper(rows)\n",
    )
    .unwrap();
    let db = dir.join("devmap.sqlite");

    let built = Command::new(devmap())
        .arg("--db")
        .arg(&db)
        .arg("build")
        .arg(&dir)
        .output()
        .expect("devmap build runs");
    assert!(
        built.status.success(),
        "the fixture index must build, or this measures the unbuilt path again: {}",
        String::from_utf8_lossy(&built.stderr)
    );

    let mut server = Server::start(&db);
    server.request(
        json!(1),
        "initialize",
        json!({"protocolVersion": "2025-11-25", "capabilities": {},
               "clientInfo": {"name": "devmap-e2e", "version": "0"}}),
        "initialize",
    );

    let reply = server.request(
        json!(2),
        "tools/call",
        json!({"name": "devmap_status", "arguments": {}}),
        "tools/call devmap_status against a built index",
    );
    assert_eq!(
        reply["result"]["isError"],
        json!(false),
        "a built index must answer without an error flag: {reply}"
    );
    let structured = &reply["result"]["structuredContent"];
    assert!(
        structured.is_object(),
        "a success must carry structuredContent — that is what outputSchema \
         describes, and it is what a client parses: {reply}"
    );
    // The server's own checker decides conformance, so this cannot drift from
    // the schema the server published.
    assert!(
        devmap_serve::mcp::structured_content_violation("devmap_status", structured).is_none(),
        "the answer delivered over the pipe does not satisfy the outputSchema the \
         server published for it: {:?}",
        devmap_serve::mcp::structured_content_violation("devmap_status", structured)
    );

    server.shutdown();
}
