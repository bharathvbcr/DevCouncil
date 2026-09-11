use devmap_query::viz::{build_payload, render_html, VizOptions};
use devmap_query::{build_preview_payload, fingerprint_for, render_map_preview_html};
use serde_json::json;

#[test]
fn documentation_visibility_classifies_files_and_owned_symbols_without_dropping_data() {
    let cases = [
        ("README.MD", "", true),
        ("guide.mdx", "", true),
        ("guide.markdown", "", true),
        ("guide.mdown", "", true),
        ("guide.mkd", "", true),
        ("guide.mkdn", "", true),
        ("guide.rst", "", true),
        ("guide.adoc", "", true),
        ("NOTES.txt", "", true),
        ("NOTES", "", true),
        ("opaque", "Markdown", true),
        ("notes/editor.ts", "typescript", false),
        ("docs/example.rs", "rust", false),
        ("readme.md.ts", "typescript", false),
        ("src/md/parser.rs", "rust", false),
    ];
    let nodes: Vec<_> = cases.iter().enumerate().map(|(i, (path, language, _))|
        json!({"id": format!("n{i}"), "path": path, "kind": "file", "language": language})
    ).collect();
    let graph = json!({"nodes": nodes, "edges": []});
    let payload = build_payload(&graph, &VizOptions::default());
    assert_eq!(payload["nodes"].as_array().unwrap().len(), cases.len());
    for (i, (path, _, expected)) in cases.iter().enumerate() {
        let node = payload["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["id"] == format!("n{i}"))
            .unwrap();
        assert_eq!(node["documentation"], json!(expected), "{path}");
    }
    let symbols = build_payload(
        &json!({"nodes": [
        {"id":"docs\\guide.MD::heading", "path":"docs\\guide.MD", "kind":"heading"},
        {"id":"note", "kind":"note"},
        {"id":"doc", "kind":"doc"},
        {"id":"fn", "path":"notes/editor.ts", "kind":"function"}
    ], "edges":[]}),
        &VizOptions {
            symbols: true,
            ..VizOptions::default()
        },
    );
    for node in symbols["nodes"].as_array().unwrap() {
        assert_eq!(node["documentation"], json!(node["id"] != "fn"));
    }
    assert!(render_html(&graph, &VizOptions::default()).contains("Hide notes &amp; Markdown"));
}

#[test]
fn documentation_visibility_uses_full_file_attribution_for_subsystems() {
    let mut files: Vec<_> = (0..30)
        .map(|i| json!({"path":format!("mixed/{i}.md"),"language":"markdown"}))
        .collect();
    files.extend([
        json!({"path":"mixed/last.rs","language":"rust"}),
        json!({"path":"notes/idea.txt","kind":"note"}),
        json!({"path":"notes/editor/lib.rs","language":"rust"}),
        json!({"path":"docs/README.MDX"}),
    ]);
    let map = json!({"files":files,"subsystems":[
        {"area":"mixed"}, {"area":"notes"}, {"area":"notes/editor"}, {"area":"docs"}, {"area":"unknown"}
    ]});
    let payload = build_preview_payload(&map);
    for node in payload["nodes"].as_array().unwrap() {
        let expected = node["id"] == "notes" || node["id"] == "docs";
        assert_eq!(node["documentation"], json!(expected), "{}", node["id"]);
    }
    assert!(
        payload.get("files").is_none(),
        "the inventory must stay out of the preview"
    );
    assert!(
        render_map_preview_html(&map, &fingerprint_for(&map)).contains("Hide notes &amp; Markdown")
    );
}
