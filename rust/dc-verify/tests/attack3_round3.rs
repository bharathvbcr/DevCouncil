//! Lane `verify-residue-2`, attack round (Sonnet adversarial pass on
//! `lane/verify-residue-2` @ 48fd2ab3). A failing test here is a finding.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use dc_verify::parse_unified;
use dc_verify::rigor::{Severity, StubReport, detect_stubs_report};

fn report(body: &str) -> StubReport {
    let lines: Vec<&str> = body.lines().collect();
    let mut diff = format!(
        "diff --git a/src/lib.rs b/src/lib.rs\nnew file mode 100644\n--- /dev/null\n+++ b/src/lib.rs\n@@ -0,0 +1,{} @@\n",
        lines.len()
    );
    for line in lines {
        diff.push('+');
        diff.push_str(line);
        diff.push('\n');
    }
    let files = parse_unified(&diff).expect("diff parses");
    let post = body.to_string();
    detect_stubs_report(&files, &|_| Some(post.clone()))
}

fn blocked(r: &StubReport) -> bool {
    r.findings
        .iter()
        .any(|f| f.gate == "stub_detection" && f.severity == Severity::Blocking)
}

fn not_clean(body: &str) {
    let r = report(body);
    assert!(
        blocked(&r) || !r.unparsed.is_empty(),
        "a stub scanned clean: {body:?}\nfindings={:?}",
        r.findings
    );
}

const CALL: &str = "pub fn f() -> u32 {\n    t!()\n}\n";
const ALIAS: &str = "use std::todo as t;\n";
const MACRO: &str = "macro_rules! t {\n    () => { 0 };\n}\n";

// ---- row 3: cfg variants the lane's own tests do not name --------------------

#[test]
fn a3_cfg_not_all_does_not_hide_the_alias() {
    not_clean(&format!("{ALIAS}#[cfg(not(all()))]\n{MACRO}{CALL}"));
}

#[test]
fn a3_cfg_feature_does_not_hide_the_alias() {
    not_clean(&format!("{ALIAS}#[cfg(feature = \"x\")]\n{MACRO}{CALL}"));
}

#[test]
fn a3_cfg_gated_macro_in_a_module_does_not_hide_the_alias_there() {
    not_clean(&format!(
        "{ALIAS}mod m {{\n    #[cfg(any())]\n    {MACRO}    pub fn g() -> u32 {{\n        t!()\n    }}\n}}\n"
    ));
}

#[test]
fn a3_cfg_gated_macro_statement_in_a_fn_body_does_not_hide_the_alias() {
    not_clean(&format!(
        "{ALIAS}pub fn f() -> u32 {{\n    #[cfg(any())]\n    {MACRO}    t!()\n}}\n"
    ));
}

#[test]
fn a3_cfg_disabled_macro_use_module_does_not_hide_the_alias_after_it() {
    not_clean(&format!(
        "{ALIAS}#[cfg(any())]\n#[macro_use]\nmod m {{\n    {MACRO}}}\n{CALL}"
    ));
}

#[test]
fn a3_macro_export_before_the_cfg_does_not_hide_the_alias() {
    not_clean(&format!(
        "{ALIAS}#[macro_export]\n#[cfg(any())]\n{MACRO}{CALL}"
    ));
}

#[test]
fn a3_block_comment_between_cfg_and_macro_does_not_hide_the_alias() {
    not_clean(&format!("{ALIAS}#[cfg(any())]\n/* why */\n{MACRO}{CALL}"));
}

#[test]
fn a3_a_cfg_gated_macro_does_not_shadow_a_path_call_either() {
    not_clean(&format!(
        "use std::todo as t;\n#[cfg(any())]\n{MACRO}pub fn f() -> u32 {{\n    crate::t!()\n}}\n"
    ));
}

// ---- row 4 / timing ---------------------------------------------------------

fn timed(body: &str) -> (Duration, StubReport) {
    let start = Instant::now();
    let r = report(body);
    (start.elapsed(), r)
}

