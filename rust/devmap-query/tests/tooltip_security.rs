//! Browser regression for the shared code-map page, run with the operator's
//! existing Node/Playwright/browser tools. No browser package is added to the
//! production dependency graph.
//!
//! The pages used to be force-graph canvases whose hover tooltip was the sink
//! worth attacking. The shared page has no tooltip: a name reaches the DOM only
//! as a text node in the title, node list, inspector and navigator, so this
//! drives those sinks instead (and the Hide notes & Markdown control).
use devmap_query::map_preview::{fingerprint_for, render_map_preview_html};
use devmap_query::viz::{render_html, VizOptions};
use serde_json::json;
use std::process::Command;

#[test]
#[ignore = "requires DEVMAP_BROWSER_NODE, DEVMAP_PLAYWRIGHT_MODULE, DEVMAP_BROWSER_EXECUTABLE, DEVMAP_TOOLTIP_FIXTURE_DIR"]
fn repository_labels_stay_inert_in_a_real_browser() {
    let output = std::path::PathBuf::from(
        std::env::var_os("DEVMAP_TOOLTIP_FIXTURE_DIR")
            .expect("set a browser artifact output directory"),
    );
    std::fs::create_dir_all(&output).unwrap();
    // Local sentinels only: these mutate a flag in an isolated test page.
    let hostile = "<img src='data:image/png,invalid' onerror='window.__markupExecuted=true'><svg onload='window.__markupExecuted=true'></svg>&\"'";
    let mut paths = Vec::new();
    for (case, label) in [("hostile", hostile), ("ordinary", "src/ordinary.rs")] {
        // A Markdown file beside the named one gives "Hide notes & Markdown"
        // something to hide; the probe asserts the count drops from 2 to 1.
        let graph = json!({"nodes":[
            {"id":"node", "kind":"file", "name":label, "path":label},
            {"id":"docs/README.md", "kind":"file", "name":"README.md", "path":"docs/README.md"}
        ],"edges":[]});
        let map = json!({"subsystems":[{"area":label}],"files":[{"path":format!("{label}/a.rs"),"language":"rust"}]});
        for (view, html) in [
            ("graph", render_html(&graph, &VizOptions::default())),
            ("map", render_map_preview_html(&map, &fingerprint_for(&map))),
        ] {
            let path = output.join(format!("{case}-{view}.html"));
            std::fs::write(&path, html).unwrap();
            paths.push(path);
        }
    }
    let mut command = Command::new(
        std::env::var_os("DEVMAP_BROWSER_NODE").expect("set the existing Node executable"),
    );
    command
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/support/page_probe.cjs"
        ))
        .args(paths);
    let result = devmap_extract::subprocess::run_bounded(
        &mut command,
        devmap_extract::subprocess::Bounds {
            deadline: std::time::Duration::from_secs(45),
            stdout_cap: 32768,
            stderr_cap: 32768,
        },
    )
    .unwrap();
    assert!(
        result.status.success(),
        "browser regression failed:\n{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    println!("{}", String::from_utf8_lossy(&result.stdout));
}
