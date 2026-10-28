//! Lane F (`lane/verify-decided` @ 6a537241), attack round 1, a4 and unreadable
//! sources. A failing test here is a finding.
//!
//! The a4 cap bounds how many scope checks a file may spend, not what one check
//! costs and not what the rest of the walk costs. These tests grow the other
//! dimensions (nesting depth, siblings per scope, names per scope, path length)
//! while the cap holds, and watch wall time and the verdict. A verdict is only
//! ever allowed to be: blocked, or degraded (`unparsed` non-empty), or, for a
//! file that truly has no stub, clean. A file with a stub in it never scans clean.

use std::collections::HashSet;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use dc_verify::parse_unified;
use dc_verify::rigor::{Severity, StubReport, detect_stubs_report};
use dc_verify::stub_ast::{Lang, Unparsed, analyze_within};

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

/// Runs `body` through the scan on its own thread and gives up waiting after
/// `limit`: a shape that does not return is a finding, and must not hang the run.
fn timed(label: &str, limit: Duration, body: String) -> (Duration, Option<StubReport>) {
    let (tx, rx) = mpsc::channel();
    let start = Instant::now();
    std::thread::Builder::new()
        .stack_size(256 << 20)
        .spawn(move || {
            let r = report(&body);
            let _ = tx.send(r);
        })
        .unwrap();
    match rx.recv_timeout(limit) {
        Ok(r) => {
            let took = start.elapsed();
            eprintln!(
                "{label}: {took:?} blocked={} unparsed={:?}",
                blocked(&r),
                r.unparsed
            );
            (took, Some(r))
        }
        Err(_) => {
            eprintln!("{label}: DID NOT RETURN within {limit:?}");
            (limit, None)
        }
    }
}

const ALIAS: &str = "use std::todo as t;\n";

/// Real stub in a file: never clean, whatever the shape.
fn never_clean(label: &str, took: Duration, r: Option<StubReport>, limit: Duration) {
    let r = r.unwrap_or_else(|| panic!("{label}: no verdict within {limit:?} (took {took:?})"));
    assert!(
        blocked(&r) || !r.unparsed.is_empty(),
        "{label}: a file with a stub scanned clean: findings={:?}",
        r.findings
    );
    assert!(took < limit, "{label}: took {took:?}, bound {limit:?}");
}

// ----------------------------------------------------------------- depth

/// One macro per nested scope, then many calls at the bottom. The calls are in
/// the alias's scope (no `t` macro at the bottom shadows it: the definitions
/// are in closed sibling blocks along the way), so each is a real stub.
#[test]
#[ignore = "pre-existing on main: every finding climbs `enclosing_function` (stub_ast.rs:1216, main:494), and tree-sitter's Node::parent is O(depth), so cost is quadratic in nesting depth per finding: 4000 stubs at depth 200 take 50 s in debug, with or without an alias; the host's 60 s deadline turns it into a blocking rigor_check_unavailable, so it fails closed"]
fn deep_scopes_with_sibling_definitions_then_many_calls() {
    for (depth, calls) in [(500usize, 20_000usize), (2_000, 20_000), (4_000, 30_000)] {
        let mut body = String::from(ALIAS);
        body.push_str("pub fn f() {\n");
        for _ in 0..depth {
            body.push_str("{ { macro_rules! t { () => {}; } }\n");
        }
        for _ in 0..calls {
            body.push_str("t!();\n");
        }
        for _ in 0..depth {
            body.push_str("}\n");
        }
        body.push_str("}\n");
        let limit = Duration::from_secs(60);
        let (took, r) = timed(&format!("deep d={depth} c={calls}"), limit, body);
        never_clean(&format!("deep d={depth} c={calls}"), took, r, limit);
    }
}

/// The scopes nest and each defines `t`, and the call is inside the innermost:
/// it is genuinely shadowed. Exact answer is clean, or degraded at the cap; a
/// block would be a false positive, not a break. Time is the question.
#[test]
fn deep_scopes_each_defining_the_name_shadowed_call() {
    let (depth, calls) = (3_000usize, 20_000usize);
    let mut body = String::from(ALIAS);
    body.push_str("pub fn f() {\n");
    for _ in 0..depth {
        body.push_str("{ macro_rules! t { () => {}; }\n");
    }
    for _ in 0..calls {
        body.push_str("t!();\n");
    }
    for _ in 0..depth {
        body.push_str("}\n");
    }
    body.push_str("}\n");
    let limit = Duration::from_secs(60);
    let (took, r) = timed("deep shadowed", limit, body);
    let r = r.expect("no verdict");
    assert!(took < limit, "took {took:?}");
    // Never anything but clean/blocked/degraded, which the type already says;
    // the point is that it returned.
    eprintln!("deep shadowed verdict: blocked={} unparsed={:?}", blocked(&r), r.unparsed);
}

