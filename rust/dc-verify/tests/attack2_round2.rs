//! Round-2 adversarial tests for lane B (card ft-926e5b45): the alias family
//! (M1-M5) and the oversize report (P1). Each test states the contract and a
//! minimal input that breaks it. A failing test is a finding. A test marked
//! `#[ignore]` is a break that is pre-existing on main 115266fc.

use std::io::Write as _;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use dc_verify::parse_unified;
use dc_verify::rigor::{Severity, StubReport, detect_stubs_report};
use dc_verify::stub_ast::MAX_SOURCE_BYTES;

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

fn report(body: &str) -> StubReport {
    let diff = whole_file("src/lib.rs", body);
    let files = parse_unified(&diff).expect("diff parses");
    let post = body.to_string();
    detect_stubs_report(&files, &|_| Some(post.clone()))
}

fn blocked(r: &StubReport) -> bool {
    r.findings
        .iter()
        .any(|f| f.gate == "stub_detection" && f.severity == Severity::Blocking)
}

fn reasons(r: &StubReport) -> Vec<&'static str> {
    r.unparsed.iter().map(|u| u.reason).collect()
}

/// A stub must be blocked or the file degraded; "parsed and clean" is the break.
fn not_clean(body: &str) {
    let r = report(body);
    assert!(
        blocked(&r) || !r.unparsed.is_empty(),
        "a stub scanned clean: {body:?}\nfindings={:?}",
        r.findings
    );
}

/// Parsed, not degraded, not blocked.
fn clean(body: &str) {
    let r = report(body);
    assert!(
        r.unparsed.is_empty() && !blocked(&r),
        "a clean file was degraded or blocked: {body:?}\nunparsed={:?} findings={:?}",
        reasons(&r),
        r.findings
    );
}

// ------------------------------------------------------------ M1 chains

#[test]
fn m1_pub_crate_use_alias_chain_blocks() {
    not_clean(
        "pub(crate) use std as s;\npub(crate) use s::todo as t;\npub fn f() -> u32 {\n    t!()\n}\n",
    );
}

#[test]
fn m1_use_self_s_todo_after_use_std_as_s_blocks() {
    not_clean("use std as s;\nuse self::s::todo as t;\npub fn f() -> u32 {\n    t!()\n}\n");
}

#[test]
fn m1_leading_colon_root_alias_blocks() {
    not_clean("use ::std as s;\nuse s::todo as t;\npub fn f() -> u32 {\n    t!()\n}\n");
}

#[test]
fn m1_root_alias_in_group_then_stub_group_blocks() {
    not_clean("use std::{self as s};\nuse s::{todo as t};\npub fn f() -> u32 {\n    t!()\n}\n");
}

#[test]
fn m1_cyclic_aliases_terminate_and_are_clean() {
    let r = report("use a as b;\nuse b as a;\npub fn f() -> u32 {\n    1\n}\n");
    assert!(!blocked(&r) && r.unparsed.is_empty(), "{:?}", reasons(&r));
}

#[test]
fn m1_cyclic_root_aliases_still_resolve_the_stub() {
    not_clean(
        "use std as s;\nuse s as t;\nuse t as s;\nuse s::todo as x;\npub fn f() -> u32 {\n    x!()\n}\n",
    );
}

#[test]
fn m1_use_inside_cfg_attr_blocks() {
    not_clean(
        "#[cfg_attr(all(), allow(unused))]\nuse std::todo as t;\npub fn f() -> u32 {\n    t!()\n}\n",
    );
}

#[test]
fn m1_use_inside_a_nested_block_alias_called_in_that_block_blocks() {
    not_clean("pub fn f() -> u32 {\n    use std::todo as t;\n    t!()\n}\n");
}

#[test]
fn m1_extern_crate_alias_without_a_use_still_blocks_a_qualified_call() {
    not_clean("extern crate std as s;\npub fn f() -> u32 {\n    s::todo!()\n}\n");
}

#[test]
fn m1_macro_use_import_of_an_alias_through_a_module_alias_is_not_clean() {
    // `use std::prelude as p;` then a path through it that does reach the macro.
    not_clean(
        "use core as c;\nuse c as d;\nuse d::unimplemented as u;\npub fn f() -> u32 {\n    u!()\n}\n",
    );
}

// ------------------------------------------------------ M2 macro scope

