//! Per-language import extraction for the languages the specialised arms miss.
//!
//! W0.3 move 2. `unwired_candidates` asks one question — does this file have an
//! inbound `Imports` edge from a non-test file — and until this module there
//! were **five** `imports.push` sites in the whole extractor: Python, JS/TS/TSX,
//! Rust `use`, Go `import_spec`, and the embedded-script merge. `#include` had
//! no handler anywhere. For 23 of 35 declared languages the answer was
//! structurally always "no", so the honest gate W0.3 shipped excluded every one
//! of them and counted the exclusion, which is a lower bound stated out loud
//! rather than an answer.
//!
//! This is the answer. The rule for whether a language belongs here is narrow
//! and is applied per language rather than by family:
//!
//! > **The specifier must name a file, or a path that maps to one by a rule the
//! > language itself fixes.**
//!
//! `#include "lib/util.h"` names a file. `import com.foo.Bar` names
//! `com/foo/Bar.java` by a rule the Java language specification fixes. `using
//! System;` names neither — a C# namespace spans files and a file may declare
//! many — and `import Foundation` names a *module*, which is the one thing that
//! cannot explain intra-module wiring because same-module Swift files need no
//! import at all. Those two are declined here with reasons, and the decline is
//! pinned by `language_import_capabilities.rs` so it stays a decision rather
//! than becoming an oversight.
//!
//! Each language lives in its own module, mirroring `langcalls`, which is the
//! shape this repository already chose for exactly this problem: `treesitter.rs`
//! is the largest file in the workspace and the dispatcher below is the only
//! shared surface.

use crate::model::ExtractedImport;
use tree_sitter::Node;

pub(crate) mod cfamily;
pub(crate) mod cfml;
pub(crate) mod dart;
pub(crate) mod erlang;
pub(crate) mod hcl;
pub(crate) mod java;
pub(crate) mod kotlin;
pub(crate) mod lua;
pub(crate) mod nix;
pub(crate) mod pascal;
pub(crate) mod php;
pub(crate) mod r;
pub(crate) mod ruby;
pub(crate) mod rust;
pub(crate) mod scala;
pub(crate) mod solidity;

/// Route one node to its language's import extractor.
///
/// Returns without doing anything for a language with no module, which is the
/// honest state and is *not* silent: `Capability::Imports` is absent for that
/// language, `extraction_gaps` charges every one of its files `ImportBlind`, and
/// `unwired_candidates` excludes them and says how many. The registry and this
/// dispatcher are checked against each other in both directions by
/// `language_import_capabilities.rs`, so a module added without its flag — or a
/// flag added without its module — fails the build rather than quietly
/// producing an answer nobody can account for.
pub(crate) fn extract_imports(
    lang: &str,
    node: Node,
    source: &str,
    imports: &mut Vec<ExtractedImport>,
) {
    match lang {
        // `#include` and `#import` are one preprocessor node in every C-family
        // grammar, ObjC included, so one module serves all four keys. Metal
        // borrows the `cpp` grammar and arrives here as `cpp`.
        "c" | "cpp" | "objc" | "cuda" => cfamily::extract_include(node, source, imports),
        "dart" => dart::extract_import(node, source, imports),
        "cfml" => cfml::extract_template_attribute(node, source, imports),
        "erlang" => erlang::extract_include(node, source, imports),
        // Terraform and OpenTofu share one grammar key.
        "hcl" => hcl::extract_module_source(node, source, imports),
        "java" => java::extract_import(node, source, imports),
        "kotlin" => kotlin::extract_import(node, source, imports),
        // Luau is a Lua superset and shares `function_call`; the same reasoning
        // `langcalls` records for its own shared arm.
        "lua" | "luau" => lua::extract_require(node, source, imports),
        "nix" => nix::extract_import(node, source, imports),
        "pascal" => pascal::extract_uses(node, source, imports),
        "php" => php::extract_use_and_require(node, source, imports),
        "r" => r::extract_source(node, source, imports),
        "ruby" => ruby::extract_require(node, source, imports),
        // Only `mod`; the `use_declaration` arm stays in
        // `treesitter.rs`. See `rust.rs` for why the split is the
        // rule this module applies rather than an accident.
        "rust" => rust::extract_mod(node, source, imports),
        "scala" => scala::extract_import(node, source, imports),
        "solidity" => solidity::extract_import(node, source, imports),
        _ => {}
    }
}

/// The language keys this module extracts imports for.
///
/// Derived from the dispatcher above by the test that reads it, never
/// hand-maintained — that is exactly how `CALL_EXTRACTION_LANGUAGES` rotted into
/// a constant with zero production readers and a wrong list, which is the defect
/// W0.1 exists to have fixed once.
pub const IMPORT_EXTRACTION_LANGUAGES: &[&str] = &[
    "c", "cfml", "cpp", "cuda", "dart", "erlang", "hcl", "java", "kotlin", "lua", "luau", "nix",
    "objc", "pascal", "php", "r", "ruby", "rust", "scala", "solidity",
];

/// A quoted specifier with its quotes removed, or `None` when the node is not a
/// quoted string.
///
/// Grammars disagree about whether the quotes are separate children
/// (`string_literal` in C, with a `string_content` inside) or part of the token
/// (`string` in Solidity and Ruby). Trimming the characters is the one rule that
/// holds for both, and it is here rather than in each module so thirteen call
/// sites cannot disagree about what a quoted path is.
pub(crate) fn unquote(raw: &str) -> String {
    raw.trim()
        .trim_matches(|c| c == '"' || c == '\'' || c == '`')
        .to_string()
}

