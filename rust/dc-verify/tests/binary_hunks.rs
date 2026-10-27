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

#[test]
fn a_deleted_binary_path_is_not_an_unreadable_read() {
    let root = Root::new();
    std::fs::write(root.0.join("src/ok.rs"), "pub fn ok() -> u8 { 1 }\n").unwrap();
    let diff = format!(
        "{TEXT}diff --git a/old.bin b/old.bin\ndeleted file mode 100644\nindex 1234567..0000000\nBinary files a/old.bin and /dev/null differ\n"
    );
    let reply = check(&root.0, "src/ok.rs\nold.bin", &diff);
    assert!(
        reply.contains("\"ok\":true"),
        "the check did not run: {reply}"
    );
    assert!(
        reply.contains("\"stub_ast_degraded\":[]"),
        "a deleted binary has no bytes to read, and was reported as a degraded read: {reply}"
    );
    assert!(!reply.contains("\"gate\":\"secret_scan\""), "{reply}");
}

#[test]
fn a_deleted_binary_with_a_git_binary_patch_is_not_an_unreadable_read() {
    let root = Root::new();
    std::fs::write(root.0.join("src/ok.rs"), "pub fn ok() -> u8 { 1 }\n").unwrap();
    let diff = format!(
        "{TEXT}diff --git a/old.bin b/old.bin\ndeleted file mode 100644\nindex 1234567..0000000\nGIT binary patch\nliteral 0\nHcmV?d00001\n\nliteral 4\nLcmZQz00000\n\n"
    );
    let reply = check(&root.0, "src/ok.rs\nold.bin", &diff);
    assert!(
        reply.contains("\"ok\":true"),
        "the check did not run: {reply}"
    );
    assert!(reply.contains("\"stub_ast_degraded\":[]"), "{reply}");
}

#[test]
fn a_binary_scan_that_could_not_read_says_it_was_the_secret_scan() {
    let root = Root::new();
    let outside = root.0.join("outside");
    std::fs::write(&outside, "x").unwrap();
    std::os::unix::fs::symlink(&outside, root.0.join("secret.bin")).unwrap();
    std::fs::write(root.0.join("src/ok.rs"), "pub fn ok() -> u8 { 1 }\n").unwrap();
    let diff = format!(
        "{TEXT}diff --git a/secret.bin b/secret.bin\nBinary files a/secret.bin and b/secret.bin differ\n"
    );
    let reply = check(&root.0, "secret.bin", &diff);
    assert!(
        reply.contains("secret.bin: secret scan skipped (binary file not read: "),
        "the degraded entry does not say the secret scan skipped the file: {reply}"
    );
}

#[test]
fn a_symlinked_directory_in_a_binary_path_is_not_followed() {
    let root = Root::new();
    let key = secret();
    let elsewhere = Root::new();
    std::fs::write(elsewhere.0.join("secret.bin"), format!("\0{key}\0")).unwrap();
    std::os::unix::fs::symlink(&elsewhere.0, root.0.join("d")).unwrap();
    std::fs::write(root.0.join("src/ok.rs"), "pub fn ok() -> u8 { 1 }\n").unwrap();
    let diff = format!(
        "{TEXT}diff --git a/d/secret.bin b/d/secret.bin\nBinary files a/d/secret.bin and b/d/secret.bin differ\n"
    );
    let reply = check(&root.0, "d/secret.bin", &diff);
    assert!(
        reply.contains("\"ok\":true"),
        "the check did not run: {reply}"
    );
    assert!(
        !reply.contains("\"gate\":\"secret_scan\""),
        "a file behind a symlinked directory was read: {reply}"
    );
    assert!(
        reply.contains("d/secret.bin: secret scan skipped"),
        "a symlinked directory was not refused: {reply}"
    );
}

#[test]
fn a_binary_with_a_git_quoted_name_is_scanned() {
    let root = Root::new();
    let key = secret();
    std::fs::write(root.0.join("caf\u{e9}.bin"), format!("\0{key}\0")).unwrap();
    std::fs::write(root.0.join("src/ok.rs"), "pub fn ok() -> u8 { 1 }\n").unwrap();
    let diff = format!(
        "{TEXT}diff --git \"a/caf\\303\\251.bin\" \"b/caf\\303\\251.bin\"\nBinary files \"a/caf\\303\\251.bin\" and \"b/caf\\303\\251.bin\" differ\n"
    );
    let reply = check(&root.0, "caf\u{e9}.bin", &diff);
    assert!(
        reply.contains("\"ok\":true"),
        "the check did not run: {reply}"
    );
    assert!(
        reply.contains("\"gate\":\"secret_scan\""),
        "a binary whose name git quotes was never read: {reply}"
    );
    assert!(!reply.contains(&key), "the reply quotes the credential");
}
