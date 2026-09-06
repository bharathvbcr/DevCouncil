//! W3.3, the cross-file half: a file only a dynamic reference reaches is wired.
//!
//! `unwired_candidates` walks `Imports` edges. A lazily imported plugin, a
//! code-split route and a worker entry point produce no such edge, so every one
//! of them was reported as unwired — confidently, on every build, with nothing
//! in the answer saying the kernel had not looked for the reference that
//! actually reaches them.
//!
//! The Python wiring module has cleared these since it was written. That is the
//! duplication W3.3 removes: two implementations deciding what "wired" means,
//! and only one of them knowing about dynamic references.
//!
//! Every test asserts both directions, because a clearing rule that is always
//! on reports nothing unwired at all — a check silently switched off rather
//! than one that failed.

use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
use devmap_query::{generate_code_graph_json, FreshnessInfo};
use devmap_resolve::Resolver;

/// `unwired_candidates` as the published artifact carries it.
///
/// Read out of the graph JSON rather than by calling the internal function, so
/// these tests exercise the value a consumer actually sees. A rule that worked
/// in the function and was dropped on the way to the artifact would pass an
/// internal test and change nothing for anybody.
fn unwired(extractions: &[Extraction]) -> Vec<String> {
    let mut resolver = Resolver::new();
    resolver.index_extractions(extractions);
    let resolution = resolver.resolve_all(extractions);
    let analysis = devmap_analyze::analyze(extractions, &resolution);
    let freshness = FreshnessInfo {
        head_sha: "test-head".into(),
        generation_id: 1,
        pending_count: 0,
        stamped: Default::default(),
    };
    let json =
        generate_code_graph_json(extractions, &analysis, &resolution.edges, &freshness, None)
            .expect("code graph builds");
    let value: serde_json::Value = serde_json::from_str(&json).expect("graph is JSON");
    value["unwired_candidates"]
        .as_array()
        .expect("unwired_candidates is an array")
        .iter()
        .filter_map(|entry| entry.as_str().map(str::to_string))
        .collect()
}

fn corpus(loader: &str) -> Vec<Extraction> {
    vec![
        // A declared entry root, so the scan has somewhere to start.
        extract_file("setup.py", "from setuptools import setup\n\nsetup()\n"),
        extract_file("pkg/loader.py", loader),
        // Reached only by whatever `loader` does — never by an import edge.
        extract_file("pkg/plugins/alpha.py", "def run():\n    return 1\n"),
    ]
}

/// OFF, and the test that makes the next one mean something.
///
/// Without it, a clearing rule that fired unconditionally would satisfy every
/// other assertion in this file.
#[test]
fn a_file_nothing_references_is_still_unwired() {
    let paths = unwired(&corpus("def load():\n    return None\n"));
    assert!(
        paths.iter().any(|p| p == "pkg/plugins/alpha.py"),
        "a genuinely unreferenced file must still be reported: {paths:?}"
    );
}

/// ON: `importlib.import_module` reaches it, so it is not unwired.
#[test]
fn an_importlib_reference_clears_the_file_it_names() {
    let paths = unwired(&corpus(
        "import importlib\n\n\ndef load():\n    return importlib.import_module('pkg.plugins.alpha')\n",
    ));
    assert!(
        !paths.iter().any(|p| p == "pkg/plugins/alpha.py"),
        "the file is reached by a dynamic import and must not be reported: {paths:?}"
    );
}

/// A relative JS specifier resolves against the file that wrote it.
#[test]
fn a_code_split_route_is_not_unwired() {
    let extractions = vec![
        extract_file(
            "package.json",
            "{\"name\":\"x\",\"main\":\"src/index.ts\"}\n",
        ),
        extract_file(
            "src/index.ts",
            "export const routes = [{ load: () => import('./routes/Settings') }];\n",
        ),
        extract_file(
            "src/routes/Settings.tsx",
            "export const Settings = () => null;\n",
        ),
    ];
    let paths = unwired(&extractions);
    assert!(
        !paths.iter().any(|p| p == "src/routes/Settings.tsx"),
        "a lazily loaded route is reachable: {paths:?}"
    );

    // OFF: the same corpus without the dynamic import reports it.
    let mut without = extractions;
    without[1] = extract_file("src/index.ts", "export const routes = [];\n");
    let paths = unwired(&without);
    assert!(
        paths.iter().any(|p| p == "src/routes/Settings.tsx"),
        "without the reference the file really is unwired: {paths:?}"
    );
}

/// A reference from a test file is not production wiring.
///
/// The same rule the import-edge walk already applies. Without it, a file used
/// only by its own test would clear itself, which is exactly the case the
/// unwired check exists to find.
#[test]
fn a_dynamic_reference_from_a_test_does_not_clear_production_code() {
    let extractions = vec![
        extract_file("setup.py", "from setuptools import setup\n\nsetup()\n"),
        extract_file(
            "tests/test_alpha.py",
            "import importlib\n\n\ndef test_it():\n    importlib.import_module('pkg.plugins.alpha')\n",
        ),
        extract_file("pkg/plugins/alpha.py", "def run():\n    return 1\n"),
    ];
    let paths = unwired(&extractions);
    assert!(
        paths.iter().any(|p| p == "pkg/plugins/alpha.py"),
        "a test-only dynamic reference must not clear production code: {paths:?}"
    );
}

/// The `allow-unwired` marker exempts the file that carries it, and only that file.
#[test]
fn the_allow_unwired_marker_exempts_only_its_own_file() {
    let marker = devmap_extract::wiring::ALLOW_UNWIRED;
    let extractions = vec![
        extract_file("setup.py", "from setuptools import setup\n\nsetup()\n"),
        extract_file(
            "pkg/plugins/alpha.py",
            &format!("# {marker} — loaded by the runtime\n\ndef run():\n    return 1\n"),
        ),
        extract_file("pkg/plugins/beta.py", "def run():\n    return 2\n"),
    ];
    let paths = unwired(&extractions);
    assert!(
        !paths.iter().any(|p| p == "pkg/plugins/alpha.py"),
        "the marked file must be exempt: {paths:?}"
    );
    assert!(
        paths.iter().any(|p| p == "pkg/plugins/beta.py"),
        "an unmarked sibling must still be reported — a marker that exempts the \
         whole corpus is a check switched off: {paths:?}"
    );
}