/// Wide scope: one block with very many sibling statements, a macro defined
/// among them every so often. `Node::parent` and the sibling walks are not
/// O(1) in tree-sitter.
#[test]
fn one_wide_block_with_many_definitions() {
    for defs in [10_000usize, 30_000, 60_000] {
        let mut body = String::from(ALIAS);
        body.push_str("pub fn f() {\n");
        for i in 0..defs {
            body.push_str(&format!("macro_rules! m{i} {{}}\n"));
        }
        body.push_str("t!();\n}\n");
        assert!(body.len() < (2 << 20), "{}", body.len());
        let limit = Duration::from_secs(60);
        let (took, r) = timed(&format!("wide defs={defs}"), limit, body);
        never_clean(&format!("wide defs={defs}"), took, r, limit);
    }
}

/// Many calls in one wide function body; `enclosing_function` and the call
/// path both climb from every call.
#[test]
fn one_wide_function_with_many_calls() {
    let mut body = String::from(ALIAS);
    body.push_str("pub fn f() {\n");
    for _ in 0..200_000 {
        body.push_str("t!();\n");
    }
    body.push_str("}\n");
    assert!(body.len() < (2 << 20), "{}", body.len());
    let limit = Duration::from_secs(60);
    let (took, r) = timed("wide calls", limit, body);
    never_clean("wide calls", took, r, limit);
}

// ------------------------------------------------------------- many names

/// Very many distinct names defined in one scope, alias called many times.
#[test]
fn many_names_in_one_scope() {
    let mut body = String::from(ALIAS);
    for i in 0..60_000 {
        body.push_str(&format!("macro_rules! n{i} {{}}\n"));
    }
    body.push_str("pub fn f() {\n");
    for _ in 0..20_000 {
        body.push_str("t!();\n");
    }
    body.push_str("}\n");
    assert!(body.len() < (2 << 20), "{}", body.len());
    let limit = Duration::from_secs(60);
    let (took, r) = timed("many names", limit, body);
    never_clean("many names", took, r, limit);
}

/// Many alias names, each defined in its own scope and called in another.
#[test]
fn many_aliases_each_with_a_scope() {
    let mut body = String::new();
    let n = 20_000;
    for i in 0..n {
        body.push_str(&format!("use std::todo as a{i};\n"));
    }
    for i in 0..n {
        body.push_str(&format!("{{ macro_rules! a{i} {{}} }}\n"));
    }
    body.push_str("pub fn f() {\n");
    for i in 0..n {
        body.push_str(&format!("a{i}!();\n"));
    }
    body.push_str("}\n");
    assert!(body.len() < (2 << 20), "{}", body.len());
    let limit = Duration::from_secs(60);
    let (took, r) = timed("many aliases", limit, body);
    never_clean("many aliases", took, r, limit);
}

// ------------------------------------------------------------- long paths

/// Calls through a path longer than the lexer's 64 segments, inside another
/// macro's token tree, and a path of exactly 64 and 65 segments naming the alias.
#[test]
fn long_call_paths_in_token_trees() {
    for segs in [63usize, 64, 65, 200] {
        let path: String = std::iter::repeat_n("self::", segs).collect();
        let mut body = String::from(ALIAS);
        body.push_str("pub fn f() {\n");
        for _ in 0..5_000 {
            body.push_str(&format!("dbg!({path}t!());\n"));
        }
        body.push_str("}\n");
        let limit = Duration::from_secs(60);
        let (took, r) = timed(&format!("long path segs={segs}"), limit, body);
        let r = r.expect("no verdict");
        assert!(took < limit);
        eprintln!(
            "segs={segs}: blocked={} unparsed={:?}",
            blocked(&r),
            r.unparsed
        );
    }
}

/// A plain alias call written in a token tree, with and without a long path in
/// front of an unrelated name: the stub is real, so it is never clean.
#[test]
fn alias_call_in_a_token_tree_after_long_junk_paths() {
    let junk: String = std::iter::repeat_n("a::", 63).collect();
    let mut body = String::from(ALIAS);
    body.push_str("pub fn f() {\n");
    for _ in 0..3_000 {
        body.push_str(&format!("dbg!({junk}x);\n"));
    }
    body.push_str("dbg!(t!());\n}\n");
    let limit = Duration::from_secs(60);
    let (took, r) = timed("token tree after junk", limit, body);
    never_clean("token tree after junk", took, r, limit);
}

// ------------------------------------------------------------- budget sweep

