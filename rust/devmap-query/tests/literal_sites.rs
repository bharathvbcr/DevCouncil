//! A string written through a const is found at the const and at its uses.
//!
//! `session.edit` and `session.command` are private consts. The call graph has
//! no edge for them. The literal index has to reach the use inside
//! `ToolCall::action` as well as the direct `session.spawn` write.

use devmap_extract::extract_file;
use devmap_query::StoreQueryEngine;
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};
use std::path::PathBuf;

const SOURCE: &str = r#"
struct ToolCall {
    tool: String,
}

const EDIT: &str = "session.edit";
const COMMAND: &str = "session.command";

impl ToolCall {
    pub fn action(&self) -> &'static str {
        match self.tool.as_str() {
            "Bash" => COMMAND,
            _ => EDIT,
        }
    }
}

fn spawn_session_inner() {
    let action = "session.spawn".to_string();
}
"#;

fn scratch() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("devmap-literals-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

#[test]
fn a_prefix_reaches_a_const_use_and_a_direct_write() {
    let root = scratch();
    std::fs::write(root.join("actions.rs"), SOURCE).unwrap();
    let extraction = extract_file("actions.rs", SOURCE);
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&extraction));
    let resolution = resolver
        .resolve_all(std::slice::from_ref(&extraction))
        .unwrap();
    let analysis = devmap_analyze::analyze(std::slice::from_ref(&extraction), &resolution);
    let store = Store::open_in_memory().unwrap();
    store
        .save_generation_with_opts(
            std::slice::from_ref(&extraction),
            &resolution,
            &analysis,
            GenerationWriteOpts {
                repo_root: Some(root.to_string_lossy().into_owned()),
                ..GenerationWriteOpts::default()
            },
        )
        .unwrap();
    let engine = StoreQueryEngine::new(&store);
    let report = engine.literals("session.", false, 8_000).unwrap();
    assert_eq!(report.shown + report.hidden, report.total);
    assert!(!report.exact);

    let action = |value: &str| {
        report.items.iter().any(|site| {
            site.file_path.ends_with("actions.rs")
                && site.value == value
                && site.qualified_name.contains("ToolCall")
                && site.qualified_name.contains("action")
        })
    };
    assert!(action("session.edit"), "const EDIT's use: {report:?}");
    assert!(action("session.command"), "const COMMAND's use: {report:?}");
    assert!(
        report.items.iter().any(|site| {
            site.value == "session.spawn"
                && site.qualified_name.contains("spawn_session_inner")
        }),
        "direct write: {report:?}"
    );
    assert!(
        report.total >= 3,
        "the const uses and the direct write are in the total: {report:?}"
    );

    let exact = engine.literals("session.spawn", true, 8_000).unwrap();
    assert_eq!(exact.shown + exact.hidden, exact.total);
    assert!(exact.items.iter().all(|site| site.value == "session.spawn"));
    assert!(exact.items.iter().any(|site| site.qualified_name.contains("spawn_session_inner")));
    assert!(!exact.items.iter().any(|site| site.value.contains("edit")));

    let starved = engine.literals("session.", false, 1).unwrap();
    assert_eq!(starved.total, report.total);
    assert_eq!(starved.shown + starved.hidden, starved.total);
    assert!(starved.hidden > 0, "budget 1 withholds sites: {starved:?}");
}
