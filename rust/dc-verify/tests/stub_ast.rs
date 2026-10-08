//! The seam between the diff and the post-change source.
//!
//! `corpus/cases.txt` measures the AST checks as classifiers on files a diff
//! adds whole. These cover what the corpus cannot: a *modified* file, whose
//! post-change source has to come from somewhere else and be believed only
//! when it agrees with the diff; and the binary's own reader, which takes a
//! path out of a diff and must not follow it outside `--root`.

use std::io::Write as _;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use dc_verify::parse_unified;
use dc_verify::rigor::{
    GATE_STUB_ALLOWED, Severity, Strength, detect_stubs_report, detect_stubs_with,
};

const MODIFIED: &str = "diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,1 +1,2 @@
 pub fn a() -> u8 { 1 }
+pub fn b() -> &'static str { \"todo!()\" }
";

const POST_IMAGE: &str = "pub fn a() -> u8 { 1 }\npub fn b() -> &'static str { \"todo!()\" }\n";

#[test]
fn a_modified_file_is_parsed_from_the_source_it_is_given() {
    let files = parse_unified(MODIFIED).unwrap();
    let findings = detect_stubs_with(&files, &|_| Some(POST_IMAGE.to_string()));
    assert!(
        findings.iter().all(|f| f.severity != Severity::Blocking),
        "a string literal blocked even with the source to parse: {findings:?}"
    );
}

#[test]
fn a_source_that_disagrees_with_the_diff_is_not_believed() {
    let files = parse_unified(MODIFIED).unwrap();
    // Line 2 of this source is not the line the diff says it added, so the
    // tree would describe some other file. The substring check runs instead,
    // and says it only derived its answer.
    let findings = detect_stubs_with(&files, &|_| Some("pub fn a() -> u8 { 1 }\n".to_string()));
    let blocking: Vec<_> = findings
        .iter()
        .filter(|f| f.severity == Severity::Blocking)
        .collect();
    assert_eq!(blocking.len(), 1, "{findings:?}");
    assert_eq!(blocking[0].strength, Strength::Derived);
}

#[test]
fn no_source_falls_back_rather_than_reading_clean() {
    let files = parse_unified(MODIFIED).unwrap();
    let findings = detect_stubs_with(&files, &|_| None);
    assert!(
        findings
            .iter()
            .any(|f| f.severity == Severity::Blocking && f.strength == Strength::Derived),
        "with nothing to parse, the substring check must still run: {findings:?}"
    );
}

#[test]
fn a_parsed_placeholder_is_proven_and_an_allowed_one_keeps_its_reason() {
    let diff = "diff --git a/src/x.rs b/src/x.rs
new file mode 100644
--- /dev/null
+++ b/src/x.rs
@@ -0,0 +1,4 @@
+pub fn a() -> u8 { todo!() }
+/// Docs.
+#[inline] // allow-stub: placeholder until the codec lands
+pub fn b() -> u8 { unimplemented!() }
";
    let files = parse_unified(diff).unwrap();
    let findings = detect_stubs_with(&files, &|_| None);
    let a = findings
        .iter()
        .find(|f| f.line == 1)
        .expect("a finding on line 1");
    assert_eq!(
        (a.gate, a.severity, a.strength),
        ("stub_detection", Severity::Blocking, Strength::Proven)
    );
    let b = findings
        .iter()
        .find(|f| f.line == 4)
        .expect("a finding on line 4");
    assert_eq!(b.gate, GATE_STUB_ALLOWED);
    assert_eq!(b.severity, Severity::Advisory);
    assert!(
        b.message.contains("placeholder until the codec lands"),
        "{}",
        b.message
    );
}

