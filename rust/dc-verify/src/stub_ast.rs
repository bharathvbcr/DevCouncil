//! AST-aware stub and test-rigor detection for Rust, Go, Python and
//! TypeScript, and stub detection alone for Java and C#.
//!
//! `rigor::detect_stubs` used to be a per-added-line substring match, which is
//! what the Python AST detector it replaced had warned against: `todo!()`
//! inside a string literal blocked a task, an empty function body passed, and
//! a newly added test that asserted nothing or was skipped outright was
//! invisible. This module parses the changed file with the same tree-sitter
//! grammars `devmap-extract` builds and answers those questions from the tree.
//!
//! It only reports constructs the diff introduced: a placeholder whose line was
//! added, or a function or test whose declaration line was added. Everything
//! else in the file is somebody else's decision, exactly as for the substring
//! gate.
//!
//! Java and C# get the placeholder and empty-body checks and not the test
//! ones — no JUnit or NUnit skip or assertion is read — which is why
//! [`Lang::has_test_rigor`] exists: a report must not count a file toward the
//! test checks when none ran on it.
//!
//! When the file cannot be parsed cleanly — a language this module does not
//! know, a source it was not given, a syntax error — [`analyze`] returns
//! `None` and the caller falls back to the substring gate, whose findings are
//! labelled [`Strength::Derived`]. A parsed finding is [`Strength::Proven`],
//! except assert-free tests: "no assertion in this body" is a fact about the
//! tree, but "this test checks nothing" is not, because the body may call a
//! helper that asserts.

use std::collections::HashSet;

use tree_sitter::{Language, Node, Parser};

use crate::rigor::{STUB_MARKERS, Severity, Strength, thrown_placeholder};

/// The gate names this module reports under, beside `stub_detection`.
pub const GATE_ASSERT_FREE_TEST: &str = "assert_free_test";
pub const GATE_SKIPPED_TEST: &str = "skipped_test";

/// A source larger than this is not parsed; the substring gate covers it.
pub const MAX_SOURCE_BYTES: usize = 2 << 20;

/// The languages this module parses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Rust,
    Go,
    Python,
    TypeScript,
    Tsx,
    Java,
    CSharp,
}

impl Lang {
    /// The language a repository path is written in, by extension.
    pub fn for_path(path: &str) -> Option<Lang> {
        let name = path.rsplit('/').next().unwrap_or(path);
        let (_, ext) = name.rsplit_once('.')?;
        match ext {
            "rs" => Some(Lang::Rust),
            "go" => Some(Lang::Go),
            "py" | "pyi" => Some(Lang::Python),
            "ts" | "mts" | "cts" => Some(Lang::TypeScript),
            "tsx" => Some(Lang::Tsx),
            "java" => Some(Lang::Java),
            "cs" => Some(Lang::CSharp),
            _ => None,
        }
    }

    /// Whether the skipped- and assert-free-test checks exist for this
    /// language. Java and C# are parsed for placeholders only.
    pub fn has_test_rigor(self) -> bool {
        !matches!(self, Lang::Java | Lang::CSharp)
    }

    fn grammar(self) -> Language {
        match self {
            Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
            Lang::Go => tree_sitter_go::LANGUAGE.into(),
            Lang::Python => tree_sitter_python::LANGUAGE.into(),
            Lang::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Lang::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Lang::Java => tree_sitter_java::LANGUAGE.into(),
            Lang::CSharp => tree_sitter_c_sharp::LANGUAGE.into(),
        }
    }
}

/// One thing the tree showed. The caller attaches path and evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AstFinding {
    pub gate: &'static str,
    pub severity: Severity,
    pub strength: Strength,
    /// 1-based line in the post-change file.
    pub line: u32,
    /// The declaration line of the function a placeholder sits in, so an
    /// allow-stub marker above the function covers what is inside it. `None`
    /// for findings already anchored on a declaration.
    pub scope_line: Option<u32>,
    pub message: String,
}

