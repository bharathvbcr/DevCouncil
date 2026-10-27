//! Kotlin one-line class bodies (extract-v98 row 16): the same-length `;` rewrite.
//!
//! tree-sitter-kotlin-ng reads a class, interface or object body whose last
//! member shares a line with the closing brace as a whole-file ERROR:
//! `class A { val x = 1 }`, `interface Cb { fun call() }`,
//! `val o = object { val x = 1 }`, `class C { companion object { fun f() = 1 } }`.
//! The grammar's separator between members is a zero-width token it accepts once
//! per line; the same shapes on several lines, or with `;` before the `}`, parse
//! cleanly (measured on tree-sitter-kotlin-ng 1.1.0, not read from its source).
//! The ERROR swallows the class and every declaration after it, and a member
//! inside it keeps the file as its owner (`K.kt::f` for `C.f`).
//!
//! # The rewrite
//!
//! A `;` is a legal separator everywhere Kotlin has one, so putting one where the
//! grammar wants it changes no meaning. The rewrite replaces **one existing
//! space or tab** with `;`, so the text keeps its length and every byte offset
//! the tree reports is an offset of the original file. The extractor then reads
//! the original text through the rewritten tree: a node's text differs from
//! the file's only where a `;` replaced a space, outside every name.
//!
//! Which spaces: the one directly before a closing `}` that has code before it
//! on its line and that closes a class-like body (the `}` is a child of a
//! `class_body`, an `enum_class_body` or an ERROR). A `}` that closes a block
//! or a lambda needs no separator and is left alone. The tokens come from the
//! tree of the failed parse, which still holds them, outside strings and
//! comments. They are not all visible to the
//! first parse (a nested `}` hides inside the ERROR until the outer one is
//! fixed), so the rewrite repeats, at most [`MAX_PASSES`] times, each pass over
//! the top-level statements that still hold an error.
//!
//! The result is accepted **only if the re-parse is fully clean**. Anything
//! else, including a deadline, leaves the file exactly as it was: `Partial`.
//!
//! # No room for a separator
//!
//! `class A { fun f() {}}` has no space before the last `}`. Inserting a `;`
//! would shift every later offset. The rewrite does not map offsets back; it
//! leaves such a file unrecovered, and the file stays `Partial`.

use std::time::Instant;

use tree_sitter::{Node, Parser, Tree};

use crate::model::TextRange;
use crate::treesitter::{bounded_parent, parse_until, walk_deadline_passed, DEADLINE_CHECK_STRIDE};

/// Parses after the first one. Each pass reveals the `}` the previous ERROR hid.
pub(crate) const MAX_PASSES: usize = 6;

/// Separators written per file. A file that wants more is not rewritten.
pub(crate) const MAX_EDITS: usize = 512;

/// What the rewrite did.
pub(crate) enum Repair {
    /// The re-parse is clean. `tree` is its tree, on the same offsets as the
    /// file; `ranges` are the top-level statements a `;` was written into, and
    /// `edits` the number of separators written.
    Repaired {
        tree: Tree,
        ranges: Vec<TextRange>,
        edits: usize,
    },
    /// Nothing changed: the original tree is handed back, with the reason.
    Unchanged { tree: Tree, why: String },
}

