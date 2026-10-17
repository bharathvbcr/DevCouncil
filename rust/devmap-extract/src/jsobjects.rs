//! How a JS/TS module-scope object is built.
//!
//! ```ts
//! export const debugLogger = { ...coreDebugLogger, clear: clearLogs };
//! const w = Object.assign({}, core);
//! export const aliased = core;
//! ```
//!
//! Each of these gives the const members its own literal never spells. The
//! resolver can only say which declaration a member call reaches if it knows
//! the *order* the properties were applied in — JavaScript lets the last write
//! win — so this pass records the steps, in source order, and decides nothing.
//! Which file an identifier names, and whether a key is defined by a later
//! source, are the resolver's questions: they depend on the rest of the tree,
//! and the extraction cache keys on `(path, source)` alone.
//!
//! Only a `const` is recorded: a `let` or `var` can be rebound, and the
//! resolver would then be reading a table the program replaced. Anything this
//! pass cannot read statically — a spread of a call's result, a computed key, a
//! conditional source — is recorded as [`JsObjectPart::Unknown`], never
//! skipped, so a reader cannot take its absence for "nothing else was written".

use crate::model::{JsObjectFact, JsObjectPart};
use crate::treesitter::{is_module_level, push_named_children, walk_deadline_passed};
use tree_sitter::Node;

/// How deep object literals may nest inside a spread or `Object.assign` source
/// before the rest is read as unknown. A real spread chain is one or two deep.
const MAX_NESTING: usize = 8;
/// Nodes between deadline checks.
const DEADLINE_STRIDE: usize = 4096;

pub(crate) fn collect(root: Node, source: &str) -> Vec<JsObjectFact> {
    let mut facts = Vec::new();
    let mut worklist = vec![root];
    let mut visited = 0usize;
    while let Some(node) = worklist.pop() {
        visited += 1;
        if visited.is_multiple_of(DEADLINE_STRIDE) && walk_deadline_passed() {
            return Vec::new();
        }
        match node.kind() {
            "variable_declarator" => {
                if let Some(fact) = declaration_fact(node, source) {
                    facts.push(fact);
                }
            }
            "call_expression" => {
                if let Some(fact) = assign_fact(node, source) {
                    facts.push(fact);
                }
            }
            _ => {}
        }
        let mark = worklist.len();
        push_named_children(node, &mut worklist);
        worklist[mark..].reverse();
    }
    facts.sort_by_key(|fact| (fact.start_byte, !fact.declaration));
    facts
}

fn text<'a>(node: Node, source: &'a str) -> &'a str {
    source.get(node.start_byte()..node.end_byte()).unwrap_or("")
}

/// `as T`, `satisfies T`, `(…)` and `x!` do not change which value an
/// expression is.
fn unwrap(mut node: Node) -> Node {
    while matches!(
        node.kind(),
        "as_expression"
            | "satisfies_expression"
            | "parenthesized_expression"
            | "non_null_expression"
    ) {
        match node.named_child(0) {
            Some(inner) => node = inner,
            None => break,
        }
    }
    node
}

/// `name` or `ns.name`: the two spellings the resolver can follow.
fn reference_text(node: Node, source: &str) -> Option<String> {
    match node.kind() {
        "identifier" => Some(text(node, source).to_string()),
        "member_expression" => {
            let object = node.child_by_field_name("object")?;
            let property = node.child_by_field_name("property")?;
            let optional = node
                .children(&mut node.walk())
                .any(|child| child.kind() == "optional_chain");
            (object.kind() == "identifier" && property.kind() == "property_identifier" && !optional)
                .then(|| format!("{}.{}", text(object, source), text(property, source)))
        }
        _ => None,
    }
}

fn declaration_fact(declarator: Node, source: &str) -> Option<JsObjectFact> {
    let name = declarator.child_by_field_name("name")?;
    if name.kind() != "identifier" {
        return None;
    }
    let is_const = declarator
        .parent()
        .filter(|parent| parent.kind() == "lexical_declaration")
        .and_then(|parent| parent.child(0))
        .is_some_and(|keyword| keyword.kind() == "const");
    if !is_const || !is_module_level(declarator) {
        return None;
    }
    let value = unwrap(declarator.child_by_field_name("value")?);
    let (parts, alias_of) = match value.kind() {
        // The one place a function written in place is the symbol
        // `name.key`: the literal is the declarator's own value.
        "object" => (object_parts(value, source, true, 0), None),
        "call_expression" => {
            let parts = assign_parts(value, source, 0)?;
            // `Object.assign(core, …)` returns `core` itself.
            let target = value
                .child_by_field_name("arguments")
                .and_then(|arguments| arguments.named_child(0))
                .map(unwrap)
                .filter(|target| target.kind() == "identifier")
                .map(|target| text(target, source).to_string());
            (parts, target)
        }
        _ => {
            let reference = reference_text(value, source)?;
            (
                vec![JsObjectPart::Spread(reference.clone())],
                Some(reference),
            )
        }
    };
    Some(JsObjectFact {
        name: text(name, source).to_string(),
        start_byte: declarator.start_byte(),
        declaration: true,
        in_callable: false,
        alias_of,
        parts,
    })
}

