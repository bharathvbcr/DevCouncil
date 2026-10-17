//! Site enumeration: every position in a Rust file at which rust-analyzer is
//! asked where something is defined.
//!
//! The walk is the labeller's own, over `tree-sitter-rust`, and is not
//! DevMap's: a site DevMap's extractor never saw must still be asked about, or
//! the labels would only ever contain the sites DevMap already finds. What the
//! grammar cannot see is counted rather than left out of the account: a macro's
//! arguments are a token tree, not parsed expressions, so a call inside one is
//! not a site. The identifiers each macro spells are listed in
//! [`FileFacts::macro_idents`], so the run can refuse to call the enclosing
//! function fully read when one of them names a function of the corpus.

use std::collections::{BTreeMap, BTreeSet};

use tree_sitter::{Node, Parser};

use crate::classify::SiteShape;

/// What kind of position a [`Site`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SiteKind {
    /// A call or method call.
    Call,
    /// A function or method used as a value.
    Value,
    /// An operator, `?` or an index: an implicit call of an operator trait.
    Operator,
}

/// One position to ask about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site {
    pub kind: SiteKind,
    pub shape: SiteShape,
    /// The name written at the site; the operator text for an operator.
    pub name: String,
    /// The byte rust-analyzer is asked about: the callee's own name, never its
    /// receiver, so `a.run()` is asked about `run` and not about `a`.
    pub query_byte: usize,
    /// Where the site begins, for naming its enclosing function.
    pub site_byte: usize,
}

/// A `fn` declaration, found by the byte its name starts at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FnDecl {
    pub name: String,
    /// Declared in a `trait` body (a signature or a default method).
    pub in_trait: bool,
}

/// Everything one file contributes.
#[derive(Debug, Clone, Default)]
pub struct FileFacts {
    pub sites: Vec<Site>,
    /// Function declarations by the byte their name starts at.
    pub fn_decls: BTreeMap<usize, FnDecl>,
    /// Names a `use ... as <alias>` introduces: a call can spell one of these
    /// and still reach a function with another name.
    pub aliases: BTreeSet<String>,
    /// Start bytes of calls whose callee is neither a path nor a field.
    pub unsupported_callees: Vec<usize>,
    /// Every macro invocation the walk met; none of their arguments is read.
    pub macro_invocations: usize,
    /// For each macro invocation and `macro_rules!` definition that spells any
    /// identifier in its tokens: `(start byte, sorted distinct identifiers)`.
    /// The tokens are never parsed, so a call, or a function named by value,
    /// inside one is invisible to the walk and visible to DevMap.
    pub macro_idents: Vec<(usize, Vec<String>)>,
    /// Names this file defines with `macro_rules!`.
    pub macro_defs: BTreeSet<String>,
    /// Every invocation: `(start byte, macro name)`. An invocation of a macro
    /// the corpus defines expands to calls nobody wrote at the site.
    pub macro_calls: Vec<(usize, String)>,
    /// The grammar recovered from a syntax error: the walk may have missed
    /// sites, so nothing in this file may be called fully read.
    pub has_error: bool,
}

/// The trait methods an operator token can reach. Empty for a token that is
/// not an overloadable operator (`&&`, `||`).
pub fn operator_candidates(token: &str) -> &'static [&'static str] {
    match token {
        "+" => &["add"],
        "+=" => &["add_assign"],
        "-" => &["sub", "neg"],
        "-=" => &["sub_assign"],
        "*" => &["deref", "deref_mut", "mul"],
        "*=" => &["mul_assign"],
        "/" => &["div"],
        "/=" => &["div_assign"],
        "%" => &["rem"],
        "%=" => &["rem_assign"],
        "&" => &["bitand"],
        "&=" => &["bitand_assign"],
        "|" => &["bitor"],
        "|=" => &["bitor_assign"],
        "^" => &["bitxor"],
        "^=" => &["bitxor_assign"],
        "<<" => &["shl"],
        "<<=" => &["shl_assign"],
        ">>" => &["shr"],
        ">>=" => &["shr_assign"],
        "!" => &["not"],
        "==" => &["eq"],
        "!=" => &["ne", "eq"],
        "<" => &["lt", "partial_cmp"],
        "<=" => &["le", "partial_cmp"],
        ">" => &["gt", "partial_cmp"],
        ">=" => &["ge", "partial_cmp"],
        "[" => &["index", "index_mut"],
        "?" => &["from", "branch", "from_residual"],
        _ => &[],
    }
}