/// Just under the record cap (49,000 of 50,000 records): the flip graph at the
/// largest size the cap lets through.
#[test]
fn a4_pingpong_just_under_the_record_cap_is_bounded() {
    let mut body = String::new();
    for _ in 0..12_000 {
        body.push_str("use std::todo as t;\nuse std::unimplemented as t;\n");
    }
    for _ in 0..12_500 {
        body.push_str("use t as u;\n");
    }
    for _ in 0..12_500 {
        body.push_str("use u as t;\n");
    }
    body.push_str(CALL);
    let (took, r) = timed(&body);
    eprintln!("pingpong 12000/12500: {took:?} blocked={}", blocked(&r));
    assert!(took < Duration::from_secs(5), "took {took:?}");
    assert!(blocked(&r) || !r.unparsed.is_empty());
}

/// A long chain `a0 -> a1 -> ...` ending at the alias, one leaf per name.
#[test]
fn a4_long_alias_chain_is_bounded_and_blocks() {
    let mut body = String::from("use std::todo as a0;\n");
    for i in 1..40_000 {
        body.push_str(&format!("use a{} as a{i};\n", i - 1));
    }
    body.push_str("use a39999 as t;\n");
    body.push_str(CALL);
    let (took, r) = timed(&body);
    eprintln!("chain: {took:?} blocked={}", blocked(&r));
    assert!(took < Duration::from_secs(5), "took {took:?}");
    assert!(blocked(&r) || !r.unparsed.is_empty());
}

/// `is_cfg_gated` walks previous siblings for every `macro_rules!`; a file made
/// of nothing else is the worst case for that walk (the file cap is 2 MiB).
#[test]
fn a4_many_macro_definitions_are_bounded() {
    let mut body = String::new();
    for i in 0..110_000 {
        body.push_str(&format!("macro_rules! m{} {{}}\n", i % 10));
    }
    assert!(body.len() < (2 << 20), "{}", body.len());
    body.push_str(CALL);
    let (took, _r) = timed(&body);
    eprintln!("many macro defs: {took:?}");
    assert!(took < Duration::from_secs(10), "took {took:?}");
}

/// Macros of one name, each in its own block, then many calls that none of
/// them is in scope for: `defined_at` scans every definition per call.
#[test]
fn a4_many_scoped_definitions_times_many_calls_is_bounded() {
    let mut body = String::from(ALIAS);
    for _ in 0..25_000 {
        body.push_str("{ macro_rules! t { () => {}; } }\n");
    }
    body.push_str("pub fn f() {\n");
    for _ in 0..60_000 {
        body.push_str("    t!();\n");
    }
    body.push_str("}\n");
    assert!(body.len() < (2 << 20), "{}", body.len());
    let (took, _r) = timed(&body);
    eprintln!("defs x calls: {took:?}");
    assert!(took < Duration::from_secs(10), "took {took:?}");
}

// ---- unreadable sources ------------------------------------------------------

struct Root(PathBuf);

