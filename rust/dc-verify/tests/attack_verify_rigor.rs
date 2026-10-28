//! Attack tests for the verify-rigor lane (card ft-b9ef3ccf4f898d019658aa5ca4bbd11c).
//!
//! Each test states a contract the lane must hold and a minimal input that
//! breaks it. A failing test here is a finding; none is ignored.

use std::io::Write as _;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use dc_verify::parse_unified;
use dc_verify::rigor::{Severity, Strength, StubReport, detect_stubs_report};
use dc_verify::stub_ast::Unparsed;

fn key() -> String {
    concat!("AKIA", "IOSFODNN7EXAMPLE").to_string()
}

fn whole_file(path: &str, body: &str) -> String {
    let lines: Vec<&str> = body.lines().collect();
    let mut diff = format!(
        "diff --git a/{path} b/{path}\nnew file mode 100644\n--- /dev/null\n+++ b/{path}\n@@ -0,0 +1,{} @@\n",
        lines.len()
    );
    for line in lines {
        diff.push('+');
        diff.push_str(line);
        diff.push('\n');
    }
    diff
}

/// Blocking stub finding, or a degradation report. Both are acceptable
/// outcomes; "scanned clean" is not. The post-image is the body itself, the way
/// the CLI reads it from the working tree.
fn blocks_or_degrades(path: &str, body: &str) {
    let diff = whole_file(path, body);
    let files = parse_unified(&diff).expect("diff parses");
    let post = body.to_string();
    let report = detect_stubs_report(&files, &|_| Some(post.clone()));
    let finding = report
        .findings
        .iter()
        .any(|f| f.gate == "stub_detection" && f.severity == Severity::Blocking);
    assert!(
        finding || !report.unparsed.is_empty(),
        "a stub scanned clean: findings={:?} unparsed={:?}",
        report.findings,
        report.unparsed
    );
}

fn report_whole(path: &str, body: &str) -> StubReport {
    let diff = whole_file(path, body);
    let files = parse_unified(&diff).expect("diff parses");
    let post = body.to_string();
    detect_stubs_report(&files, &|_| Some(post.clone()))
}

/// A Blocking, Proven `stub_detection` finding on `line`, found by the tree:
/// the file was parsed, so nothing is reported as degraded.
fn blocks_proven(path: &str, body: &str, line: u32) {
    let report = report_whole(path, body);
    assert!(
        report.unparsed.is_empty(),
        "the file was reported unparsed: {:?}",
        report.unparsed
    );
    let hit = report.findings.iter().any(|f| {
        f.gate == "stub_detection"
            && f.severity == Severity::Blocking
            && f.strength == Strength::Proven
            && f.line == line
    });
    assert!(
        hit,
        "no Blocking, Proven stub finding on line {line} of {body:?}: {:?}",
        report.findings
    );
}

/// The file is neither blocked nor degraded: it was parsed and is clean.
fn clean_and_parsed(path: &str, body: &str) {
    let report = report_whole(path, body);
    assert!(
        report.unparsed.is_empty(),
        "a clean file was reported unparsed: {:?} for {body:?}",
        report.unparsed
    );
    assert!(
        !report
            .findings
            .iter()
            .any(|f| f.severity == Severity::Blocking),
        "a clean file was blocked: {:?} for {body:?}",
        report.findings
    );
}

/// Degraded with exactly the named reason, and no Blocking finding.
fn degrades_unresolved(path: &str, body: &str) {
    let report = report_whole(path, body);
    let reasons: Vec<&str> = report.unparsed.iter().map(|u| u.reason).collect();
    assert_eq!(
        reasons,
        vec![Unparsed::UnresolvedMacroImport.as_str()],
        "wrong degradation for {body:?}: findings={:?}",
        report.findings
    );
}

// ---------------------------------------------------------------- stubs

#[test]
fn attack_stub_through_a_use_alias_is_not_clean() {
    blocks_or_degrades(
        "src/lib.rs",
        "use std::todo as t;\npub fn f() -> u32 {\n    t!()\n}\n",
    );
}

#[test]
fn same_file_std_alias_is_detected_proven() {
    // (body, 1-based line of the finding)
    let forms: &[(&str, u32)] = &[
        ("use std::todo as t;\npub fn f() -> u32 {\n    t!()\n}\n", 3),
        (
            "use std::{fmt, todo as t};\npub fn f() -> u32 {\n    t!()\n}\n",
            3,
        ),
        (
            "use ::core::unimplemented as u;\npub fn f() -> u32 {\n    u!()\n}\n",
            3,
        ),
        (
            "use core::{unimplemented as u};\npub fn f() -> u32 {\n    u!()\n}\n",
            3,
        ),
        (
            "use ::core::{self, unimplemented as u, panic};\npub fn f() -> u32 {\n    u!()\n}\n",
            3,
        ),
        (
            "use alloc::todo as t;\npub fn f() -> u32 {\n    t!()\n}\n",
            3,
        ),
        (
            "pub use std::todo as t;\npub fn f() -> u32 {\n    t!()\n}\n",
            3,
        ),
        (
            "pub(crate) use std::todo as t;\npub fn f() -> u32 {\n    t!()\n}\n",
            3,
        ),
        (
            "use std::todo as r#t;\npub fn f() -> u32 {\n    t!()\n}\n",
            3,
        ),
        (
            "use r#std::r#todo as t;\npub fn f() -> u32 {\n    r#t!()\n}\n",
            3,
        ),
        (
            "use std::todo as t;\nuse self::t as u;\npub fn f() -> u32 {\n    u!()\n}\n",
            4,
        ),
        (
            "use std::todo as t;\nuse t as u;\npub fn f() -> u32 {\n    u!()\n}\n",
            4,
        ),
        (
            "use std::todo as t;\npub fn f() -> u32 {\n    self::t!()\n}\n",
            3,
        ),
        // A `use` inside a function body counts for the whole file.
        (
            "pub fn f() -> u32 {\n    use std::todo as t;\n    t!()\n}\n",
            3,
        ),
        (
            "mod m {\n    use std::todo as t;\n}\npub fn f() -> u32 {\n    t!()\n}\n",
            5,
        ),
        (
            "use std::panic as p;\npub fn f() -> u32 {\n    p!(\"TODO\")\n}\n",
            3,
        ),
        // The wrapper's own name is not a stub, its token tree holds the alias.
        (
            "use std::todo as t;\npub fn f() -> u32 {\n    wrap!(t!());\n    1\n}\n",
            3,
        ),
        (
            "use std::todo as t;\nmacro_rules! m {\n    () => { t!() };\n}\n",
            3,
        ),
    ];
    for (body, line) in forms {
        blocks_proven("src/lib.rs", body, *line);
    }
}

