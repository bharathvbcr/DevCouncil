//! What a CommonJS module publishes is a symbol and an export of its file.
//!
//! `exports.helper = function helper() {}`, `module.exports.helper = …` and
//! `module.exports = main` published nothing the extractor recorded, so a
//! `require` of such a file bound a name the file declared nowhere: Orbit's
//! `javascript/commonjs_module_resolution` scored 3 of 18, every miss on the
//! producing side. (`module.exports = { helper }` already worked, because
//! `helper` is an ordinary declaration.)
//!
//! Every positive case sits beside one that must stay silent: a conditional
//! export, one inside a function, and a constant that is not callable.

use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, Extraction};
use devmap_resolve::{ResolutionResult, Resolver};

fn resolve(files: &[(&str, &str)]) -> ResolutionResult {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, body)| extract_file(path, body))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    resolver.resolve_all(&extractions).expect("resolution")
}

fn calls(result: &ResolutionResult, from: &str) -> Vec<String> {
    let mut found: Vec<String> = result
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls && edge.source_symbol == from)
        .map(|edge| edge.target_symbol.clone())
        .collect();
    found.sort();
    found.dedup();
    found
}

fn symbols(files: &[(&str, &str)], path: &str) -> Vec<String> {
    let extraction = extract_file(
        path,
        files.iter().find(|(file, _)| *file == path).unwrap().1,
    );
    extraction
        .symbols
        .iter()
        .map(|symbol| symbol.qualified_name.clone())
        .collect()
}

const UTIL: &str = "exports.helper = function helper() {};\n";

#[test]
fn a_destructured_require_binds_an_exports_member_function() {
    let result = resolve(&[
        ("named/util.js", UTIL),
        (
            "named/main.js",
            "const { helper } = require('./util');\nfunction run() { helper(); }\n",
        ),
    ]);
    assert_eq!(
        calls(&result, "named/main.js::run"),
        ["named/util.js::helper"],
        "{:?}",
        result.unresolved
    );
}

#[test]
fn a_namespace_require_binds_an_exports_member_function() {
    let result = resolve(&[
        ("ns/util.js", UTIL),
        (
            "ns/main.js",
            "const util = require('./util');\nfunction run() { util.helper(); }\n",
        ),
    ]);
    assert_eq!(calls(&result, "ns/main.js::run"), ["ns/util.js::helper"]);
}

#[test]
fn a_renamed_destructure_binds_the_exported_function() {
    let result = resolve(&[
        (
            "alias/util.js",
            "exports.resolve = function resolve() { return 'ok'; };\n",
        ),
        (
            "alias/main.js",
            "const { resolve: presolve } = require('./util');\nfunction run() { return presolve(); }\n",
        ),
    ]);
    assert_eq!(calls(&result, "alias/main.js::run"), ["alias/util.js::resolve"]);
}

#[test]
fn module_exports_a_function_is_the_default_a_bare_require_binds() {
    let result = resolve(&[
        (
            "default/util.js",
            "function main() { return 'ok'; }\nmodule.exports = main;\n",
        ),
        (
            "default/main.js",
            "const runMain = require('./util');\nfunction boot() { return runMain(); }\n",
        ),
    ]);
    assert_eq!(
        calls(&result, "default/main.js::boot"),
        ["default/util.js::main"],
        "{:?}",
        result.unresolved
    );
}

#[test]
fn exports_default_is_reached_as_a_member_of_the_namespace() {
    let result = resolve(&[
        (
            "interop/util.js",
            "exports.default = function defaultHelper(value) { return value; };\n",
        ),
        (
            "interop/main.js",
            "const util = require('./util');\nfunction run(value) { return util.default(value); }\n",
        ),
    ]);
    assert_eq!(
        calls(&result, "interop/main.js::run"),
        ["interop/util.js::defaultHelper"],
        "{:?}",
        result.unresolved
    );
}

#[test]
fn an_object_export_with_a_renamed_member_binds_the_local() {
    let result = resolve(&[
        (
            "obj/util.js",
            "function internalName() { return 1; }\nmodule.exports = { publicName: internalName };\n",
        ),
        (
            "obj/main.js",
            "const { publicName } = require('./util');\nfunction run() { return publicName(); }\n",
        ),
    ]);
    assert_eq!(calls(&result, "obj/main.js::run"), ["obj/util.js::internalName"]);
}

#[test]
fn an_exported_function_owns_the_calls_in_its_body() {
    let result = resolve(&[
        (
            "own/util.js",
            "function leaf() { return 1; }\nexports.entry = function entry() { return leaf(); };\n",
        ),
    ]);
    assert_eq!(calls(&result, "own/util.js::entry"), ["own/util.js::leaf"]);
}

#[test]
fn a_constant_export_is_never_a_call_target() {
    let result = resolve(&[
        (
            "k/util.js",
            "exports.HTTP_STATUS_OK = 200;\nexports.trimValue = function trimValue(value) { return value; };\n",
        ),
        (
            "k/main.js",
            "const { HTTP_STATUS_OK, trimValue } = require('./util');\n\
             function run(value) { return trimValue(value); }\n\
             function bad() { return HTTP_STATUS_OK(); }\n",
        ),
    ]);
    assert_eq!(calls(&result, "k/main.js::run"), ["k/util.js::trimValue"]);
    assert!(
        calls(&result, "k/main.js::bad").is_empty(),
        "{:?}",
        calls(&result, "k/main.js::bad")
    );
}

#[test]
fn a_conditional_or_nested_export_publishes_nothing() {
    let conditional = "if (process.env.X) { exports.helper = function helper() {}; }\n";
    let nested = "function install() { exports.helper = function helper() {}; }\n";
    let chained = "const x = exports.helper = function helper() {};\n";
    for (name, body) in [("c.js", conditional), ("n.js", nested), ("ch.js", chained)] {
        let found = symbols(&[(name, body)], name);
        assert!(
            found.iter().all(|symbol| !symbol.ends_with("::helper")),
            "{name}: {found:?}"
        );
    }
}

#[test]
fn an_anonymous_function_published_as_the_whole_module_has_no_symbol() {
    let found = symbols(&[("a.js", "module.exports = function () {};\n")], "a.js");
    assert_eq!(found, ["a.js"], "only the file symbol: {found:?}");
}

#[test]
fn a_local_rename_export_is_followed_for_es_modules_too() {
    // `export { local as Public }`: the hop stays in the declaring file.
    let result = resolve(&[
        (
            "es/util.ts",
            "function local() { return 1; }\nexport { local as Public };\n",
        ),
        (
            "es/main.ts",
            "import { Public } from './util';\nexport function run() { return Public(); }\n",
        ),
    ]);
    assert_eq!(calls(&result, "es/main.ts::run"), ["es/util.ts::local"]);
}

#[test]
fn module_exports_an_identifier_is_a_default_export_of_that_binding() {
    let extraction = extract_file(
        "default/util.js",
        "function main() { return 'ok'; }\nmodule.exports = main;\n",
    );
    let defaults: Vec<_> = extraction
        .exports
        .iter()
        .filter(|export| export.exported_name == "default")
        .map(|export| export.local_name.clone())
        .collect();
    assert_eq!(defaults, [Some("main".to_string())], "{:?}", extraction.exports);
}
