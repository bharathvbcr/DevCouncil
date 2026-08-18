//! Caller attribution for the languages served by the generic declaration arm.
//!
//! A call edge names its caller by qualified name, and that name is a join key:
//! an edge whose `source_symbol` matches no node's `qualified_name` is an
//! orphan, which is the defect SC9 and SC10 were both instances of. So the name
//! this module builds must be *the same string* the symbol emitter builds for
//! the enclosing declaration — not merely a plausible one.
//!
//! `enclosing_callable_qualified` cannot supply it for these languages, and the
//! disagreements are not exotic:
//!
//! * It is blind to node kinds the emitter *does* qualify by. `enclosing_type_name`
//!   matches `class_definition` but not `object_definition`, while
//!   `generic_symbol_kind` maps `object_definition` to a class — so every method
//!   of a Scala `object` is emitted as `f.scala::Registry.register` and would
//!   have had its calls attributed to `f.scala::register`. Swift `protocol`
//!   bodies disagree the same way.
//! * It qualifies a nested callable by its enclosing callable
//!   (`f.swift::outer.inner`), which is right for the grammars that own an arm
//!   in `treesitter.rs` because their emitters do the same. The generic emitter
//!   does not: `generic_enclosing_type` returns `None` as soon as the nearest
//!   declaration ancestor is a function, so a nested `func`/`def` is emitted as
//!   `f.swift::inner`.
//! * It stops only at callables, so a call in a Swift `init` body or a SwiftUI
//!   `var body` — neither of which the generic emitter names — falls all the way
//!   to file scope, discarding the type that does have a symbol.
//!
//! Mirroring the emitter is therefore the requirement, not a preference. The
//! mirror is held to that by `every_caller_symbol_names_an_emitted_symbol` in
//! `tests/langcalls_swift_scala.rs`, which reads the emitter's own output rather
//! than restating the rule.

use tree_sitter::Node;

use crate::treesitter::{get_child_text, get_node_text, is_callee_identity};

/// What the generic declaration arm emits for a node kind.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Emitted {
    /// A function, method or other callable body.
    Callable,
    /// A declaration that *owns* callables — a class, struct, protocol, module.
    Owner,
}

/// The kind of symbol `extract_node`'s generic arm emits for `kind`, if any.
///
/// A transcription of `generic_symbol_kind`, split only by whether the kind is
/// `SymbolKind::Function` — which is the one distinction the emitter's
/// qualification rule turns on. It is deliberately the *whole* table rather than
/// the Swift and Scala subset: a kind missing here would be a declaration the
/// emitter names and this module walks straight past, which is how a caller
/// name silently stops matching its node.
fn emitted_symbol(kind: &str) -> Option<Emitted> {
    Some(match kind {
        "function_definition"
        | "async_function_definition"
        | "function_declaration"
        | "function_item"
        | "method_declaration"
        | "method_definition"
        | "method"
        | "singleton_method"
        | "constructor_declaration"
        | "subroutine_declaration"
        | "function_declarator"
        | "create_function"
        | "create_trigger" => Emitted::Callable,
        "class_declaration"
        | "class_definition"
        | "class_specifier"
        | "class"
        | "object_definition"
        | "singleton_class"
        | "contract_declaration"
        | "library_declaration"
        | "interface_declaration"
        | "protocol_declaration"
        | "struct_specifier"
        | "struct_declaration"
        | "struct_item"
        | "enum_declaration"
        | "enum_specifier"
        | "enum_item"
        | "trait_item"
        | "trait_declaration"
        | "module"
        | "namespace_declaration"
        | "package_declaration"
        | "create_table"
        | "create_view"
        | "create_materialized_view"
        | "create_type"
        | "create_schema" => Emitted::Owner,
        _ => return None,
    })
}

