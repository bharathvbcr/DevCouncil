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

/// Send `frame` and return the reply.
pub fn exchange(endpoint: &Path, frame: &str) -> io::Result<String> {
    let deadline = Instant::now() + EXCHANGE_LIMIT;
    let remaining = || {
        deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::TimedOut, "IPC exchange exceeded 30 seconds")
            })
    };
    let mut stream = UnixStream::connect(endpoint)?;
    stream.set_write_timeout(Some(remaining()?))?;
    stream.write_all(frame.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;
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
            Err(e) => return Err(e),
        }
    }
    String::from_utf8(reply).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
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
}