/// Try to make `tree`, which has an error, parse cleanly by writing `;` over
/// spaces. `source` is the text `tree` was parsed from.
pub(crate) fn repair(parser: &mut Parser, tree: Tree, source: &str, deadline: Instant) -> Repair {
    let mut text = source.as_bytes().to_vec();
    let mut edited: Vec<usize> = Vec::new();
    let mut current = tree.clone();
    for pass in 0..=MAX_PASSES {
        if !current.root_node().has_error() {
            let ranges = touched(&current, &edited);
            return Repair::Repaired {
                tree: current,
                ranges,
                edits: edited.len(),
            };
        }
        if pass == MAX_PASSES {
            break;
        }
        let fresh = match candidates(current.root_node(), &text, &edited) {
            Ok(fresh) => fresh,
            Err(why) => return Repair::Unchanged { tree, why },
        };
        if fresh.is_empty() {
            return Repair::Unchanged {
                tree,
                why: "no space before a closing brace is left to write a separator over".into(),
            };
        }
        if edited.len() + fresh.len() > MAX_EDITS {
            return Repair::Unchanged {
                tree,
                why: format!("more than {MAX_EDITS} separators would be written"),
            };
        }
        for at in fresh {
            text[at] = b';';
            edited.push(at);
        }
        // Only an ASCII space or tab was replaced by an ASCII byte.
        let Ok(rewritten) = std::str::from_utf8(&text) else {
            return Repair::Unchanged {
                tree,
                why: "the rewritten text is not UTF-8".into(),
            };
        };
        match parse_until(parser, rewritten, deadline) {
            (Some(next), false) => current = next,
            _ => {
                return Repair::Unchanged {
                    tree,
                    why: "the re-parse did not finish within the budget".into(),
                }
            }
        }
    }
    Repair::Unchanged {
        tree,
        why: format!("still an error after {MAX_PASSES} rewrites"),
    }
}

/// Bytes a token of these kinds may end with when a `;` is wanted after it.
/// An arrow, an operator or an opening bracket before `}` is not a member.
fn ends_a_member(before: &[u8]) -> bool {
    match before.last() {
        Some(b'>') => before.len() < 2 || before[before.len() - 2] != b'-',
        Some(byte) => {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'_' | b')' | b']' | b'}' | b'"' | b'\'' | b'`' | b'!' | b'?'
                )
        }
        None => false,
    }
}

/// The offsets of the spaces to write `;` over: one directly before each
/// closing `}` that follows code on its line, in the top-level statements that
/// hold an error, outside strings and comments.
fn candidates(root: Node, text: &[u8], done: &[usize]) -> Result<Vec<usize>, String> {
    let mut found: Vec<usize> = Vec::new();
    let mut cursor = root.walk();
    let mut stack: Vec<Node> = Vec::new();
    let mut top = root.walk();
    for statement in root.children(&mut top) {
        if statement.is_error() || statement.has_error() {
            stack.push(statement);
        }
    }
    let mut visited = 0u32;
    while let Some(node) = stack.pop() {
        visited += 1;
        if visited >= DEADLINE_CHECK_STRIDE {
            visited = 0;
            if walk_deadline_passed() {
                return Err("the scan for separators ran past the deadline".into());
            }
        }
        let kind = node.kind();
        if kind.contains("string") || kind.contains("template") || kind.ends_with("comment") {
            continue;
        }
        if !node.is_named() && kind == "}" {
            let at = node.start_byte();
            let closes_a_body = bounded_parent(node).is_some_and(|parent| {
                parent.is_error() || matches!(parent.kind(), "class_body" | "enum_class_body")
            });
            if closes_a_body && at > 0 && matches!(text.get(at - 1), Some(b' ' | b'\t')) {
                let mut start = at;
                while start > 0 && matches!(text[start - 1], b' ' | b'\t') {
                    start -= 1;
                }
                if start > 0
                    && ends_a_member(&text[..start])
                    && !matches!(text[start - 1], b'\n' | b'\r')
                    && !done.contains(&(at - 1))
                {
                    found.push(at - 1);
                }
            }
            continue;
        }
        cursor.reset(node);
        if cursor.goto_first_child() {
            loop {
                stack.push(cursor.node());
                if !cursor.goto_next_sibling() {
                    break;
                }
            }
        }
    }
    found.sort_unstable();
    found.dedup();
    Ok(found)
}

/// The top-level statements of the repaired tree a separator was written into.
fn touched(tree: &Tree, edits: &[usize]) -> Vec<TextRange> {
    let root = tree.root_node();
    let mut ranges: Vec<TextRange> = Vec::new();
    for &at in edits {
        let Some(mut node) = root.descendant_for_byte_range(at, at + 1) else {
            continue;
        };
        while let Some(parent) = bounded_parent(node) {
            if parent.id() == root.id() {
                break;
            }
            node = parent;
        }
        if node.id() != root.id() {
            ranges.push(TextRange {
                start_byte: node.start_byte(),
                end_byte: node.end_byte(),
            });
        }
    }
    ranges.sort();
    ranges.dedup();
    ranges
}
