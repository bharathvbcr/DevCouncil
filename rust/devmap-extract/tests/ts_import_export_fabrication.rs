//! Imports and exports read off the parse tree, not off the statement's text.
//!
//! The TS/JS `"import_statement" | "export_statement"` arm found its binding
//! list with `text.find('{')` .. `text.find('}')` over `get_node_text(node)` —
//! the *whole* statement, function body included. For
//! `export function helper(n) { if (n > 0) { return n; } return 0; }` the first
//! `{` is the function body's and the first `}` closes the `if`, so the slice
//! was `"\n  if (n > 0) {\n    return n;\n  "` and `parse_import_bindings` read
//! `if` out of it — producing an `ExtractedImport` with `module_specifier: ""`
//! and `imported_names: ["if"]`, plus a matching fabricated export.
//!
//! Neither `""` as a module nor `if` as an imported name is a value any valid
//! program can produce, so every consumer joining on them gets an endpoint that
//! can never resolve — the SC26/SC32 whole-expression shape, in the
//! import/export emitter instead of the call emitter.
//!
//! Found while measuring embedded `<script>` blocks, and reproduced on a plain
//! `.ts` file with no embedding involved, which is what proves the owner is
//! this arm and not the embedding.

use devmap_extract::extract_file;

/// A function body is not a binding list.
#[test]
fn an_exported_function_body_is_not_read_as_an_import_list() {
    let extraction = extract_file(
        "control.ts",
        "export function helper(n: number): number {\n\
         \x20 if (n > 0) {\n\
         \x20   return n;\n\
         \x20 }\n\
         \x20 return 0;\n\
         }\n",
    );

    assert!(
        extraction.imports.is_empty(),
        "a function declaration imports nothing, got {:?}",
        extraction
            .imports
            .iter()
            .map(|import| (&import.module_specifier, &import.imported_names))
            .collect::<Vec<_>>()
    );
    let exported: Vec<&str> = extraction
        .exports
        .iter()
        .map(|export| export.exported_name.as_str())
        .collect();
    assert!(
        !exported.contains(&"if"),
        "`if` is a keyword in the body, not an exported name, got {exported:?}"
    );
}

/// The same shape in TSX, where the body's `const` was the name it invented.
#[test]
fn an_exported_arrow_body_is_not_read_as_an_import_list() {
    let extraction = extract_file(
        "control.tsx",
        "export const C = () => {\n  const f = 1;\n  return f;\n};\n",
    );
    assert!(
        extraction.imports.is_empty(),
        "an exported const imports nothing, got {:?}",
        extraction
            .imports
            .iter()
            .map(|import| (&import.module_specifier, &import.imported_names))
            .collect::<Vec<_>>()
    );
}

/// A re-export with no `from` is not an import.
///
/// This one fired on *correct* source: `export { NAME as ALIAS };` pushed an
/// `ExtractedImport` with an empty module specifier, because the push was
/// gated on `imported_names` being non-empty rather than on the statement
/// having a source at all.
#[test]
fn a_local_re_export_is_an_export_and_not_an_import() {
    let extraction = extract_file(
        "m.ts",
        "function NAME() { return 1; }\nexport { NAME as ALIAS };\n",
    );

    assert!(
        extraction.imports.is_empty(),
        "an export with no `from` imports nothing, got {:?}",
        extraction
            .imports
            .iter()
            .map(|import| (&import.module_specifier, &import.imported_names))
            .collect::<Vec<_>>()
    );
    let renamed = extraction
        .exports
        .iter()
        .find(|export| export.exported_name == "ALIAS")
        .expect("the aliased export must still be recorded");
    assert_eq!(renamed.local_name.as_deref(), Some("NAME"));
    assert_eq!(renamed.module_specifier, None);
}

/// The good paths, so the fix is a narrowing and not a removal.
#[test]
fn real_bindings_are_still_read_with_their_aliases() {
    let extraction = extract_file(
        "u.ts",
        "import { alpha, beta as gamma } from './lib';\n\
         export { delta as epsilon } from './other';\n",
    );

    let named = extraction
        .imports
        .iter()
        .find(|import| import.module_specifier == "./lib")
        .expect("the named import must be recorded");
    assert_eq!(named.imported_names, vec!["alpha", "beta"]);
    assert_eq!(named.local_names, vec!["alpha", "gamma"]);

    let re_exported = extraction
        .imports
        .iter()
        .find(|import| import.module_specifier == "./other")
        .expect("a re-export WITH a source is also an import edge");
    assert_eq!(re_exported.imported_names, vec!["delta"]);

    let export = extraction
        .exports
        .iter()
        .find(|export| export.exported_name == "epsilon")
        .expect("the re-exported alias must be recorded");
    assert_eq!(export.local_name.as_deref(), Some("delta"));
    assert_eq!(export.module_specifier.as_deref(), Some("./other"));
}

/// A side-effect import has no bindings and is still an import.
#[test]
fn a_side_effect_import_survives_the_source_gate() {
    let extraction = extract_file("s.ts", "import './polyfill';\n");
    let import = extraction
        .imports
        .first()
        .expect("a bare `import 'x'` is a real module edge");
    assert_eq!(import.module_specifier, "./polyfill");
    assert!(import.imported_names.is_empty());
}

/// Multi-line bodies containing commas were the reported symptom.
#[test]
fn a_body_with_commas_does_not_become_a_binding_list() {
    let extraction = extract_file(
        "r.ts",
        "export function computeTotal(items: number[]): number {\n\
         \x20 return items.reduce((sum, n) => sum + n, 0);\n\
         }\n",
    );
    let fabricated: Vec<&str> = extraction
        .imports
        .iter()
        .flat_map(|import| import.imported_names.iter().map(String::as_str))
        .collect();
    assert!(
        fabricated.is_empty(),
        "the reducer body is not a binding list, got {fabricated:?}"
    );
}