fn text<'a>(node: Node, source: &'a str) -> &'a str {
    source.get(node.start_byte()..node.end_byte()).unwrap_or("")
}

fn compact(raw: &str) -> String {
    raw.split_whitespace().collect()
}

/// Walk `source` and enumerate its sites and declarations. `references` adds
/// functions used as values to the sites. `Err` only when the grammar could
/// not be loaded or the parse produced no tree.
pub fn scan(source: &str, references: bool) -> Result<FileFacts, String> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .map_err(|e| format!("tree-sitter-rust: {e}"))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| "tree-sitter produced no tree".to_string())?;
    let root = tree.root_node();
    let mut facts = FileFacts {
        has_error: root.has_error(),
        ..FileFacts::default()
    };
    // An explicit worklist: a deeply nested file must not exhaust the stack.
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        match node.kind() {
            "function_item" | "function_signature_item" => declaration(node, source, &mut facts),
            "use_as_clause" => {
                if let Some(alias) = node.child_by_field_name("alias") {
                    facts.aliases.insert(text(alias, source).to_string());
                }
            }
            "macro_invocation" => {
                facts.macro_invocations += 1;
                list_identifiers(node, node.child_by_field_name("macro"), source, &mut facts);
                if let Some(name) = node.child_by_field_name("macro") {
                    let last = text(name, source).rsplit("::").next().unwrap_or("").trim();
                    facts
                        .macro_calls
                        .push((node.start_byte(), last.to_string()));
                }
                // Its arguments are tokens, not expressions.
                continue;
            }
            "macro_definition" => {
                let name = node.child_by_field_name("name");
                if let Some(name) = name {
                    facts.macro_defs.insert(text(name, source).to_string());
                }
                list_identifiers(node, name, source, &mut facts);
                continue;
            }
            "call_expression" => call(node, source, &mut facts),
            "binary_expression" | "compound_assignment_expr" => {
                if let Some(op) = node.child_by_field_name("operator") {
                    operator(op, node, source, &mut facts);
                }
            }
            "unary_expression" => {
                if let Some(op) = node.child(0).filter(|c| !c.is_named()) {
                    operator(op, node, source, &mut facts);
                }
            }
            "index_expression" => {
                let bracket = (0..node.child_count())
                    .filter_map(|i| node.child(i))
                    .find(|c| c.kind() == "[");
                if let Some(op) = bracket {
                    operator(op, node, source, &mut facts);
                }
            }
            "try_expression" => {
                let last = node.child(node.child_count().saturating_sub(1));
                if let Some(op) = last.filter(|c| c.kind() == "?") {
                    operator(op, node, source, &mut facts);
                }
            }
            "identifier" | "scoped_identifier" if references => value(node, source, &mut facts),
            _ => {}
        }
        for i in (0..node.child_count()).rev() {
            if let Some(child) = node.child(i) {
                stack.push(child);
            }
        }
    }
    facts.sites.sort_by_key(|s| (s.query_byte, s.site_byte));
    facts.macro_idents.sort();
    facts.unsupported_callees.sort_unstable();
    Ok(facts)
}

fn declaration(node: Node, source: &str, facts: &mut FileFacts) {
    let Some(name) = node.child_by_field_name("name") else {
        return;
    };
    let in_trait = node
        .parent()
        .and_then(|body| body.parent())
        .is_some_and(|owner| owner.kind() == "trait_item");
    facts.fn_decls.insert(
        name.start_byte(),
        FnDecl {
            name: text(name, source).to_string(),
            in_trait,
        },
    );
}

