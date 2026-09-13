//! Browser regression, run with the operator's existing Node/Playwright/browser
//! tools. No browser package is added to the production dependency graph.
use devmap_query::map_preview::{fingerprint_for, render_map_preview_html};
use devmap_query::viz::{render_html, VizOptions};
use serde_json::json;
use std::process::Command;

#[test]
#[ignore = "requires DEVMAP_BROWSER_NODE, DEVMAP_PLAYWRIGHT_MODULE, DEVMAP_BROWSER_EXECUTABLE, DEVMAP_TOOLTIP_FIXTURE_DIR"]
fn repository_labels_stay_inert_during_real_browser_hover() {
    let output = std::path::PathBuf::from(
        std::env::var_os("DEVMAP_TOOLTIP_FIXTURE_DIR")
            .expect("set a browser artifact output directory"),
    );
    std::fs::create_dir_all(&output).unwrap();
    // Local sentinels only: these mutate a flag in an isolated test page.
    let hostile = "<img src='data:image/png,invalid' onerror='window.__tooltipExecuted=true'><svg onload='window.__tooltipExecuted=true'></svg>&\"'";
    let mut paths = Vec::new();
    for (case, label) in [("hostile", hostile), ("ordinary", "src/ordinary.rs")] {
        let graph =
            json!({"nodes":[{"id":"node", "kind":"file", "name":label, "path":label}],"edges":[]});
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
            "/tests/support/tooltip_probe.cjs"
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
