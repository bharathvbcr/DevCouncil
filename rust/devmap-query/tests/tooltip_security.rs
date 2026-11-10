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
    run_probe(&paths);
}

/// Run `page_probe.cjs` over rendered pages, failing with its own report.
fn run_probe(paths: &[std::path::PathBuf]) {
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
            deadline: std::time::Duration::from_secs(120),
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

/// `render_html`'s page with every node pinned to `(x, y)`: the renderer lays
/// nodes out itself, so a graph whose nodes coincide has to be handed to the
/// shell as a payload, which the page accepts (supplied coordinates win).
fn with_nodes_at(html: &str, x: f64, y: f64) -> String {
    let open = "id=\"devmap-payload\">";
    let start = html.find(open).expect("the page has a payload element") + open.len();
    let end = start
        + html[start..]
            .find("</script>")
            .expect("the payload is closed");
    let mut payload: serde_json::Value = serde_json::from_str(&html[start..end]).unwrap();
    for node in payload["nodes"].as_array_mut().unwrap() {
        node["x"] = json!(x);
        node["y"] = json!(y);
    }
    format!(
        "{}{}{}",
        &html[..start],
        serde_json::to_string(&payload).unwrap(),
        &html[end..]
    )
}

#[test]
#[ignore = "requires DEVMAP_BROWSER_NODE, DEVMAP_PLAYWRIGHT_MODULE, DEVMAP_BROWSER_EXECUTABLE, DEVMAP_TOOLTIP_FIXTURE_DIR"]
fn the_camera_filters_and_globe_behave_in_a_real_browser() {
    let output = std::path::PathBuf::from(
        std::env::var_os("DEVMAP_TOOLTIP_FIXTURE_DIR")
            .expect("set a browser artifact output directory"),
    )
    .join("camera");
    std::fs::create_dir_all(&output).unwrap();

    let one = json!({"nodes":[{"id":"src/only.rs","kind":"file","name":"only.rs","path":"src/only.rs"}],"edges":[]});
    // Five unconnected files, then pinned onto one coordinate.
    let several = json!({"nodes":(0..5).map(|i| {
        let path = format!("src/n{i}.rs");
        json!({"id":path,"kind":"file","name":format!("n{i}.rs"),"path":path})
    }).collect::<Vec<_>>(),"edges":[]});
    // Five areas of eighteen files each, a ring of dependencies inside each and
    // one across, and a Markdown file in each so Hide notes has work to do.
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    for area in ["core", "ui", "io", "docs", "tests"] {
        for i in 0..18 {
            let (path, language) = if area == "docs" || i == 0 {
                (format!("{area}/f{i}.md"), "markdown")
            } else {
                (format!("{area}/f{i}.rs"), "rust")
            };
            nodes.push(json!({"id":path,"kind":"file","name":format!("f{i}"),"path":path,"community":area,"language":language}));
            if i > 0 && area != "docs" {
                edges.push(json!({"source":path,"target":format!("{area}/f{}.rs", i % 17 + 1),"kind":"imports"}));
            }
        }
    }
    edges.push(json!({"source":"core/f1.rs","target":"ui/f2.rs","kind":"imports"}));
    let scene = json!({"nodes":nodes,"edges":edges});

    let mut paths = Vec::new();
    for (name, html) in [
        (
            "fit-single-graph.html",
            render_html(&one, &VizOptions::default()),
        ),
        (
            "fit-coincident-graph.html",
            with_nodes_at(&render_html(&several, &VizOptions::default()), 123.0, -45.0),
        ),
        (
            "scene-graph.html",
            render_html(&scene, &VizOptions::default()),
        ),
    ] {
        let path = output.join(name);
        std::fs::write(&path, html).unwrap();
        paths.push(path);
    }
    run_probe(&paths);
}
