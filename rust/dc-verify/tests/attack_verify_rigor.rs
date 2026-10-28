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