fn call(node: Node, source: &str, facts: &mut FileFacts) {
    let Some(mut callee) = node.child_by_field_name("function") else {
        facts.unsupported_callees.push(node.start_byte());
        return;
    };
    while callee.kind() == "generic_function" {
        match callee.child_by_field_name("function") {
            Some(inner) => callee = inner,
            None => break,
        }
    }
    let site = |shape, name: Node| Site {
        kind: SiteKind::Call,
        shape,
        name: text(name, source).to_string(),
        query_byte: name.start_byte(),
        site_byte: node.start_byte(),
    };
    match callee.kind() {
        "identifier" => facts.sites.push(site(SiteShape::BareCall, callee)),
        "scoped_identifier" => match callee.child_by_field_name("name") {
            Some(name) => {
                let qualifier = callee
                    .child_by_field_name("path")
                    .map(|p| compact(text(p, source)))
                    .unwrap_or_default();
                facts
                    .sites
                    .push(site(SiteShape::PathCall { qualifier }, name));
            }
            None => facts.unsupported_callees.push(node.start_byte()),
        },
        "field_expression" => {
            let field = callee
                .child_by_field_name("field")
                .filter(|f| f.kind() == "field_identifier");
            match (field, callee.child_by_field_name("value")) {
                (Some(field), Some(receiver)) => facts.sites.push(site(
                    SiteShape::MethodCall {
                        receiver: text(receiver, source).trim().to_string(),
                    },
                    field,
                )),
                _ => facts.unsupported_callees.push(node.start_byte()),
            }
        }
        _ => facts.unsupported_callees.push(node.start_byte()),
    }
}

fn operator(op: Node, owner: Node, source: &str, facts: &mut FileFacts) {
    let token = text(op, source);
    if operator_candidates(token).is_empty() {
        return;
    }
    facts.sites.push(Site {
        kind: SiteKind::Operator,
        shape: SiteShape::Operator,
        name: token.to_string(),
        query_byte: op.start_byte(),
        site_byte: owner.start_byte(),
    });
}

/// A function named where a value is expected: an argument, an element, a
/// field's value, a `let` or assignment right-hand side, a `return`, a closure
/// body, a borrow. The callee of a call is a `function` child of the call, not
/// one of these.
fn value(node: Node, source: &str, facts: &mut FileFacts) {
    let Some(parent) = node.parent() else {
        return;
    };
    let is_field = |name: &str| {
        parent
            .child_by_field_name(name)
            .is_some_and(|child| child.id() == node.id())
    };
    let in_value_position = match parent.kind() {
        "arguments"
        | "array_expression"
        | "tuple_expression"
        | "return_expression"
        | "parenthesized_expression"
        | "reference_expression" => true,
        "field_initializer" => is_field("value"),
        "let_declaration" | "const_item" | "static_item" => is_field("value"),
        "assignment_expression" => is_field("right"),
        "closure_expression" => is_field("body"),
        _ => false,
    };
    if !in_value_position {
        return;
    }
    let (name, qualifier) = if node.kind() == "scoped_identifier" {
        let Some(name) = node.child_by_field_name("name") else {
            return;
        };
        let qualifier = node
            .child_by_field_name("path")
            .map(|p| compact(text(p, source)));
        (name, qualifier)
    } else {
        (node, None)
    };
    facts.sites.push(Site {
        kind: SiteKind::Value,
        shape: SiteShape::ValuePath { qualifier },
        name: text(name, source).to_string(),
        query_byte: name.start_byte(),
        site_byte: node.start_byte(),
    });
}

/// Record every identifier a macro invocation or definition spells, except its
/// own name (`skip`). Nothing in a token tree is parsed, so a call, a method
/// call or a function named by value inside one is only an identifier here.
fn list_identifiers(item: Node, skip: Option<Node>, source: &str, facts: &mut FileFacts) {
    let mut found = BTreeSet::new();
    let mut stack: Vec<Node> = (0..item.child_count())
        .filter_map(|i| item.child(i))
        .collect();
    while let Some(node) = stack.pop() {
        if skip.is_some_and(|s| s.id() == node.id()) {
            continue;
        }
        if node.kind() == "identifier" {
            found.insert(text(node, source).to_string());
        }
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                stack.push(child);
            }
        }
    }
    if !found.is_empty() {
        facts
            .macro_idents
            .push((item.start_byte(), found.into_iter().collect()));
    }
}