/// One import, built from the two things every caller has.
///
/// `imported_names` is left empty for every language here on purpose. The
/// question this module exists to answer is "does an inbound `Imports` edge
/// exist", which is a *file*-level relation; a per-name list would be a second,
/// weaker claim that no consumer reads and that several of these syntaxes
/// (`#include`, `uses`, `require`) cannot support at all.
pub(crate) fn file_import(
    raw: &str,
    specifier: String,
    span: crate::model::Span,
) -> ExtractedImport {
    ExtractedImport {
        raw_import: raw.to_string(),
        module_specifier: specifier,
        imported_names: Vec::new(),
        local_names: Vec::new(),
        alias: None,
        span,
    }
}

/// The first quoted string anywhere under `node`, within a bounded walk.
///
/// Grammars disagree about how deeply a quoted path is nested under an import
/// node: Dart wraps it in `configurable_uri` → `uri` → `string_literal`, PHP's
/// `require` puts it directly in the argument, Ruby's sits inside an
/// `argument_list`. Rather than encode each shape — several of which were read
/// from a node-kind probe rather than from a specification, and any of which a
/// grammar bump can rearrange — this finds the string and stops.
///
/// The walk is depth-bounded on purpose. `panic = "abort"` is set for release
/// builds in this workspace, so a stack overflow on hostile input is not a
/// recoverable error but a process death, and no import node any of these
/// languages produces is deeper than the budget.
pub(crate) fn first_string_literal(node: Node, source: &str) -> Option<String> {
    first_string_literal_within(node, source, MAX_STRING_SEARCH_DEPTH)
}

/// Deep enough for Dart's three-level `import_or_export` nesting with room to
/// spare, shallow enough that a pathological tree cannot recurse the stack out.
const MAX_STRING_SEARCH_DEPTH: u8 = 6;

fn first_string_literal_within(node: Node, source: &str, depth: u8) -> Option<String> {
    if depth == 0 {
        return None;
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        let kind = child.kind();
        // A string that is an argument to something *else* is not this import's
        // path. `require File.join(dir, \'x\')` computes its path at runtime, and
        // an unrestricted walk reaches the `\'x\'` inside the inner call and
        // reports `x` — a specifier that is not the file, was never written by
        // the author, and resolves either to nothing or, worse, to a real file
        // of that name. Found by the adversarial case rather than reasoned out;
        // fixed here, at the one walk all thirteen languages share, rather than
        // in the Ruby module where it happened to surface.
        if is_invocation_kind(kind) {
            continue;
        }
        if kind.contains("string") || kind == "uri" || kind.contains("path") {
            let text = unquote(&crate::treesitter::get_node_text(child, source));
            if !text.is_empty() {
                return Some(text);
            }
        }
        if let Some(found) = first_string_literal_within(child, source, depth - 1) {
            return Some(found);
        }
    }
    None
}

/// Node kinds that mean "a call is being made here".
///
/// Only ever consulted for a *descendant* of the node a module started from, so
/// the invocation an import is written as — Ruby's `call`, Lua's
/// `function_call`, R's `call`, Nix's `apply_expression` — is never itself
/// refused; only a second one nested inside its arguments is.
///
/// Concatenation is deliberately absent from this list. PHP's
/// `require __DIR__ . \'/util.php\'` is the idiomatic form and the string in it
/// really is the path, so a `binary_expression` stays transparent.
fn is_invocation_kind(kind: &str) -> bool {
    matches!(
        kind,
        "call"
            | "call_expression"
            | "function_call"
            | "function_call_expression"
            | "method_invocation"
            | "method_call"
            | "member_call_expression"
            | "scoped_call_expression"
            | "nullsafe_member_call_expression"
            | "apply_expression"
    )
}

/// The dotted name under a JVM-family import node, with a trailing wildcard
/// preserved.
///
/// ```text
/// import com.foo.Bar;         -> com.foo.Bar
/// import com.foo.*;           -> com.foo.*
/// import static com.foo.B.c;  -> com.foo.B     (the member is dropped)
/// import static com.foo.B.*;  -> com.foo.B.*
/// ```
///
/// The dotted name is read from the *named* child, not by stripping keywords
/// out of the node's text: `import`, `static`, `.`, `*` and `;` are all
/// anonymous tokens in these grammars, so the named child is the exact answer
/// where text surgery is a guess that a formatting difference breaks.
///
/// A static import's last segment is a member of the type named by the segments
/// before it, so dropping it is what names the file. That is a rule the language
/// specification fixes, not a convention — which is the test this module applies
/// before claiming a language at all.
pub(crate) fn jvm_dotted_specifier(node: Node, source: &str) -> Option<String> {
    let mut cursor = node.walk();
    let dotted = node
        .named_children(&mut cursor)
        .find(|child| {
            matches!(
                child.kind(),
                "scoped_identifier" | "qualified_identifier" | "identifier" | "type_identifier"
            )
        })
        .map(|child| crate::treesitter::get_node_text(child, source))?;
    let dotted = dotted.trim().trim_end_matches('.').trim().to_string();
    if dotted.is_empty() {
        return None;
    }

    let mut token_cursor = node.walk();
    let children: Vec<Node> = node.children(&mut token_cursor).collect();
    let has_wildcard = children
        .iter()
        .any(|child| matches!(child.kind(), "asterisk" | "*"));
    let is_static = children.iter().any(|child| child.kind() == "static");

    let base = if is_static && !has_wildcard {
        match dotted.rsplit_once('.') {
            Some((head, _member)) if !head.is_empty() => head.to_string(),
            // `import static Foo;` does not parse, so this arm is unreachable
            // through valid Java. Keeping the single segment rather than
            // returning nothing is the safer behaviour for a grammar that
            // accepts it anyway: it can still match a default-package file.
            _ => dotted.clone(),
        }
    } else {
        dotted
    };

    Some(if has_wildcard {
        format!("{base}.*")
    } else {
        base
    })
}