/// Java and C# are parsed for placeholders, so their findings are `proven` —
/// but no test check reads them, and a report counting them toward
/// `parsed_files` would tell the Go client the test gates ran when they did
/// not.
#[test]
fn a_parsed_java_or_csharp_file_is_proven_and_not_counted_for_test_rigor() {
    let diff = "diff --git a/src/A.java b/src/A.java
new file mode 100644
--- /dev/null
+++ b/src/A.java
@@ -0,0 +1,5 @@
+class A {
+    int size() {
+        throw new UnsupportedOperationException(\"not implemented\");
+    }
+}
diff --git a/src/B.cs b/src/B.cs
new file mode 100644
--- /dev/null
+++ b/src/B.cs
@@ -0,0 +1,4 @@
+class B
+{
+    public int Size() => throw new NotImplementedException();
+}
";
    let files = parse_unified(diff).unwrap();
    let report = detect_stubs_report(&files, &|_| None);
    assert_eq!(report.parsed_files, 0, "{report:?}");
    for (path, line) in [("src/A.java", 3), ("src/B.cs", 3)] {
        let f = report
            .findings
            .iter()
            .find(|f| f.path == path)
            .unwrap_or_else(|| panic!("no finding in {path}: {report:?}"));
        assert_eq!(
            (f.gate, f.severity, f.strength, f.line),
            ("stub_detection", Severity::Blocking, Strength::Proven, line),
            "{path}"
        );
    }

    // A Rust file beside them is still counted: the count is per language,
    // not switched off.
    let rust = "diff --git a/src/x.rs b/src/x.rs
new file mode 100644
--- /dev/null
+++ b/src/x.rs
@@ -0,0 +1 @@
+pub fn a() -> u8 { 1 }
";
    let mut both = files;
    both.extend(parse_unified(rust).unwrap());
    assert_eq!(detect_stubs_report(&both, &|_| None).parsed_files, 1);
}

/// A Java method's node begins at its annotations. The finding is reported on
/// its name's line, where Rust and Python report on `fn` and `def`; the
/// annotation is still what an allow-stub marker above it must reach.
#[test]
fn a_java_empty_method_is_reported_on_its_name_and_covered_through_its_annotation() {
    let diff = "diff --git a/src/H.java b/src/H.java
new file mode 100644
--- /dev/null
+++ b/src/H.java
@@ -0,0 +1,11 @@
+class H {
+    @Override
+    public void onClose() {
+    }
+
+    // allow-stub: the retry policy is decided in TASK-31
+    @Override
+    public void retry() {
+        throw new UnsupportedOperationException();
+    }
+}
";
    let files = parse_unified(diff).unwrap();
    let findings = detect_stubs_with(&files, &|_| None);
    let empty = findings
        .iter()
        .find(|f| f.gate == "stub_detection")
        .expect("the empty method is reported");
    assert_eq!(
        (empty.line, empty.severity, empty.strength),
        (3, Severity::Advisory, Strength::Proven),
        "{findings:?}"
    );
    assert!(empty.evidence.contains("onClose"), "{}", empty.evidence);

    let allowed = findings
        .iter()
        .find(|f| f.gate == GATE_STUB_ALLOWED)
        .expect("the covered placeholder is recorded");
    assert_eq!(allowed.line, 9, "{findings:?}");
    assert!(allowed.message.contains("TASK-31"), "{}", allowed.message);
    assert_eq!(findings.len(), 2, "{findings:?}");
}

/// In the substring fallback a marker on one function must not reach code
/// that follows it at the same depth: the covering declaration has to be less
/// indented than the placeholder.
#[test]
fn a_fallback_marker_does_not_cover_code_after_its_function() {
    let diff = "diff --git a/src/y.rs b/src/y.rs
--- a/src/y.rs
+++ b/src/y.rs
@@ -1,0 +1,3 @@
+// allow-stub: a is waiting on the codec
+fn a() -> u8 { todo!() }
+const B: u8 = todo!();
";
    let files = parse_unified(diff).unwrap();
    let findings = detect_stubs_with(&files, &|_| None);
    let a = findings
        .iter()
        .find(|f| f.line == 2)
        .expect("a finding on line 2");
    assert_eq!(a.gate, GATE_STUB_ALLOWED, "{findings:?}");
    let b = findings
        .iter()
        .find(|f| f.line == 3)
        .expect("a finding on line 3");
    assert_eq!(
        (b.gate, b.severity),
        ("stub_detection", Severity::Blocking),
        "the marker on `a` covered the placeholder after it: {findings:?}"
    );
}

struct Root(PathBuf);

impl Root {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "dcv-stub-ast-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(base.join("repo/src")).unwrap();
        Self(base)
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn check(root: &std::path::Path, diff: &str) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_dcverify"))
        .args(["check", "--root"])
        .arg(root)
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
fn the_binary_reads_the_working_tree_under_root() {
    let root = Root::new();
    std::fs::write(root.0.join("repo/src/lib.rs"), POST_IMAGE).unwrap();
    let reply = check(&root.0.join("repo"), MODIFIED);
    assert!(reply.contains("\"ok\":true"), "{reply}");
    assert!(
        !reply.contains("\"severity\":\"blocking\""),
        "the binary did not parse the working tree: {reply}"
    );
}

#[test]
fn the_binary_does_not_follow_a_diff_path_out_of_root() {
    let root = Root::new();
    // A file outside the repository whose content would make the parsed
    // answer clean. If the reader followed `../`, the literal would pass.
    std::fs::write(root.0.join("outside.rs"), POST_IMAGE).unwrap();
    let escaping = MODIFIED.replace("src/lib.rs", "../outside.rs");
    let reply = check(&root.0.join("repo"), &escaping);
    if !reply.contains("\"ok\":true") {
        // The diff parser refusing the path outright is also containment.
        return;
    }
    assert!(
        reply.contains("\"severity\":\"blocking\""),
        "a path outside --root was read as the post-change source: {reply}"
    );
}