/// Parses `source` and reports the stubs and weak tests the diff introduced.
///
/// `added` holds the 1-based post-change line numbers the diff added. `None`
/// means the file was not analysed — too large, unparseable, or carrying a
/// syntax error — and the caller must fall back rather than read it as clean.
pub fn analyze(lang: Lang, source: &str, added: &HashSet<u32>) -> Option<Vec<AstFinding>> {
    if source.len() > MAX_SOURCE_BYTES {
        return None;
    }
    let mut parser = Parser::new();
    parser.set_language(&lang.grammar()).ok()?;
    let tree = parser.parse(source, None)?;
    let root = tree.root_node();
    if root.has_error() {
        return None;
    }
    let mut walker = Walker {
        lang,
        src: source.as_bytes(),
        added,
        out: Vec::new(),
    };
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        walker.visit(node);
        let mut cursor = node.walk();
        stack.extend(node.named_children(&mut cursor));
    }
    walker
        .out
        .sort_by(|a, b| a.line.cmp(&b.line).then(a.gate.cmp(b.gate)));
    walker.out.dedup();
    Some(walker.out)
}

struct Walker<'a> {
    lang: Lang,
    src: &'a [u8],
    added: &'a HashSet<u32>,
    out: Vec<AstFinding>,
}

fn line_of(node: Node<'_>) -> u32 {
    u32::try_from(node.start_position().row)
        .unwrap_or(u32::MAX - 1)
        .saturating_add(1)
}

fn is_comment(node: Node<'_>) -> bool {
    matches!(node.kind(), "line_comment" | "block_comment" | "comment")
}

/// Every descendant of `node`, including itself.
fn descendants(node: Node<'_>) -> Vec<Node<'_>> {
    let mut out = Vec::new();
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        out.push(n);
        let mut cursor = n.walk();
        stack.extend(n.children(&mut cursor));
    }
    out
}

fn contains_marker(text: &str) -> bool {
    let lowered = text.to_ascii_lowercase();
    STUB_MARKERS.iter().any(|m| lowered.contains(m))
}

/// Whether a string literal's text reads as "this is not written yet".
fn placeholder_text(text: &str) -> bool {
    let lowered = text.to_ascii_lowercase();
    [
        "todo",
        "not implemented",
        "unimplemented",
        "fixme",
        "not yet implemented",
    ]
    .iter()
    .any(|m| lowered.contains(m))
}

/// The last `::`/`.` segment of a path-like name.
fn last_segment(text: &str) -> &str {
    text.rsplit([':', '.']).next().unwrap_or(text)
}