/// `Object.assign(target, ...sources)` where `target` is an identifier: the
/// sources are written into the value the identifier names.
fn assign_fact(call: Node, source: &str) -> Option<JsObjectFact> {
    if !is_object_assign(call, source) {
        return None;
    }
    let arguments = call.child_by_field_name("arguments")?;
    let target = unwrap(arguments.named_child(0)?);
    if target.kind() != "identifier" {
        return None;
    }
    let mut parts = Vec::new();
    let mut cursor = arguments.walk();
    for argument in arguments.named_children(&mut cursor).skip(1) {
        parts.extend(source_parts(argument, source, 0));
    }
    Some(JsObjectFact {
        name: text(target, source).to_string(),
        start_byte: call.start_byte(),
        declaration: false,
        in_callable: !is_module_level(call),
        alias_of: None,
        parts,
    })
}

fn is_object_assign(call: Node, source: &str) -> bool {
    call.child_by_field_name("function")
        .is_some_and(|function| {
            function.kind() == "member_expression"
                && function
                    .child_by_field_name("object")
                    .is_some_and(|object| text(object, source) == "Object")
                && function
                    .child_by_field_name("property")
                    .is_some_and(|property| text(property, source) == "assign")
        })
}

/// The parts of `Object.assign(target, ...sources)` as one new object: the
/// target is the first step, each source after it. `None` when `node` is not
/// an `Object.assign` call.
fn assign_parts(call: Node, source: &str, depth: usize) -> Option<Vec<JsObjectPart>> {
    if !is_object_assign(call, source) {
        return None;
    }
    let arguments = call.child_by_field_name("arguments")?;
    let mut parts = Vec::new();
    let mut cursor = arguments.walk();
    for argument in arguments.named_children(&mut cursor) {
        parts.extend(source_parts(argument, source, depth));
    }
    if parts.is_empty() {
        parts.push(JsObjectPart::Unknown);
    }
    Some(parts)
}

/// What one spread element or `Object.assign` argument contributes.
fn source_parts(argument: Node, source: &str, depth: usize) -> Vec<JsObjectPart> {
    // `Object.assign({}, ...list)` and `{ ...a }` both name their source as a
    // child of a spread; the argument itself is the source otherwise.
    let expression = if argument.kind() == "spread_element" {
        // A spread *argument* is a list of sources, which is not nameable.
        let in_arguments = argument.parent().is_some_and(|p| p.kind() == "arguments");
        match argument.named_child(0) {
            Some(inner) if !in_arguments => inner,
            _ => return vec![JsObjectPart::Unknown],
        }
    } else {
        argument
    };
    let expression = unwrap(expression);
    if expression.kind() == "object" {
        if depth >= MAX_NESTING {
            return vec![JsObjectPart::Unknown];
        }
        // A literal nested in a source is no declarator's own value, so its
        // functions are not the symbols `name.key` — they are opaque.
        return object_parts(expression, source, false, depth + 1);
    }
    match reference_text(expression, source) {
        Some(reference) => vec![JsObjectPart::Spread(reference)],
        None => vec![JsObjectPart::Unknown],
    }
}

fn object_parts(object: Node, source: &str, owned: bool, depth: usize) -> Vec<JsObjectPart> {
    let mut parts = Vec::new();
    let mut cursor = object.walk();
    for member in object.named_children(&mut cursor) {
        match member.kind() {
            "comment" => {}
            "spread_element" => parts.extend(source_parts(member, source, depth)),
            "shorthand_property_identifier" => {
                let name = text(member, source).to_string();
                parts.push(JsObjectPart::Forward {
                    key: name.clone(),
                    value: name,
                });
            }
            "method_definition" => match member
                .child_by_field_name("name")
                .and_then(|name| key_text(name, source))
            {
                Some(key) if owned => parts.push(JsObjectPart::Inline(key)),
                Some(key) => parts.push(JsObjectPart::Opaque(key)),
                None => parts.push(JsObjectPart::Unknown),
            },
            "pair" => {
                let Some(key) = member
                    .child_by_field_name("key")
                    .and_then(|key| key_text(key, source))
                else {
                    parts.push(JsObjectPart::Unknown);
                    continue;
                };
                let value = member.child_by_field_name("value").map(unwrap);
                parts.push(match value {
                    Some(value)
                        if matches!(
                            value.kind(),
                            "arrow_function"
                                | "function_expression"
                                | "function"
                                | "generator_function"
                        ) =>
                    {
                        if owned {
                            JsObjectPart::Inline(key)
                        } else {
                            JsObjectPart::Opaque(key)
                        }
                    }
                    Some(value) if value.kind() == "identifier" && key != "__proto__" => {
                        JsObjectPart::Forward {
                            key,
                            value: text(value, source).to_string(),
                        }
                    }
                    _ if key == "__proto__" => JsObjectPart::Unknown,
                    _ => JsObjectPart::Opaque(key),
                });
            }
            _ => parts.push(JsObjectPart::Unknown),
        }
    }
    parts
}

/// The static name of a property key; `None` for a computed one.
fn key_text(key: Node, source: &str) -> Option<String> {
    match key.kind() {
        "property_identifier" | "number" => Some(text(key, source).to_string()),
        "string" => {
            let raw = text(key, source);
            let inner = raw.get(1..raw.len().checked_sub(1)?)?;
            (!inner.contains('\\')).then(|| inner.to_string())
        }
        _ => None,
    }
}