#[test]
fn raw_identifier_todo_is_detected() {
    blocks_proven("src/lib.rs", "pub fn f() -> u32 {\n    r#todo!()\n}\n", 2);
    blocks_proven(
        "src/lib.rs",
        "pub fn f() -> u32 {\n    ::core::r#unimplemented!()\n}\n",
        2,
    );
    blocks_proven(
        "src/lib.rs",
        "pub fn f() -> u32 {\n    std::r#panic!(\"TODO\")\n}\n",
        2,
    );
}

#[test]
fn alias_declared_on_an_unchanged_line_still_blocks_an_added_call() {
    let body = "use std::todo as t;\npub fn f() -> u32 {\n    t!()\n}\n";
    let diff = "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,3 +1,4 @@\n use std::todo as t;\n pub fn f() -> u32 {\n+    t!()\n }\n";
    let files = parse_unified(diff).expect("diff parses");
    let post = body.to_string();
    let report = detect_stubs_report(&files, &|_| Some(post.clone()));
    assert!(report.unparsed.is_empty(), "{:?}", report.unparsed);
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.gate == "stub_detection" && f.severity == Severity::Blocking && f.line == 3),
        "{:?}",
        report.findings
    );
}

#[test]
fn alias_used_only_on_unchanged_lines_is_not_reported() {
    // Only the `use` line is added; the call is somebody else's decision.
    let body = "use std::todo as t;\npub fn f() -> u32 {\n    t!()\n}\n";
    let diff = "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,3 +1,4 @@\n+use std::todo as t;\n pub fn f() -> u32 {\n     t!()\n }\n";
    let files = parse_unified(diff).expect("diff parses");
    let post = body.to_string();
    let report = detect_stubs_report(&files, &|_| Some(post.clone()));
    assert!(report.unparsed.is_empty(), "{:?}", report.unparsed);
    assert!(
        !report
            .findings
            .iter()
            .any(|f| f.severity == Severity::Blocking),
        "{:?}",
        report.findings
    );
}

#[test]
fn a_same_file_macro_rules_shadowing_the_alias_is_not_a_stub() {
    // `t` is this file's own macro, not std's `todo`.
    clean_and_parsed(
        "src/lib.rs",
        "use std::todo as t;\nmacro_rules! t {\n    () => { 1 };\n}\npub fn f() -> u32 {\n    t!()\n}\n",
    );
}

#[test]
fn crate_macro_import_degrades_with_named_reason() {
    let forms = [
        // (b) an import from a crate, super or self path.
        "use crate::macros::stub as t;\npub fn f() -> u32 {\n    t!()\n}\n",
        "use crate::macros::stub;\npub fn f() -> u32 {\n    stub!()\n}\n",
        "use super::stub;\npub fn f() -> u32 {\n    stub!()\n}\n",
        "use self::inner::{stub};\npub fn f() -> u32 {\n    stub!()\n}\n",
        // ... or from a module this file declares with `mod x;`.
        "mod macros;\nuse macros::stub;\npub fn f() -> u32 {\n    stub!()\n}\n",
        "mod macros;\npub fn f() -> u32 {\n    macros::stub!()\n}\n",
        // (c) an invocation qualified by crate, super or self.
        "pub fn f() -> u32 {\n    crate::macros::stub!()\n}\n",
        "pub fn f() -> u32 {\n    super::stub!()\n}\n",
        "pub fn f() -> u32 {\n    self::stub!()\n}\n",
    ];
    for body in forms {
        degrades_unresolved("src/lib.rs", body);
    }
}

#[test]
fn aliased_pub_use_reexport_degrades() {
    degrades_unresolved("src/lib.rs", "pub use std::todo as t;\n");
    degrades_unresolved("src/lib.rs", "pub(crate) use core::unimplemented as u;\n");
    degrades_unresolved("src/lib.rs", "pub use std::{fmt, todo as t};\n");
}

#[test]
fn imports_that_hide_nothing_stay_clean_and_not_degraded() {
    // A same-name re-export: `todo!()` is already caught by its own text.
    clean_and_parsed("src/lib.rs", "pub use std::todo;\n");
    // A private alias nobody calls.
    clean_and_parsed("src/lib.rs", "use std::todo as t;\n");
    // An external crate's macro, which is not another file of this tree.
    clean_and_parsed(
        "src/lib.rs",
        "use serde_json::json;\npub fn f() {\n    let _ = json!({});\n}\n",
    );
    // A macro this file defines is read by its body, however it is named.
    clean_and_parsed(
        "src/lib.rs",
        "macro_rules! stub {\n    () => { 1 };\n}\nuse crate::stub;\npub fn f() -> u32 {\n    stub!() + crate::stub!()\n}\n",
    );
    clean_and_parsed("src/lib.rs", "use std::*;\n");
    // The unrelated `mod x;` and a `crate::` path that names no macro call.
    clean_and_parsed(
        "src/lib.rs",
        "mod helpers;\nuse crate::helpers::Thing;\npub fn f() -> Thing {\n    Thing::new()\n}\n",
    );
}

#[test]
fn a_use_tree_nested_past_the_bound_degrades_instead_of_being_half_read() {
    let depth = 40;
    let body = format!(
        "use {}std::todo as t{};\npub fn f() -> u32 {{\n    t!()\n}}\n",
        "a::{".repeat(depth),
        "}".repeat(depth)
    );
    let report = report_whole("src/lib.rs", &body);
    let reasons: Vec<&str> = report.unparsed.iter().map(|u| u.reason).collect();
    assert_eq!(
        reasons,
        vec![Unparsed::UnresolvedMacroImport.as_str()],
        "{:?}",
        report.findings
    );
    assert!(
        !report
            .findings
            .iter()
            .any(|f| f.severity == Severity::Blocking),
        "{:?}",
        report.findings
    );
}

#[test]
fn a_blocking_stub_wins_over_a_degraded_import() {
    // The file is blocked on what the tree proved; it is not also degraded.
    let report = report_whole(
        "src/lib.rs",
        "use crate::macros::stub;\npub fn f() -> u32 {\n    stub!();\n    todo!()\n}\n",
    );
    assert!(report.unparsed.is_empty(), "{:?}", report.unparsed);
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.gate == "stub_detection" && f.severity == Severity::Blocking),
        "{:?}",
        report.findings
    );
}

#[test]
fn attack_stub_inside_a_wrapper_with_a_space_before_the_bang_is_not_clean() {
    // `todo !()` is the same macro; the wrapper's token tree is only read as text.
    blocks_or_degrades(
        "src/lib.rs",
        "pub fn f() -> u32 {\n    wrap!(todo !());\n    1\n}\n",
    );
}

