//! A CommonJS `require` binds names exactly as an ES `import` does.
//!
//! The JavaScript extractor recorded `require("./util")` as an import with no
//! bound names at all, so the declaration around it bound nothing:
//!
//! ```js
//! const { helper } = require("./util");   // `helper()` matched no rung
//! const path = require("node:path");      // `path.join()` was "uninferred"
//! ```
//!
//! Measured on this repository at generation 4150: `bin/devcouncil.js` imports
//! every `fs` function it calls through a destructured `require`, and each of
//! those calls was filed `no_namesake` — "nothing in the corpus declares it" —
//! when the file's own first lines say where it comes from. Against a local
//! module the same shape is a lost edge, not a mislabelled miss.

use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, Extraction};
use devmap_resolve::model::{ResolutionResult, UnresolvedClass};
use devmap_resolve::Resolver;

fn resolve(files: &[(&str, &str)]) -> ResolutionResult {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    resolver.resolve_all(&extractions).unwrap()
}

const UTIL: &str = "function helper() { return 1; }\n\
                    function other() { return 2; }\n\
                    module.exports = { helper, other };\n";

fn calls(result: &ResolutionResult, from: &str) -> Vec<String> {
    let mut targets: Vec<String> = result
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls && edge.source_symbol == from)
        .map(|edge| edge.target_symbol.clone())
        .collect();
    targets.sort();
    targets
}

fn class_of<'a>(result: &'a ResolutionResult, callee: &str) -> Vec<&'a UnresolvedClass> {
    result
        .unresolved
        .iter()
        .filter(|row| row.callee_name == callee)
        .map(|row| &row.class)
        .collect()
}

#[test]
fn a_destructured_local_require_binds_each_name_and_its_rename() {
    let result = resolve(&[
        (
            "src/main.js",
            "const { helper, other: renamed } = require(\"./util\");\n\
             function main() { helper(); renamed(); }\n",
        ),
        ("src/util.js", UTIL),
    ]);
    assert_eq!(
        calls(&result, "src/main.js::main"),
        vec!["src/util.js::helper", "src/util.js::other"],
        "unresolved: {:?}",
        result.unresolved
    );
}

#[test]
fn a_whole_module_local_require_is_a_namespace_handle() {
    let result = resolve(&[
        (
            "src/main.js",
            "const util = require(\"./util\");\nfunction main() { util.helper(); }\n",
        ),
        ("src/util.js", UTIL),
    ]);
    assert_eq!(
        calls(&result, "src/main.js::main"),
        vec!["src/util.js::helper"],
        "unresolved: {:?}",
        result.unresolved
    );
}

#[test]
fn an_external_require_files_its_calls_as_external() {
    let result = resolve(&[(
        "bin/cli.js",
        "const { existsSync } = require(\"node:fs\");\n\
         const path = require(\"node:path\");\n\
         function main() { existsSync(path.join(\"a\", \"b\")); }\n",
    )]);
    for callee in ["existsSync", "join"] {
        let classes = class_of(&result, callee);
        assert!(
            !classes.is_empty()
                && classes
                    .iter()
                    .all(|class| matches!(class, UnresolvedClass::External { .. })),
            "{callee}: {classes:?}"
        );
    }
}

#[test]
fn a_require_outside_a_declaration_still_binds_nothing() {
    // `require("./util").helper` and a bare side-effect `require` name no
    // local handle for the module: the import is recorded, and no binding is
    // invented for it.
    let extraction = extract_file(
        "src/main.js",
        "require(\"./util\");\nconst h = require(\"./util\").helper;\n",
    );
    let requires: Vec<_> = extraction
        .imports
        .iter()
        .filter(|import| import.module_specifier == "./util")
        .collect();
    assert_eq!(requires.len(), 2, "{:?}", extraction.imports);
    for import in requires {
        assert!(
            import.alias.is_none() && import.local_names.is_empty(),
            "{import:?}"
        );
    }
}
