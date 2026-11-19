//! Adversarial same-line join and prefix cases that hold today (pinning tests).
use dc_redact::{contains_secret, redact_secrets};

fn aws() -> String {
    ["AKIA", "QJZXVBNMLKPOIUYT"].concat()
}
fn proj() -> String {
    ["sk-proj-", "Fx7Qa9Lm2Zt4Wc8Rk3Vn6Yb1Hd5Gj0PsFx7Qa9Lm2Zt4Wc8Rk3Vn6Yb1"].concat()
}

/// Detected, tail removed, output clean, second pass a no-op.
fn holds(case: &str, line: &str, tail: &str) {
    assert!(contains_secret(line), "{case}: missed: {line}");
    let out = redact_secrets(line);
    assert!(!out.contains(tail), "{case}: tail survived: {out}");
    assert!(!contains_secret(&out), "{case}: output still a secret: {out}");
    assert_eq!(redact_secrets(&out), out, "{case}: not idempotent");
}

fn untouched(case: &str, line: &str) {
    assert!(!contains_secret(line), "{case}: false positive: {line}");
    assert_eq!(redact_secrets(line), line, "{case}: line changed");
}

#[test]
fn joins_across_prefixes_quotes_and_piece_counts_are_found() {
    let k = aws();
    let (a, b) = (&k[..4], &k[4..]);
    let five = format!("k = \"{a}\" + \"{}\" + \"{}\" + \"{}\" + \"{}\"", &b[..4], &b[4..8], &b[8..12], &b[12..]);
    holds("five pieces", &five, b);
    let seven: Vec<String> = k.as_bytes().chunks(3).map(|c| format!("\"{}\"", std::str::from_utf8(c).unwrap())).collect();
    holds("seven pieces", &format!("k = {}", seven.join(" + ")), &k[4..]);
    for (name, line) in [
        ("single quotes", format!("k = '{a}' + '{b}'")),
        ("python implicit", format!("k = \"{a}\" \"{b}\"")),
        ("b prefix", format!("k = b\"{a}\" b\"{b}\"")),
        ("u prefix", format!("k = u\"{a}\" + u\"{b}\"")),
        ("c prefix", format!("k = c\"{a}\" c\"{b}\"")),
        ("cr prefix", format!("k = cr\"{a}\" cr\"{b}\"")),
        ("triple quotes", format!("k = \"\"\"{a}\"\"\" + \"\"\"{b}\"\"\"")),
        ("go define", format!("k := \"{a}\" + \"{b}\"")),
        ("multibyte neighbours", format!("k = \"é{a}\" + \"{b}é\"")),
        ("trailing code kept", format!("f(\"{a}\" + \"{b}\", other)")),
        ("escaped quote after the key", format!("k = \"{a}\" + \"{b}\\\"x\"")),
        ("unbalanced quote after", format!("k = \"{a}\" + \"{b}\" + \"")),
        ("rust lifetimes before", format!("fn f<'a>(x: &'a str) {{ let k = \"{a}\" + \"{b}\"; }}")),
    ] {
        holds(name, &line, b);
    }
    holds("two keys one line", &format!("a = \"{a}\" + \"{b}\"; b = \"{a}\" + \"{b}\""), b);
}

#[test]
fn a_separated_prefix_literal_does_not_hide_the_key() {
    let (k, p) = (aws(), proj());
    holds("space-ended lead", &format!("k = \"key: \" + \"{}\" + \"{}\"", &k[..4], &k[4..]), &k[4..]);
    holds("url lead", &format!("u = \"https://x/\" + \"{}\" + \"{}\"", &p[..8], &p[8..]), &p[8..]);
    holds("bearer lead", &format!("h = \"Bearer \" + \"{}\" + \"{}\"", &p[..8], &p[8..]), &p[8..]);
    holds("three pieces", &format!("h = \"{}\" + \"{}\" + \"{}\"", &p[..8], &p[8..30], &p[30..]), &p[30..]);
}

#[test]
fn a_joined_value_past_512_bytes_is_still_redacted_whole() {
    let p = proj();
    let long = "Fx7Qa9Lm2Zt4Wc8Rk3Vn6Yb1Hd5Gj0Ps".repeat(40);
    let two = format!("h = \"{}\" + \"{}{long}\"", &p[..8], &p[8..]);
    holds("two pieces", &two, &long[long.len() - 40..]);
    let three = format!("h = \"{}\" + \"{}\" + \"{long}\"", &p[..8], &p[8..]);
    holds("three pieces", &three, &long[long.len() - 40..]);
}

#[test]
fn what_is_not_a_literal_run_is_left_alone() {
    let k = aws();
    let (a, b) = (&k[..4], &k[4..]);
    untouched("php dot", &format!("$k = \"{a}\" . \"{b}\";"));
    untouched("sql concat", &format!("select '{a}' || '{b}'"));
    untouched("comment between", &format!("k = \"{a}\" /* x */ + \"{b}\""));
    untouched("variable between", &format!("k = \"{a}\" + x + \"{b}\""));
    untouched("backticks", &format!("k = `{a}` + `{b}`"));
    untouched("ordinary join", "t = \"sk-\" + \"proj\" + \"ect\"");
    untouched("example key", "k = \"AKIA\" + \"EXAMPLE\"");
    untouched("dapi identifier", "let dapiClientFactory = makeClient(\"a\") + \"b\";");
    untouched("gsk identifier", "GSK_CONFIG = \"x\" + \"y\"");
}

/// Peak RSS of a child that scans one line, via /usr/bin/time -l (no unsafe
/// allocator: the workspace forbids unsafe code).
#[cfg(target_os = "macos")]
fn rss_of(shape: &str, n: usize) -> usize {
    let exe = std::env::current_exe().unwrap();
    let out = std::process::Command::new("/usr/bin/time")
        .arg("-l")
        .arg(exe)
        .args(["--exact", "child_scan"])
        .env("ADV_SHAPE", shape)
        .env("ADV_N", n.to_string())
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    let line = err.lines().find(|l| l.contains("maximum resident set size")).expect("no rss line");
    line.split_whitespace().next().unwrap().parse().unwrap()
}

#[test]
fn child_scan() {
    let Ok(shape) = std::env::var("ADV_SHAPE") else { return };
    let n: usize = std::env::var("ADV_N").unwrap().parse().unwrap();
    let line = match shape.as_str() {
        "tiny" => format!("k = {}\"b\"", "\"a\"+".repeat(n / 4)),
        "none" => format!("k = \"{}\"", "x".repeat(n)),
        other => panic!("unknown shape {other}"),
    };
    assert!(!contains_secret(&line));
}

/// Memory is linear in the line, not superlinear: 8x the input costs under
/// 12x the memory. The constant is large (see the breaks file), and this does
/// not bound it.
#[cfg(target_os = "macos")]
#[test]
fn many_tiny_literals_cost_memory_linear_in_the_line() {
    let base = rss_of("none", 0);
    let small = rss_of("tiny", 1 << 20).saturating_sub(base).max(1);
    let large = rss_of("tiny", 8 << 20).saturating_sub(base);
    assert!(large < small * 12, "memory grew {small} -> {large} for 8x input");
}
