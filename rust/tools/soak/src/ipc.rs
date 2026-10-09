//! One bounded exchange with `devmap serve`: a request line in, the whole
//! response out. The wire form is the daemon's own — newline-framed JSON over
//! a Unix socket, the server closing after its reply.

use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

/// An exchange that has not finished in this long has failed. Separate from,
/// and much shorter than, the settle deadline a whole convergence wait gets.
pub const EXCHANGE_LIMIT: Duration = Duration::from_secs(30);

/// Replies past this are refused rather than buffered without bound.
pub const REPLY_CAP: usize = 2_000_000;

/// Send `frame` and return the reply, within [`EXCHANGE_LIMIT`].
pub fn exchange(endpoint: &Path, frame: &str) -> io::Result<String> {
    exchange_within(endpoint, frame, EXCHANGE_LIMIT)
}

/// [`exchange`] under an explicit bound: the whole exchange — connect, send,
/// and every read of a reply that may trickle in — finishes within `limit`
/// or fails with `TimedOut`.
pub fn exchange_within(endpoint: &Path, frame: &str, limit: Duration) -> io::Result<String> {
    let deadline = Instant::now() + limit;
    let expired = || {
        io::Error::new(
            io::ErrorKind::TimedOut,
            format!("IPC exchange exceeded {} ms", limit.as_millis()),
        )
    };
    let remaining = || {
        deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or_else(expired)
    };
    let timed = |e: io::Error| {
        if matches!(
            e.kind(),
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
        ) {
            expired()
        } else {
            e
        }
    };
    let mut stream = connect_within(endpoint, remaining()?).map_err(timed)?;
    stream.set_write_timeout(Some(remaining()?))?;
    stream.write_all(frame.as_bytes()).map_err(timed)?;
    stream.write_all(b"\n").map_err(timed)?;
    stream.flush().map_err(timed)?;
    let mut reply = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    stream.set_read_timeout(Some(remaining()?))?;
    loop {
        // Narrow the read timeout to what is left of the deadline. macOS
        // refuses the option (EINVAL) once the peer has closed; the timeout
        // already set still bounds the read, and the next one returns EOF.
        let _ = stream.set_read_timeout(Some(remaining()?));
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                reply.extend_from_slice(&chunk[..n]);
                if reply.len() > REPLY_CAP {
                    return Err(io::Error::other("IPC response exceeded 2 MB"));
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            // A socket timeout surfaces as EAGAIN on Unix. Say what it is: the
            // bound fired, not that a resource was briefly unavailable.
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                return Err(expired())
            }
            Err(e) => return Err(e),
        }
    }
    String::from_utf8(reply).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Connect, or give up after `limit`. `connect(2)` on a Unix stream socket
/// blocks on Linux while the listener's backlog is full — a daemon too busy to
/// accept — and std offers no timeout for it, so the connect runs on a thread
/// the caller stops waiting for. A connect still blocked at the bound ends when
/// the daemon accepts or goes away; the soak itself has moved on.
fn connect_within(endpoint: &Path, limit: Duration) -> io::Result<UnixStream> {
    let (tx, rx) = std::sync::mpsc::channel();
    let endpoint = endpoint.to_path_buf();
    std::thread::spawn(move || {
        let _ = tx.send(UnixStream::connect(endpoint));
    });
    rx.recv_timeout(limit)
        .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::os::unix::net::UnixListener;

    fn socket(name: &str) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("soak-ipc-{name}-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn a_request_line_goes_out_and_the_whole_reply_comes_back() {
        let path = socket("echo");
        let listener = UnixListener::bind(&path).expect("bind");
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            let mut line = String::new();
            BufReader::new(&stream)
                .read_line(&mut line)
                .expect("request");
            let mut stream = stream;
            stream
                .write_all(format!("{{\"echo\":{}}}", line.trim()).as_bytes())
                .expect("reply");
        });
        let reply = exchange(&path, r#"{"version":1,"cmd":"status"}"#).expect("exchange");
        assert_eq!(reply, r#"{"echo":{"version":1,"cmd":"status"}}"#);
        server.join().expect("server");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn an_absent_endpoint_is_an_error() {
        assert!(exchange(&socket("absent"), "{}").is_err());
    }

    #[test]
    fn an_oversized_reply_is_refused() {
        let path = socket("big");
        let listener = UnixListener::bind(&path).expect("bind");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let block = vec![b' '; 1 << 20];
            for _ in 0..3 {
                if stream.write_all(&block).is_err() {
                    break;
                }
            }
        });
        let err = exchange(&path, "{}").unwrap_err();
        assert!(err.to_string().contains("exceeded 2 MB"), "{err}");
        server.join().expect("server");
        let _ = std::fs::remove_file(&path);
    }

    /// A server that accepts and never answers, and one that answers a byte
    /// at a time forever: both end at the bound, not at the server's pleasure.
    #[test]
    fn a_silent_or_trickling_server_cannot_outlast_the_bound() {
        for trickle in [false, true] {
            let path = socket(if trickle { "trickle" } else { "silent" });
            let listener = UnixListener::bind(&path).expect("bind");
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().expect("accept");
                let end = Instant::now() + Duration::from_secs(3);
                while Instant::now() < end {
                    if trickle && stream.write_all(b" ").is_err() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            });
            let start = Instant::now();
            let err = exchange_within(&path, "{}", Duration::from_millis(400)).unwrap_err();
            let took = start.elapsed();
            assert_eq!(
                err.kind(),
                io::ErrorKind::TimedOut,
                "trickle={trickle}: {err}"
            );
            assert!(
                took < Duration::from_millis(1500),
                "trickle={trickle}: took {took:?}"
            );
            server.join().expect("server");
            let _ = std::fs::remove_file(&path);
        }
    }

    /// A listener that never accepts fills its backlog; on Linux the next
    /// connect then blocks in the kernel. It must still end at the bound.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_daemon_too_busy_to_accept_cannot_hold_the_connect() {
        let path = socket("backlog");
        let listener = UnixListener::bind(&path).expect("bind");
        let mut held = Vec::new();
        let start = Instant::now();
        let mut timed_out = false;
        for _ in 0..4096 {
            match connect_within(&path, Duration::from_millis(200)) {
                Ok(stream) => held.push(stream),
                Err(e) => {
                    assert_eq!(e.kind(), io::ErrorKind::TimedOut, "{e}");
                    timed_out = true;
                    break;
                }
            }
        }
        assert!(
            timed_out,
            "{} connects never filled the backlog",
            held.len()
        );
        assert!(start.elapsed() < Duration::from_secs(5));
        drop(listener);
        let _ = std::fs::remove_file(&path);
    }
}
