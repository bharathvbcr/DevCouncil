//! A binary diff has a marker and no hunk. The bytes are not in the diff, so
//! a planned path in that state has to be read from the working tree — a text
//! hunk elsewhere in the same diff does not make the binary path clean.

use std::io::Write as _;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

const TEXT: &str = "diff --git a/src/ok.rs b/src/ok.rs
--- a/src/ok.rs
+++ b/src/ok.rs
@@ -1 +1 @@
-pub fn ok() -> u8 { 0 }
+pub fn ok() -> u8 { 1 }
";

fn secret() -> String {
    concat!("AKIA", "IOSFODNN7EXAMPLE").to_string()
}

struct Root(PathBuf);

impl Root {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "dcv-binhunk-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(base.join("src")).unwrap();
        Self(base)
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn check(root: &std::path::Path, planned: &str, diff: &str) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_dcverify"))
        .args(["check", "--root"])
        .arg(root)
        .args(["--planned", planned])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(diff.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn a_planned_binary_path_with_no_hunk_is_not_clean() {
    let root = Root::new();
    let key = secret();
    std::fs::write(root.0.join("src/ok.rs"), "pub fn ok() -> u8 { 1 }\n").unwrap();
    std::fs::write(root.0.join("secret.bin"), format!("\0{key}\0")).unwrap();
    let diff = format!(
        "{TEXT}diff --git a/secret.bin b/secret.bin\nBinary files a/secret.bin and b/secret.bin differ\n"
    );
    let reply = check(&root.0, "src/ok.rs\nsecret.bin", &diff);
    assert!(
        reply.contains("\"ok\":true"),
        "the check did not run: {reply}"
    );
    let scanned = reply.contains("\"gate\":\"secret_scan\"")
        || reply.contains("stub_ast_degraded\":[") && !reply.contains("stub_ast_degraded\":[]");
    assert!(
        scanned,
        "binary path with no hunk scanned clean beside a text hunk: {reply}"
    );
    assert!(
        !reply.contains(&key),
        "the reply quotes the credential in full"
    );
}

#[test]
fn adv_key_past_the_probe_is_not_clean() {
    let root = Root::new();
    let key = secret();
    let mut body = vec![0u8];
    body.extend(std::iter::repeat(b'B').take(9000));
    body.push(b'\n');
    body.extend(format!("let aws_key = \"{key}\";\n").into_bytes());
    std::fs::write(root.0.join("src/ok.rs"), "pub fn ok() -> u8 { 1 }\n").unwrap();
    std::fs::write(root.0.join("secret.bin"), &body).unwrap();
    let diff = format!(
        "{TEXT}diff --git a/secret.bin b/secret.bin\nBinary files a/secret.bin and b/secret.bin differ\n"
    );
    let reply = check(&root.0, "src/ok.rs\nsecret.bin", &diff);
    assert!(reply.contains("\"ok\":true"), "the check did not run");
    let scanned = reply.contains("\"gate\":\"secret_scan\"")
        || (reply.contains("stub_ast_degraded\":[") && !reply.contains("stub_ast_degraded\":[]"));
    assert!(scanned, "key past the binary probe was a clean scan");
    assert!(!reply.contains(&key), "the reply quotes the credential");
}

#[test]
fn adv_binary_symlink_target_is_not_in_the_reply() {
    let root = Root::new();
    let sentinel = "FOLLOWED-SYMLINK-BYTES";
    let outside = root.0.join("outside");
    std::fs::write(&outside, sentinel.as_bytes()).unwrap();
    std::os::unix::fs::symlink(&outside, root.0.join("secret.bin")).unwrap();
    std::fs::write(root.0.join("src/ok.rs"), "pub fn ok() -> u8 { 1 }\n").unwrap();
    let diff = format!(
        "{TEXT}diff --git a/secret.bin b/secret.bin\nBinary files a/secret.bin and b/secret.bin differ\n"
    );
    let reply = check(&root.0, "secret.bin", &diff);
    assert!(
        !reply.contains(sentinel),
        "a binary symlink target was read"
    );
    let refused = (reply.contains("stub_ast_degraded\":[")
        && !reply.contains("stub_ast_degraded\":[]"))
        || reply.contains("\"ok\":false");
    assert!(refused, "a binary symlink was a clean scan");
}

#[test]
fn a_planned_binary_symlink_is_not_followed() {
    let root = Root::new();
    let key = secret();
    let outside = root.0.join("outside");
    std::fs::write(&outside, &key).unwrap();
    std::os::unix::fs::symlink(&outside, root.0.join("secret.bin")).unwrap();
    std::fs::write(root.0.join("src/ok.rs"), "pub fn ok() -> u8 { 1 }\n").unwrap();
    let diff = format!(
        "{TEXT}diff --git a/secret.bin b/secret.bin\nBinary files a/secret.bin and b/secret.bin differ\n"
    );
    let reply = check(&root.0, "secret.bin", &diff);
    assert!(
        !reply.contains(&key),
        "a symlink target's bytes were quoted"
    );
    let refused = reply.contains("stub_ast_degraded\":[")
        && !reply.contains("stub_ast_degraded\":[]")
        || reply.contains("\"ok\":false");
    assert!(
        refused,
        "a binary symlink was treated as a clean scan: {reply}"
    );
}