impl Root {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "dcv-attack3-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(base.join("src")).unwrap();
        Self(base)
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::set_permissions(
            self.0.join("src/a.rs"),
            std::os::unix::fs::PermissionsExt::from_mode(0o644),
        );
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn check_bytes(root: &Path, planned: &str, diff: &[u8]) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_dcverify"))
        .arg("check")
        .arg("--root")
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
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn modify(path: &str) -> String {
    format!(
        "diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -1,2 +1,3 @@\n // pad\n // pad\n+pub fn f() -> u32 {{ todo!() }}\n"
    )
}

/// The contract: a changed Rust source the tree could not read is named in
/// `stub_ast_degraded`, or the stub is found. Silence is the break.
fn named_or_found(out: &str) {
    assert!(out.contains("\"ok\":true"), "did not run: {out}");
    let degraded = out.contains("stub_ast_degraded\":[\"");
    let found = out.contains("stub_detection");
    assert!(
        degraded || found,
        "the changed source was neither read nor named: {}",
        out.split("stub_ast_degraded").nth(1).unwrap_or(&out)
    );
}

#[test]
fn u_a_fifo_in_place_of_the_source_is_named_and_does_not_hang() {
    let root = Root::new();
    let path = root.0.join("src/a.rs");
    assert!(
        Command::new("mkfifo")
            .arg(&path)
            .status()
            .map(|s| s.success())
            .unwrap_or(false),
        "mkfifo"
    );
    let start = Instant::now();
    let out = check_bytes(&root.0, "src/a.rs", modify("src/a.rs").as_bytes());
    assert!(start.elapsed() < Duration::from_secs(20));
    named_or_found(&out);
}

#[test]
fn u_a_symlink_loop_as_the_source_is_named() {
    let root = Root::new();
    std::os::unix::fs::symlink("a.rs", root.0.join("src/a.rs")).unwrap();
    named_or_found(&check_bytes(
        &root.0,
        "src/a.rs",
        modify("src/a.rs").as_bytes(),
    ));
}

#[test]
fn u_a_symlinked_directory_component_is_named() {
    let root = Root::new();
    std::fs::create_dir_all(root.0.join("real")).unwrap();
    std::fs::write(
        root.0.join("real/a.rs"),
        "// pad\n// pad\npub fn f() -> u32 { todo!() }\n",
    )
    .unwrap();
    std::os::unix::fs::symlink("real", root.0.join("lnk")).unwrap();
    named_or_found(&check_bytes(
        &root.0,
        "lnk/a.rs",
        modify("lnk/a.rs").as_bytes(),
    ));
}

#[test]
fn u_an_empty_source_is_named() {
    let root = Root::new();
    std::fs::write(root.0.join("src/a.rs"), "").unwrap();
    named_or_found(&check_bytes(
        &root.0,
        "src/a.rs",
        modify("src/a.rs").as_bytes(),
    ));
}

#[test]
fn u_a_permission_denied_source_is_named() {
    let root = Root::new();
    let p = root.0.join("src/a.rs");
    std::fs::write(&p, "// pad\n// pad\npub fn f() -> u32 { todo!() }\n").unwrap();
    std::fs::set_permissions(&p, std::os::unix::fs::PermissionsExt::from_mode(0o000)).unwrap();
    named_or_found(&check_bytes(
        &root.0,
        "src/a.rs",
        modify("src/a.rs").as_bytes(),
    ));
}

#[test]
fn u_a_path_with_a_newline_is_named_or_found() {
    let root = Root::new();
    // The diff quotes the name; the file does not exist under that name.
    let diff = "diff --git \"a/src/a\\nb.rs\" \"b/src/a\\nb.rs\"\n--- \"a/src/a\\nb.rs\"\n+++ \"b/src/a\\nb.rs\"\n@@ -1,2 +1,3 @@\n // pad\n // pad\n+pub fn f() -> u32 { todo!() }\n";
    named_or_found(&check_bytes(&root.0, "src/x.rs", diff.as_bytes()));
}

#[test]
fn u_a_non_utf8_path_is_named_or_found() {
    let root = Root::new();
    let mut diff = Vec::new();
    diff.extend_from_slice(b"diff --git a/src/\xff.rs b/src/\xff.rs\n--- a/src/\xff.rs\n+++ b/src/\xff.rs\n@@ -1,2 +1,3 @@\n // pad\n // pad\n+pub fn f() -> u32 { todo!() }\n");
    named_or_found(&check_bytes(&root.0, "src/x.rs", &diff));
}

// ---- leaks -------------------------------------------------------------------

#[test]
fn l_the_fake_key_never_appears_in_the_check_output() {
    let root = Root::new();
    let key = format!("AKIA{}", "IOSFODNN7EXAMPLE");
    std::fs::write(
        root.0.join("src/a.rs"),
        format!("// pad\n// pad\npub const K: &str = \"{key}\";\n"),
    )
    .unwrap();
    let diff = format!(
        "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,2 +1,3 @@\n // pad\n // pad\n+pub const K: &str = \"{key}\";\n"
    );
    let out = check_bytes(&root.0, "src/a.rs", diff.as_bytes());
    assert!(
        out.contains("secret"),
        "the key was not found at all: {out}"
    );
    assert!(!out.contains(&key), "the key is in the output: {out}");
}