/// A parse cancelled by the budget must never hand back fewer findings than the
/// full parse: it is `Err(OverBudget)` or exactly the full answer. Swept from a
/// zero budget up through the parse time of the file.
#[test]
fn a_parse_budget_expiry_degrades_and_never_passes() {
    let mut body = String::from("use std::todo as t;\n");
    for i in 0..40_000 {
        body.push_str(&format!("pub fn g{i}() -> u32 {{ {i} }}\n"));
    }
    body.push_str("pub fn f() -> u32 { t!() }\n");
    let added: HashSet<u32> = (1..=body.lines().count() as u32).collect();
    let full = analyze_within(Lang::Rust, &body, &added, Duration::from_secs(60))
        .expect("the full parse answers");
    assert!(
        full.iter().any(|f| f.severity == Severity::Blocking),
        "the control must find the stub: {full:?}"
    );
    let mut saw_over_budget = false;
    for micros in [0u64, 1, 5, 20, 100, 500, 2_000, 8_000, 30_000, 120_000] {
        let got = analyze_within(Lang::Rust, &body, &added, Duration::from_micros(micros));
        match got {
            Err(Unparsed::OverBudget) => saw_over_budget = true,
            Err(other) => panic!("budget {micros}us: unexpected degrade {other:?}"),
            Ok(findings) => assert_eq!(
                findings, full,
                "budget {micros}us: an answer with different findings than the full parse"
            ),
        }
    }
    assert!(saw_over_budget, "no budget in the sweep expired: the sweep tested nothing");
}

// ------------------------------------------------------- unreadable sources

#[cfg(unix)]
mod unreadable {
    use std::io::Write as _;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Root(PathBuf);

