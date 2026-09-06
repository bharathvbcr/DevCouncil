//! The `routes` / `shape-check` / `api-impact` views, end to end.
//!
//! The unit tests in `devmap_query::api_routes` cover the logic. These prove
//! the two properties that only exist once a real index and a real process are
//! in the picture: that a route the resolver bound is actually visible to the
//! CLI, and that a scan which did not finish cannot reach a caller looking like
//! one that did.

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

/// A repository with one Flask route, its handler, and one client that calls it.
fn fixture(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "devmap-api-{label}-{}-{stamp}-{seq}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("api")).unwrap();
    std::fs::create_dir_all(root.join("web")).unwrap();
    // `@app.get`, not `@app.route`: the extractor's pattern names the verb, so
    // Flask's classic decorator is not one it binds. Using a shape the kernel
    // does not extract would make this test pass or fail on the fixture rather
    // than on the view.
    std::fs::write(
        root.join("api/server.py"),
        "from fastapi import FastAPI\n\
         app = FastAPI()\n\
         \n\
         @app.get('/api/users/{uid}')\n\
         def get_user(uid):\n\
         \x20   return {'name': 'x', 'email': 'y'}\n",
    )
    .unwrap();
    std::fs::write(
        root.join("web/client.js"),
        "async function load(id) {\n\
        \x20 const res = await fetch(`/api/users/${id}`);\n\
        \x20 const data = await res.json();\n\
        \x20 return data.name;\n\
         }\n",
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

fn build(root: &Path) {
    let output = run(root, &["--progress", "never", "build", "."]);
    assert!(
        output.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn json(output: &std::process::Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
        panic!(
            "not JSON ({err}): {}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

#[test]
fn a_route_the_resolver_bound_is_visible_with_its_handler_and_its_caller() {
    let root = fixture("bound");
    build(&root);

    let mapped = json(&run(&root, &["routes", ".", "--json"]));
    let routes = mapped["routes"].as_array().expect("routes array");
    assert!(!routes.is_empty(), "no routes: {mapped}");

    let route = &routes[0];
    assert_eq!(route["verb"], "GET");
    assert_eq!(route["path"], "/api/users/{uid}");
    assert_eq!(route["normalized_path"], "/api/users/*");
    assert_eq!(route["handlers"][0]["name"], "get_user");

    // The framework comes off the route node, and the view says where it came
    // from. This used to assert `framework.is_null()`, which was right while
    // `SymbolKind::Route` was never constructed and wrong the moment
    // `code_graph` began emitting route nodes.
    assert_eq!(
        route["framework"], "fastapi/flask",
        "the declaring framework is read off the route node: {route}"
    );
    assert_eq!(route["framework_resolution"], "node");
    assert!(
        route["node_ids"]
            .as_array()
            .is_some_and(|ids| ids.len() == 1),
        "the row names the route node it came from: {route}"
    );
    assert_eq!(mapped["capabilities"]["framework_available"], true);

    // Middleware still has no kernel source: there is no `Registers` edge kind,
    // so it is null by name rather than an empty list that would read as "this
    // route has none".
    assert!(route["middleware"].is_null());
    assert_eq!(mapped["capabilities"]["middleware_available"], false);

    // The scan reached both files and says what "complete" covers.
    assert_eq!(mapped["scan"]["complete"], true);
    assert!(mapped["scan"]["scope"]
        .as_str()
        .unwrap()
        .contains("not the whole working tree"));

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_scan_that_stopped_early_cannot_report_a_route_as_uncalled() {
    let root = fixture("budget");
    build(&root);

    let full = json(&run(
        &root,
        &["api-impact", "/api/users/{uid}", ".", "--json"],
    ));
    assert_eq!(full["found"], true);
    assert_eq!(full["scan"]["complete"], true);

    // The same question with the scan cut short. Whatever it finds, it must not
    // reach a risk band that reads as "safe to change".
    let starved = json(&run(
        &root,
        &[
            "api-impact",
            "/api/users/{uid}",
            ".",
            "--max-files",
            "0",
            "--json",
        ],
    ));
    assert_eq!(starved["scan"]["complete"], false);
    assert_eq!(starved["scan"]["files_read"], 0);
    assert_eq!(starved["consumers"].as_array().unwrap().len(), 0);
    assert_eq!(
        starved["risk"], "unknown",
        "an unfinished scan with no caller found must not report a risk band \
         that a finished one would earn"
    );
    assert!(starved["risk_reason"]
        .as_str()
        .unwrap()
        .contains("not evidence"));

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn shape_check_separates_agreement_from_having_nothing_to_compare() {
    let root = fixture("shape");
    build(&root);

    let checked = json(&run(&root, &["shape-check", ".", "--json"]));
    let checks = checked["checks"].as_array().expect("checks");
    assert!(!checks.is_empty(), "no checks: {checked}");

    // Every verdict is one of the four the module defines, and none of them is
    // a bare boolean that conflates "agrees" with "nothing was read".
    for check in checks {
        let verdict = check["verdict"].as_str().unwrap_or("");
        assert!(
            matches!(
                verdict,
                "agrees" | "agrees_on_what_was_scanned" | "mismatch" | "no_consumer_keys"
            ),
            "unexpected verdict {verdict:?}"
        );
        if verdict == "no_consumer_keys" {
            assert_eq!(check["mismatch"], false);
            assert!(
                check["consumer_keys"].as_array().unwrap().is_empty(),
                "a route with keys to compare must not report having none"
            );
        }
    }

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_text_output_names_the_scan_when_it_did_not_finish() {
    let root = fixture("text");
    build(&root);

    let quiet = run(&root, &["routes", "."]);
    let text = String::from_utf8_lossy(&quiet.stdout);
    assert!(
        !text.contains("scan incomplete"),
        "a complete scan must not warn: {text}"
    );

    let starved = run(&root, &["routes", ".", "--max-files", "0"]);
    let text = String::from_utf8_lossy(&starved.stdout);
    assert!(
        text.contains("scan incomplete") && text.contains("lower bounds"),
        "an incomplete scan must say so in the human output too: {text}"
    );

    let _ = std::fs::remove_dir_all(&root);
}
