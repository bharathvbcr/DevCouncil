//! ft-37dcb404. A build's JSON names every file counted in
//! `file_progress.extraction.failed`, with the parse outcome. A call site in
//! HCL, HTML, or CSS — languages that do not extract calls — is an unresolved
//! ledger row whose reason says so, and `extracts_calls` stays false. Impact
//! and dead on a symbol whose caller file failed to parse name that file
//! instead of answering with an empty caller list.
//!
//! Driven through the installed binary and the store it writes. A partial
//! Python parse and a call-blind file with no call syntax stay out of both
//! lists.

use std::path::{Path, PathBuf};
use std::process::Command;

fn devmap() -> PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap")
}

fn run(root: &Path, args: &[std::ffi::OsString]) -> (String, String, bool) {
    let out = Command::new(devmap())
        .args(args)
        .current_dir(root)
        .output()
        .unwrap_or_else(|error| panic!("devmap {args:?}: {error}"));
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.success(),
    )
}

fn json(root: &Path, args: &[std::ffi::OsString]) -> serde_json::Value {
    let (stdout, stderr, ok) = run(root, args);
    serde_json::from_str(stdout.trim()).unwrap_or_else(|error| {
        panic!(
            "devmap {args:?} stdout is not JSON (ok={ok}, {error}): {stdout}\n--- stderr ---\n{stderr}"
        )
    })
}

fn arg(text: &str) -> std::ffi::OsString {
    std::ffi::OsString::from(text)
}

