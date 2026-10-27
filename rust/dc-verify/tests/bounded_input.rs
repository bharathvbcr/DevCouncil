//! stdin and the coverage file are bounded. A stream past the cap is an
//! error, not an allocation of the whole stream, and a non-UTF-8 coverage
//! path does not drop findings the diff already produced.

use std::io::Write as _;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

/// Same number the binary documents as its stdin and coverage cap. The test
/// names it so a move of the constant fails here rather than scanning a
/// different size than the host's git diff cap.
const MAX_CHECK_BYTES: usize = 32 << 20;

fn dcverify() -> Command {
    Command::new(env!("CARGO_BIN_EXE_dcverify"))
}

const DIFF: &str = "diff --git a/src/handler.rs b/src/handler.rs
new file mode 100644
--- /dev/null
+++ b/src/handler.rs
@@ -0,0 +1,4 @@
+pub fn handle() {
+    let aws_key = \"AKIAIOSFODNN7EXAMPLE\";
+    todo!()
+}
";

#[test]
fn a_non_utf8_coverage_path_keeps_findings_and_names_the_file() {
    let dir = scratch("cov");
    let profile = dir.join("lcov.info");
    let mut bytes = b"SF:src/han".to_vec();
    bytes.push(0xff);
    bytes.extend_from_slice(b"dler.rs\nDA:1,1\nend_of_record\n");
    std::fs::write(&profile, bytes).unwrap();
    let reply = run_check(&[&profile], DIFF.as_bytes());
    assert!(reply.contains("\"ok\":true"), "{reply}");
    assert!(reply.contains("\"gate\":\"secret_scan\""), "{reply}");
    assert!(reply.contains("\"gate\":\"stub_detection\""), "{reply}");
    assert!(
        reply.contains("src/handler.rs"),
        "the changed path is missing from coverage_unmeasured: {reply}"
    );
    assert!(reply.contains("coverage_unmeasured\":["), "{reply}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn stdin_past_the_cap_is_not_ok() {
    let mut body = vec![b'x'; MAX_CHECK_BYTES + 1];
    // Not a diff. The refusal is the size, before a parse.
    let reply = run_check(&[], &body);
    assert!(
        reply.contains("\"ok\":false")
            && reply.contains(&format!("exceeds {MAX_CHECK_BYTES} bytes")),
        "ok={} exceeds={} reply_len={}",
        reply.contains("\"ok\":false"),
        reply.contains("exceeds"),
        reply.len()
    );
    body.clear();
}

#[test]
fn a_diff_exactly_at_the_cap_is_ok() {
    let head = "diff --git a/a.txt b/a.txt\n--- /dev/null\n+++ b/a.txt\n@@ -0,0 +1,1 @@\n+";
    assert!(head.len() < MAX_CHECK_BYTES);
    let mut diff = Vec::with_capacity(MAX_CHECK_BYTES);
    diff.extend_from_slice(head.as_bytes());
    diff.resize(MAX_CHECK_BYTES - 1, b'a');
    diff.push(b'\n');
    assert_eq!(diff.len(), MAX_CHECK_BYTES);
    let reply = run_check(&[], &diff);
    assert!(reply.contains("\"ok\":true"), "{reply}");
}

#[test]
fn a_coverage_file_past_the_cap_is_not_ok() {
    let dir = scratch("bigcov");
    let profile = dir.join("lcov.info");
    let f = std::fs::File::create(&profile).unwrap();
    f.set_len(MAX_CHECK_BYTES as u64 + 1).unwrap();
    let reply = run_check(&[&profile], DIFF.as_bytes());
    assert!(
        reply.contains("\"ok\":false")
            && reply.contains(&format!("exceeds {MAX_CHECK_BYTES} bytes")),
        "ok={} exceeds={} reply_len={}",
        reply.contains("\"ok\":false"),
        reply.contains("exceeds"),
        reply.len()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn adv_stdin_one_past_the_cap_does_not_echo_the_body() {
    let marker = "STDIN-BODY-MARKER";
    let mut body = vec![b'q'; MAX_CHECK_BYTES + 1];
    body[..marker.len()].copy_from_slice(marker.as_bytes());
    let reply = run_check(&[], &body);
    assert!(
        reply.contains("\"ok\":false")
            && reply.contains(&format!("exceeds {MAX_CHECK_BYTES} bytes")),
        "ok={} exceeds={}",
        reply.contains("\"ok\":false"),
        reply.contains("exceeds")
    );
    assert!(
        !reply.contains(marker),
        "the exceeds error echoes the stdin body"
    );
}

#[test]
fn adv_exact_cap_utf8_is_borrowed() {
    let bytes = vec![b'a'; MAX_CHECK_BYTES];
    let text = dc_verify::bounded::decode_lossy(&bytes);
    assert!(
        matches!(text, std::borrow::Cow::Borrowed(_)),
        "valid UTF-8 at the cap was copied"
    );
    assert_eq!(text.len(), MAX_CHECK_BYTES);
}

#[test]
fn adv_coverage_one_past_the_cap_does_not_echo_the_tail() {
    let dir = scratch("bigcov-tail");
    let profile = dir.join("lcov.info");
    let marker = b"COVERAGE-TAIL-MARKER";
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(&profile)
        .unwrap();
    f.set_len(MAX_CHECK_BYTES as u64 + 1).unwrap();
    std::io::Seek::seek(
        &mut f,
        std::io::SeekFrom::Start((MAX_CHECK_BYTES + 1 - marker.len()) as u64),
    )
    .unwrap();
    std::io::Write::write_all(&mut f, marker).unwrap();
    let reply = run_check(&[&profile], DIFF.as_bytes());
    assert!(
        reply.contains("\"ok\":false")
            && reply.contains(&format!("exceeds {MAX_CHECK_BYTES} bytes")),
        "ok={} exceeds={}",
        reply.contains("\"ok\":false"),
        reply.contains("exceeds")
    );
    assert!(
        !reply.contains("COVERAGE-TAIL-MARKER"),
        "the exceeds error echoes the coverage body"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn adv_coverage_dev_zero_stops_at_the_cap() {
    // The device itself. A coverage path that is a symlink is refused and
    // never opened, so the cap is measured on /dev/zero directly.
    let profile = std::path::Path::new("/dev/zero");
    let mut child = dcverify()
        .arg("check")
        .arg("--coverage")
        .arg(profile)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(DIFF.as_bytes())
        .unwrap();
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() > std::time::Duration::from_secs(30) => {
                let pid = child.id();
                let _ = child.kill();
                let _ = child.wait();
                panic!("coverage read of /dev/zero did not stop within 30s (pid {pid})");
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(20)),
            Err(e) => panic!("{e}"),
        }
    }
    let mut buf = Vec::new();
    std::io::Read::read_to_end(&mut stdout, &mut buf).unwrap();
    let reply = String::from_utf8_lossy(&buf);
    assert!(
        reply.contains("\"ok\":false")
            && reply.contains(&format!("exceeds {MAX_CHECK_BYTES} bytes")),
        "ok={} exceeds={}",
        reply.contains("\"ok\":false"),
        reply.contains("exceeds")
    );
}

#[test]
fn adv_coverage_symlink_to_a_small_profile_is_not_a_silent_success() {
    let dir = scratch("covlink");
    let real = dir.join("real.info");
    std::fs::write(
        &real,
        "SF:src/handler.rs\nDA:1,1\nDA:2,1\nDA:3,1\nDA:4,1\nend_of_record\n",
    )
    .unwrap();
    let link = dir.join("link.info");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let reply = run_check(&[&link], DIFF.as_bytes());
    let key = "AKIA".to_string() + "IOSFODNN7EXAMPLE";
    let shown = reply.replace(&key, "<key>");
    assert!(
        !reply.contains("\"ok\":true"),
        "coverage symlink was followed and the check succeeded without saying so: {shown}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

fn scratch(kind: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "dcv-{kind}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run_check(coverage: &[&std::path::Path], stdin: &[u8]) -> String {
    let mut cmd = dcverify();
    cmd.arg("check");
    if let Some(path) = coverage.first() {
        cmd.arg("--coverage").arg(path);
    }
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(stdin).unwrap();
    let out = child.wait_with_output().unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}