impl<'a> Walker<'a> {
    fn text(&self, node: Node<'_>) -> &'a str {
        node.utf8_text(self.src).unwrap_or("")
    }

    fn is_added(&self, node: Node<'_>) -> bool {
        self.added.contains(&line_of(node))
    }

    fn push(
        &mut self,
        gate: &'static str,
        severity: Severity,
        strength: Strength,
        line: u32,
        message: String,
    ) {
        self.out.push(AstFinding {
            gate,
            severity,
            strength,
            line,
            scope_line: None,
            message,
        });
    }

    fn placeholder(&mut self, node: Node<'_>, what: &str) {
        self.placeholder_finding(
            node,
            Severity::Blocking,
            format!("added code whose body is `{what}`; the task is not implemented"),
        );
    }

    /// A placeholder at `node`, attributed to the function it sits in.
    fn placeholder_finding(&mut self, node: Node<'_>, severity: Severity, message: String) {
        self.push(
            "stub_detection",
            severity,
            Strength::Proven,
            line_of(node),
            message,
        );
        let scope = enclosing_function(node).map(line_of);
        if let Some(last) = self.out.last_mut() {
            last.scope_line = scope;
        }
    }

    /// An added function whose body does nothing. Advisory: a no-op is often
    /// the correct implementation of a hook or a trait method. Blocking when a
    /// comment inside it says it is unfinished, which is the one reading of an
    /// empty body that is not ambiguous.
    ///
    /// `func` is where the finding is reported: the declaration, or for Java
    /// and C# its name, since their declaration node begins at the annotations
    /// above it.
    fn empty_body(&mut self, func: Node<'_>, body: Node<'_>, what: &str) {
        if !self.is_added(func) {
            return;
        }
        let unfinished = descendants(body)
            .into_iter()
            .any(|n| is_comment(n) && contains_marker(self.text(n)));
        if unfinished {
            self.push(
                "stub_detection",
                Severity::Blocking,
                Strength::Proven,
                line_of(func),
                format!("added a function whose body is {what} and marked unfinished"),
            );
        } else {
            self.push(
                "stub_detection",
                Severity::Advisory,
                Strength::Proven,
                line_of(func),
                format!("added a function whose body is {what}"),
            );
        }
    }

    fn skipped(&mut self, at: Node<'_>, how: &str) {
        self.push(
            GATE_SKIPPED_TEST,
            Severity::Advisory,
            Strength::Proven,
            line_of(at),
            format!("added a test that is skipped ({how}); it proves nothing until it runs"),
        );
    }

    fn assert_free(&mut self, at: Node<'_>) {
        self.push(
            GATE_ASSERT_FREE_TEST,
            Severity::Advisory,
            Strength::Derived,
            line_of(at),
            "added a test whose body contains no assertion; unless a helper it calls asserts, \
             it passes whatever the code does"
                .to_string(),
        );
    }

    fn visit(&mut self, node: Node<'_>) {
        match self.lang {
            Lang::Rust => self.visit_rust(node),
            Lang::Go => self.visit_go(node),
            Lang::Python => self.visit_python(node),
            Lang::TypeScript | Lang::Tsx => self.visit_ts(node),
            Lang::Java | Lang::CSharp => self.visit_c_family(node),
        }
    }

    // ---- Rust ----

    fn visit_rust(&mut self, node: Node<'_>) {
        match node.kind() {
            "macro_invocation" => {
                if !self.is_added(node) {
                    return;
                }
                let name = node
                    .child_by_field_name("macro")
                    .map(|m| last_segment(self.text(m)))
                    .unwrap_or("");
                match name {
                    "todo" | "unimplemented" => self.placeholder(node, &format!("{name}!()")),
                    "panic" if self.has_placeholder_string(node) => {
                        self.placeholder(node, "panic!(\"TODO\")")
                    }
                    _ => {}
                }
            }
            "function_item" => self.rust_function(node),
            _ => {}
        }
    }

    fn rust_function(&mut self, func: Node<'_>) {
        let Some(body) = func.child_by_field_name("body") else {
            return;
        };
        // The attributes above the function, with the `#[` `]` stripped.
        let mut attrs: Vec<(Node<'_>, &str)> = Vec::new();
        let mut prev = func.prev_named_sibling();
        while let Some(p) = prev {
            match p.kind() {
                "attribute_item" => {
                    let inner = self
                        .text(p)
                        .trim_start_matches("#[")
                        .trim_end_matches(']')
                        .trim();
                    attrs.push((p, inner));
                }
                k if is_comment_kind(k) => {}
                _ => break,
            }
            prev = p.prev_named_sibling();
        }
        let is_test = attrs.iter().any(|(_, inner)| {
            let path = inner.split(['(', '=']).next().unwrap_or("").trim();
            last_segment(path) == "test"
        });
        if !is_test {
            if body_is_empty(body) {
                self.empty_body(func, body, "empty");
            }
            return;
        }
        for (node, inner) in &attrs {
            let name = inner.split(['(', '=', ' ']).next().unwrap_or("");
            if name == "ignore" && (self.is_added(*node) || self.is_added(func)) {
                self.skipped(*node, "#[ignore]");
            }
        }
        let should_panic = attrs
            .iter()
            .any(|(_, inner)| inner.starts_with("should_panic"));
        if self.is_added(func) && !should_panic && !self.rust_asserts(body) {
            self.assert_free(func);
        }
    }

    fn rust_asserts(&self, body: Node<'_>) -> bool {
        descendants(body).into_iter().any(|n| match n.kind() {
            "try_expression" => true,
            "macro_invocation" => {
                let name = n
                    .child_by_field_name("macro")
                    .map(|m| last_segment(self.text(m)))
                    .unwrap_or("");
                name.starts_with("assert")
                    || name.starts_with("debug_assert")
                    || name.starts_with("prop_assert")
                    || matches!(name, "panic" | "unreachable")
            }
            "call_expression" => n
                .child_by_field_name("function")
                .is_some_and(|f| asserting_name(last_segment(self.text(f)))),
            _ => false,
        })
    }

    fn has_placeholder_string(&self, node: Node<'_>) -> bool {
        descendants(node).into_iter().any(|n| {
            matches!(
                n.kind(),
                "string_literal" | "raw_string_literal" | "interpreted_string_literal" | "string"
            ) && placeholder_text(self.text(n))
        })
    }

    // ---- Go ----

    fn visit_go(&mut self, node: Node<'_>) {
        match node.kind() {
            "call_expression" => {
                let is_panic = node
                    .child_by_field_name("function")
                    .is_some_and(|f| f.kind() == "identifier" && self.text(f) == "panic");
                if is_panic && self.is_added(node) && self.has_placeholder_string(node) {
                    self.placeholder(node, "panic(\"TODO\")");
                }
            }
            "function_declaration" | "method_declaration" => self.go_function(node),
            _ => {}
        }
    }

    fn go_function(&mut self, func: Node<'_>) {
        let Some(body) = func.child_by_field_name("body") else {
            return;
        };
        let name = func
            .child_by_field_name("name")
            .map(|n| self.text(n))
            .unwrap_or("");
        let t_name = if func.kind() == "function_declaration" && name.starts_with("Test") {
            self.go_testing_t(func)
        } else {
            None
        };
        let Some(t) = t_name else {
            if body_is_empty(body) {
                self.empty_body(func, body, "empty");
            }
            return;
        };
        // An unconditional skip is the body's first statement. A skip under
        // an `if` (testing.Short, a missing tool) is how this repository runs
        // tests conditionally, and is not flagged.
        let first_call = first_statement(body)
            .and_then(|stmt| stmt.named_child(0))
            .filter(|c| c.kind() == "call_expression");
        if let Some(call) = first_call
            && let Some((operand, field)) = self.selector(call)
            && operand == t
            && matches!(field, "Skip" | "Skipf" | "SkipNow")
            && self.is_added(call)
        {
            self.skipped(call, &format!("{t}.{field} as its first statement"));
        }
        if self.is_added(func) && !self.go_asserts(body, &t) {
            self.assert_free(func);
        }
    }

    fn go_testing_t(&self, func: Node<'_>) -> Option<String> {
        let params = func.child_by_field_name("parameters")?;
        let mut cursor = params.walk();
        for p in params.named_children(&mut cursor) {
            let ty = p
                .child_by_field_name("type")
                .map(|t| self.text(t))
                .unwrap_or("");
            if ty.replace(' ', "") == "*testing.T" {
                return p
                    .child_by_field_name("name")
                    .map(|n| self.text(n).to_string());
            }
        }
        None
    }

    fn selector<'n>(&self, call: Node<'n>) -> Option<(&'a str, &'a str)> {
        let f = call.child_by_field_name("function")?;
        if f.kind() != "selector_expression" {
            return None;
        }
        let operand = self.text(f.child_by_field_name("operand")?);
        let field = self.text(f.child_by_field_name("field")?);
        Some((operand, field))
    }

    fn go_asserts(&self, body: Node<'_>, t: &str) -> bool {
        descendants(body).into_iter().any(|n| {
            if n.kind() != "call_expression" {
                return false;
            }
            if let Some((operand, field)) = self.selector(n) {
                if operand == t
                    && matches!(
                        field,
                        "Error" | "Errorf" | "Fatal" | "Fatalf" | "Fail" | "FailNow" | "Run"
                    )
                {
                    return true;
                }
                if matches!(operand, "require" | "assert" | "is" | "qt") || asserting_name(field) {
                    return true;
                }
            } else if n
                .child_by_field_name("function")
                .is_some_and(|f| asserting_name(self.text(f)))
            {
                return true;
            }
            // A helper handed the test's T can fail it.
            n.child_by_field_name("arguments").is_some_and(|args| {
                let mut cursor = args.walk();
                args.named_children(&mut cursor)
                    .any(|a| a.kind() == "identifier" && self.text(a) == t)
            })
        })
    }

    // ---- Python ----

    fn visit_python(&mut self, node: Node<'_>) {
        if node.kind() == "function_definition" {
            self.python_function(node);
        }
    }

    fn python_function(&mut self, func: Node<'_>) {
        let Some(body) = func.child_by_field_name("body") else {
            return;
        };
        let name = func
            .child_by_field_name("name")
            .map(|n| self.text(n))
            .unwrap_or("");
        let decorators: Vec<Node<'_>> = func
            .parent()
            .filter(|p| p.kind() == "decorated_definition")
            .map(|p| {
                let mut cursor = p.walk();
                p.named_children(&mut cursor)
                    .filter(|c| c.kind() == "decorator")
                    .collect()
            })
            .unwrap_or_default();
        let statements = python_statements(body);

        if name.starts_with("test") {
            for d in &decorators {
                let text = self.text(*d).trim_start_matches('@').trim();
                if python_unconditional_skip(text) && (self.is_added(*d) || self.is_added(func)) {
                    self.skipped(*d, &format!("@{}", text.split('(').next().unwrap_or(text)));
                }
            }
            if let Some(first) = statements.first() {
                let t = self.text(*first);
                if (t.starts_with("pytest.skip(") || t.starts_with("self.skipTest("))
                    && self.is_added(*first)
                {
                    self.skipped(*first, t.split('(').next().unwrap_or(t));
                }
            }
            if self.is_added(func) && !self.python_asserts(body) {
                self.assert_free(func);
            }
            return;
        }

        let abstract_ = decorators.iter().any(|d| {
            let t = self.text(*d);
            t.contains("abstractmethod")
                || t.contains("abstractproperty")
                || last_segment(t) == "overload"
        });
        if abstract_ || statements.len() != 1 {
            return;
        }
        let only = statements[0];
        match only.kind() {
            "pass_statement" => self.empty_body(func, body, "`pass`"),
            "raise_statement" => {
                let raised = only.named_child(0);
                let is_nie = raised.is_some_and(|r| {
                    let target = if r.kind() == "call" {
                        r.child_by_field_name("function")
                    } else {
                        Some(r)
                    };
                    target.is_some_and(|t| self.text(t) == "NotImplementedError")
                });
                if is_nie && self.is_added(only) {
                    self.placeholder(only, "raise NotImplementedError");
                }
            }
            _ => {}
        }
    }

    fn python_asserts(&self, body: Node<'_>) -> bool {
        descendants(body).into_iter().any(|n| match n.kind() {
            "assert_statement" | "raise_statement" => true,
            "call" => n.child_by_field_name("function").is_some_and(|f| {
                let name = last_segment(self.text(f));
                asserting_name(name) || matches!(name, "raises" | "fail" | "warns")
            }),
            _ => false,
        })
    }

    // ---- TypeScript ----

    fn visit_ts(&mut self, node: Node<'_>) {
        match node.kind() {
            "throw_statement" => {
                if !self.is_added(node) {
                    return;
                }
                let Some(created) = node.named_child(0).filter(|c| c.kind() == "new_expression")
                else {
                    return;
                };
                let is_error = created
                    .child_by_field_name("constructor")
                    .is_some_and(|c| self.text(c).ends_with("Error"));
                if is_error && self.has_ts_placeholder_string(created) {
                    self.placeholder(node, "throw new Error(\"not implemented\")");
                }
            }
            "function_declaration" | "generator_function_declaration" | "method_definition" => {
                let name = node
                    .child_by_field_name("name")
                    .map(|n| self.text(n))
                    .unwrap_or("");
                if name == "constructor" {
                    // `constructor(private readonly x: X) {}` is the
                    // parameter-property idiom: an empty body is the whole
                    // implementation.
                    return;
                }
                if let Some(body) = node.child_by_field_name("body")
                    && body_is_empty(body)
                {
                    self.empty_body(node, body, "empty");
                }
            }
            "call_expression" => self.ts_test_call(node),
            _ => {}
        }
    }

    fn has_ts_placeholder_string(&self, node: Node<'_>) -> bool {
        descendants(node).into_iter().any(|n| {
            matches!(n.kind(), "string" | "template_string") && placeholder_text(self.text(n))
        })
    }

    fn ts_test_call(&mut self, call: Node<'_>) {
        if !self.is_added(call) {
            return;
        }
        let Some(callee) = call.child_by_field_name("function") else {
            return;
        };
        let callee = self.text(callee);
        let skipped = matches!(
            callee,
            "it.skip"
                | "test.skip"
                | "describe.skip"
                | "context.skip"
                | "it.todo"
                | "test.todo"
                | "xit"
                | "xtest"
                | "xdescribe"
        );
        if skipped {
            self.skipped(call, callee);
            return;
        }
        if !matches!(
            callee,
            "it" | "test" | "it.only" | "test.only" | "it.concurrent" | "test.concurrent"
        ) {
            return;
        }
        let Some(args) = call.child_by_field_name("arguments") else {
            return;
        };
        let mut cursor = args.walk();
        let callback = args.named_children(&mut cursor).find(|a| {
            matches!(
                a.kind(),
                "arrow_function" | "function_expression" | "function"
            )
        });
        let Some(body) = callback.and_then(|c| c.child_by_field_name("body")) else {
            return;
        };
        if !self.ts_asserts(body) {
            self.assert_free(call);
        }
    }

    fn ts_asserts(&self, body: Node<'_>) -> bool {
        descendants(body).into_iter().any(|n| match n.kind() {
            "throw_statement" => true,
            "call_expression" => n.child_by_field_name("function").is_some_and(|f| {
                let text = self.text(f);
                let root = text.split(['.', '(']).next().unwrap_or(text);
                matches!(root, "expect" | "assert" | "expectTypeOf")
                    || asserting_name(last_segment(text))
                    || last_segment(text).starts_with("should")
            }),
            _ => false,
        })
    }

    // ---- Java and C# ----

    fn visit_c_family(&mut self, node: Node<'_>) {
        match node.kind() {
            // C#'s `throw_expression` is the throw in `=> throw …` and
            // `x ?? throw …`; it throws exactly as a statement does.
            "throw_statement" | "throw_expression" => {
                if !self.is_added(node) {
                    return;
                }
                let Some(created) = node
                    .named_child(0)
                    .filter(|c| c.kind() == "object_creation_expression")
                else {
                    return;
                };
                let ty = created
                    .child_by_field_name("type")
                    .map(|t| self.text(t).to_ascii_lowercase())
                    .unwrap_or_default();
                let args = created.child_by_field_name("arguments");
                let bare = args.is_none_or(|a| a.named_child_count() == 0);
                // Only what the arguments say in string literals: an
                // identifier such as `TODO_LIST` is not a message.
                let message = args
                    .map(|a| {
                        descendants(a)
                            .into_iter()
                            .filter(|n| c_family_string(n.kind()))
                            .map(|n| self.text(n).to_ascii_lowercase())
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                    .unwrap_or_default();
                if let Some((severity, msg)) = thrown_placeholder(&ty, &message, bare) {
                    self.placeholder_finding(node, severity, msg);
                }
            }
            // Constructors are not checked for empty bodies: an empty private
            // constructor is how a static class is written. Accessors are not
            // either: `set { }` is a deliberate no-op.
            "method_declaration" | "local_function_statement" => {
                let Some(body) = node
                    .child_by_field_name("body")
                    .filter(|b| b.kind() == "block")
                else {
                    return;
                };
                if body_is_empty(body) {
                    let anchor = node.child_by_field_name("name").unwrap_or(node);
                    self.empty_body(anchor, body, "empty");
                }
            }
            _ => {}
        }
    }
}

/// A string literal, in either grammar.
fn c_family_string(kind: &str) -> bool {
    matches!(
        kind,
        "string_literal"
            | "text_block"
            | "verbatim_string_literal"
            | "raw_string_literal"
            | "interpolated_string_expression"
    )
}

/// The nearest function-like node containing `node`.
fn enclosing_function(node: Node<'_>) -> Option<Node<'_>> {
    let mut current = node.parent();
    while let Some(n) = current {
        if matches!(
            n.kind(),
            "function_item"
                | "function_declaration"
                | "method_declaration"
                | "function_definition"
                | "method_definition"
                | "generator_function_declaration"
                | "constructor_declaration"
                | "compact_constructor_declaration"
                | "local_function_statement"
                | "property_declaration"
                | "indexer_declaration"
                | "operator_declaration"
                | "conversion_operator_declaration"
                | "destructor_declaration"
        ) {
            return Some(n);
        }
        current = n.parent();
    }
    None
}

fn is_comment_kind(kind: &str) -> bool {
    matches!(kind, "line_comment" | "block_comment" | "comment")
}

/// A body with nothing in it but comments.
fn body_is_empty(body: Node<'_>) -> bool {
    let mut cursor = body.walk();
    body.named_children(&mut cursor).all(|c| is_comment(c))
}

fn first_statement(body: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = body.walk();
    body.named_children(&mut cursor).find(|c| !is_comment(*c))
}

/// A Python body's statements, without comments or a leading docstring.
fn python_statements<'t>(body: Node<'t>) -> Vec<Node<'t>> {
    let mut cursor = body.walk();
    let mut statements: Vec<Node<'t>> = body
        .named_children(&mut cursor)
        .filter(|c| !is_comment(*c))
        .collect();
    let docstring = statements.first().is_some_and(|s| {
        s.kind() == "expression_statement"
            && s.named_child_count() == 1
            && s.named_child(0).is_some_and(|c| c.kind() == "string")
    });
    if docstring {
        statements.remove(0);
    }
    statements
}

/// `pytest.mark.skip` and `unittest.skip`, but not their conditional
/// siblings (`skipif`, `skipIf`, `skipUnless`), which is how a test that needs
/// a platform or a tool is written.
fn python_unconditional_skip(decorator: &str) -> bool {
    let path = decorator.split('(').next().unwrap_or(decorator).trim();
    matches!(last_segment(path), "skip")
        && (path.contains("mark") || path.contains("unittest") || path == "skip")
}

/// A called name that, by convention, fails the test when its check fails.
fn asserting_name(name: &str) -> bool {
    let lowered = name.to_ascii_lowercase();
    ["assert", "expect", "check", "verify", "must", "require"]
        .iter()
        .any(|p| lowered.contains(p))
        || matches!(name, "unwrap" | "unwrap_err" | "fatal")
}
