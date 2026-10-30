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
    assert_eq!(
        calls(&result, "alias/main.js::run"),
        ["alias/util.js::resolve"]
    );
}

/// `exports.callee = function () {}`: an anonymous function published as a
/// *member*. The name the export gives is the symbol; both a plain
/// destructure and a renamed one (`{ callee: c }`) call it. (This is the shape
/// a query-crate fixture once relied on staying unbound.)
#[test]
fn an_anonymous_member_export_is_bound_by_a_plain_and_an_aliased_destructure() {
    let result = resolve(&[
        (
            "anon/cjs.js",
            "exports.callee = function () { return 1; };\n",
        ),
        (
            "anon/plain.js",
            "const { callee } = require('./cjs.js');\nfunction caller_plain() { return callee(); }\n",
        ),
        (
            "anon/aliased.js",
            "const { callee: c } = require('./cjs.js');\nfunction caller_aliased() { return c(); }\n",
        ),
    ]);
    assert_eq!(
        calls(&result, "anon/plain.js::caller_plain"),
        ["anon/cjs.js::callee"],
        "{:?}",
        result.unresolved
    );
    assert_eq!(
        calls(&result, "anon/aliased.js::caller_aliased"),
        ["anon/cjs.js::callee"],
        "{:?}",
        result.unresolved
    );
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
    assert_eq!(
        calls(&result, "obj/main.js::run"),
        ["obj/util.js::internalName"]
    );
}

#[test]
fn an_exported_function_owns_the_calls_in_its_body() {
    let result = resolve(&[(
        "own/util.js",
        "function leaf() { return 1; }\nexports.entry = function entry() { return leaf(); };\n",
    )]);
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
    assert_eq!(
        defaults,
        [Some("main".to_string())],
        "{:?}",
        extraction.exports
    );
}

#[test]
fn a_computed_require_binds_nothing_and_a_literal_export_is_not_published() {
    // `require(name)` names no file, so no edge. A literal export is no longer a
    // symbol at all: only a `const` is known to be inert, and `exports.LIMIT = 3`
    // is a slot another statement may assign, so the lane publishes nothing for
    // it. (`LIMIT` is also assigned twice here, which withholds it on its own.)
    let result = resolve(&[
        (
            "c/util.js",
            "exports.LIMIT = 3;\nexports.LIMIT = 4;\nexports.go = function go() {};\n",
        ),
        (
            "c/main.js",
            "const name = './util';\n\
             const dyn = require(name);\n\
             const { LIMIT, go } = require('./util');\n\
             function viaDynamic() { return dyn.go(); }\n\
             function viaLiteral() { return go() + LIMIT(); }\n",
        ),
    ]);
    assert!(
        calls(&result, "c/main.js::viaDynamic").is_empty(),
        "{:?}",
        calls(&result, "c/main.js::viaDynamic")
    );
    assert_eq!(calls(&result, "c/main.js::viaLiteral"), ["c/util.js::go"]);
    let declared = symbols(
        &[("c/util.js", "exports.LIMIT = 3;\nexports.LIMIT = 4;\n")],
        "c/util.js",
    );
    assert!(
        declared.iter().all(|name| !name.ends_with("::LIMIT")),
        "a literal export declares nothing: {declared:?}"
    );
}

#[test]
fn a_hoisted_void_zero_then_a_real_assignment_is_withheld() {
    // A compiled module opens with `exports.run = void 0;` and assigns the real
    // value below. That is two assignments of `run`, and which one a `require`
    // sees is not a thing one statement proves: the name is withheld, nothing is
    // bound (base bound nothing), and the row says why.
    for util in [
        "exports.run = void 0;\nexports.run = function run() {};\n",
        "exports.run = 1;\nexports.run = function run() {};\n",
    ] {
        let result = resolve(&[
            ("v/util.js", util),
            (
                "v/main.js",
                "const { run } = require('./util');\nfunction go() { return run(); }\n",
            ),
        ]);
        assert!(
            calls(&result, "v/main.js::go").is_empty(),
            "{util}\n{:?}",
            calls(&result, "v/main.js::go")
        );
        assert!(
            result.unresolved.iter().any(|row| matches!(
                &row.resolution,
                devmap_resolve::model::Resolution::Unresolved { reason }
                    if reason.starts_with("commonjs export withheld")
            )),
            "{util}\n{:?}",
            result.unresolved
        );
    }
}

#[test]
fn two_module_exports_assignments_withhold_the_default() {
    // Which `module.exports =` a `require` sees is not one statement's claim.
    let result = resolve(&[
        (
            "m/util.js",
            "function a() {}\nfunction b() {}\nmodule.exports = a;\nmodule.exports = b;\n",
        ),
        (
            "m/main.js",
            "const run = require('./util');\nfunction go() { return run(); }\n",
        ),
    ]);
    assert!(calls(&result, "m/main.js::go").is_empty());
    assert!(
        result.unresolved.iter().any(|row| matches!(
            &row.resolution,
            devmap_resolve::model::Resolution::Unresolved { reason }
                if reason.starts_with("commonjs export withheld")
        )),
        "{:?}",
        result.unresolved
    );
}

#[test]
fn a_name_assigned_once_beside_a_twice_assigned_one_is_still_published() {
    // The count is per name: `dup` is withheld, `once` is not.
    let result = resolve(&[
        (
            "p/util.js",
            "exports.dup = function dup() {};\nexports.dup = function dup2() {};\nexports.once = function once() {};\n",
        ),
        (
            "p/main.js",
            "const { dup, once } = require('./util');\nfunction go() { dup(); once(); }\n",
        ),
    ]);
    assert_eq!(calls(&result, "p/main.js::go"), ["p/util.js::once"]);
}
