//! Swift call extraction (SC34).
//!
//! Swift reached `extract_node`'s generic arm, which emits declarations and
//! nothing else, so a Swift file produced `Contains` edges and **zero** `Calls`
//! — verified on a two-function file before this module existed. The Python
//! implementation this port replaces recovers Swift calls with a regex, so the
//! blackout was a migration regression rather than a shared gap.
//!
//! The shapes come from tree-sitter-swift's own parse trees rather than from
//! assumption, because several of them are not what the surface syntax suggests:
//!
//! * A call is `call_expression` with **no** `function` field — the callee is
//!   the first named child and the arguments are a sibling `call_suffix`. A
//!   trailing closure lives in that same suffix, so `items.map { … }` and
//!   `run(a) { … } onError: { … }` need no separate handling at all.
//! * `obj.method()` puts a `navigation_expression` in callee position, which
//!   `split_call_target` does not know; the name hangs off `suffix.suffix`.
//!   Optional chaining and force-unwrap change only the *target* subtree, never
//!   the suffix, so `a?.b()` and `a!.b()` fall out of the same path.
//! * `try`, `try?`, `try!` and `await` **wrap** the call rather than standing
//!   between it and its callee, so the callee of `try await foo()` is `foo`.
//!   That is the SC26 shape (`await invoke<Raw>('x')` recorded `await invoke`)
//!   and it is pinned by a test.
//! * `Set<Int>()` is a `constructor_expression`, a different node kind from
//!   `Person(name:)`, which is an ordinary `call_expression`.
//!
//! Everything that names no identifier is dropped rather than recorded as text:
//! `split_call_target` returns `Option` and `is_callee_identity` rejects
//! expressions, which is the SC26/SC32 rule.

use tree_sitter::Node;

use crate::model::{ExtractedCall, ExtractedReference, ReferenceKind};
use crate::treesitter::{
    get_node_text, is_anonymous_callable, is_callee_identity, node_span, split_call_target,
};

use super::scope::{clamp_receiver, enclosing_emitted_symbol, receiver_from};

/// Calls made by Swift code.
pub fn extract_swift_call(
    node: Node,
    source: &str,
    file_symbol_name: &str,
    calls: &mut Vec<ExtractedCall>,
    references: &mut Vec<ExtractedReference>,
) {
    let Some(call) = swift_call_identity(node, source) else {
        return;
    };
    if !is_callee_identity(&call.callee_name) {
        return;
    }
    let enclosing = enclosing_emitted_symbol(node, source, file_symbol_name);
    references.push(ExtractedReference {
        name: call.callee_name.clone(),
        kind: call.kind,
        span: node_span(call.name_node),
        enclosing_symbol: enclosing.clone(),
        assigned_to: swift_assignment_binding(node, source),
    });
    calls.push(ExtractedCall {
        caller_symbol: enclosing,
        callee_name: call.callee_name,
        receiver_expr: call.receiver_expr,
        span: node_span(node),
    });
}

/// One call site's identity: what is called, what it is called on, and the node
/// that carries the name.
struct SwiftCall<'tree> {
    callee_name: String,
    receiver_expr: Option<String>,
    name_node: Node<'tree>,
    kind: ReferenceKind,
}

fn swift_call_identity<'tree>(node: Node<'tree>, source: &str) -> Option<SwiftCall<'tree>> {
    match node.kind() {
        "call_expression" => {
            // `items[0]` is a `call_expression` too — the grammar tells a
            // subscript apart only by the bracket that opens its argument list.
            // Recording it would name the *collection variable* as a callee, so
            // an array named after a function in the same file would resolve to
            // that function at deterministic confidence: a fabricated edge of
            // the SC9 class. Swift's own `subscript` declarations are not
            // symbols either (`generic_symbol_kind` has no `subscript_declaration`),
            // so there is nothing a subscript could correctly point at.
            if swift_is_subscript(node) {
                return None;
            }
            let target = swift_callee_target(node)?;
            // An immediately-invoked closure has no callee identity; recording
            // its text would manufacture a name no symbol can carry (SC32).
            if is_anonymous_callable(target.kind()) || target.kind() == "lambda_literal" {
                return None;
            }
            if target.kind() == "navigation_expression" {
                return swift_navigation_call(target, source);
            }
            let (callee_name, receiver) = split_call_target(target, source)?;
            Some(SwiftCall {
                callee_name,
                receiver_expr: receiver.as_deref().and_then(clamp_receiver),
                name_node: target,
                kind: ReferenceKind::Call,
            })
        }
        // `Set<Int>()`, `Box<Int>(value: 1)`. The constructed type is a field,
        // and the generic arguments are not part of the callee's identity.
        "constructor_expression" => {
            let constructed = node.child_by_field_name("constructed_type")?;
            let (callee_name, receiver_expr) = swift_user_type_identity(constructed, source)?;
            Some(SwiftCall {
                callee_name,
                receiver_expr,
                name_node: constructed,
                kind: ReferenceKind::Constructor,
            })
        }
        _ => None,
    }
}

/// Whether a `call_expression` is really a subscript: `items[0]`, `dict["k"]`.
fn swift_is_subscript(node: Node) -> bool {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() != "call_suffix" {
            continue;
        }
        let mut suffix_cursor = child.walk();
        for grandchild in child.named_children(&mut suffix_cursor) {
            if grandchild.kind() == "value_arguments" {
                return grandchild.child(0).is_some_and(|open| open.kind() == "[");
            }
        }
    }
    false
}

