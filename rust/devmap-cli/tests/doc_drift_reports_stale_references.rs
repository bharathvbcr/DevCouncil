//! `devmap doc-drift` through the binary, over a real temporary repository.
//!
//! One document holds every shape the report exists for: references that still
//! hold (which must not be reported), a symbol that moved, a file that is gone,
//! a line anchor past the end of a file, a heading anchor nothing produces, a
//! symbol that exists nowhere, and spans that are deliberately not checked.
//! Beside it: a store with no generation must say it is unavailable rather than
//! return an empty list, and a budget that cuts the list must say what it hid.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const VERIFY_JS: &str = "function VerifyTask(a) {\n  return a;\n}\n";
const OLD_JS: &str =
    "// ExternalName is only ever mentioned here, in a comment.\nfunction Other() {}\n";
const NEW_JS: &str = "function RelocatedTask() {}\n";
const OTHER_MD: &str = "# Other\n\n## Real Heading\n";
const GUIDE_MD: &str = "# Guide

Valid: [`VerifyTask`](../src/verify.js) and `VerifyTask` and `src/verify.js:2`.
Anchors ok: [other](other.md#real-heading) and [line](../src/verify.js#L2).

Moved: [`RelocatedTask`](../src/old.js).
Missing file: [gone](../src/gone.js).
Bad line: [big](../src/verify.js#L99).
Bad anchor: [x](other.md#no-such-heading).
Gone symbol: `Vanished()`.
Bad path line: `src/verify.js:50`.
Not checked: `tmp/out.sh` `elsewhere/lib.rs` `ExternalName()` `plainword` `HTTP`.

```
[fenced](nowhere.md) `FencedSymbol()`
```
";

fn devmap(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_devmap"))
        .current_dir(root)
        .env("DEVMAP_AUTOSPAWN", "0")
        .args(["--progress", "never"])
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("devmap {args:?}: {error}"))
}

#[track_caller]
fn ok(root: &Path, args: &[&str]) -> String {
    let output = devmap(root, args);
    assert!(
        output.status.success(),
        "devmap {args:?} failed:\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[track_caller]
fn json(root: &Path, args: &[&str]) -> serde_json::Value {
    let mut full = vec!["--json"];
    full.extend_from_slice(args);
    let stdout = ok(root, &full);
    serde_json::from_str(&stdout).unwrap_or_else(|error| panic!("{full:?}: {error}: {stdout}"))
}

fn scratch(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "devmap-doc-drift-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn git_init(root: &Path) {
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.email", "t@example.invalid"],
        vec!["config", "user.name", "t"],
        vec!["add", "-A"],
        vec!["commit", "-qm", "fixture"],
    ] {
        let out = Command::new("git")
            .args(&args)
            .current_dir(root)
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

fn fixture() -> PathBuf {
    let root = scratch("repo");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("docs")).unwrap();
    for (path, text) in [
        ("src/verify.js", VERIFY_JS),
        ("src/old.js", OLD_JS),
        ("src/new.js", NEW_JS),
        ("docs/other.md", OTHER_MD),
        ("docs/guide.md", GUIDE_MD),
    ] {
        std::fs::write(root.join(path), text).unwrap();
    }
    git_init(&root);
    let root = root.canonicalize().unwrap();
    ok(&root, &["build"]);
    root
}

/// `(doc, line, kind, reference)` of each finding, in report order.
fn findings(report: &serde_json::Value) -> Vec<(String, u64, String, String)> {
    report["findings"]
        .as_array()
        .unwrap_or_else(|| panic!("no findings array: {report}"))
        .iter()
        .map(|f| {
            (
                f["doc"].as_str().unwrap().to_string(),
                f["line"].as_u64().unwrap(),
                f["kind"].as_str().unwrap().to_string(),
                f["reference"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn expected() -> Vec<(String, u64, String, String)> {
    [
        (6, "symbol_moved", "RelocatedTask"),
        (7, "missing_file", "../src/gone.js"),
        (8, "line_out_of_range", "../src/verify.js#L99"),
        (9, "missing_anchor", "other.md#no-such-heading"),
        (10, "missing_symbol", "Vanished()"),
        (11, "line_out_of_range", "src/verify.js:50"),
    ]
    .into_iter()
    .map(|(line, kind, reference)| {
        (
            "docs/guide.md".to_string(),
            line,
            kind.to_string(),
            reference.to_string(),
        )
    })
    .collect()
}

#[test]
fn stale_references_are_reported_and_current_ones_are_not() {
    let root = fixture();
    let report = json(&root, &["doc-drift"]);

    assert_eq!(report["available"], true, "{report}");
    assert_eq!(report["docs_scanned"], 2, "docs/guide.md and docs/other.md");
    assert_eq!(report["docs_total"], 2);
    // Exactly the six drifted references: the valid link, symbol, path, line
    // anchor and heading anchor on lines 3 and 4 are absent, and so is
    // everything inside the fence.
    assert_eq!(findings(&report), expected(), "{report}");
    assert_eq!(report["total"], 6);
    assert_eq!(report["shown"], 6);
    assert_eq!(report["hidden"], 0);
    assert_eq!(report["truncated"], false);
    assert!(
        report.get("walk_incomplete").is_none(),
        "every document was read and checked: {report}"
    );

    let moved = &report["findings"][0];
    assert_eq!(
        moved["current_locations"],
        serde_json::json!(["src/new.js"])
    );
    assert!(
        moved["detail"].as_str().unwrap().contains("src/old.js"),
        "{moved}"
    );

    // What was checked, kind by kind: seven links, two path spans, two line
    // suffixes plus two `#L` anchors, two heading anchors, five symbols.
    assert_eq!(
        report["checked"],
        serde_json::json!({"link": 7, "path": 2, "line": 4, "anchor": 2, "symbol": 5}),
        "{report}"
    );
    // What was deliberately not checked, by reason.
    let skipped = &report["skipped"];
    assert_eq!(skipped["generated_path"], 1, "tmp/out.sh: {report}");
    assert_eq!(skipped["unrooted_path"], 1, "elsewhere/lib.rs: {report}");
    assert_eq!(
        skipped["defined_outside_symbol_index"], 1,
        "ExternalName() appears in a comment, so it is not missing: {report}"
    );
    assert_eq!(
        skipped["ambiguous_identifier"], 2,
        "plainword and HTTP: {report}"
    );
    // Seven links, and nine code spans: `VerifyTask`, the two line-suffixed
    // paths, `Vanished()`, `ExternalName()`, the two unchecked paths, and the
    // two ambiguous words. Nothing from the fence.
    assert_eq!(report["references"], 16, "{report}");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_text_report_names_each_finding_and_summarises() {
    let root = fixture();
    let text = ok(&root, &["doc-drift"]);
    assert!(
        text.contains("docs/guide.md:7 missing_file ../src/gone.js — "),
        "{text}"
    );
    assert!(
        text.contains("docs/guide.md:6 symbol_moved RelocatedTask — ")
            && text.contains("now in: src/new.js"),
        "{text}"
    );
    assert!(
        text.contains("doc-drift: 2 of 2 docs scanned")
            && text.contains("checked: anchor 2, line 4, link 7, path 2, symbol 5"),
        "{text}"
    );
    assert!(!text.contains("warning:"), "{text}");
    // A report is not a failure: findings leave the exit status at zero, which
    // `ok` already asserted.
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn paths_and_excludes_narrow_the_scan() {
    let root = fixture();

    let only_other = json(&root, &["doc-drift", "docs/other.md"]);
    assert_eq!(only_other["docs_scanned"], 1);
    assert_eq!(findings(&only_other), Vec::new(), "{only_other}");
    assert!(only_other.get("walk_incomplete").is_none(), "{only_other}");

    let directory = json(&root, &["doc-drift", "docs"]);
    assert_eq!(findings(&directory), expected());

    let excluded = json(&root, &["doc-drift", "--exclude", "docs/guide.md"]);
    assert_eq!(excluded["docs_scanned"], 1);
    assert_eq!(findings(&excluded), Vec::new());

    // A path that names nothing indexed is not a clean answer.
    let unmatched = json(&root, &["doc-drift", "docs/nope.md"]);
    assert_eq!(unmatched["docs_scanned"], 0);
    assert!(
        unmatched["walk_incomplete"]
            .as_str()
            .is_some_and(|reason| reason.contains("docs/nope.md")),
        "{unmatched}"
    );

    // A prefix that would leave the repository is refused, not followed.
    for bad in ["../outside.md", "/etc/hosts"] {
        let output = devmap(&root, &["doc-drift", bad]);
        assert!(
            !output.status.success(),
            "{bad} must be refused:\n{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_store_with_no_generation_is_unavailable_not_clean() {
    let root = scratch("empty");
    std::fs::write(root.join("README.md"), "See [gone](gone.md).\n").unwrap();
    git_init(&root);
    let root = root.canonicalize().unwrap();
    // A store that exists and holds no generation: what an aborted first build
    // leaves behind.
    let db = devmap_extract::paths::store_path(&root);
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();
    drop(devmap_store::Store::open(db).unwrap());

    let report = json(&root, &["doc-drift"]);
    assert_eq!(report["available"], false, "{report}");
    assert_eq!(report["docs_scanned"], 0);
    assert_eq!(findings(&report), Vec::new());
    assert!(
        report["walk_incomplete"]
            .as_str()
            .is_some_and(|reason| reason.contains("no persisted generation")),
        "an empty list over a store that could not be read must say so: {report}"
    );
    let text = ok(&root, &["doc-drift"]);
    assert!(text.starts_with("unavailable: "), "{text}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_budget_that_cuts_the_list_says_what_it_hid() {
    let root = fixture();
    let report = json(&root, &["doc-drift", "--budget", "40"]);
    let shown = report["shown"].as_u64().unwrap();
    let hidden = report["hidden"].as_u64().unwrap();
    assert_eq!(report["total"], 6, "{report}");
    assert!(
        shown >= 1 && shown < 6,
        "a budget of 40 keeps some: {report}"
    );
    assert_eq!(shown + hidden, 6, "{report}");
    assert_eq!(report["truncated"], true, "{report}");
    assert_eq!(report["findings"].as_array().unwrap().len() as u64, shown);
    // The kept findings are the first ones, in document order.
    assert_eq!(
        findings(&report),
        expected()
            .into_iter()
            .take(shown as usize)
            .collect::<Vec<_>>()
    );
    assert!(
        report["tokens_used"].as_u64().unwrap() <= 40,
        "the budget is a ceiling: {report}"
    );

    let text = ok(&root, &["doc-drift", "--budget", "40"]);
    assert!(
        text.contains(&format!("shown {shown} of 6 ({hidden} hidden)")),
        "{text}"
    );

    let refused = devmap(&root, &["doc-drift", "--budget", "0"]);
    assert!(!refused.status.success(), "a zero budget must be refused");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_secret_document_is_withheld_and_not_scanned() {
    let root = scratch("secret");
    std::fs::create_dir_all(root.join("secrets")).unwrap();
    // The link to the secret file has a line anchor far past its end. Its
    // length is a fact about the file's contents, so it must not be read to
    // answer the anchor: existence only.
    std::fs::write(
        root.join("README.md"),
        "Fine: [ok](README.md) [key](secrets/key.txt#L99) `secrets/key.txt:99`\n",
    )
    .unwrap();
    std::fs::write(root.join("secrets/key.txt"), "one line\n").unwrap();
    // A secret-path document with a broken link: never scanned, never reported.
    std::fs::write(root.join("secrets/notes.md"), "[gone](gone.md)\n").unwrap();
    git_init(&root);
    let root = root.canonicalize().unwrap();
    ok(&root, &["build"]);
    let report = json(&root, &["doc-drift"]);
    assert_eq!(
        report["docs_scanned"], 1,
        "only README.md is scanned: {report}"
    );
    assert_eq!(findings(&report), Vec::new(), "{report}");
    assert_eq!(report["skipped"]["secret_path"], 2, "{report}");
    assert!(
        report["walk_incomplete"]
            .as_str()
            .is_some_and(|reason| reason.contains("secret path")),
        "a withheld document is named, not silently dropped: {report}"
    );
    let _ = std::fs::remove_dir_all(&root);
}
