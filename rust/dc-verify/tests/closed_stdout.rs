//! Exercise the actual protocol binary with a disconnected output consumer.
#[cfg(unix)]
#[test]
fn a_closed_stdout_is_a_quiet_transport_failure_for_success_and_error() {
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;
    use std::process::{Command, Stdio};
    for command in ["health", "unknown-command"] {
        let (reader, writer) = UnixStream::pair().unwrap();
        drop(reader);
        let output = Command::new(env!("CARGO_BIN_EXE_dcverify"))
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
}
