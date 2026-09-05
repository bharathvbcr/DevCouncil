//! What `--version` says, and what `--json` promises on every exit.
//!
//! These are the two surfaces a *program* reads rather than a person: the
//! Python seam parses the version line to decide which kernel binary to run,
//! and every `--json` caller reads one line off stdout and parses it. Both were
//! stated only by convention, and both had a path that broke the convention —
//! a version line with two different numbers called "schema", and a `--json`
//! failure that printed nothing at all to stdout.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn devmap() -> String {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap").to_string_lossy().into_owned()
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(devmap())
        .args(args)
        .current_dir(root)
        .output()
        .unwrap_or_else(|error| panic!("devmap {args:?}: {error}"))
}

fn fixture(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "devmap-json-contract-{name}-{}-{}",
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

/// Exactly one line of valid JSON on stdout, and nothing else there.
fn one_json_line(output: &Output, what: &str) -> serde_json::Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines.len(),
        1,
        "{what}: --json must print exactly one line on stdout, got {} \
         line(s):\n{stdout}\n--- stderr ---\n{}",
        lines.len(),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_str(lines[0]).unwrap_or_else(|error| {
        panic!("{what}: stdout line is not JSON ({error}): {}", lines[0]);
    })
}

/// The version line names both numbers called "schema", and says which is which.
///
/// K3 put the store schema in the version line because the package version
/// (`0.1.0` on every build ever made) cannot tell a caller whether the binary
/// can open the store in hand. That left `devmap 0.1.0 (schema 13)` beside a
/// `code_graph.json` whose first field reads `"schema_version": 2` — two
/// different numbers, one word, and no way to tell which was meant without
/// reading the source.
#[test]
fn the_version_line_names_both_schemas_and_says_which_is_which() {
    let root = fixture("version");
    let output = run(&root, &["--version"]);
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();

    assert!(
        text.contains("store schema"),
        "the store schema must be named as such: {text:?}"
    );
    assert!(
        text.contains("code graph schema"),
        "the artifact schema must be named as such: {text:?}"
    );
    assert!(
        text.contains(&format!(
            "store schema {}",
            devmap_store::CURRENT_SCHEMA_VERSION
        )),
        "the store number must be the one this binary writes: {text:?}"
    );
    assert!(
        text.contains(&format!(
            "code graph schema {}",
            devmap_query::CODE_GRAPH_SCHEMA_VERSION
        )),
        "the artifact number must be the one this binary writes: {text:?}"
    );

    // `src/devcouncil/devmap_health.py` parses this line by splitting on the
    // first occurrence of "schema" and taking the first integer after it. That
    // parser must keep reading the *store* schema, so the store number has to
    // stay first. Reproduced here rather than described, because a rewording
    // that breaks it is silent on the Rust side.
    let tail = text.split("schema").nth(1).expect("a schema is named");
    let first_number: String = tail
        .chars()
        .map(|c| if c.is_ascii_digit() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .next()
        .expect("a number follows")
        .to_string();
    assert_eq!(
        first_number.parse::<i32>().unwrap(),
        devmap_store::CURRENT_SCHEMA_VERSION,
        "the Python health probe reads the first number after the first \
         'schema' and must still get the store version: {text:?}"
    );
}

/// The no-op build carries `timings` like every other build result.
///
/// The unchanged early return is the most frequent build in the system — a
/// watcher hits it on almost every tick — and it was the one build shape with
/// no `timings` key at all, so the build a profiler most wants to see was the
/// one it could not. It is not an empty truth either: the path hashes every
/// file in the tree to prove nothing changed, and runs the reclaim decision.
#[test]
fn the_unchanged_build_reports_its_timings() {
    let root = fixture("timings");
    std::fs::write(root.join("a.py"), "def a():\n    return 1\n").unwrap();

    let cold = one_json_line(&run(&root, &["--json", "build", "."]), "cold build");
    assert!(
        cold["timings"]["total_seconds"].is_number(),
        "precondition: a cold build reports timings: {cold}"
    );

    let warm = one_json_line(&run(&root, &["--json", "build", "."]), "unchanged build");
    assert_eq!(warm["unchanged"], true, "{warm}");
    let stages = warm["timings"]["stages"]
        .as_array()
        .unwrap_or_else(|| panic!("the unchanged build must carry timings: {warm}"));
    assert!(
        !stages.is_empty(),
        "an empty stage list is the same silence with a key on it: {warm}"
    );
    assert!(
        warm["timings"]["total_seconds"].as_f64().unwrap_or(-1.0) >= 0.0,
        "{warm}"
    );
    // The reclaim runs on this path and is a timed stage, so it must appear:
    // its absence is how "the warm path costs nothing" went unmeasured. Looked
    // for at any nesting, because `timed` records a sub-stage under whichever
    // stage is open when it runs.
    fn names_a_stage(stages: &[serde_json::Value], wanted: &str) -> bool {
        stages.iter().any(|stage| {
            stage["stage"] == wanted
                || stage["sub"]
                    .as_array()
                    .is_some_and(|nested| names_a_stage(nested, wanted))
        })
    }
    assert!(
        names_a_stage(stages, "persist:vacuum"),
        "the warm path's reclaim is a timed stage: {warm}"
    );
}

/// Every numeric argument that reaches the engine is bounded at the boundary.
///
/// S-1 follow-up. `validate_request` refuses each of these over the IPC
/// transport; argv reached `StoreQueryEngine` unchecked, and the failures are
/// silent rather than loud — `--min-confidence nan` makes every `>=` in the
/// edge filter false, so the answer is an empty edge list that reads exactly
/// like "this symbol has no callers".
#[test]
fn out_of_range_numeric_arguments_are_refused_with_one_json_line() {
    let root = fixture("limits");
    std::fs::write(
        root.join("a.py"),
        "def helper():\n    return 1\n\ndef main():\n    return helper()\n",
    )
    .unwrap();
    run(&root, &["build", "."]);

    let cases: Vec<(&str, Vec<&str>, &str)> = vec![
        (
            "nan confidence",
            vec!["--json", "deps", "helper", "--min-confidence", "nan"],
            "min-confidence",
        ),
        (
            "negative confidence",
            vec!["--json", "deps", "helper", "--min-confidence=-1"],
            "min-confidence",
        ),
        (
            "confidence above one",
            vec!["--json", "deps", "helper", "--min-confidence", "2"],
            "min-confidence",
        ),
        (
            "infinite confidence",
            vec!["--json", "deps", "helper", "--min-confidence", "inf"],
            "min-confidence",
        ),
        (
            "zero budget",
            vec!["--json", "search", "helper", "--budget", "0"],
            "budget",
        ),
        (
            "budget past the ceiling",
            vec!["--json", "search", "helper", "--budget", "100001"],
            "budget",
        ),
        (
            "zero depth",
            vec!["--json", "impact", "helper", "--depth", "0"],
            "depth",
        ),
        (
            "huge depth",
            vec![
                "--json",
                "impact",
                "helper",
                "--depth",
                "18446744073709551615",
            ],
            "depth",
        ),
        (
            "zero explore limit",
            vec!["--json", "explore", "helper", "--limit", "0"],
            "limit",
        ),
        (
            "zero history window",
            vec!["--json", "history", "--last", "0"],
            "last",
        ),
    ];

    for (label, args, expected) in cases {
        let output = run(&root, &args);
        assert!(
            !output.status.success(),
            "{label}: an out-of-range argument must fail, not be clamped: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        let payload = one_json_line(&output, label);
        let message = payload["error"].as_str().unwrap_or_default();
        assert!(
            message.contains(expected),
            "{label}: the error must name the argument it is about, got {message:?}"
        );
    }

    // The positive control: the boundary values themselves are accepted, or the
    // fix is "refuse everything" wearing the same test result.
    for (label, args) in [
        (
            "confidence 0",
            vec!["--json", "deps", "helper", "--min-confidence", "0"],
        ),
        (
            "confidence 1",
            vec!["--json", "deps", "helper", "--min-confidence", "1"],
        ),
        (
            "budget 1",
            vec!["--json", "search", "helper", "--budget", "1"],
        ),
        (
            "budget at the ceiling",
            vec!["--json", "search", "helper", "--budget", "100000"],
        ),
        (
            "depth 1",
            vec!["--json", "impact", "helper", "--depth", "1"],
        ),
        (
            "depth at the ceiling",
            vec!["--json", "impact", "helper", "--depth", "64"],
        ),
    ] {
        let output = run(&root, &args);
        assert!(
            output.status.success(),
            "{label}: a boundary value must be accepted: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        one_json_line(&output, label);
    }
}

/// `--json` prints one line of JSON on stdout on the failing paths too.
///
/// Missing store, wrong schema, a bad target, an unreadable file: each is a
/// case a caller has to handle, and each used to leave stdout *empty* with a
/// human sentence on stderr. A caller reading one line and parsing it saw an
/// empty string and could not tell a failure from a command that answered
/// nothing.
#[test]
fn every_json_exit_path_prints_one_json_line_and_nothing_else() {
    let root = fixture("exits");
    std::fs::write(root.join("a.py"), "def helper():\n    return 1\n").unwrap();

    // No store yet: an answer, not an error — and one line of it.
    let missing = run(&root, &["--json", "status"]);
    let payload = one_json_line(&missing, "status with no store");
    assert_eq!(payload["is_fresh"], false);
    assert!(payload["degraded_reason"]
        .as_str()
        .unwrap_or_default()
        .contains("no devmap store"));

    // A store this binary cannot read: `user_version = 2` is the *Python*
    // engine's schema, the one shape the kernel is guaranteed to refuse.
    let foreign = root.join("foreign.sqlite");
    {
        let conn = rusqlite::Connection::open(&foreign).unwrap();
        conn.pragma_update(None, "user_version", 2).unwrap();
    }
    let wrong_schema = run(&root, &["--db", "foreign.sqlite", "--json", "status"]);
    let payload = one_json_line(&wrong_schema, "status against a foreign schema");
    assert_eq!(payload["schema_outdated"], true, "{payload}");

    // Queries against a store that was never built must fail loudly, in JSON.
    for (label, args) in [
        ("search with no store", vec!["--json", "search", "helper"]),
        ("dead with no store", vec!["--json", "dead"]),
        (
            "manifest with no generation",
            vec!["--json", "manifest", "."],
        ),
    ] {
        let output = run(&root, &args);
        assert!(
            !output.status.success(),
            "{label}: querying a store that does not exist must fail"
        );
        let payload = one_json_line(&output, label);
        assert!(
            payload["error"].as_str().is_some_and(|m| !m.is_empty()),
            "{label}: the failure must carry a message: {payload}"
        );
    }

    // And a real failure after the store exists: `preview` against a file it
    // cannot read.
    run(&root, &["build", "."]);
    let unreadable = run(
        &root,
        &[
            "--json",
            "preview",
            "--file",
            "a.py",
            "--content",
            "no-such-file.py",
        ],
    );
    assert!(!unreadable.status.success());
    let payload = one_json_line(&unreadable, "preview of an unreadable file");
    assert!(
        payload["error"]
            .as_str()
            .unwrap_or_default()
            .contains("no-such-file.py"),
        "the failure must name the file: {payload}"
    );
}

/// K-A6: an empty store is not a fresh one.
///
/// `is_fresh` was `pending_count == 0` in both the CLI and the daemon, so the
/// file a crashed `devmap build` leaves behind — a migrated store with no
/// generation, and therefore nothing queued — reported `is_fresh: true` to
/// every health check that asked. Nothing to do is not the same as nothing to
/// do it to. `devmap_serve::index_is_fresh` / `freshness_degraded_reason` are
/// the one owner, shared with the daemon's `status`; this pins that the CLI
/// asks them rather than recomputing.
#[test]
fn a_store_with_no_generation_is_not_fresh() {
    let root = fixture("freshness");
    let db = root.join(".devcouncil/codeintel/devmap.sqlite");
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();
    // A real, migrated, empty store: exactly what a build that died before its
    // first commit leaves behind.
    drop(devmap_store::Store::open(&db).unwrap());

    let empty = one_json_line(
        &run(&root, &["--json", "status"]),
        "status of an empty store",
    );
    assert_eq!(
        empty["pending_count"], 0,
        "precondition: an empty store has nothing queued, which is what made \
         `pending_count == 0` read as fresh: {empty}"
    );
    assert_eq!(
        empty["generation_id"],
        serde_json::Value::Null,
        "precondition: no generation has been persisted: {empty}"
    );
    assert_eq!(
        empty["is_fresh"], false,
        "a store with no generation cannot answer anything, let alone freshly: {empty}"
    );
    assert!(
        empty["degraded_reason"]
            .as_str()
            .unwrap_or_default()
            .contains("holds no generation"),
        "and must say why: {empty}"
    );

    // Positive control: once a generation exists and nothing is queued, fresh.
    std::fs::write(root.join("a.py"), "def a():\n    return 1\n").unwrap();
    run(&root, &["build", "."]);
    let built = one_json_line(&run(&root, &["--json", "status"]), "status after a build");
    assert_eq!(built["is_fresh"], true, "{built}");
    assert_eq!(
        built["degraded_reason"],
        serde_json::Value::Null,
        "a healthy repository must keep reporting clean: {built}"
    );
}
