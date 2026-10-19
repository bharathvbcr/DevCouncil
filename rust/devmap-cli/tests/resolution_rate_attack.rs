//! Adversarial probes for the `devmap build` resolution-rate printer.

use std::path::{Path, PathBuf};
use std::process::Command;

fn devmap() -> String {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap").to_string_lossy().into_owned()
}

fn run(root: &Path, args: &[&str]) -> String {
    let out = Command::new(devmap())
        .args(args)
        .current_dir(root)
        .output()
        .unwrap_or_else(|error| panic!("devmap {args:?}: {error}"));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn fixture(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "devmap-rr-attack-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    for (rel, body) in files {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }
    root
}

fn misses(n: usize) -> String {
    let mut s = String::from("class K:\n");
    for i in 0..n {
        s.push_str(&format!("    def m{i}(self):\n        return 1\n"));
    }
    for i in 0..n {
        s.push_str(&format!("def v{i}():\n    return m{i}()\n"));
    }
    s
}

const PY: &str = "def helper():\n    return 1\n\n\ndef main():\n    return helper()\n";

fn headline(text: &str) -> String {
    text.lines()
        .find(|l| l.contains("Resolution rate:"))
        .unwrap_or_else(|| panic!("no Resolution rate line in:\n{text}"))
        .to_string()
}

/// Probe 2: the printed raw totals beside the headline percent must be
/// reproducible: net == resolved / (resolved + unresolved - explained).
#[test]
fn p2_printed_totals_reproduce_the_printed_headline() {
    let root = fixture(
        "p2",
        &[("lib.py", PY), ("vendor/dep.py", &misses(30)), ("index.html", "<a href=\"x.html\">x</a>\n")],
    );
    let text = run(&root, &["--verbose", "build", "."]);
    let line = headline(&text);
    eprintln!("{text}");
    // "  Resolution rate: N.M% net, G.H% gross (R resolved / U unresolved, E explained)"
    let net: f64 = line
        .split("% net")
        .next()
        .unwrap()
        .rsplit(' ')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let nums: Vec<u64> = line
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .map(|s| s.parse().unwrap())
        .collect();
    // [net_i, net_d, gross_i, gross_d, resolved, unresolved, explained]
    let (r, u, e) = (nums[4] as f64, nums[5] as f64, nums[6] as f64);
    let recomputed = (r * 1000.0 / (r + u - e)).floor() / 10.0;
    assert_eq!(
        net, recomputed,
        "headline {net}% is not derivable from the totals printed beside it ({r}/{u}/{e} -> {recomputed}%): {line}"
    );
}

/// Probe 5: everything excluded prints neither NaN nor a percent.
#[test]
fn p5_all_excluded_prints_no_nan_and_no_percent() {
    let root = fixture(
        "p5",
        &[("vendor/dep.py", &misses(5)), ("index.html", "<a href=\"x.html\">x</a>\n")],
    );
    let text = run(&root, &["--verbose", "build", "."]);
    eprintln!("{text}");
    let line = headline(&text);
    assert!(!line.contains("NaN"), "{line}");
    assert!(!line.contains("% net"), "no measured site, so no percent: {line}");
    assert!(line.contains("not measured"), "{line}");
}

/// Probe 5: zero resolved with unresolved prints 0.0%, not "not measured".
#[test]
fn p5_zero_resolved_prints_zero_percent() {
    let root = fixture("p5z", &[("a.py", &misses(3))]);
    let text = run(&root, &["--verbose", "build", "."]);
    eprintln!("{text}");
    let line = headline(&text);
    assert!(line.contains("0.0% net"), "{line}");
}

/// Probe 2: the persisted rate carries the headline's own denominators, so a
/// JSON consumer can reproduce the headline without the printed raw totals.
#[test]
fn p2_json_lets_a_consumer_reproduce_the_headline() {
    let root = fixture("p2j", &[("lib.py", PY), ("vendor/dep.py", &misses(30))]);
    run(&root, &["build", "."]);
    let status = run(&root, &["--json", "status"]);
    let v: serde_json::Value = serde_json::from_str(status.trim()).unwrap();
    let rr = &v["resolution_rate"];
    let net = rr["net_permille"].as_u64().expect("headline");
    let py = &rr["by_language"]["python"];
    let (r, u, e) = (
        py["resolved_sites"].as_u64().unwrap(),
        py["unresolved_sites"].as_u64().unwrap(),
        py["explained_sites"].as_u64().unwrap(),
    );
    // A consumer holding only the top-level fields:
    // Repointed to measured_* by Opus ruling 2026-10-19: the raw totals are a public contract kept for attribution coverage; the headline's denominators are the additive measured_* fields.
    let (tr, tu, te) = (
        rr["measured_resolved_sites"].as_u64().unwrap(),
        rr["measured_unresolved_sites"].as_u64().unwrap(),
        rr["measured_explained_sites"].as_u64().unwrap(),
    );
    assert_eq!(net, r * 1000 / (r + u - e), "per-language rows reproduce it");
    assert_eq!(
        net,
        tr * 1000 / (tr + tu - te),
        "top-level measured_* fields do not reproduce the headline: {rr}"
    );
    // Pinned invariant: the raw totals still count every attribution site.
    assert_eq!(
        rr["resolved_sites"].as_u64().unwrap() + rr["unresolved_sites"].as_u64().unwrap(),
        tr + tu + rr["vendored_sites"].as_u64().unwrap() + rr["call_blind_sites"].as_u64().unwrap(),
        "raw totals must stay the attribution totals: {rr}"
    );
}
