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

// A panic prints where it happened and nothing it carried. The default hook
// prints the panic message, and a message can hold data from the diff (a
// string-slicing panic prints up to 256 bytes of the sliced string); the Go
// host copies stderr into its errors, so a credential in it would reach the
// agent and the JSON report. The planting hook exists in debug builds only.
#[cfg(debug_assertions)]
#[test]
fn a_panic_carrying_a_secret_prints_only_its_location() {
    use std::process::{Command, Stdio};

    let secret = concat!("AKIA", "IOSFODNN7EXAMPLE");
    let out = Command::new(env!("CARGO_BIN_EXE_dcverify"))
        .arg("health")
        .env("DCVERIFY_TEST_PANIC_WITH", secret)
        .stdin(Stdio::null())
        .output()
        .expect("dcverify runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "the planted panic did not happen");
    assert!(
        !stderr.contains(secret),
        "the panic message reached stderr: {stderr}"
    );
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains(secret),
        "the panic message reached stdout"
    );
    assert!(
        stderr.contains("dcverify panicked at ") && stderr.contains(".rs:"),
        "the hook did not say where: {stderr}"
    );
    assert!(
        !stderr.contains("planted payload"),
        "the payload text reached stderr: {stderr}"
    );
}
