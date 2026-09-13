//! Exercise the actual protocol binary with a disconnected output consumer.
#[cfg(unix)]
mod support;
#[cfg(unix)]
#[test]
fn a_closed_stdout_is_a_quiet_transport_failure_for_success_and_error() {
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;
    use std::process::{Command, Stdio};
    let db = support::seeded("closed-stdout");
    let root = db.parent().unwrap();
    for command in ["health", "unknown-command"] {
        let (reader, writer) = UnixStream::pair().unwrap();
        drop(reader);
        let output = Command::new(env!("CARGO_BIN_EXE_dcstore"))
            .arg("--db")
            .arg(&db)
            .arg(command)
            .stdout(Stdio::from(OwnedFd::from(writer)))
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{command}");
        assert!(
            output.stderr.is_empty(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}