#[test]
fn attack_stub_after_a_block_comment_holding_a_quote_is_not_clean() {
    // The line lexer has no block-comment state: the quote opens a string that
    // hides `unimplemented!()`.
    blocks_or_degrades(
        "src/lib.rs",
        "macro_rules! m {\n    () => { /* \" */ unimplemented!() };\n}\npub fn f() { m!(); }\n",
    );
}

#[test]
fn attack_stub_after_a_block_comment_holding_a_quote_in_a_wrapper_call_is_not_clean() {
    blocks_or_degrades(
        "src/lib.rs",
        "pub fn f() -> u32 {\n    wrap!(/* \" */ todo!());\n    1\n}\n",
    );
}

#[test]
fn attack_stub_in_a_nested_macro_rules_is_not_clean() {
    blocks_or_degrades(
        "src/lib.rs",
        "macro_rules! outer {\n    () => {\n        macro_rules! inner {\n            () => { unimplemented!() };\n        }\n    };\n}\n",
    );
}

#[test]
fn attack_stub_after_a_raw_string_with_hashes_is_not_clean() {
    blocks_or_degrades(
        "src/lib.rs",
        "macro_rules! m {\n    () => { r##\"a \"# b\"## ; unimplemented!() };\n}\n",
    );
}

#[test]
fn attack_stub_after_lifetimes_and_char_quotes_is_not_clean() {
    blocks_or_degrades(
        "src/lib.rs",
        "macro_rules! m {\n    ($x:expr) => { { let _c: &'static str = \"\"; let _q = '\"'; let _e = '\\''; unimplemented!() } };\n}\n",
    );
}

#[test]
fn attack_panic_with_format_args_saying_not_implemented_is_not_clean() {
    blocks_or_degrades(
        "src/lib.rs",
        "pub fn f(name: &str) {\n    panic!(\"not implemented: {}\", name);\n}\n",
    );
}

#[test]
fn attack_panic_capitalised_placeholder_is_not_clean() {
    blocks_or_degrades(
        "src/lib.rs",
        "pub fn f() {\n    panic!(\"Not Implemented\");\n}\n",
    );
}

#[test]
fn attack_core_path_todo_is_not_clean() {
    blocks_or_degrades(
        "src/lib.rs",
        "pub fn f() -> u32 {\n    ::core::todo!()\n}\n",
    );
}

#[test]
fn attack_unimplemented_in_cfg_test_helper_module_is_reported_or_degraded() {
    // A test-only helper that is itself a stub makes every test that calls it
    // vacuous. Whatever the lane intends for #[cfg(test)], a silent clean scan
    // of an added `todo!()` in a non-#[test] fn is the weakest reading.
    blocks_or_degrades(
        "src/lib.rs",
        "#[cfg(test)]\nmod t {\n    pub fn helper() -> u32 {\n        todo!()\n    }\n}\n",
    );
}

// ------------------------------------------------- CLI, end to end

struct Root(PathBuf);

