//! String literals that name something a runtime looks up by name.
//!
//! A host program names a GPU kernel with a string — Rust
//! `rt.pipeline("mlp_silu")`, Swift `library.makeFunction(name: "mlp_silu")`,
//! Objective-C `[library newFunctionWithName:@"mlp_silu"]`, metal-cpp
//! `NS::String::string("mlp_silu", …)`, PyObjC
//! `library.newFunctionWithName_("mlp_silu")` — and just as often hands it to a
//! wrapper (`pipeline(rt, "x", WHAT)`), keeps it in a dispatch table
//! (`("encoder_attn_rows_h256_r16_g32", 16, 32)`) or in a constant. So the
//! literal is recorded wherever it is, as a [`ReferenceKind::EntryName`]; which
//! literals name a kernel is the resolver's question, answered from the
//! entry points the index holds.
//!
//! Only a literal whose whole text is an identifier qualifies — the only shape a
//! lookup by name accepts. An escape sequence, an interpolation, whitespace or
//! punctuation makes it a message, a path or a format, never a name.

use tree_sitter::Node;

use crate::model::{ExtractedReference, ReferenceKind};

/// Longest literal read as a name. No entry point in any corpus measured comes
/// near it; past it the text is data.
const MAX_ENTRY_NAME_BYTES: usize = 128;

/// Most syntax-tree nodes one file's pass visits before it stops.
const MAX_NODES: usize = 2_000_000;

/// How each host grammar spells a plain string literal: the literal's node
/// kinds, and the one content node a literal with no escape or interpolation
/// holds. A grammar absent here is not a host language this pass reads.
fn literal_shape(lang: &str) -> Option<(&'static [&'static str], &'static [&'static str])> {
    Some(match lang {
        "rust" => (&["string_literal", "raw_string_literal"], &["string_content"]),
        "swift" => (&["line_string_literal"], &["line_str_text"]),
        "c" | "cpp" | "objc" | "cuda" => (
            &["string_literal", "raw_string_literal"],
            &["string_content", "raw_string_content"],
        ),
        "python" => (&["string"], &["string_content"]),
        _ => return None,
    })
}

/// Python's delimiters are named nodes; they carry no text of the literal.
fn is_delimiter(kind: &str) -> bool {
    matches!(kind, "string_start" | "string_end")
}

/// Every identifier-shaped string literal in `root`, as an `EntryName`
/// reference owned by its enclosing callable — or, at the top level, carrying
/// the constant it initializes as `assigned_to` so the resolver can hand the
/// edge to whatever reads that constant.
///
/// Returns `false` when the walk ran out of its node budget or the extraction
/// deadline, so the caller can refuse the file rather than publish a partial
/// read as a complete one.
pub(crate) fn collect(
    root: Node,
    source: &str,
    lang: &str,
    file_symbol_name: &str,
    references: &mut Vec<ExtractedReference>,
    deadline: std::time::Instant,
) -> bool {
    let Some((literals, contents)) = literal_shape(lang) else {
        return true;
    };
    let mut stack = vec![root];
    let mut visited = 0usize;
    while let Some(node) = stack.pop() {
        visited += 1;
        if visited > MAX_NODES
            || (visited % 4_096 == 0 && crate::treesitter::extraction_overran(deadline))
        {
            return false;
        }
        if literals.contains(&node.kind()) {
            if let Some(name) = identifier_text(node, source, contents) {
                let enclosing_symbol =
                    crate::treesitter::enclosing_callable_qualified(node, source, file_symbol_name);
                let assigned_to = enclosing_symbol
                    .is_none()
                    .then(|| initialized_binding(node, source, lang))
                    .flatten();
                references.push(ExtractedReference {
                    name,
                    kind: ReferenceKind::EntryName,
                    span: crate::treesitter::node_span(node),
                    enclosing_symbol,
                    assigned_to,
                    receiver_expr: None,
                });
            }
            continue;
        }
        crate::treesitter::push_named_children(node, &mut stack);
    }
    true
}

/// The literal's text when it is exactly one content node that is an
/// identifier, else `None`.
fn identifier_text(node: Node, source: &str, contents: &[&str]) -> Option<String> {
    let mut cursor = node.walk();
    let mut parts = node
        .named_children(&mut cursor)
        .filter(|child| !is_delimiter(child.kind()));
    let content = parts.next()?;
    if parts.next().is_some() || !contents.contains(&content.kind()) {
        return None;
    }
    // A Python `b"…"` is bytes, not a name a lookup accepts as `str`.
    if node
        .named_child(0)
        .filter(|start| start.kind() == "string_start")
        .and_then(|start| source.get(start.byte_range()))
        .is_some_and(|prefix| prefix.to_ascii_lowercase().contains('b'))
    {
        return None;
    }
    let text = source.get(content.byte_range())?;
    let identifier = text.len() >= 2
        && text.len() <= MAX_ENTRY_NAME_BYTES
        && text
            .bytes()
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic() || first == b'_')
        && text.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
    identifier.then(|| text.to_string())
}

/// The name of the top-level constant whose initializer holds `node`: a Rust
/// `const`/`static`, a Python module assignment, a Swift `let`/`var`, a C-family
/// file-scope declaration. `None` when the literal is not such an initializer.
fn initialized_binding(node: Node, source: &str, lang: &str) -> Option<String> {
    let text = |node: Node| source.get(node.byte_range()).map(str::to_string);
    let mut current = node.parent();
    for _ in 0..64 {
        let parent = current?;
        match (lang, parent.kind()) {
            ("rust", "const_item" | "static_item") => {
                return parent.child_by_field_name("name").and_then(text);
            }
            ("python", "assignment") => {
                let left = parent.child_by_field_name("left")?;
                return (left.kind() == "identifier").then(|| text(left)).flatten();
            }
            ("swift", "property_declaration") => {
                let pattern = parent.child_by_field_name("name")?;
                let mut cursor = pattern.walk();
                let identifier = pattern
                    .named_children(&mut cursor)
                    .find(|child| child.kind() == "simple_identifier")
                    .or_else(|| (pattern.kind() == "simple_identifier").then_some(pattern))?;
                return text(identifier);
            }
            ("c" | "cpp" | "objc" | "cuda", "init_declarator") => {
                let mut declarator = parent.child_by_field_name("declarator")?;
                for _ in 0..8 {
                    if matches!(declarator.kind(), "identifier" | "field_identifier") {
                        return text(declarator);
                    }
                    declarator = declarator.child_by_field_name("declarator")?;
                }
                return None;
            }
            // A body is a scope boundary: a string past it belongs to that
            // scope, not to a binding further out.
            (
                _,
                "function_item" | "closure_expression" | "function_definition"
                | "function_declaration" | "lambda_literal" | "compound_statement" | "block",
            ) => return None,
            _ => current = parent.parent(),
        }
    }
    None
}