#[test]
fn a_failed_parse_is_named_and_a_call_blind_site_is_a_ledger_row() {
    let root = std::env::temp_dir().join(format!(
        "devmap-call-blind-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("kept.py"), "def kept():\n    return 1\n").unwrap();
    // No linked grammar: a real parse failure, and the text names `kept`.
    std::fs::write(root.join("caller.ps1"), "kept()\n").unwrap();
    std::fs::write(
        root.join("main.tf"),
        "resource \"aws_s3_bucket\" \"b\" {\n  bucket = file(\"x\")\n}\n",
    )
    .unwrap();
    std::fs::write(
        root.join("page.html"),
        "<!DOCTYPE html><html><body><button onclick=\"kept()\">go</button></body></html>\n",
    )
    .unwrap();
    std::fs::write(
        root.join("site.css"),
        ".a { background: url(\"x.png\"); }\n",
    )
    .unwrap();
    // Call-blind files with no call syntax must not grow a call ledger row.
    std::fs::write(
        root.join("quiet.html"),
        "<!DOCTYPE html><html><body><p class=\"probe\">hello</p></body></html>\n",
    )
    .unwrap();
    std::fs::write(root.join("quiet.css"), ".probe { color: red; }\n").unwrap();
    // A syntax error tree-sitter still reads is Partial, not extraction.failed.
    std::fs::write(root.join("broken.py"), "def still_here(\n    return 1\n").unwrap();

    let db = root.join("index.sqlite");
    let build = json(
        &root,
        &[
            arg("--db"),
            db.clone().into_os_string(),
            arg("--json"),
            arg("build"),
            arg("."),
        ],
    );

    let failed_count = build["file_progress"]["extraction"]["failed"]
        .as_u64()
        .unwrap_or_else(|| panic!("extraction.failed missing: {build}"));
    assert!(
        failed_count >= 1,
        "fixture must count caller.ps1 in extraction.failed, got {failed_count}: {build}"
    );
    let named = build.get("parse_failed_files").unwrap_or_else(|| {
        panic!("parse_failed_files absent while extraction.failed is {failed_count}: {build}")
    });
    let named = named
        .as_array()
        .unwrap_or_else(|| panic!("parse_failed_files is not a list: {named}"));
    assert_eq!(
        named.len() as u64,
        failed_count,
        "the named list must be the failed set, not a sample: {named:?}"
    );
    let caller = named
        .iter()
        .find(|row| {
            row["path"]
                .as_str()
                .is_some_and(|path| path.ends_with("caller.ps1"))
        })
        .unwrap_or_else(|| panic!("caller.ps1 is counted but not named: {named:?}"));
    assert_eq!(caller["outcome"].as_str(), Some("failed"), "{caller}");
    let reason = caller["reason"].as_str().unwrap_or("");
    assert!(
        !reason.is_empty(),
        "a named failure needs its parse reason: {caller}"
    );
    for spared in [
        "page.html",
        "site.css",
        "main.tf",
        "kept.py",
        "broken.py",
        "quiet.html",
        "quiet.css",
    ] {
        assert!(
            named.iter().all(|row| {
                !row["path"]
                    .as_str()
                    .is_some_and(|path| path.ends_with(spared))
            }),
            "{spared} is not a parse failure and must not be named: {named:?}"
        );
    }

    for language in ["html", "css", "hcl"] {
        let row = &build["resolution_rate"]["by_language"][language];
        assert_eq!(
            row["extracts_calls"], false,
            "{language} has no call extractor; reporting it true would score sites it does not bind: {row}"
        );
    }

    let conn = rusqlite::Connection::open(&db).unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT source_file, callee_name, reason FROM generation_unresolved \
             WHERE generation_id = (SELECT max(id) FROM generations)",
        )
        .unwrap();
    let ledger: Vec<(String, String, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let blind = |file: &str, callee: &str| {
        ledger.iter().any(|(source, name, reason)| {
            source.ends_with(file) && name == callee && reason.contains("extracts no calls")
        })
    };
    assert!(
        blind("page.html", "kept"),
        "an HTML event-handler call must be a ledger row: {ledger:?}"
    );
    assert!(
        blind("main.tf", "file"),
        "an HCL function_call must be a ledger row: {ledger:?}"
    );
    assert!(
        blind("site.css", "url"),
        "a CSS declaration call must be a ledger row: {ledger:?}"
    );
    for quiet in ["quiet.html", "quiet.css"] {
        assert!(
            ledger.iter().all(|(source, _, reason)| {
                !(source.ends_with(quiet) && reason.contains("extracts no calls"))
            }),
            "{quiet} has no call syntax and must not gain a call ledger row: {ledger:?}"
        );
    }
    assert!(
        ledger
            .iter()
            .all(|(source, _, _)| !source.ends_with("caller.ps1")),
        "a file that failed to parse publishes no call row: {ledger:?}"
    );

    let impact = json(
        &root,
        &[
            arg("--db"),
            db.clone().into_os_string(),
            arg("--json"),
            arg("impact"),
            arg("kept"),
        ],
    );
    let impact_walk = impact["walk_incomplete"].as_str().unwrap_or("");
    assert!(
        impact_walk.contains("caller.ps1"),
        "impact of kept must name the failed caller file, not an empty caller list: {impact}"
    );
    let items = impact["items"].as_array().cloned().unwrap_or_default();
    assert!(
        items.iter().all(|edge| {
            !edge["source_file"]
                .as_str()
                .is_some_and(|path| path.ends_with("caller.ps1"))
        }),
        "a failed parse must not be invented as a Calls edge: {items:?}"
    );

    let dead = json(
        &root,
        &[arg("--db"), db.into_os_string(), arg("--json"), arg("dead")],
    );
    let dead_walk = dead["walk_incomplete"].as_str().unwrap_or("");
    assert!(
        dead_walk.contains("caller.ps1"),
        "dead must name the failed caller file: {dead}"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// A build that parsed everything still publishes the list, empty. Impact
/// then has nothing to name.
#[test]
fn a_clean_build_names_an_empty_failure_list() {
    let root = temp_root("clean");
    std::fs::write(root.join("kept.py"), "def kept():\n    return 1\n").unwrap();
    let db = root.join("index.sqlite");
    let build = json(
        &root,
        &[
            arg("--db"),
            db.clone().into_os_string(),
            arg("--json"),
            arg("build"),
            arg("."),
        ],
    );
    assert_eq!(build["file_progress"]["extraction"]["failed"], 0, "{build}");
    let named = build["parse_failed_files"]
        .as_array()
        .unwrap_or_else(|| panic!("parse_failed_files absent on a clean build: {build}"));
    assert!(named.is_empty(), "{named:?}");
    let impact = json(
        &root,
        &[
            arg("--db"),
            db.into_os_string(),
            arg("--json"),
            arg("impact"),
            arg("kept"),
        ],
    );
    let walk = impact["walk_incomplete"].as_str().unwrap_or("");
    assert!(
        !walk.contains("failed to parse"),
        "a clean corpus must not grow a parse-failure note: {impact}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Hostile names, a call that is not a call, a language that does extract
/// calls, and the scan cap. One build, so the assertions describe one store.
#[test]
fn hostile_call_blind_inputs_stay_named_and_bounded() {
    let root = temp_root("hostile");
    std::fs::write(root.join("kept.py"), "def kept():\n    return 1\n").unwrap();
    std::fs::write(root.join("调用.ps1"), "kept()\n").unwrap();
    std::fs::write(root.join("odd name.ps1"), "kept()\n").unwrap();
    std::fs::write(root.join("onlyvar.css"), ".a { color: var(--gap); }\n").unwrap();
    std::fs::write(
        root.join("click.html"),
        "<button onclick=\"kept(\">go</button>\n",
    )
    .unwrap();
    let mut many = String::from(".a {\n");
    for index in 0..30 {
        many.push_str(&format!("  background: url(a{index}.png);\n"));
    }
    many.push_str("}\n");
    std::fs::write(root.join("many.css"), many).unwrap();
    let mut hostile = b".a { background: url(ok); content: \"".to_vec();
    hostile.push(0);
    hostile.extend("😀".as_bytes());
    hostile.extend(b"\"; }\n");
    std::fs::write(root.join("hostile.css"), hostile).unwrap();
    std::fs::write(
        root.join("attr.tf"),
        "resource \"aws_s3_bucket\" \"b\" {\n  bucket = aws_s3_bucket.b.id\n}\n",
    )
    .unwrap();
    std::fs::write(
        root.join("uni.html"),
        "<button onclick=\"名前()\">go</button>\n",
    )
    .unwrap();
    std::fs::write(root.join("run.sh"), "kept\n").unwrap();
    let mut capped_css = String::new();
    for index in 0..8_001 {
        capped_css.push_str(&format!(".a{index} {{ background: url(x); }}\n"));
    }
    std::fs::write(root.join("capped.css"), capped_css).unwrap();
    let mut capped_tf = String::from("locals {\n");
    for index in 0..8_001 {
        capped_tf.push_str(&format!("  a{index} = file(\"x\")\n"));
    }
    capped_tf.push_str("}\n");
    std::fs::write(root.join("capped.tf"), capped_tf).unwrap();

    let db = root.join("index.sqlite");
    let build = json(
        &root,
        &[
            arg("--db"),
            db.clone().into_os_string(),
            arg("--json"),
            arg("build"),
            arg("."),
        ],
    );
    let named = build["parse_failed_files"]
        .as_array()
        .unwrap_or_else(|| panic!("parse_failed_files absent: {build}"));
    for required in ["调用.ps1", "odd name.ps1"] {
        let row = named
            .iter()
            .find(|row| {
                row["path"]
                    .as_str()
                    .is_some_and(|path| path.ends_with(required))
            })
            .unwrap_or_else(|| panic!("{required} was not named: {named:?}"));
        assert_eq!(row["outcome"], "failed", "{row}");
        assert!(!row["reason"].as_str().unwrap_or("").is_empty(), "{row}");
    }
    assert_eq!(
        named.len() as u64,
        build["file_progress"]["extraction"]["failed"]
            .as_u64()
            .unwrap(),
        "{named:?}"
    );

    let conn = rusqlite::Connection::open(&db).unwrap();
    let rows_for = |suffix: &str| -> Vec<(String, String)> {
        let mut stmt = conn
            .prepare(
                "SELECT callee_name, reason FROM generation_unresolved \
                 WHERE generation_id = (SELECT max(id) FROM generations) \
                   AND source_file LIKE ?1",
            )
            .unwrap();
        stmt.query_map([format!("%{suffix}")], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    let blind = |suffix: &str, callee: &str| {
        rows_for(suffix)
            .iter()
            .any(|(name, reason)| name == callee && reason.contains("extracts no calls"))
    };
    assert!(
        blind("click.html", "kept"),
        "a call missing its close still counts"
    );
    assert!(
        blind("uni.html", "名前"),
        "a non-ascii callee is still a call"
    );
    assert!(
        blind("hostile.css", "url"),
        "nul and an emoji must not drop the call"
    );
    let many_urls = rows_for("many.css")
        .iter()
        .filter(|(name, reason)| name == "url" && reason.contains("extracts no calls"))
        .count();
    assert_eq!(many_urls, 30, "every url() in the rule is a row");
    assert!(
        rows_for("onlyvar.css")
            .iter()
            .all(|(_, reason)| !reason.contains("extracts no calls")),
        "var(--gap) is a selector use, not a call: {:?}",
        rows_for("onlyvar.css")
    );
    assert!(
        rows_for("attr.tf")
            .iter()
            .all(|(_, reason)| !reason.contains("extracts no calls")),
        "an attribute read is not a function call: {:?}",
        rows_for("attr.tf")
    );
    assert!(
        rows_for("run.sh")
            .iter()
            .all(|(_, reason)| !reason.contains("extracts no calls")),
        "shell extracts calls, so a command is not a call-blind row: {:?}",
        rows_for("run.sh")
    );
    let shell = &build["resolution_rate"]["by_language"]["shell"];
    assert_eq!(
        shell["extracts_calls"], true,
        "shell has a call extractor: {shell}"
    );

    let capped_css_urls = rows_for("capped.css")
        .iter()
        .filter(|(name, reason)| name == "url" && reason.contains("extracts no calls"))
        .count();
    assert_eq!(
        capped_css_urls, 8_000,
        "the css scan cap is the ledger length"
    );
    let capped_tf_calls = rows_for("capped.tf")
        .iter()
        .filter(|(name, reason)| name == "file" && reason.contains("extracts no calls"))
        .count();
    assert_eq!(
        capped_tf_calls, 8_000,
        "the hcl scan cap is the ledger length"
    );
    let gap_reason = |suffix: &str| -> String {
        let mut stmt = conn
            .prepare(
                "SELECT reason FROM generation_coverage_gaps \
                 WHERE generation_id = (SELECT max(id) FROM generations) \
                   AND path LIKE ?1",
            )
            .unwrap();
        stmt.query_map([format!("%{suffix}")], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .join("\n")
    };
    let css_gap = gap_reason("capped.css");
    assert!(
        css_gap.contains("dropped"),
        "a capped stylesheet says the call list is a prefix: {css_gap}"
    );
    let tf_gap = gap_reason("capped.tf");
    assert!(
        tf_gap.contains("call sites dropped"),
        "a capped HCL file says the call list is a prefix: {tf_gap}"
    );

    let impact = json(
        &root,
        &[
            arg("--db"),
            db.into_os_string(),
            arg("--json"),
            arg("impact"),
            arg("kept"),
        ],
    );
    let walk = impact["walk_incomplete"].as_str().unwrap_or("");
    assert!(walk.contains("调用.ps1"), "{impact}");
    assert!(walk.contains("odd name.ps1"), "{impact}");
    let _ = std::fs::remove_dir_all(&root);
}

fn temp_root(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "devmap-call-blind-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}