impl Root {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "dcv-attack-{}-{}",
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

struct Run {
    stdout: String,
    stderr: String,
}

impl Run {
    fn all(&self) -> String {
        format!("{}\n{}", self.stdout, self.stderr)
    }
}

fn check(root: &std::path::Path, planned: &str, diff: &[u8]) -> Run {
    let mut child = Command::new(env!("CARGO_BIN_EXE_dcverify"))
        .args(["check", "--root"])
        .arg(root)
        .args(["--planned", planned])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let body = diff.to_vec();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&body);
    });
    let out = child.wait_with_output().unwrap();
    let _ = writer.join();
    Run {
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

fn degraded_nonempty(reply: &str) -> bool {
    reply.contains("stub_ast_degraded\":[") && !reply.contains("stub_ast_degraded\":[]")
}

#[test]
fn attack_malformed_diff_error_does_not_quote_an_added_secret_line() {
    // One added line more than the hunk declares. The parse error used to carry
    // the raw line.
    let root = Root::new();
    let k = key();
    let diff = format!(
        "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -0,0 +1,1 @@\n+fn a() {{}}\n+let aws_key = \"{k}\";\n"
    );
    let run = check(&root.0, "src/a.rs", diff.as_bytes());
    assert!(
        !run.all().contains(&k),
        "a diff parse error quotes the credential: {}",
        run.all()
    );
}

#[test]
fn attack_malformed_hunk_header_error_does_not_quote_a_secret_line() {
    let root = Root::new();
    let k = key();
    let diff = format!(
        "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -0,0 +1,1 @@\n+fn a() {{}}\n@@ nonsense {k} @@\n"
    );
    let run = check(&root.0, "src/a.rs", diff.as_bytes());
    assert!(
        !run.all().contains(&k),
        "a diff parse error quotes the credential: {}",
        run.all()
    );
}

#[test]
fn attack_unreadable_header_error_does_not_quote_a_secret_path() {
    // A header naming a path that carries a credential-shaped token. The error
    // for an unreadable header echoes the raw line.
    let root = Root::new();
    let k = key();
    let diff = format!("diff --git {k}\n");
    let run = check(&root.0, "src/a.rs", diff.as_bytes());
    assert!(
        !run.all().contains(&k),
        "a header parse error quotes the credential: {}",
        run.all()
    );
}

#[test]
fn attack_binary_marked_source_file_is_stub_scanned_or_degraded() {
    // `*.rs -diff` in .gitattributes makes git print `Binary files differ` for
    // a text file. The secret scan reads it back; the stub scan does not.
    let root = Root::new();
    std::fs::write(
        root.0.join("src/work.rs"),
        "pub fn work() -> u32 {\n    todo!()\n}\n",
    )
    .unwrap();
    let diff = "diff --git a/src/work.rs b/src/work.rs\nindex 111..222 100644\nBinary files a/src/work.rs and b/src/work.rs differ\n";
    let run = check(&root.0, "src/work.rs", diff.as_bytes());
    assert!(
        run.stdout.contains("\"ok\":true"),
        "did not run: {}",
        run.all()
    );
    let seen = run.stdout.contains("\"gate\":\"stub_detection\"") || degraded_nonempty(&run.stdout);
    assert!(
        seen,
        "a planned text file hidden behind `-diff` carried todo!() and scanned clean: {}",
        run.stdout
    );
}

#[test]
fn attack_binary_renamed_with_edits_is_scanned_or_degraded() {
    let root = Root::new();
    let k = key();
    std::fs::write(root.0.join("new b/blob.bin"), "").ok();
    std::fs::create_dir_all(root.0.join("new b")).unwrap();
    std::fs::write(root.0.join("new b/blob.bin"), format!("\0{k}\0")).unwrap();
    let diff = "diff --git a/old.bin b/new b/blob.bin\nsimilarity index 80%\nrename from old.bin\nrename to new b/blob.bin\nBinary files a/old.bin and b/new b/blob.bin differ\n";
    let run = check(&root.0, "new b/blob.bin", diff.as_bytes());
    assert!(
        run.stdout.contains("\"ok\":true"),
        "did not run: {}",
        run.all()
    );
    let seen = run.stdout.contains("\"gate\":\"secret_scan\"") || degraded_nonempty(&run.stdout);
    assert!(
        seen,
        "renamed binary with a credential scanned clean: {}",
        run.stdout
    );
    assert!(!run.all().contains(&k), "reply quotes the credential");
}

#[test]
fn attack_binary_in_a_symlinked_parent_directory_is_not_read() {
    let root = Root::new();
    let outside = Root::new();
    std::fs::write(outside.0.join("blob.bin"), "OUTSIDE-SENTINEL-BYTES").unwrap();
    std::os::unix::fs::symlink(&outside.0, root.0.join("d")).unwrap();
    let diff =
        "diff --git a/d/blob.bin b/d/blob.bin\nBinary files a/d/blob.bin and b/d/blob.bin differ\n";
    let run = check(&root.0, "d/blob.bin", diff.as_bytes());
    assert!(
        !run.all().contains("OUTSIDE-SENTINEL-BYTES"),
        "the reply carries bytes read through a symlinked parent"
    );
    assert!(
        degraded_nonempty(&run.stdout),
        "a binary path behind a symlinked parent was neither read nor degraded: {}",
        run.stdout
    );
}

#[test]
fn attack_binary_dotdot_path_is_degraded_not_read() {
    let root = Root::new();
    let parent = root.0.parent().unwrap().to_path_buf();
    let marker = parent.join(format!("dcv-attack-escape-{}.bin", std::process::id()));
    std::fs::write(&marker, format!("\0{}\0", key())).unwrap();
    let rel = format!("../{}", marker.file_name().unwrap().to_string_lossy());
    let diff = format!("diff --git a/{rel} b/{rel}\nBinary files a/{rel} and b/{rel} differ\n");
    let run = check(&root.0, &rel, diff.as_bytes());
    let _ = std::fs::remove_file(&marker);
    assert!(
        !run.stdout.contains("\"gate\":\"secret_scan\""),
        "a path with `..` was read outside the root: {}",
        run.stdout
    );
}

#[test]
fn attack_stub_scan_reads_nothing_through_a_symlinked_parent() {
    // A symlinked parent directory must not feed the AST a file from outside
    // the root; the line checks run instead and the file is reported degraded.
    let root = Root::new();
    let outside = Root::new();
    std::fs::write(
        outside.0.join("x.rs"),
        "pub fn clean() -> u32 {\n    1\n}\npub fn also() -> u32 {\n    2\n}\n",
    )
    .unwrap();
    std::os::unix::fs::symlink(&outside.0, root.0.join("d")).unwrap();
    let diff = whole_file("d/x.rs", "pub fn clean() -> u32 {\n    todo!()\n}\n");
    let run = check(&root.0, "d/x.rs", diff.as_bytes());
    assert!(
        run.stdout.contains("\"ok\":true"),
        "did not run: {}",
        run.all()
    );
    assert!(
        run.stdout.contains("\"gate\":\"stub_detection\""),
        "the added todo!() was not found: {}",
        run.stdout
    );
}

#[test]
fn attack_stdin_one_byte_over_the_cap_is_refused_without_quoting_a_secret() {
    let root = Root::new();
    let k = key();
    let mut diff = format!(
        "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -0,0 +1,1 @@\n+let aws_key = \"{k}\";\n"
    )
    .into_bytes();
    diff.resize((32 << 20) + 1, b'x');
    let run = check(&root.0, "src/a.rs", &diff);
    assert!(
        !run.all().contains(&k),
        "the over-cap refusal quotes the credential"
    );
    assert!(
        !run.stdout.contains("\"ok\":true") || run.stdout.contains("exceeds"),
        "an over-cap diff was accepted: {}",
        run.stdout.chars().take(200).collect::<String>()
    );
}

#[test]
fn attack_stub_finding_on_a_line_that_also_holds_a_secret_withholds_the_value() {
    let root = Root::new();
    let k = key();
    let body = format!("pub fn f() -> &'static str {{\n    todo!(\"use {k}\")\n}}\n");
    std::fs::write(root.0.join("src/a.rs"), &body).unwrap();
    let diff = whole_file("src/a.rs", &body);
    let run = check(&root.0, "src/a.rs", diff.as_bytes());
    assert!(
        !run.all().contains(&k),
        "a stub finding quotes the credential: {}",
        run.all()
    );
}

// ======================================================================
// Round 2 (Sonnet adversarial pass on lane/verify-residue @ 6c270782).
// Each test is one attack; a failure is a finding.
// ======================================================================

/// Like `whole_file`, but an added line keeps a `\r` when the body has one.
fn whole_file_raw(path: &str, body: &str) -> String {
    let lines: Vec<&str> = body.split_terminator('\n').collect();
    let mut diff = format!(
        "diff --git a/{path} b/{path}\nnew file mode 100644\n--- /dev/null\n+++ b/{path}\n@@ -0,0 +1,{} @@\n",
        lines.len()
    );
    for line in lines {
        diff.push('+');
        diff.push_str(line);
        diff.push('\n');
    }
    diff
}

fn report_raw(path: &str, body: &str) -> StubReport {
    let diff = whole_file_raw(path, body);
    let files = parse_unified(&diff).expect("diff parses");
    let post = body.to_string();
    detect_stubs_report(&files, &|_| Some(post.clone()))
}

fn blocked(report: &StubReport) -> bool {
    report
        .findings
        .iter()
        .any(|f| f.gate == "stub_detection" && f.severity == Severity::Blocking)
}

// ---- item 1: evasion that must still block or degrade ----

#[test]
fn adv2_nested_tree_self_alias_then_qualified_call_blocks() {
    // `s::todo!()` is caught by its last segment.
    blocks_or_degrades(
        "src/lib.rs",
        "use std::{self as s};\npub fn f() -> u32 {\n    s::todo!()\n}\n",
    );
    blocks_or_degrades(
        "src/lib.rs",
        "extern crate std as s;\npub fn f() -> u32 {\n    s::todo!()\n}\n",
    );
    blocks_or_degrades(
        "src/lib.rs",
        "use core::panic as p;\npub fn f() -> u32 {\n    p!(\"not implemented\")\n}\n",
    );
}

#[test]
fn adv2_std_as_s_then_use_s_todo_as_t_is_not_clean() {
    blocks_or_degrades(
        "src/lib.rs",
        "use std as s;\nuse s::todo as t;\npub fn f() -> u32 {\n    t!()\n}\n",
    );
}

#[test]
fn adv2_self_as_s_then_use_s_todo_as_t_is_not_clean() {
    blocks_or_degrades(
        "src/lib.rs",
        "use std::{self as s};\nuse s::todo as t;\npub fn f() -> u32 {\n    t!()\n}\n",
    );
}

#[test]
fn adv2_extern_crate_std_as_s_then_use_s_todo_as_t_is_not_clean() {
    blocks_or_degrades(
        "src/lib.rs",
        "extern crate std as s;\nuse s::todo as t;\npub fn f() -> u32 {\n    t!()\n}\n",
    );
}

#[test]
fn adv2_extern_crate_core_as_c_then_use_c_unimplemented_as_u_is_not_clean() {
    blocks_or_degrades(
        "src/lib.rs",
        "extern crate core as c;\nuse c::unimplemented as u;\npub fn f() -> u32 {\n    u!()\n}\n",
    );
}

#[test]
fn adv2_prelude_path_alias_blocks() {
    blocks_proven(
        "src/lib.rs",
        "use std::prelude::rust_2021::todo as t;\npub fn f() -> u32 {\n    t!()\n}\n",
        3,
    );
}

#[test]
fn adv2_alias_defined_after_use_blocks() {
    blocks_proven(
        "src/lib.rs",
        "pub fn f() -> u32 {\n    t!()\n}\nuse std::todo as t;\n",
        2,
    );
}

#[test]
fn adv2_braced_leading_use_list_alias_blocks() {
    blocks_proven(
        "src/lib.rs",
        "use {std::todo as t};\npub fn f() -> u32 {\n    t!()\n}\n",
        3,
    );
}

#[test]
fn adv2_alias_in_fn_scope_used_in_other_fn_blocks() {
    blocks_proven(
        "src/lib.rs",
        "fn a() {\n    use std::todo as t;\n}\nfn b() -> u32 {\n    t!()\n}\n",
        5,
    );
}

#[test]
fn adv2_raw_identifier_alias_forms_block() {
    blocks_proven(
        "src/lib.rs",
        "use std::todo as r#t;\npub fn f() -> u32 {\n    r#t!()\n}\n",
        3,
    );
    blocks_proven(
        "src/lib.rs",
        "use std::r#todo as r#match;\npub fn f() -> u32 {\n    r#match!()\n}\n",
        3,
    );
    blocks_proven(
        "src/lib.rs",
        "use std::todo as t;\npub fn f() -> u32 {\n    r#t!()\n}\n",
        3,
    );
}

#[test]
fn adv2_unicode_alias_blocks_directly_and_inside_a_wrapper() {
    blocks_proven(
        "src/lib.rs",
        "use std::todo as 世界;\npub fn f() -> u32 {\n    世界!()\n}\n",
        3,
    );
    blocks_proven(
        "src/lib.rs",
        "use std::todo as 世界;\npub fn f() -> u32 {\n    wrap!(世界!());\n    1\n}\n",
        3,
    );
    blocks_proven(
        "src/lib.rs",
        "use std::todo as é;\npub fn f() -> u32 {\n    wrap!(é!());\n    1\n}\n",
        3,
    );
}

#[test]
fn adv2_uppercase_alias_blocks_directly_and_in_a_wrapper() {
    blocks_proven(
        "src/lib.rs",
        "use std::todo as T;\npub fn f() -> u32 {\n    T!()\n}\n",
        3,
    );
    blocks_proven(
        "src/lib.rs",
        "use std::todo as T;\npub fn f() -> u32 {\n    wrap!(T!());\n    1\n}\n",
        3,
    );
}

#[test]
fn adv2_crlf_alias_blocks() {
    let report = report_raw(
        "src/lib.rs",
        "use std::todo as t;\r\npub fn f() -> u32 {\r\n    t!()\r\n}\r\n",
    );
    assert!(report.unparsed.is_empty(), "{:?}", report.unparsed);
    assert!(blocked(&report), "{:?}", report.findings);
}

#[test]
fn adv2_same_file_wrapper_macro_with_added_definition_blocks() {
    blocks_or_degrades(
        "src/lib.rs",
        "macro_rules! t {\n    () => { todo!() };\n}\npub fn f() -> u32 {\n    t!()\n}\n",
    );
    blocks_or_degrades(
        "src/lib.rs",
        "#[macro_export]\nmacro_rules! t {\n    () => { todo!() };\n}\n",
    );
}

#[test]
#[ignore = "pre-existing on main: a call of an old, unchanged stub wrapper is not a new stub; main scans it clean too"]
fn adv2_wrapper_defined_on_unchanged_lines_called_on_added_line() {
    // The wrapper is old; the added line only calls it. Not a new stub, but a
    // scan that says "clean" for a call to a stub is worth knowing about.
    let body = "macro_rules! t {\n    () => { todo!() };\n}\npub fn f() -> u32 {\n    t!()\n}\n";
    let diff = "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,5 +1,6 @@\n macro_rules! t {\n     () => { todo!() };\n }\n pub fn f() -> u32 {\n+    t!()\n }\n";
    let files = parse_unified(diff).expect("diff parses");
    let post = body.to_string();
    let report = detect_stubs_report(&files, &|_| Some(post.clone()));
    assert!(
        blocked(&report) || !report.unparsed.is_empty(),
        "an added call of an old stub wrapper scanned clean: {:?}",
        report.findings
    );
}

#[test]
fn adv2_macro_named_like_the_alias_in_a_test_module_does_not_hide_the_alias() {
    // macro_rules `t` lives in `mod tests` (textual scope ends with the module);
    // the top-level `t!()` is std's todo!.
    blocks_or_degrades(
        "src/lib.rs",
        "use std::todo as t;\npub fn f() -> u32 {\n    t!()\n}\n#[cfg(test)]\nmod tests {\n    macro_rules! t {\n        () => { 1 };\n    }\n}\n",
    );
}

#[test]
fn adv2_alias_of_panic_named_todo_is_still_a_stub() {
    // `use std::panic as todo;` makes `todo!()` a plain panic!(). Not a TODO
    // marker, but the old name-based check flagged this `todo!()` on main.
    blocks_or_degrades(
        "src/lib.rs",
        "use std::panic as todo;\npub fn f() -> u32 {\n    todo!()\n}\n",
    );
}

#[test]
fn adv2_syntax_error_with_alias_degrades() {
    let report = report_whole(
        "src/lib.rs",
        "use std::todo as t;\npub fn f( -> u32 {\n    t!()\n",
    );
    assert!(
        !report.unparsed.is_empty() || blocked(&report),
        "{:?}",
        report.findings
    );
}

#[test]
fn adv2_oversize_rust_source_with_alias_degrades() {
    let mut body = String::from("use std::todo as t;\npub fn f() -> u32 {\n    t!()\n}\n");
    while body.len() <= dc_verify::stub_ast::MAX_SOURCE_BYTES {
        body.push_str("// padding padding padding padding padding padding\n");
    }
    let report = report_whole("src/lib.rs", &body);
    assert!(
        !report.unparsed.is_empty() || blocked(&report),
        "an oversize source scanned clean: unparsed={:?}",
        report.unparsed
    );
}

fn degrades_or_blocks(body: &str) {
    let report = report_whole("src/lib.rs", body);
    assert!(
        blocked(&report) || !report.unparsed.is_empty(),
        "a call of an unresolved macro scanned clean: {body:?}"
    );
}

// The call sites sit inside another macro's token tree, which tree-sitter does
// not parse into macro_invocation nodes, so the walker never sees them.
#[test]
fn adv2_imported_macro_inside_dbg_args_degrades() {
    degrades_or_blocks("use crate::macros::stub;\npub fn f() -> u32 {\n    dbg!(stub!())\n}\n");
}

#[test]
fn adv2_imported_macro_inside_vec_args_degrades() {
    degrades_or_blocks(
        "use crate::macros::stub;\npub fn f() -> Vec<u32> {\n    vec![stub!()]\n}\n",
    );
}

#[test]
fn adv2_crate_qualified_macro_inside_dbg_args_degrades() {
    degrades_or_blocks("pub fn f() -> u32 {\n    dbg!(crate::macros::stub!())\n}\n");
}

#[test]
fn adv2_mod_qualified_macro_inside_dbg_args_degrades() {
    degrades_or_blocks("mod macros;\npub fn f() -> u32 {\n    dbg!(macros::stub!())\n}\n");
}

#[test]
fn adv2_imported_macro_inside_format_args_in_let_degrades() {
    degrades_or_blocks(
        "use crate::macros::stub;\npub fn f() -> String {\n    let s = format!(\"{}\", stub!());\n    s\n}\n",
    );
}

#[test]
fn adv2_pub_use_self_alias_of_std_todo_degrades() {
    // `pub use std::todo;` is same-name (exempt), the second line re-exports it
    // under a new name through `self::`.
    let report = report_whole(
        "src/lib.rs",
        "pub use std::todo;\npub use self::todo as t;\n",
    );
    assert!(
        !report.unparsed.is_empty(),
        "an aliased pub re-export through self:: scanned clean: {:?}",
        report.findings
    );
}

#[test]
fn adv2_pub_use_alias_of_alias_degrades() {
    let report = report_whole("src/lib.rs", "use std::todo as t;\npub use t as u;\n");
    assert!(!report.unparsed.is_empty(), "{:?}", report.findings);
}

#[test]
fn adv2_pub_use_crate_path_alias_of_alias_degrades() {
    // an alias re-exported through `crate::`
    let report = report_whole(
        "src/lib.rs",
        "use std::todo as t;\npub use crate::t as u;\n",
    );
    assert!(!report.unparsed.is_empty(), "{:?}", report.findings);
}

#[test]
#[ignore = "named limit: brief 3.4(a) scopes aliased pub-use degradation to todo/unimplemented; a pub alias of panic lets another file call p!(\"TODO\") unseen"]
fn adv2_pub_use_panic_alias_reexport_documents_gap() {
    // panic! with a TODO message is a stub too; only todo/unimplemented degrade
    // on a pub re-export.
    let report = report_whole("src/lib.rs", "pub use std::panic as p;\n");
    assert!(
        !report.unparsed.is_empty(),
        "an aliased pub re-export of std panic scanned clean"
    );
}

#[test]
#[ignore = "pre-existing on main: `use m::*` brings a crate macro in with no name to resolve; the lane names glob imports as a limit"]
fn adv2_glob_import_of_crate_macro_is_not_clean() {
    for body in [
        "use super::*;\npub fn f() -> u32 {\n    stub!()\n}\n",
        "use self::macros::*;\npub fn f() -> u32 {\n    stub!()\n}\n",
        "use crate::macros::*;\npub fn f() -> u32 {\n    stub!()\n}\n",
    ] {
        let report = report_whole("src/lib.rs", body);
        assert!(
            blocked(&report) || !report.unparsed.is_empty(),
            "a macro called through a glob import scanned clean: {body:?}"
        );
    }
}

#[test]
#[ignore = "pre-existing on main: #[macro_use] extern crate is a named limit of the lane"]
fn adv2_macro_use_extern_crate_is_not_clean() {
    let report = report_whole(
        "src/lib.rs",
        "#[macro_use]\nextern crate helpers;\npub fn f() -> u32 {\n    stub!()\n}\n",
    );
    assert!(
        blocked(&report) || !report.unparsed.is_empty(),
        "#[macro_use] crate macro scanned clean"
    );
}

// ---- item 1: false positives and noise ----

#[test]
fn adv2_non_std_alias_named_like_a_stub_is_clean_when_not_called_as_stub() {
    clean_and_parsed(
        "src/lib.rs",
        "use log::warn as w;\npub fn f() {\n    w!(\"x\");\n}\n",
    );
    clean_and_parsed(
        "src/lib.rs",
        "use log::warn as t;\npub fn f() {\n    t!(\"x\");\n}\n",
    );
    clean_and_parsed("src/lib.rs", "fn todo() {}\npub fn f() {\n    todo();\n}\n");
    clean_and_parsed(
        "src/lib.rs",
        "use std::fmt::Debug as t;\npub fn f<T: t>() {}\n",
    );
    clean_and_parsed("src/lib.rs", "use std::todo as _;\n");
}

#[test]
#[ignore = "pre-existing on main: `todo!` is matched by name, so a non-std macro imported as `todo` is flagged; main flags it too"]
fn adv2_non_std_alias_named_todo_is_not_flagged() {
    // `use log::warn as todo; todo!("x")` is a different macro that happens to
    // be called todo. It is flagged by NAME on main too (pre-existing FP).
    clean_and_parsed(
        "src/lib.rs",
        "use log::warn as todo;\npub fn f() {\n    todo!(\"x\");\n}\n",
    );
}

#[test]
fn adv2_std_panic_catch_unwind_imports_are_clean() {
    clean_and_parsed(
        "src/lib.rs",
        "use std::panic::{self, catch_unwind};\npub fn f() {\n    let _ = catch_unwind(|| 1);\n    panic::set_hook(Box::new(|_| {}));\n}\n",
    );
    clean_and_parsed(
        "src/lib.rs",
        "use std::panic as p;\npub fn f() {\n    let _ = p::catch_unwind(|| 1);\n}\n",
    );
    clean_and_parsed(
        "src/lib.rs",
        "use std::panic as p;\npub fn f(x: u32) {\n    p!(\"bad value {}\", x);\n}\n",
    );
}

#[test]
fn adv2_glob_pub_use_does_not_degrade_unrelated_edits() {
    for body in [
        "pub use foo::*;\npub fn f() -> u32 {\n    1\n}\n",
        "pub use self::inner::*;\npub use crate::a::b::*;\npub fn f() -> u32 {\n    1\n}\n",
        "pub use std::{fmt, todo};\npub fn f() -> u32 {\n    1\n}\n",
        "pub use super::t;\npub fn f() -> u32 {\n    1\n}\n",
        "mod inner;\npub use inner::Thing;\npub fn f() -> u32 {\n    1\n}\n",
    ] {
        clean_and_parsed("src/lib.rs", body);
    }
}

#[test]
fn adv2_crate_import_of_a_non_macro_does_not_degrade_ordinary_macros() {
    // A crate-local import must not degrade std macros that share no name.
    clean_and_parsed(
        "src/lib.rs",
        "use crate::error::{Error, Result};\nuse super::helper;\npub fn f(x: u32) -> Result<String> {\n    let s = format!(\"{x}\");\n    assert!(!s.is_empty());\n    println!(\"{}\", helper(x));\n    Ok(vec![s].concat())\n}\n",
    );
}

#[test]
#[ignore = "L1, follow-up of ft-926e5b45: the import match is namespace-blind, so a function imported from crate::util under the name of a called std macro degrades the file"]
fn adv2_crate_import_function_named_like_a_called_macro_does_not_degrade() {
    // `use crate::util::format;` is a function; the `format!` is std's. Same
    // namespace-blind match used by (b): this is the noise cost.
    clean_and_parsed(
        "src/lib.rs",
        "use crate::util::vec;\npub fn f() -> Vec<u32> {\n    let a = vec(1);\n    vec![a]\n}\n",
    );
}

// ---- item 1: cost bounds ----

fn timed_report(body: &str) -> (std::time::Duration, StubReport) {
    let start = std::time::Instant::now();
    let report = report_whole("src/lib.rs", body);
    (start.elapsed(), report)
}

#[test]
fn adv2_forward_alias_chain_is_bounded_in_time() {
    // `use t{n} as t{n+1};` written in order is visited by the DFS in reverse, so
    // one name resolves per round of the pre-pass chain loop: records x records.
    // PARSE_BUDGET covers only the tree-sitter parse, not this pre-pass or the
    // walk. n=8000 (158 KB) measured 13.5 s in debug against the 2 s budget.
    let mut worst = std::time::Duration::ZERO;
    for n in [2000usize, 8000] {
        let mut body = String::new();
        for i in 1..n {
            body.push_str(&format!("use t{} as t{};\n", i - 1, i));
        }
        body.push_str("use std::todo as t0;\npub fn f() -> u32 {\n    t0!()\n}\n");
        let (took, _report) = timed_report(&body);
        eprintln!("reverse alias chain n={n}: {took:?} ({} bytes)", body.len());
        worst = worst.max(took);
    }
    assert!(
        worst < std::time::Duration::from_secs(2),
        "alias chain pre-pass took {worst:?}"
    );
}

#[test]
fn adv2_many_aliases_times_many_invocations_is_bounded_in_time() {
    // Every added macro invocation that is not itself a stub rebuilds the alias
    // list and scans its text for each alias name.
    let mut worst = std::time::Duration::ZERO;
    for (aliases, calls) in [(500usize, 2000usize), (1000, 4000), (2000, 8000)] {
        let mut body = String::new();
        for i in 0..aliases {
            body.push_str(&format!("use std::todo as a{i};\n"));
        }
        body.push_str("pub fn f() {\n");
        for _ in 0..calls {
            body.push_str("    w!(x);\n");
        }
        body.push_str("}\n");
        let (took, _report) = timed_report(&body);
        eprintln!(
            "aliases={aliases} calls={calls}: {took:?} ({} bytes)",
            body.len()
        );
        worst = worst.max(took);
    }
    assert!(
        worst < std::time::Duration::from_secs(2),
        "alias x invocation scan took {worst:?}"
    );
}

#[test]
fn adv2_oversize_rust_source_through_the_binary_is_degraded_not_clean() {
    let root = Root::new();
    let mut body = String::from("use std::todo as t;\npub fn f() -> u32 {\n    t!()\n}\n");
    while body.len() <= dc_verify::stub_ast::MAX_SOURCE_BYTES {
        body.push_str("// padding padding padding padding padding padding\n");
    }
    std::fs::write(root.0.join("src/a.rs"), &body).unwrap();
    let diff = whole_file("src/a.rs", &body);
    let run = check(&root.0, "src/a.rs", diff.as_bytes());
    assert!(
        degraded_nonempty(&run.stdout) || run.stdout.contains("\"stub_detection\""),
        "an oversize source was neither degraded nor blocked: {}",
        run.stdout
            .chars()
            .filter(|_| true)
            .collect::<String>()
            .split("stub_ast_degraded")
            .nth(1)
            .unwrap_or("NO-DEGRADED-KEY")
            .chars()
            .take(300)
            .collect::<String>()
    );
}

// ======================================================================
// Round 2 fixes (lane/verify-residue): the reasons a degraded file names.
// ======================================================================

fn reasons_of(body: &str) -> Vec<&'static str> {
    report_whole("src/lib.rs", body)
        .unparsed
        .iter()
        .map(|u| u.reason)
        .collect()
}