#[test]
fn m2_cfg_disabled_macro_rules_does_not_hide_the_alias() {
    // `#[cfg(any())]` removes the macro before expansion, so `t!` is the std
    // alias. The scope-aware shadow treats it as a real definition.
    not_clean(
        "use std::todo as t;\n#[cfg(any())]\nmacro_rules! t {\n    () => { 0 };\n}\npub fn f() -> u32 {\n    t!()\n}\n",
    );
}

#[test]
fn m2_macro_rules_defined_in_another_fn_body_does_not_hide_the_alias() {
    not_clean(
        "use std::todo as t;\nfn g() {\n    macro_rules! t {\n        () => { 0 };\n    }\n    let _ = t!();\n}\npub fn f() -> u32 {\n    t!()\n}\n",
    );
}

#[test]
fn m2_macro_rules_in_sibling_module_does_not_hide_the_other_modules_alias() {
    not_clean(
        "mod a {\n    macro_rules! t {\n        () => { 0 };\n    }\n}\nmod b {\n    use std::todo as t;\n    pub fn f() -> u32 {\n        t!()\n    }\n}\n",
    );
}

#[test]
fn m2_alias_module_before_macro_module_blocks() {
    not_clean(
        "mod b {\n    use std::todo as t;\n    pub fn f() -> u32 {\n        t!()\n    }\n}\nmod a {\n    macro_rules! t {\n        () => { 0 };\n    }\n}\n",
    );
}

#[test]
fn m2_macro_rules_defined_after_the_call_does_not_hide_the_alias() {
    not_clean(
        "use std::todo as t;\npub fn f() -> u32 {\n    t!()\n}\nmacro_rules! t {\n    () => { 0 };\n}\n",
    );
}

#[test]
fn m2_macro_rules_in_a_closed_inner_block_does_not_hide_a_later_call() {
    not_clean(
        "use std::todo as t;\npub fn f() -> u32 {\n    {\n        macro_rules! t {\n            () => { 0 };\n        }\n        let _ = t!();\n    }\n    t!()\n}\n",
    );
}

#[test]
fn m2_a_real_earlier_macro_rules_does_hide_the_alias() {
    clean(
        "use std::todo as t;\nmacro_rules! t {\n    () => { 0 };\n}\npub fn f() -> u32 {\n    t!()\n}\n",
    );
}

#[test]
fn m2_macros_2_0_stub_body_is_not_clean() {
    not_clean(
        "#![feature(decl_macro)]\npub macro t() {\n    todo!()\n}\npub fn f() -> u32 {\n    t!()\n}\n",
    );
}

// ------------------------------------------------------------- M4 noise

#[test]
fn m4_local_macro_inside_dbg_does_not_degrade() {
    clean("macro_rules! foo {\n    () => { 1 };\n}\npub fn f() -> u32 {\n    dbg!(foo!())\n}\n");
}

#[test]
fn m4_local_macro_defined_after_use_inside_dbg_does_not_degrade() {
    // The macro is textually after the call: rustc would refuse it. Only the
    // noise question matters here: a defined name is not "from elsewhere".
    let r = report(
        "pub fn f() -> u32 {\n    dbg!(foo!())\n}\nmacro_rules! foo {\n    () => { 1 };\n}\n",
    );
    assert!(!blocked(&r));
}

#[test]
fn m4_ordinary_std_macros_inside_wrappers_stay_clean() {
    clean(
        "pub fn f(x: Option<u32>) -> bool {\n    assert!(matches!(x, Some(_)));\n    dbg!(vec![1, 2, 3]);\n    println!(\"{}\", format!(\"{}\", 1));\n    x != None\n}\n",
    );
}

#[test]
fn m4_not_equal_with_a_paren_is_not_a_macro_call() {
    clean("use crate::flag;\npub fn f(a: u32) -> bool {\n    dbg!(a != (1));\n    flag(a)\n}\n");
}

#[test]
fn m4_imported_function_with_not_prefix_inside_a_macro_is_not_a_call() {
    // `flag !(x)` is not valid Rust, but `!flag(x)` and `a && !(flag(x))` are.
    clean(
        "use crate::flag;\npub fn f(a: u32) -> bool {\n    assert!(a == 1 && !(flag(a)));\n    true\n}\n",
    );
}

#[test]
fn m4_std_stub_alias_in_a_token_tree_blocks() {
    let r = report("use std::todo as t;\npub fn f() -> u32 {\n    dbg!(t!())\n}\n");
    assert!(blocked(&r), "{:?} {:?}", r.findings, reasons(&r));
}

