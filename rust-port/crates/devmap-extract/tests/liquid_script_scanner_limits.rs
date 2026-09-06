//! What the vendored Liquid grammar cannot read, stated rather than discovered.
//!
//! Found while extending the capability corpus: adding a `class` declaration to
//! `testdata/capabilities/probe.liquid` dropped the file from
//! `CALLS | IMPORTS | REFERENCES` to `REFERENCES` alone. The diagnostic reads
//! "unterminated `<script>` … no `</script>` in the same run of template text",
//! which is true of the *parse* and misleading about the *file*: the closing tag
//! is right there.
//!
//! The cause is one level down. `tree-sitter-liquid` produces an `ERROR` node
//! over the opening of the script block for certain JavaScript bodies, so the
//! template text is no longer one run and the embedded-script merge has nothing
//! to bound. Two triggers are pinned below.
//!
//! **Not fixed here, and the direction of the error is why.** A liquid file
//! whose script does not parse extracts no calls and no imports, so the file is
//! charged `Partial` and every symbol in it is already held below the confident
//! tier by the coverage machinery. The capability registry declares liquid
//! without `HERITAGE` for the same reason — under-claiming makes a consumer
//! more conservative, and a capability claimed but not delivered is the failure
//! W0.1 exists to prevent. Fixing the vendored grammar is a separate change
//! with a separate blast radius.
//!
//! These tests pin the current behaviour so that a grammar bump which fixes it
//! is *visible* rather than silently widening what liquid reports.

use devmap_extract::extract_file;
use devmap_extract::model::ParseOutcome;

/// `{{` inside a script body is read as an output-tag delimiter.
#[test]
fn adjacent_braces_in_a_script_break_the_template_run() {
    let extraction = extract_file("t.liquid", "<script>\n  const o = {{}};\n</script>\n");
    assert!(
        matches!(extraction.parse_outcome, ParseOutcome::Partial { .. }),
        "if this now parses cleanly the grammar was fixed — widen the liquid \
         capability probe and re-derive: {:?}",
        extraction.parse_outcome
    );
    assert!(
        extraction
            .diagnostics
            .iter()
            .any(|d| d.contains("unterminated <script>")),
        "the limitation must stay reported rather than silently dropping the \
         script: {:?}",
        extraction.diagnostics
    );
}

/// A body combining an import, a class and a function trips it too.
///
/// Each of those alone parses; the combination does not, which is why this is
/// pinned by example rather than by a rule. Reduced from `probe.liquid`.
#[test]
fn a_class_beside_an_import_and_a_function_breaks_the_template_run() {
    let source = "<script>\n  import { helper } from \"./helper.js\";\n\n\
                  \x20 export class Widget extends BaseWidget {\n    draw(name) {\n\
                  \x20     return helper(name);\n    }\n  }\n\n\
                  \x20 export function render(name) {\n    return helper(name);\n  }\n\n\
                  \x20 const value = render(\"x\");\n</script>\n\
                  {% if value %}{{ value }}{% endif %}\n";
    let extraction = extract_file("t.liquid", source);
    assert!(
        matches!(extraction.parse_outcome, ParseOutcome::Partial { .. }),
        "if this now parses cleanly the grammar was fixed: {:?}",
        extraction.parse_outcome
    );
    assert!(
        extraction.calls.is_empty() && extraction.imports.is_empty(),
        "a broken template run yields no embedded script at all, which is what \
         makes the file `Partial` and holds its symbols below the confident tier"
    );
}

/// The OFF direction: an ordinary liquid script still works.
///
/// Without this the two assertions above would pass against a liquid extractor
/// that had stopped working entirely.
#[test]
fn an_ordinary_liquid_script_still_extracts() {
    let extraction = extract_file(
        "t.liquid",
        "<script>\n  import { helper } from \"./helper.js\";\n\n\
         \x20 export function render(name) {\n    return helper(name);\n  }\n</script>\n\
         {% if v %}{{ v }}{% endif %}\n",
    );
    assert!(
        matches!(extraction.parse_outcome, ParseOutcome::Clean),
        "{:?} / {:?}",
        extraction.parse_outcome,
        extraction.diagnostics
    );
    assert!(!extraction.calls.is_empty(), "calls are extracted");
    assert!(!extraction.imports.is_empty(), "imports are extracted");
}
