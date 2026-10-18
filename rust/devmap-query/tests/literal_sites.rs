//! A string written through a const is found at the const and at its uses.
//!
//! `session.edit` and `session.command` are private consts. The call graph has
//! no edge for them. The literal index has to reach the use inside
//! `ToolCall::action` as well as the direct `session.spawn` write.

use devmap_extract::extract_file;
use devmap_query::StoreQueryEngine;
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};
use std::path::{Path, PathBuf};

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

/// One directory per test: the tests share a process, so a dir keyed on the
/// pid alone would be wiped by whichever test started second.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("devmap-literals-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

/// A store holding one extracted file, written as a build would write it.
fn store_of(root: &Path, file: &str, source: &str) -> Store {
    std::fs::write(root.join(file), source).unwrap();
    let extraction = extract_file(file, source);
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
    store
}

#[test]
fn a_substring_reaches_a_const_use_and_a_direct_write() {
    let root = scratch("const-use");
    let store = store_of(&root, "actions.rs", SOURCE);
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
            site.value == "session.spawn" && site.qualified_name.contains("spawn_session_inner")
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
    assert!(exact
        .items
        .iter()
        .any(|site| site.qualified_name.contains("spawn_session_inner")));
    assert!(!exact.items.iter().any(|site| site.value.contains("edit")));

    let starved = engine.literals("session.", false, 1).unwrap();
    assert_eq!(starved.total, report.total);
    assert_eq!(starved.shown + starved.hidden, starved.total);
    assert!(starved.hidden > 0, "budget 1 withholds sites: {starved:?}");
}

/// The two lines from the report: a dataset write whose key is an identifier,
/// and a CSS custom property passed as a string.
const AMBIENCE: &str = "function seed(pal) {
  document.documentElement.dataset.timeOfDay = pal.mode;
  document.documentElement.style.setProperty('--sky-time', pal.mode);
}
";

#[test]
fn a_query_matches_anywhere_inside_the_value_not_only_at_its_start() {
    let root = scratch("substring");
    let store = store_of(&root, "ambience.js", AMBIENCE);
    let engine = StoreQueryEngine::new(&store);

    // The fixture extracts: the whole value finds the call's argument.
    let whole = engine.literals("--sky-time", true, 8_000).unwrap();
    assert_eq!(whole.total, 1, "{whole:?}");
    assert_eq!(whole.items[0].line, 3, "{whole:?}");

    // `sky-time` is inside `--sky-time`, not at its start.
    let inner = engine.literals("sky-time", false, 8_000).unwrap();
    assert!(!inner.exact);
    assert_eq!(inner.total, 1, "{inner:?}");
    assert_eq!(inner.shown + inner.hidden, inner.total);
    let site = &inner.items[0];
    assert_eq!(site.file_path, "ambience.js");
    assert_eq!(site.line, 3, "the setProperty line: {inner:?}");
    assert_eq!(site.value, "--sky-time");
    assert!(site.qualified_name.contains("seed"), "{inner:?}");

    // Mid-string, the tail, and the head all reach the one site.
    for query in ["ky-ti", "time", "--sky"] {
        let report = engine.literals(query, false, 8_000).unwrap();
        assert_eq!(report.total, 1, "{query:?}: {report:?}");
        assert_eq!(report.items[0].value, "--sky-time", "{query:?}");
    }

    // The dataset key is a property name, not a string: line 2 has no literal.
    for query in ["timeOfDay", "dataset", "mode"] {
        let report = engine.literals(query, false, 8_000).unwrap();
        assert_eq!(report.total, 0, "{query:?} is no literal: {report:?}");
    }

    // The comparison stays binary.
    let upper = engine.literals("SKY-TIME", false, 8_000).unwrap();
    assert_eq!(upper.total, 0, "case-sensitive: {upper:?}");

    // Exact still needs the whole value.
    let partial = engine.literals("sky-time", true, 8_000).unwrap();
    assert!(partial.exact);
    assert_eq!(partial.total, 0, "exact is equality: {partial:?}");

    // `%` and `_` are text, not LIKE wildcards.
    for query in ["%", "_", "sky%time", "sky_time"] {
        let report = engine.literals(query, false, 8_000).unwrap();
        assert_eq!(
            report.total, 0,
            "{query:?} is matched literally: {report:?}"
        );
    }
}
