//! Three behaviours the retired Python engine had tests for and the kernel did
//! not (IMPROVEMENTS.md, "Gaps found, not covered by a kernel test today").
//!
//! * **Subsystem inference on a tree that is not DevCouncil.** The kernel's
//!   subsystems come from Louvain communities over the import/call graph and
//!   the directories their members live in. Nothing pinned that a foreign
//!   layout — an Express service beside a Django app — yields subsystems named
//!   by *its* directories, rather than by anything this repository is shaped
//!   like.
//! * **Primary-stack `languages`.** The Python test
//!   (`tests/unit/test_primary_stack_map_languages.py`, removed in d232dea)
//!   pinned *coverage*: TypeScript, Go, Python, Rust, Swift, Kotlin, Markdown
//!   and HTML all detected. It did not pin a rank order. The kernel lists
//!   languages sorted by name, which is the R4 determinism rule; both halves
//!   are pinned here — every primary-stack language is advertised, and the
//!   order does not depend on file order.
//! * **Comments do not keep code alive.** The Python engine stripped comments
//!   with regexes before scanning for calls, and the risk was that a mention in
//!   a comment or string read as a call. Tree-sitter makes it moot, but nothing
//!   pinned it: a function named only in comments and string literals must
//!   still be dead, and a real call must still make it live.
//!
//! Each test was proved to discriminate on 2026-10-08 by breaking what it
//! guards and watching it go red, then restoring:
//!
//! * subsystems — `manifest.rs` keyed the area by the community's name
//!   (`community-N`) instead of its directory: "a two-service tree inferred no
//!   subsystem". (The DevCouncil-name loop is a guard against a leak, and
//!   cannot fail on a fixture that never contains those names; the directory
//!   assertions are the teeth.)
//! * languages — `languages` collected in arrival order (a `Vec`) instead of a
//!   `BTreeSet`: "languages are listed by name (R4)"; and `.kt` dropped from
//!   the language registry: "kotlin is missing".
//! * comments — one `// orphan_rs();` turned into a real call: red on the
//!   "named only in comments and strings" assertion.

#![cfg(feature = "parse")]

use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
use devmap_query::{generate_manifest_with_edges, FreshnessInfo};
use devmap_resolve::Resolver;
use serde_json::Value;

fn analyse(extractions: &[Extraction]) -> (devmap_analyze::model::AnalysisSummary, Value) {
    let mut resolver = Resolver::new();
    resolver.index_extractions(extractions);
    let resolution = resolver.resolve_all(extractions).unwrap();
    let analysis = devmap_analyze::analyze(extractions, &resolution);
    let (_, json) = generate_manifest_with_edges(
        extractions,
        &analysis,
        FreshnessInfo::new("head".into(), 1, 0),
        &resolution.edges,
        None,
    );
    (
        analysis,
        serde_json::from_str(&json).expect("the manifest is JSON"),
    )
}

fn extract_all(files: &[(&str, &str)]) -> Vec<Extraction> {
    files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect()
}

/// An Express API and a Django app, nothing DevCouncil-shaped about either.
const FOREIGN_TREE: &[(&str, &str)] = &[
    (
        "server/routes/users.js",
        "const { findUser, saveUser } = require('../models/user');\n\
         function getUser(id) { return findUser(id); }\n\
         function putUser(user) { return saveUser(user); }\n\
         module.exports = { getUser, putUser };\n",
    ),
    (
        "server/routes/orders.js",
        "const { findOrder } = require('../models/order');\n\
         const { getUser } = require('./users');\n\
         function getOrder(id) { return [findOrder(id), getUser(id)]; }\n\
         module.exports = { getOrder };\n",
    ),
    (
        "server/models/user.js",
        "function findUser(id) { return { id }; }\n\
         function saveUser(user) { return user; }\n\
         module.exports = { findUser, saveUser };\n",
    ),
    (
        "server/models/order.js",
        "function findOrder(id) { return { id }; }\nmodule.exports = { findOrder };\n",
    ),
    (
        "webapp/shop/views.py",
        "from shop.forms import CartForm\nfrom shop.pricing import total\n\n\
         def cart(request):\n    return total(CartForm(request).items())\n",
    ),
    (
        "webapp/shop/forms.py",
        "class CartForm:\n    def __init__(self, request):\n        self.request = request\n\n\
         \x20   def items(self):\n        return []\n",
    ),
    (
        "webapp/shop/pricing.py",
        "def total(items):\n    return sum(items)\n",
    ),
];