#[test]
fn a_stub_macro_alias_chain_through_a_renamed_crate_root_blocks() {
    for (body, line) in [
        (
            "use std as s;\nuse s::todo as t;\npub fn f() -> u32 {\n    t!()\n}\n",
            4,
        ),
        (
            "use std::{self as s};\nuse s::todo as t;\npub fn f() -> u32 {\n    t!()\n}\n",
            4,
        ),
        (
            "extern crate std as s;\nuse s::todo as t;\npub fn f() -> u32 {\n    t!()\n}\n",
            4,
        ),
        (
            "extern crate core as c;\nuse c::unimplemented as u;\npub fn f() -> u32 {\n    u!()\n}\n",
            4,
        ),
        (
            "use std as s;\nuse s as r;\nuse r::todo as t;\npub fn f() -> u32 {\n    t!()\n}\n",
            5,
        ),
        (
            "pub fn f() -> u32 {\n    t!()\n}\nuse s::todo as t;\nuse std as s;\n",
            2,
        ),
    ] {
        blocks_proven("src/lib.rs", body, line);
    }
}

#[test]
fn a_macro_rules_in_an_inner_module_hides_no_top_level_alias() {
    blocks_proven(
        "src/lib.rs",
        "use std::todo as t;\npub fn f() -> u32 {\n    t!()\n}\nmod tests {\n    macro_rules! t {\n        () => { 1 };\n    }\n}\n",
        3,
    );
    // One in a function body hides the alias only inside that body, after it.
    blocks_proven(
        "src/lib.rs",
        "use std::todo as t;\nfn a() -> u32 {\n    macro_rules! t {\n        () => { 1 };\n    }\n    t!()\n}\nfn b() -> u32 {\n    t!()\n}\n",
        9,
    );
    // One at the top of the same module still does.
    clean_and_parsed(
        "src/lib.rs",
        "use std::todo as t;\nmacro_rules! t {\n    () => { 1 };\n}\npub fn f() -> u32 {\n    t!()\n}\n",
    );
}