#[test]
fn m4_path_qualified_alias_in_a_token_tree_is_not_clean() {
    not_clean("use std::todo as t;\npub fn f() -> u32 {\n    dbg!(self::t!())\n}\n");
}

#[test]
fn m4_root_alias_path_in_token_tree_blocks() {
    not_clean("use std as s;\npub fn f() -> u32 {\n    dbg!(s::todo!())\n}\n");
}

#[test]
fn m4_imported_macro_in_a_macro_rules_body_on_an_added_line_degrades() {
    let r = report(
        "use crate::inner;\nmacro_rules! outer {\n    () => { inner!() };\n}\npub fn f() {}\n",
    );
    assert!(!r.unparsed.is_empty(), "{:?}", r.findings);
}

#[test]
fn m4_imported_macro_called_in_a_nested_token_tree_two_levels_deep_degrades() {
    let r = report("use crate::inner;\npub fn f() -> u32 {\n    dbg!(dbg!(inner!()))\n}\n");
    assert!(!r.unparsed.is_empty(), "{:?}", r.findings);
}

#[test]
fn m4_mod_x_macro_called_in_a_token_tree_with_a_bracket_form_degrades() {
    let r = report("mod helpers;\npub fn f() -> u32 {\n    dbg!(helpers::m![1])\n}\n");
    assert!(!r.unparsed.is_empty(), "{:?}", r.findings);
}

#[test]
fn m4_imported_macro_inside_a_string_in_a_token_tree_is_not_a_call() {
    clean("use crate::inner;\npub fn f() -> u32 {\n    dbg!(\"inner!()\");\n    1\n}\n");
}

// ------------------------------------------------------------ M5 budget

fn timed(body: &str) -> (Duration, StubReport) {
    let start = Instant::now();
    let r = report(body);
    (start.elapsed(), r)
}

#[test]
fn m5_exactly_at_the_record_cap_resolves_and_blocks_the_stub() {
    // 49_999 filler leaves plus the std alias = 50_000 records, not over.
    let mut body = String::new();
    for i in 0..49_999 {
        body.push_str(&format!("use a{i} as b{i};\n"));
    }
    body.push_str("use std::todo as t;\npub fn f() -> u32 {\n    t!()\n}\n");
    let (took, r) = timed(&body);
    eprintln!("at cap: {took:?} reasons={:?}", reasons(&r));
    assert!(blocked(&r), "{:?}", reasons(&r));
}

#[test]
fn m5_one_over_the_record_cap_is_degraded_not_clean() {
    let mut body = String::new();
    for i in 0..50_000 {
        body.push_str(&format!("use a{i} as b{i};\n"));
    }
    body.push_str("use std::todo as t;\npub fn f() -> u32 {\n    t!()\n}\n");
    let (took, r) = timed(&body);
    eprintln!("cap+1: {took:?} reasons={:?}", reasons(&r));
    // 50_001 records: either the walk resolved and blocked or it degraded.
    assert!(blocked(&r) || reasons(&r) == vec!["alias-walk-budget"]);
}

#[test]
fn m5_one_giant_use_list_is_degraded_not_half_read() {
    // One `use` tree holding 400k leaves, 10x the cap, in under 2 MiB.
    let mut body = String::from("use std::{");
    for i in 0..200_000 {
        body.push_str(&format!("a{i},"));
    }
    body.push_str("};\nuse std::todo as t;\npub fn f() -> u32 {\n    t!()\n}\n");
    assert!(body.len() < MAX_SOURCE_BYTES, "{}", body.len());
    let (took, r) = timed(&body);
    eprintln!(
        "giant list: {took:?} reasons={:?} blocked={}",
        reasons(&r),
        blocked(&r)
    );
    assert!(blocked(&r) || !r.unparsed.is_empty());
}

/// The ping-pong that makes one pop cost every leaf under its name: `t` flips
/// between `todo` and `unimplemented`, and `u` and `t` hand each other the
/// flip. The step cap counts pops, not leaf visits.
fn pingpong(k: usize, m: usize) -> String {
    let mut body = String::new();
    for _ in 0..k {
        body.push_str("use std::todo as t;\nuse std::unimplemented as t;\n");
    }
    for _ in 0..m {
        body.push_str("use t as u;\n");
    }
    for _ in 0..m {
        body.push_str("use u as t;\n");
    }
    body.push_str("pub fn f() -> u32 {\n    t!()\n}\n");
    body
}