/// The callee subtree of a `call_expression`.
///
/// tree-sitter-swift gives `call_expression` no `function` field: the callee is
/// the first named child and the argument list — parenthesised, a trailing
/// closure, or both — is a `call_suffix` sibling.
fn swift_callee_target<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if !matches!(
            child.kind(),
            "call_suffix" | "comment" | "multiline_comment"
        ) {
            return Some(child);
        }
    }
    None
}

/// `receiver.method()`, including `receiver?.method()` and `receiver!.method()`.
fn swift_navigation_call<'tree>(nav: Node<'tree>, source: &str) -> Option<SwiftCall<'tree>> {
    let suffix = nav.child_by_field_name("suffix")?;
    let name_node = suffix.child_by_field_name("suffix")?;
    let (callee_name, _) = split_call_target(name_node, source)?;
    let target = nav.child_by_field_name("target")?;
    // `Person.init(name:)` constructs a `Person`; recording `init` as the callee
    // would name a symbol that does not exist, because the generic declaration
    // arm emits nothing for an `init_declaration`. `self.init` and `super.init`
    // deliberately produce nothing: the first names the type the call is already
    // inside, and the second names a superclass this node cannot identify.
    if callee_name == "init" {
        let (type_name, qualifier) = swift_init_type(target, source)?;
        return Some(SwiftCall {
            callee_name: type_name,
            receiver_expr: qualifier,
            name_node: target,
            kind: ReferenceKind::Constructor,
        });
    }
    Some(SwiftCall {
        callee_name,
        receiver_expr: receiver_from(target, source),
        name_node,
        kind: ReferenceKind::Call,
    })
}

/// The type named by a `user_type`, and the type or module qualifying it.
fn swift_user_type_identity(ty: Node, source: &str) -> Option<(String, Option<String>)> {
    let mut names: Vec<String> = Vec::new();
    let mut cursor = ty.walk();
    for child in ty.named_children(&mut cursor) {
        if matches!(child.kind(), "type_identifier" | "simple_identifier") {
            names.push(get_node_text(child, source));
        }
    }
    // The last segment is the type; anything before it qualifies the type.
    let name = names.pop()?;
    if !is_callee_identity(&name) {
        return None;
    }
    let qualifier = names.pop().filter(|q| is_callee_identity(q));
    Some((name, qualifier))
}

/// The type a `T.init(…)` constructs.
///
/// Requires the name to be capitalised. This is the one place where a *value*
/// expression could reach the type path — `anything.init(…)` parses the same way
/// — and Swift's naming convention is the only available evidence that the
/// target is a type. Fail-closed: an unrecognised target yields no call rather
/// than a call named `init`, which could never resolve.
fn swift_init_type(target: Node, source: &str) -> Option<(String, Option<String>)> {
    let identity = match target.kind() {
        "simple_identifier" | "type_identifier" => (get_node_text(target, source), None),
        "user_type" => swift_user_type_identity(target, source)?,
        "navigation_expression" => {
            let suffix = target.child_by_field_name("suffix")?;
            let name_node = suffix.child_by_field_name("suffix")?;
            let qualifier = target
                .child_by_field_name("target")
                .and_then(|node| receiver_from(node, source));
            (get_node_text(name_node, source), qualifier)
        }
        _ => return None,
    };
    (starts_capitalised(&identity.0) && is_callee_identity(&identity.0)).then_some(identity)
}

/// Whether a name follows Swift's convention for a type rather than a value.
fn starts_capitalised(name: &str) -> bool {
    name.chars().next().is_some_and(char::is_uppercase)
}

/// The local variable a call's result is bound to, when the grammar proves one.
///
/// This is what lets the resolver bind `let service = Service()` and then
/// resolve `service.run()` by receiver type. The shared `assignment_binding`
/// helper knows neither Swift binding node, so it returns `None` for every Swift
/// call; the walk here is bounded and stops at argument and closure boundaries
/// so an argument's own call (`outer(Inner())`) cannot claim the outer binding.
fn swift_assignment_binding(node: Node, source: &str) -> Option<String> {
    let mut current = node;
    for _ in 0..4 {
        let parent = current.parent()?;
        match parent.kind() {
            // `let service = Service()`
            "property_declaration" => {
                let value = parent.child_by_field_name("value")?;
                if value.id() != current.id() {
                    return None;
                }
                let pattern = parent.child_by_field_name("name")?;
                let bound = pattern.child_by_field_name("bound_identifier")?;
                let name = get_node_text(bound, source);
                return is_callee_identity(&name).then_some(name);
            }
            // `service = Service()`. Only a bare local target binds: `self.x = …`
            // assigns a property, whose type the file-wide receiver map must not
            // learn from a name that is not in scope as a variable.
            "assignment" => {
                let result = parent.child_by_field_name("result")?;
                if result.id() != current.id() {
                    return None;
                }
                let target = parent.child_by_field_name("target")?;
                let name = get_node_text(target, source);
                return is_callee_identity(&name).then_some(name);
            }
            "value_argument" | "value_arguments" | "call_suffix" | "lambda_literal"
            | "statements" | "function_body" => return None,
            _ => current = parent,
        }
    }
    None
}