#[test]
fn a_re_export_chain_of_a_stub_alias_degrades_with_the_named_reason() {
    for body in [
        "pub use std::todo;\npub use self::todo as t;\n",
        "use std::todo as t;\npub use crate::t as u;\n",
        "use std::todo as t;\npub use super::t as u;\n",
        "use std as s;\npub use s::todo as t;\n",
        "extern crate std as s;\npub use s::unimplemented as u;\n",
    ] {
        assert_eq!(
            reasons_of(body),
            vec![Unparsed::UnresolvedMacroImport.as_str()],
            "{body:?}"
        );
    }
}

#[test]
fn an_imported_or_qualified_macro_in_a_token_tree_degrades_with_the_named_reason() {
    assert_eq!(Unparsed::AliasInTokenTree.as_str(), "alias-in-token-tree");
    for body in [
        "use crate::macros::stub;\npub fn f() -> u32 {\n    dbg!(stub!())\n}\n",
        "use crate::macros::stub;\npub fn f() -> Vec<u32> {\n    vec![stub!()]\n}\n",
        "pub fn f() -> u32 {\n    dbg!(crate::macros::stub!())\n}\n",
        "mod macros;\npub fn f() -> u32 {\n    dbg!(macros::stub!())\n}\n",
        "use crate::macros::stub;\nmacro_rules! m {\n    () => { stub!() };\n}\n",
        "pub fn f() -> u32 {\n    dbg!(\n        1,\n        ::crate::macros::stub!()\n    )\n}\n",
    ] {
        assert_eq!(reasons_of(body), vec!["alias-in-token-tree"], "{body:?}");
    }
    // Not on an added line, not in a string or a comment, and not a macro this
    // file defines.
    clean_and_parsed(
        "src/lib.rs",
        "use crate::macros::stub;\npub fn f() -> String {\n    format!(\"{}\", \"stub!()\") // stub!()\n}\n",
    );
    clean_and_parsed(
        "src/lib.rs",
        "macro_rules! stub {\n    () => { 1 };\n}\nuse crate::stub;\npub fn f() -> u32 {\n    dbg!(stub!())\n}\n",
    );
}