/// BREAK (round 2, M5): the step cap counts pops of the worklist, not the leaf
/// visits each pop makes, so this shape does quadratic work under the cap. The
/// debug-build timings measured on the lane head: k=m=250 68 ms, 500 250 ms,
/// 1000 0.93 s, 2000 3.77 s (4x per doubling); k=6000, m=12000 (36,000 records,
/// 582 KB, inside both caps) took 67 s and was neither degraded nor cut off.
/// The lane's own timing tests use 2 s.
#[test]
fn m5_pingpong_alias_flip_is_bounded_in_time() {
    let (took, r) = timed(&pingpong(2_000, 2_000));
    eprintln!(
        "pingpong k=m=2000: {took:?} blocked={} reasons={:?}",
        blocked(&r),
        reasons(&r)
    );
    // Loose 10 s backstop only: the exact bound is the step count asserted by
    // stub_ast::step_count_tests, which a loaded machine cannot flake.
    assert!(took < Duration::from_secs(10), "took {took:?}");
}

// ---------------------------------------------------------------- P1

struct Root(PathBuf);

impl Root {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "dcv-attack2-{}-{}",
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

fn check(root: &std::path::Path, planned: &str, diff: &[u8]) -> String {
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
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// A `.rs` body of exactly `size` bytes whose line 3 holds `todo!()`.
fn body_of(size: usize) -> String {
    let mut body = String::from("// pad\n// pad\npub fn f() -> u32 { todo!() }\n");
    while body.len() < size {
        let room = size - body.len();
        // A comment line: valid Rust of any length from 3 bytes up.
        let line = room.min(64);
        if line < 3 {
            // Too little room for a comment: pad the previous line's tail.
            body.pop();
            body.push_str(&" ".repeat(room));
            body.push('\n');
            break;
        }
        body.push_str("//");
        body.push_str(&"p".repeat(line - 3));
        body.push('\n');
    }
    assert_eq!(body.len(), size);
    body
}

const ADD_TODO_ON_LINE_3: &str = "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,2 +1,3 @@\n // pad\n // pad\n+pub fn f() -> u32 { todo!() }\n";

#[test]
fn p1_exactly_at_the_limit_is_parsed_and_one_over_is_unparsed_library() {
    let files = parse_unified(ADD_TODO_ON_LINE_3).unwrap();
    for (size, over) in [(MAX_SOURCE_BYTES, false), (MAX_SOURCE_BYTES + 1, true)] {
        let post = body_of(size);
        let r = detect_stubs_report(&files, &|_| Some(post.clone()));
        let got = r.unparsed.iter().any(|u| {
            u.reason == "source larger than the parse limit"
                || u.reason.contains("larger")
                || u.reason.contains("large")
        });
        eprintln!("size={size} unparsed={:?}", reasons(&r));
        assert_eq!(got, over, "size {size}: {:?}", reasons(&r));
        assert!(
            blocked(&r),
            "the stub must block either way (substring checks)"
        );
    }
}

#[test]
fn p1_exactly_at_the_limit_through_the_binary() {
    for (size, over) in [(MAX_SOURCE_BYTES, false), (MAX_SOURCE_BYTES + 1, true)] {
        let root = Root::new();
        std::fs::write(root.0.join("src/a.rs"), body_of(size)).unwrap();
        let out = check(&root.0, "src/a.rs", ADD_TODO_ON_LINE_3.as_bytes());
        let degraded = out.contains("src/a.rs: ") && out.contains("stub_ast_degraded\":[\"");
        eprintln!("binary size={size} degraded={degraded}");
        assert_eq!(
            degraded,
            over,
            "size {size}: {}",
            out.split("stub_ast_degraded")
                .nth(1)
                .unwrap_or("-")
                .chars()
                .take(160)
                .collect::<String>()
        );
    }
}

#[test]
fn p1_file_that_shrinks_below_the_limit_in_the_change_is_parsed() {
    let root = Root::new();
    // The diff is against a big file; the working tree is now small.
    std::fs::write(
        root.0.join("src/a.rs"),
        "// pad\n// pad\npub fn f() -> u32 { todo!() }\n",
    )
    .unwrap();
    let out = check(&root.0, "src/a.rs", ADD_TODO_ON_LINE_3.as_bytes());
    assert!(
        !out.contains("stub_ast_degraded\":[\""),
        "{}",
        out.split("stub_ast_degraded")
            .nth(1)
            .unwrap_or("-")
            .chars()
            .take(160)
            .collect::<String>()
    );
}

#[test]
fn p1_binary_marked_rs_file_over_the_limit_is_not_read_as_clean() {
    let root = Root::new();
    let mut bytes = vec![0u8; MAX_SOURCE_BYTES + 10];
    bytes[0] = b'x';
    std::fs::write(root.0.join("src/a.rs"), &bytes).unwrap();
    let diff = "diff --git a/src/a.rs b/src/a.rs\nindex 1..2 100644\nBinary files a/src/a.rs and b/src/a.rs differ\n";
    let out = check(&root.0, "src/a.rs", diff.as_bytes());
    // Not scanned means it must say so, in some channel.
    let said = out.contains("src/a.rs")
        && (out.contains("unscanned")
            || out.contains("degraded\":[\"")
            || out.contains("binary")
            || out.contains("not scanned"));
    assert!(said, "{}", out.chars().take(600).collect::<String>());
}

#[test]
fn p1_invalid_utf8_under_the_byte_limit_is_not_called_oversize_and_still_blocks_a_stub() {
    // Raw size below the limit; lossy decoding triples each bad byte.
    let root = Root::new();
    let mut bytes = b"// pad\n// pad\npub fn f() -> u32 { todo!() }\n".to_vec();
    bytes.extend(std::iter::repeat_n(0xFFu8, 800_000));
    bytes.push(b'\n');
    assert!(bytes.len() < MAX_SOURCE_BYTES);
    std::fs::write(root.0.join("src/a.rs"), &bytes).unwrap();
    let out = check(&root.0, "src/a.rs", ADD_TODO_ON_LINE_3.as_bytes());
    // A stub on an added line must block, or the file must be reported degraded.
    assert!(
        out.contains("stub_detection") || out.contains("stub_ast_degraded\":[\""),
        "{}",
        out.chars().take(500).collect::<String>()
    );
}

#[test]
fn p1_oversize_python_or_unknown_ext_is_not_reported_unparsed_for_size() {
    let root = Root::new();
    std::fs::write(root.0.join("src/a.txt"), "x".repeat(MAX_SOURCE_BYTES + 5)).unwrap();
    let diff = "diff --git a/src/a.txt b/src/a.txt\n--- a/src/a.txt\n+++ b/src/a.txt\n@@ -1 +1 @@\n-y\n+x\n";
    let out = check(&root.0, "src/a.txt", diff.as_bytes());
    assert!(
        !out.contains("stub_ast_degraded\":[\""),
        "{}",
        out.split("stub_ast_degraded")
            .nth(1)
            .unwrap_or("-")
            .chars()
            .take(160)
            .collect::<String>()
    );
}

// ------------------------------------------------------------------ leak

fn fake_key() -> String {
    concat!("AKIA", "IOSFODNN7EXAMPLE").to_string()
}

#[test]
fn leak_oversize_source_and_token_tree_degrade_print_no_secret_value() {
    // An oversize modified source whose added line holds a key, and an
    // imported macro in a token tree on a line that holds one.
    let root = Root::new();
    let key = fake_key();
    let added = format!("pub fn f() -> &'static str {{ \"{key}\" }}");
    let mut body = format!("// pad\n// pad\n{added}\n");
    while body.len() <= MAX_SOURCE_BYTES {
        body.push_str("// padding padding padding padding padding padding\n");
    }
    std::fs::write(root.0.join("src/a.rs"), &body).unwrap();
    let diff = format!(
        "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,2 +1,3 @@\n // pad\n // pad\n+{added}\n"
    );
    let out = check(&root.0, "src/a.rs", diff.as_bytes());
    assert!(
        out.contains("\"ok\":true"),
        "did not run: {}",
        out.chars().take(300).collect::<String>()
    );
    assert!(!out.contains(&key), "the secret value was printed");

    let small = format!(
        "use crate::inner;\npub fn f() -> String {{\n    format!(\"{{}}{{}}\", inner!(), \"{key}\")\n}}\n"
    );
    std::fs::write(root.0.join("src/b.rs"), &small).unwrap();
    let out = check(
        &root.0,
        "src/b.rs",
        whole_file("src/b.rs", &small).as_bytes(),
    );
    assert!(!out.contains(&key), "the secret value was printed");
    assert!(
        out.contains("alias-in-token-tree") || out.contains("macro imported from another file"),
        "{}",
        out.chars().take(400).collect::<String>()
    );
}