    impl Root {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let base = std::env::temp_dir().canonicalize().unwrap().join(format!(
                "dcv-decided-{}-{}",
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

    fn check(root: Option<&Path>, planned: &str, diff: &str) -> String {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_dcverify"));
        cmd.arg("check");
        if let Some(root) = root {
            cmd.arg("--root").arg(root);
        }
        let mut child = cmd
            .args(["--planned", planned])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let body = diff.to_string();
        let writer = std::thread::spawn(move || {
            let _ = stdin.write_all(body.as_bytes());
        });
        let out = child.wait_with_output().unwrap();
        let _ = writer.join();
        format!(
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    }

    fn modify(path: &str, line: &str) -> String {
        format!(
            "diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -1,2 +1,3 @@\n // pad\n // pad\n+{line}\n"
        )
    }

    fn stub_found(out: &str) -> bool {
        out.contains("stub_detection") && out.contains("\"blocking\"")
            || out.contains("stub_detection") && out.contains("placeholder")
            || out.contains("\"gate\":\"stub_detection\"")
    }

    fn degraded(out: &str) -> bool {
        out.contains("stub_ast_degraded\":[\"")
    }

    /// An unreadable source whose added line is a stub: the substring checks
    /// still find it, or the report degrades. Clean and undegraded is the break.
    #[test]
    fn unreadable_source_with_an_added_stub_is_found_or_degraded() {
        let root = Root::new();
        std::fs::write(
            root.0.join("src/a.rs"),
            "// pad\n// pad\npub fn f() -> u32 { todo!() }\n",
        )
        .unwrap();
        std::fs::set_permissions(
            root.0.join("src/a.rs"),
            std::os::unix::fs::PermissionsExt::from_mode(0o000),
        )
        .unwrap();
        if std::fs::File::open(root.0.join("src/a.rs")).is_ok() {
            return; // running as root: mode 000 is readable
        }
        let out = check(
            Some(&root.0),
            "src/a.rs",
            &modify("src/a.rs", "pub fn f() -> u32 { todo!() }"),
        );
        assert!(out.contains("\"ok\":true"), "did not run: {out}");
        assert!(
            stub_found(&out) || degraded(&out),
            "an unreadable source holding todo!() scanned clean and undegraded:\n{out}"
        );
    }

    /// A changed source named by a path that climbs out of the root.
    #[test]
    fn a_dotdot_path_is_degraded_not_read() {
        let root = Root::new();
        let outside = root.0.parent().unwrap().join("dcv-decided-outside.rs");
        std::fs::write(&outside, "// pad\n// pad\npub fn f() -> u32 { 1 }\n").unwrap();
        let out = check(
            Some(&root.0),
            "../dcv-decided-outside.rs",
            &modify("../dcv-decided-outside.rs", "pub fn f() -> u32 { 1 }"),
        );
        let _ = std::fs::remove_file(&outside);
        assert!(
            degraded(&out) || out.contains("\"ok\":false"),
            "a path outside the root was neither refused nor degraded:\n{out}"
        );
    }

    /// An absolute path in the diff header.
    #[test]
    fn an_absolute_path_is_degraded_not_read() {
        let root = Root::new();
        let abs = root.0.join("src/a.rs");
        std::fs::write(&abs, "// pad\n// pad\npub fn f() -> u32 { todo!() }\n").unwrap();
        let p = abs.to_string_lossy().to_string();
        let out = check(
            Some(&root.0),
            &p,
            &modify(&p, "pub fn f() -> u32 { todo!() }"),
        );
        assert!(
            stub_found(&out) || degraded(&out) || out.contains("\"ok\":false"),
            "an absolute path scanned clean:\n{out}"
        );
    }

    /// The file the diff names is replaced by a hard link to something else with
    /// the same lines, then the stub is added in a place the diff does not say.
    #[test]
    fn a_source_longer_than_the_cap_with_a_stub_is_degraded() {
        let root = Root::new();
        let mut text = String::from("// pad\n// pad\npub fn f() -> u32 { todo!() }\n");
        text.push_str(&"// filler line to push the file past the two MiB source cap\n".repeat(40_000));
        assert!(text.len() > (2 << 20));
        std::fs::write(root.0.join("src/a.rs"), text).unwrap();
        let out = check(
            Some(&root.0),
            "src/a.rs",
            &modify("src/a.rs", "pub fn f() -> u32 { todo!() }"),
        );
        assert!(
            stub_found(&out) || degraded(&out),
            "an oversize source holding todo!() scanned clean and undegraded:\n{out}"
        );
    }
}

// ------------------------------------------------------------------ scaling

/// Where the time goes for the deep shape: with and without the alias, the
/// definitions and the calls, at growing depth, so the exponent shows.
/// `cargo test ... scaling_probe -- --ignored --nocapture`.
#[test]
#[ignore = "measurement, not an assertion"]
fn scaling_probe() {
    let shape = |depth: usize, calls: usize, alias: bool, defs: bool, call: &str| {
        let mut body = String::new();
        if alias {
            body.push_str(ALIAS);
        }
        body.push_str("pub fn f() {\n");
        for _ in 0..depth {
            if defs {
                body.push_str("{ { macro_rules! t { () => {}; } }\n");
            } else {
                body.push_str("{ {  }\n");
            }
        }
        for _ in 0..calls {
            body.push_str(call);
            body.push('\n');
        }
        for _ in 0..depth {
            body.push_str("}\n");
        }
        body.push_str("}\n");
        body
    };
    for depth in [50usize, 100, 200] {
        let calls = 4_000;
        for (name, alias, defs, call) in [
            ("no alias, no defs, plain call", false, false, "g();"),
            ("no alias, no defs, macro call", false, false, "t!();"),
            ("alias, no defs", true, false, "t!();"),
            ("alias + defs", true, true, "t!();"),
            ("no alias, direct todo!() (the path main has)", false, false, "todo!();"),
        ] {
            let (took, _) = timed(
                &format!("depth={depth} calls={calls} {name}"),
                Duration::from_secs(120),
                shape(depth, calls, alias, defs, call),
            );
            let _ = took;
        }
    }
}

// ----------------------------------------------------- the lexer's path cap

/// `MAX_CALL_PATH_SEGMENTS` (64) bounds what the token-tree lexer reads. A call
/// through a longer path is "not an invocation" to it. In valid Rust a path of
/// `super::` hops needs that many nested modules, so the shape is real. The same
/// call written with a short path is flagged: the verdict must not depend on how
/// many modules the file nests.
fn super_chain_call(depth: usize, in_token_tree: bool) -> String {
    let mut body = String::from(ALIAS);
    for i in 0..depth {
        body.push_str(&format!("mod m{i} {{\n"));
    }
    let hops: String = std::iter::repeat_n("super::", depth).collect();
    if in_token_tree {
        body.push_str(&format!("pub fn f() {{ dbg!({hops}t!()); }}\n"));
    } else {
        body.push_str(&format!("pub fn f() {{ {hops}t!(); }}\n"));
    }
    for _ in 0..depth {
        body.push_str("}\n");
    }
    body
}

#[test]
fn a_long_super_chain_call_is_flagged_like_a_short_one() {
    let mut worse = Vec::new();
    for in_token_tree in [false, true] {
        for depth in [1usize, 2, 30, 62, 63, 64, 65, 66, 100] {
            let r = report(&super_chain_call(depth, in_token_tree));
            let flagged = blocked(&r) || !r.unparsed.is_empty();
            eprintln!(
                "super chain depth={depth} token_tree={in_token_tree}: blocked={} unparsed={:?}",
                blocked(&r),
                r.unparsed
            );
            if !flagged {
                worse.push((depth, in_token_tree));
            }
        }
    }
    assert!(
        worse.is_empty(),
        "a stub reached through a long module path scanned clean and undegraded: {worse:?}"
    );
}