#[test]
fn an_alias_in_a_token_tree_is_proven_when_it_is_a_std_stub() {
    // The wrapper's own name is not a stub; its token tree holds std's `todo!`
    // under another name, which the tree can resolve.
    blocks_proven(
        "src/lib.rs",
        "use std::todo as t;\npub fn f() -> u32 {\n    dbg!(t!())\n}\n",
        3,
    );
}

#[test]
fn a_use_walk_past_its_cap_degrades_with_the_named_reason() {
    assert_eq!(Unparsed::AliasWalkBudget.as_str(), "alias-walk-budget");
    let mut body = String::new();
    for i in 0..50_100 {
        body.push_str(&format!("use a{i} as b{i};\n"));
    }
    body.push_str("pub fn f() -> u32 {\n    1\n}\n");
    assert!(body.len() < dc_verify::stub_ast::MAX_SOURCE_BYTES);
    let start = std::time::Instant::now();
    let reasons = reasons_of(&body);
    assert_eq!(reasons, vec!["alias-walk-budget"]);
    assert!(
        start.elapsed() < std::time::Duration::from_secs(2),
        "{:?}",
        start.elapsed()
    );
}

#[test]
fn a_long_alias_chain_resolves_in_one_indexed_pass() {
    // The same shape as the attack's 158 KB chain, written in both orders.
    for forward in [false, true] {
        let n = 8000usize;
        let mut body = String::new();
        let order: Vec<usize> = if forward {
            (1..n).collect()
        } else {
            (1..n).rev().collect()
        };
        for i in order {
            body.push_str(&format!("use t{} as t{};\n", i - 1, i));
        }
        body.push_str("use std::todo as t0;\npub fn f() -> u32 {\n    t7999!()\n}\n");
        let start = std::time::Instant::now();
        blocks_proven("src/lib.rs", &body, n as u32 + 2);
        let took = start.elapsed();
        eprintln!("alias chain n={n} forward={forward}: {took:?}");
        assert!(took < std::time::Duration::from_secs(2), "{took:?}");
    }
}

#[test]
fn an_oversize_new_rust_file_with_a_stub_is_reported_unparsed() {
    let mut body = String::from("pub fn f() -> u32 {\n    todo!()\n}\n");
    while body.len() <= dc_verify::stub_ast::MAX_SOURCE_BYTES {
        body.push_str("// padding padding padding padding padding padding\n");
    }
    let report = report_whole("src/lib.rs", &body);
    let reasons: Vec<&str> = report.unparsed.iter().map(|u| u.reason).collect();
    assert_eq!(reasons, vec![Unparsed::TooLarge.as_str()]);
    // The substring checks still ran and still found the stub.
    assert!(blocked(&report), "{:?}", report.findings);
    // A language the tree does not read is not reported as unparsed for size.
    let mut text = String::from("TODO: later\n");
    while text.len() <= dc_verify::stub_ast::MAX_SOURCE_BYTES {
        text.push_str("padding padding padding padding padding padding\n");
    }
    assert!(report_whole("notes.txt", &text).unparsed.is_empty());
}
