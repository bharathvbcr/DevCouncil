//! `devmap ast`, end to end.
//!
//! The two properties the Python it replaces cannot express: an exact total
//! beside a truncated page, and a symbol's real extraction provenance rather
//! than whether a parser happened to be installed.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

fn devmap() -> PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap")
}

fn fixture(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "devmap-ast-{label}-{}-{stamp}-{seq}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/a.py"),
        "def alpha():\n    return 1\n\n\ndef alpha_two():\n    return 2\n\n\nclass Alpha:\n    pass\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/b.rs"),
        "pub fn alpha_rust() -> u32 { 3 }\npub struct AlphaKind;\n",
    )
    .unwrap();
    root
}

fn run(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(devmap())
        .args(args)
        .current_dir(root)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .output()
        .expect("devmap invocation")
}

fn json(output: &std::process::Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("JSON")
}

fn build(root: &Path) {
    let output = run(root, &["--progress", "never", "build", "."]);
    assert!(
        output.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_truncated_page_still_reports_the_number_it_was_cut_from() {
    let root = fixture("total");
    build(&root);

    let all = json(&run(&root, &["ast", "alpha", "--json"]));
    let total = all["total"].as_u64().unwrap();
    assert!(total >= 4, "expected the four alpha* symbols, got {all}");
    assert_eq!(all["truncated"], false);
    assert_eq!(all["shown"], all["total"]);

    let page = json(&run(&root, &["ast", "alpha", "--limit", "1", "--json"]));
    assert_eq!(page["shown"], 1);
    assert_eq!(
        page["total"], total,
        "the total is of the match set, not the page"
    );
    assert_eq!(page["truncated"], true);
    assert_eq!(page["hidden"], total - 1);

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_kind_or_language_filter_narrows_the_set_before_it_is_cut() {
    let root = fixture("filter");
    build(&root);

    let classes = json(&run(&root, &["ast", "alpha", "--kind", "Class", "--json"]));
    for hit in classes["matches"].as_array().unwrap() {
        assert_eq!(hit["kind"], "Class");
    }

    let rust = json(&run(
        &root,
        &["ast", "alpha", "--language", "rust", "--json"],
    ));
    for hit in rust["matches"].as_array().unwrap() {
        assert_eq!(hit["language"], "rust");
        assert!(hit["path"].as_str().unwrap().ends_with(".rs"));
    }
    // The filtered total is the filtered match count, not the unfiltered one.
    let unfiltered = json(&run(&root, &["ast", "alpha", "--json"]));
    assert!(rust["total"].as_u64().unwrap() < unfiltered["total"].as_u64().unwrap());

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_filter_the_index_cannot_satisfy_is_named_not_answered_as_zero() {
    let root = fixture("unmatched");
    build(&root);

    // Nothing matches, and the reason is that the filter names something the
    // generation does not hold — a different problem from a genuine zero.
    let typo = json(&run(&root, &["ast", "--language", "cobolol", "--json"]));
    assert_eq!(typo["total"], 0);
    let named = typo["unmatched_filters"].as_array().unwrap();
    assert_eq!(named.len(), 1);
    assert_eq!(named[0]["filter"], "language");
    assert_eq!(named[0]["value"], "cobolol");

    // A real filter that simply matches nothing reports no unmatched filter.
    let genuine = json(&run(
        &root,
        &["ast", "zzzz-no-such-symbol", "--language", "rust", "--json"],
    ));
    assert_eq!(genuine["total"], 0);
    assert!(genuine["unmatched_filters"].as_array().unwrap().is_empty());

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_hit_carries_the_extraction_that_produced_it() {
    let root = fixture("engine");
    build(&root);

    let hits = json(&run(&root, &["ast", "alpha", "--json"]));
    let matches = hits["matches"].as_array().unwrap();
    assert!(!matches.is_empty());
    for hit in matches {
        let engine = hit["engine"].as_str().unwrap_or("");
        // The file's recorded outcome, not "a parser was importable". A regex
        // fallback and a real parse must not arrive under one label.
        assert!(
            engine.starts_with("TreeSitter")
                || engine.starts_with("RegexFallback")
                || engine.starts_with("Unavailable")
                || engine.starts_with("NotApplicable"),
            "unexpected engine {engine:?}"
        );
        assert!(hit["parse_outcome"].is_string());
    }

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn facets_name_the_filters_this_generation_can_answer() {
    let root = fixture("facets");
    build(&root);

    let facets = json(&run(&root, &["ast", "--facets", "--json"]));
    let kinds = facets["kinds"].as_object().unwrap();
    let languages = facets["languages"].as_object().unwrap();
    assert!(kinds.contains_key("Function"), "{kinds:?}");
    assert!(languages.contains_key("python") && languages.contains_key("rust"));

    // Every facet it advertises must actually return rows, or the list is a
    // menu of filters that do not work.
    for kind in kinds.keys() {
        let hit = json(&run(
            &root,
            &["ast", "--kind", kind, "--limit", "1", "--json"],
        ));
        assert!(
            hit["total"].as_u64().unwrap() > 0,
            "advertised kind {kind:?} matches nothing"
        );
    }

    let _ = std::fs::remove_dir_all(&root);
}