#[test]
fn subsystems_on_a_foreign_tree_are_named_by_its_own_directories() {
    let extractions = extract_all(FOREIGN_TREE);
    let (_, manifest) = analyse(&extractions);
    let areas: Vec<&str> = manifest["subsystems"]
        .as_array()
        .expect("subsystems array")
        .iter()
        .filter_map(|subsystem| subsystem["area"].as_str())
        .collect();
    assert!(
        !areas.is_empty(),
        "a two-service tree inferred no subsystem"
    );
    let directories: Vec<String> = FOREIGN_TREE
        .iter()
        .filter_map(|(path, _)| path.rsplit_once('/').map(|(dir, _)| dir.to_string()))
        .collect();
    for area in &areas {
        assert!(
            directories.iter().any(|dir| dir == area),
            "subsystem {area:?} is not a directory of this tree: {areas:?}"
        );
    }
    assert!(
        areas.iter().any(|area| area.starts_with("server/"))
            && areas.iter().any(|area| area.starts_with("webapp/")),
        "both services should surface as subsystems: {areas:?}"
    );
    for foreign in [
        "src/devcouncil",
        "backend",
        "rust",
        "go_orchestrator",
        "devmap",
    ] {
        assert!(
            !areas.iter().any(|area| area.contains(foreign)),
            "a DevCouncil name {foreign:?} leaked into a foreign tree: {areas:?}"
        );
    }
}

/// The Python test's sample, one file per primary-stack language.
const PRIMARY_STACK: &[(&str, &str)] = &[
    (
        "src/app.ts",
        "export function app(): number { return 1; }\n",
    ),
    ("cmd/server/main.go", "package main\n\nfunc main() {}\n"),
    ("src/pkg/main.py", "def main():\n    return 1\n"),
    ("crates/core/src/lib.rs", "pub fn core() -> u32 { 1 }\n"),
    ("Sources/App/App.swift", "func app() -> Int { return 1 }\n"),
    ("app/src/main/java/MainActivity.kt", "fun main() {}\n"),
    ("README.md", "# sample\n"),
    ("public/index.html", "<!doctype html><title>x</title>\n"),
];

fn languages(manifest: &Value) -> Vec<String> {
    manifest["languages"]
        .as_array()
        .expect("languages array")
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

#[test]
fn every_primary_stack_language_is_advertised_in_a_deterministic_order() {
    let forward = extract_all(PRIMARY_STACK);
    let mut reversed = forward.clone();
    reversed.reverse();
    let advertised = languages(&analyse(&forward).1);
    for expected in [
        "typescript",
        "go",
        "python",
        "rust",
        "swift",
        "kotlin",
        "markdown",
        "html",
    ] {
        assert!(
            advertised.iter().any(|language| language == expected),
            "{expected} is missing from {advertised:?}"
        );
    }
    let mut sorted = advertised.clone();
    sorted.sort();
    assert_eq!(advertised, sorted, "languages are listed by name (R4)");
    assert_eq!(
        languages(&analyse(&reversed).1),
        advertised,
        "the order must not depend on the order files arrive in"
    );
}

fn dead_names(analysis: &devmap_analyze::model::AnalysisSummary) -> Vec<String> {
    analysis
        .dead_symbols
        .iter()
        .map(|row| format!("{}::{}", row.file_path, row.symbol_name))
        .collect()
}

#[test]
fn a_function_named_only_in_comments_and_strings_stays_dead() {
    let mentioned_only = extract_all(&[
        (
            "rs/src/lib.rs",
            "fn orphan_rs() -> u32 { 1 }\n\
             pub fn api() -> &'static str {\n    // orphan_rs();\n    /* orphan_rs() */\n    \"orphan_rs()\"\n}\n",
        ),
        (
            "py/app.py",
            "def orphan_py():\n    return 1\n\n\
             def api():\n    # orphan_py()\n    \"\"\"orphan_py()\"\"\"\n    return \"orphan_py(\"\n",
        ),
        (
            "js/app.js",
            "function orphanJs() { return 1; }\n\
             export function api() {\n  // orphanJs();\n  /* orphanJs() */\n  return 'orphanJs(';\n}\n",
        ),
    ]);
    let dead = dead_names(&analyse(&mentioned_only).0);
    for symbol in [
        "rs/src/lib.rs::orphan_rs",
        "py/app.py::orphan_py",
        "js/app.js::orphanJs",
    ] {
        assert!(
            dead.iter().any(|row| row == symbol),
            "{symbol} is named only in comments and strings, so it is dead: {dead:?}"
        );
    }

    // The positive control: one real call each, and none of them is dead.
    let called = extract_all(&[
        (
            "rs/src/lib.rs",
            "fn orphan_rs() -> u32 { 1 }\npub fn api() -> u32 {\n    // orphan_rs();\n    orphan_rs()\n}\n",
        ),
        (
            "py/app.py",
            "def orphan_py():\n    return 1\n\ndef api():\n    # orphan_py()\n    return orphan_py()\n",
        ),
        (
            "js/app.js",
            "function orphanJs() { return 1; }\n\
             export function api() {\n  // orphanJs();\n  return orphanJs();\n}\n",
        ),
    ]);
    let dead = dead_names(&analyse(&called).0);
    for symbol in [
        "rs/src/lib.rs::orphan_rs",
        "py/app.py::orphan_py",
        "js/app.js::orphanJs",
    ] {
        assert!(
            !dead.iter().any(|row| row == symbol),
            "{symbol} has a real caller: {dead:?}"
        );
    }
}
