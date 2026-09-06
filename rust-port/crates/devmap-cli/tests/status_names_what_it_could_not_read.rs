//! `devmap status` names the files behind its coverage numbers.
//!
//! `degraded_reason` has always carried three numbers — "2 file(s) failed to
//! parse, 1 recovered by pattern (no calls extracted), 1 refused by discovery
//! and never read at all" — and no surface anywhere carried a path. On this
//! repository the refusal is a 30.6 MB vendored `parser.c` against a 1 MiB
//! ceiling, which is the *correct* verdict; the gap was that an operator could
//! not tell that from a broken indexer without opening the database by hand,
//! and the Python doctor prints the same three numbers for the same reason.
//!
//! Each list is capped at `devmap_store::COVERAGE_GAP_SAMPLE` and carries
//! `{shown, total, truncated}`, because a list that stops at fifty without
//! saying so reads exactly like a corpus with fifty holes in it.

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

fn run(root: &Path, args: &[&str]) -> (String, String) {
    let out = Command::new(devmap())
        .args(args)
        .current_dir(root)
        .output()
        .unwrap_or_else(|error| panic!("devmap {args:?}: {error}"));
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn json(root: &Path, args: &[&str]) -> serde_json::Value {
    let (stdout, stderr) = run(root, args);
    serde_json::from_str(stdout.trim()).unwrap_or_else(|error| {
        panic!("devmap {args:?} stdout is not JSON ({error}): {stdout}\n--- stderr ---\n{stderr}")
    })
}

fn fixture(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "devmap-status-names-{name}-{}-{}",
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

/// One file over the source ceiling, one with no linked grammar, one ordinary.
fn write_a_corpus_with_two_holes(root: &Path) {
    std::fs::write(root.join("lib.py"), "def helper():\n    return 42\n").unwrap();
    let pad = format!("# {}\n", "x".repeat(78));
    let mut body = String::from("from lib import helper\n\n\ndef main():\n    return helper()\n");
    while body.len() as u64 <= devmap_extract::MAX_SOURCE_BYTES {
        body.push_str(&pad);
    }
    std::fs::write(root.join("app.py"), body).unwrap();
    std::fs::write(
        root.join("svc.proto"),
        "syntax = \"proto3\";\nmessage Ping { string id = 1; }\n",
    )
    .unwrap();
}

#[test]
fn status_names_the_refused_and_the_unparsed_paths() {
    let root = fixture("holes");
    write_a_corpus_with_two_holes(&root);

    let build = json(&root, &["--json", "build", "."]);
    assert_eq!(
        build["discovery_refused_files"], 1,
        "fixture precondition: the oversized caller is refused: {build}"
    );

    let status = json(&root, &["--json", "status"]);
    let gaps = &status["coverage_gaps"];
    assert!(
        !gaps.is_null(),
        "`degraded_reason` states three numbers and nothing stated the paths: {status}"
    );

    let refused = &gaps["discovery_refused"];
    assert_eq!(refused["total"], 1, "{status}");
    assert_eq!(refused["shown"], 1, "{status}");
    assert_eq!(refused["truncated"], false, "{status}");
    assert_eq!(
        refused["paths"][0]["path"], "app.py",
        "the path is the whole point of the list: {status}"
    );
    assert!(
        refused["paths"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("source ceiling"),
        "the verdict is what distinguishes a correct refusal from a broken \
         indexer: {status}"
    );

    let recovered = &gaps["pattern_recovered"];
    assert_eq!(recovered["total"], 1, "{status}");
    assert_eq!(
        recovered["paths"][0]["path"], "svc.proto",
        "a `.proto` has no linked grammar here, so its declarations are \
         recovered by pattern and it contributes no calls: {status}"
    );

    // The off direction. `lib.py` parsed, so it appears in no list — a marker
    // that names every file is worth as much as one that names none.
    let named: Vec<&str> = ["discovery_refused", "parse_failed", "pattern_recovered"]
        .iter()
        .flat_map(|key| gaps[*key]["paths"].as_array().unwrap())
        .map(|row| row["path"].as_str().unwrap())
        .collect();
    assert!(!named.contains(&"lib.py"), "{named:?}");

    // The number in `degraded_reason` and the length of the list are one
    // measurement, not two that happen to agree.
    let reason = status["degraded_reason"].as_str().unwrap_or_default();
    assert!(
        reason.contains("1 refused by discovery"),
        "the prose and the inventory must describe the same corpus: {status}"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// A corpus with no holes reports empty lists, not absent ones.
///
/// `null` here would mean "not measured" and is the answer a store with no
/// generation gets. A built generation that read everything has a measurement,
/// and it is three empty lists.
#[test]
fn a_corpus_with_nothing_to_report_reports_nothing_rather_than_saying_nothing() {
    let root = fixture("clean");
    std::fs::write(root.join("lib.py"), "def helper():\n    return 42\n").unwrap();
    std::fs::write(
        root.join("app.py"),
        "from lib import helper\n\n\ndef main():\n    return helper()\n",
    )
    .unwrap();
    // Skipped as `NonSource` on every walk. If that counted, no repository
    // would ever report a clean corpus again.
    std::fs::write(root.join("README.md"), "# notes\n").unwrap();

    json(&root, &["--json", "build", "."]);
    let status = json(&root, &["--json", "status"]);
    for key in ["discovery_refused", "parse_failed", "pattern_recovered"] {
        assert_eq!(status["coverage_gaps"][key]["total"], 0, "{key}: {status}");
        assert_eq!(
            status["coverage_gaps"][key]["paths"]
                .as_array()
                .unwrap()
                .len(),
            0,
            "{key}: {status}"
        );
    }
    assert!(status["degraded_reason"].is_null(), "{status}");

    let _ = std::fs::remove_dir_all(&root);
}