/// Qualified name of the nearest declaration enclosing `node` that the generic
/// arm emits a symbol for, or `None` at file scope.
///
/// Nearest wins whether it is a callable or a type. A Swift `init` body and a
/// SwiftUI `var body` are not declarations the emitter names, so their calls are
/// attributed to the type that encloses them — a symbol that exists, and a
/// strictly better answer than the file, which is where the shared helper lands
/// them. `None` means the file itself is the caller, exactly as elsewhere.
pub(crate) fn enclosing_emitted_symbol(
    node: Node,
    source: &str,
    file_symbol_name: &str,
) -> Option<String> {
    let mut ancestor = node.parent();
    while let Some(parent) = ancestor {
        if emitted_symbol(parent.kind()).is_some() {
            // A declaration the emitter cannot name emits no symbol, so keep
            // walking rather than inventing one.
            if let Some(name) = get_child_text(parent, "name", source).filter(|n| !n.is_empty()) {
                return Some(match emitted_owner(parent, source) {
                    Some(owner) => format!("{file_symbol_name}::{owner}.{name}"),
                    None => format!("{file_symbol_name}::{name}"),
                });
            }
        }
        ancestor = parent.parent();
    }
    None
}

/// The owner segment the emitter puts in front of `node`'s own name.
///
/// A transcription of `generic_enclosing_type`: the nearest declaration
/// ancestor, `None` when that ancestor is itself a callable, and `None` — not a
/// continued walk — when it has no readable name.
fn emitted_owner(node: Node, source: &str) -> Option<String> {
    let mut ancestor = node.parent();
    while let Some(parent) = ancestor {
        if let Some(emitted) = emitted_symbol(parent.kind()) {
            if emitted == Emitted::Callable {
                return None;
            }
            return get_child_text(parent, "name", source).filter(|name| !name.is_empty());
        }
        ancestor = parent.parent();
    }
    None
}

/// Longest receiver expression worth carrying on a call.
///
/// A receiver is looked up as a *variable name* (`receiver_types`,
/// `scoped_receiver_types`), so nothing longer than an identifier can ever
/// match. Swift method chains make that limit matter: the receiver of the
/// outermost `.onAppear` in a SwiftUI `body` is the entire view expression
/// underneath it, so a deeply chained file would store a large fraction of
/// itself once per link.
const MAX_RECEIVER_BYTES: usize = 96;

/// A receiver expression, bounded, or `None` when there is nothing to record.
///
/// Truncation keeps the call *receiver-bearing*, which is the property that
/// matters: a method call whose receiver is dropped becomes indistinguishable
/// from a bare call, and the resolution ladder would then let `.padding()`
/// resolve to a same-file `padding` function at deterministic confidence — the
/// SC9 class of confidently-wrong edge. An over-long or multi-line receiver
/// resolves no better truncated than whole; both miss every lookup and land in
/// the `uninferred_receiver` tier, which is where an unnameable receiver
/// belongs.
pub(crate) fn clamp_receiver(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let single_line = !trimmed.contains('\n');
    if single_line && trimmed.len() <= MAX_RECEIVER_BYTES {
        return Some(trimmed.to_string());
    }
    let head = trimmed.lines().next().unwrap_or(trimmed).trim_end();
    let mut end = head.len().min(MAX_RECEIVER_BYTES);
    while end > 0 && !head.is_char_boundary(end) {
        end -= 1;
    }
    let clipped = &head[..end];
    if clipped.is_empty() {
        return None;
    }
    Some(format!("{clipped}…"))
}

/// A receiver taken from a node, unwrapping Swift's `?` and `!` postfixes.
///
/// `opt?.warm()` and `opt!.warm()` are calls on `opt`, and the binding the
/// resolver would match is keyed by the bare name, so leaving the operator on
/// the text costs a receiver-type resolution for no gain.
pub(crate) fn receiver_from(node: Node, source: &str) -> Option<String> {
    let text = get_node_text(node, source);
    let trimmed = text.trim();
    let unwrapped = trimmed.trim_end_matches(['!', '?']);
    if is_callee_identity(unwrapped) {
        return Some(unwrapped.to_string());
    }
    clamp_receiver(trimmed)
}
