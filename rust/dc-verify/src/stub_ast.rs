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

/// The longest one parse may run. `ts_parser_parse` has no cost bound of its
/// own: a one-line 100 KB Python file took 16 s inside it and was then thrown
/// away for its syntax errors. The same bound `devmap-extract` puts on a parse.
pub const PARSE_BUDGET: std::time::Duration = std::time::Duration::from_secs(2);

/// Why a file was not analysed by the tree. Each one sends the file to the
/// substring checks, and each is reported so the fallback is never silent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unparsed {
    /// Larger than [`MAX_SOURCE_BYTES`].
    TooLarge,
    /// The grammar could not be loaded or the parser returned no tree.
    NoTree,
    /// The parse ran past [`PARSE_BUDGET`] and was cancelled.
    OverBudget,
    /// The tree has syntax errors, so a construct in it could be misread.
    SyntaxError,
}

impl Unparsed {
    /// The reason as it is reported to the host.
    pub fn as_str(self) -> &'static str {
        match self {
            Unparsed::TooLarge => "source too large to parse",
            Unparsed::NoTree => "parser produced no tree",
            Unparsed::OverBudget => "parse exceeded its time budget",
            Unparsed::SyntaxError => "syntax error",
        }
    }
}

/// Parses `source` and reports the stubs and weak tests the diff introduced.
///
/// `added` holds the 1-based post-change line numbers the diff added. `None`
/// means the file was not analysed — too large, unparseable, or carrying a
/// syntax error — and the caller must fall back rather than read it as clean.
/// [`analyze_outcome`] says which.
pub fn analyze(lang: Lang, source: &str, added: &HashSet<u32>) -> Option<Vec<AstFinding>> {
    analyze_outcome(lang, source, added).ok()
}

/// [`analyze`], naming why a file was not analysed.
pub fn analyze_outcome(
    lang: Lang,
    source: &str,
    added: &HashSet<u32>,
) -> Result<Vec<AstFinding>, Unparsed> {
    analyze_within(lang, source, added, PARSE_BUDGET)
}

/// [`analyze_outcome`] with the parse budget as a parameter.
pub fn analyze_within(
    lang: Lang,
    source: &str,
    added: &HashSet<u32>,
    budget: std::time::Duration,
) -> Result<Vec<AstFinding>, Unparsed> {
    if source.len() > MAX_SOURCE_BYTES {
        return Err(Unparsed::TooLarge);
    }
    let mut parser = Parser::new();
    parser
        .set_language(&lang.grammar())
        .map_err(|_| Unparsed::NoTree)?;
    let deadline = std::time::Instant::now() + budget;
    let mut over_budget = false;
    let tree = {
        // Polled by tree-sitter during the parse; returning true cancels it.
        let mut cancel = |_: &tree_sitter::ParseState| -> bool {
            if std::time::Instant::now() >= deadline {
                over_budget = true;
                return true;
            }
            false
        };
        let options = tree_sitter::ParseOptions::new().progress_callback(&mut cancel);
        let bytes = source.as_bytes();
        parser.parse_with_options(
            &mut |offset: usize, _| {
                if offset < bytes.len() {
                    &bytes[offset..]
                } else {
                    &[][..]
                }
            },
            None,
            Some(options),
        )
    };
    // A cancelled parse can hand back a partial tree describing a prefix of the
    // file; it is never read.
    if over_budget {
        return Err(Unparsed::OverBudget);
    }
    let tree = tree.ok_or(Unparsed::NoTree)?;
    let root = tree.root_node();
    if root.has_error() {
        return Err(Unparsed::SyntaxError);
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
    Ok(walker.out)
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

/// The first line, counted from 0 within `text`, that `accept` allows and that
/// holds `todo!` or `unimplemented!` in code position.
///
/// A small lexer, not a parser, and it reads the whole text so that what spans
/// lines (a string, a raw string, a block comment) is skipped as one thing. It
/// skips everything that is not code: string literals; raw strings
/// (`r#"…"#`); char literals, so a `'"'` does not open a string that hides what
/// follows it; line comments; and block comments, which nest and are skipped
/// before a quote inside one is looked at. A tick that is not a char literal is
/// a lifetime and is skipped alone. The macro name may be separated from its
/// `!` by whitespace or comments (`todo !()`), and must be followed by a
/// delimiter, so `todo != x` is not a stub. Something left unterminated is not
/// code.
fn first_visible_stub_line(text: &str, accept: &dyn Fn(usize) -> bool) -> Option<usize> {
    let lower = text.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let newlines: Vec<usize> = bytes
        .iter()
        .enumerate()
        .filter_map(|(at, &b)| (b == b'\n').then_some(at))
        .collect();
    let line_at = |at: usize| newlines.partition_point(|&n| n < at);
    let mut i = 0;
    let mut in_string = false;
    let mut escaped = false;
    while i < bytes.len() {
        let b = bytes[i];
        if in_string {
            match b {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
            i += 1;
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'\'' => {
                if let Some(len) = char_literal_len(&bytes[i..]) {
                    i += len;
                    continue;
                }
            }
            b'r' if raw_string_hashes(bytes, i).is_some() => {
                let hashes = raw_string_hashes(bytes, i).unwrap_or(0);
                match raw_string_end(&bytes[i + 2 + hashes..], hashes) {
                    Some(len) => {
                        i += 2 + hashes + len;
                        continue;
                    }
                    None => return None,
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                i = skip_trivia(bytes, i);
                continue;
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i = skip_trivia(bytes, i);
                continue;
            }
            _ => {
                let boundary =
                    i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_');
                if boundary {
                    for name in ["todo", "unimplemented"] {
                        if !bytes[i..].starts_with(name.as_bytes()) {
                            continue;
                        }
                        let bang = skip_trivia(bytes, i + name.len());
                        if bytes.get(bang) != Some(&b'!') {
                            continue;
                        }
                        let open = skip_trivia(bytes, bang + 1);
                        if matches!(bytes.get(open), Some(b'(' | b'[' | b'{')) {
                            let line = line_at(i);
                            if accept(line) {
                                return Some(line);
                            }
                        }
                    }
                }
            }
        }
        i += 1;
    }
    None
}

/// The offset of the first byte at or after `at` that is not whitespace or the
/// inside of a comment. An unterminated block comment runs to the end.
fn skip_trivia(bytes: &[u8], mut at: usize) -> usize {
    loop {
        match bytes.get(at) {
            Some(c) if c.is_ascii_whitespace() => at += 1,
            Some(b'/') if bytes.get(at + 1) == Some(&b'/') => {
                while bytes.get(at).is_some_and(|&c| c != b'\n') {
                    at += 1;
                }
            }
            Some(b'/') if bytes.get(at + 1) == Some(&b'*') => {
                let mut depth = 1usize;
                at += 2;
                while depth > 0 && at < bytes.len() {
                    if bytes[at..].starts_with(b"/*") {
                        depth += 1;
                        at += 2;
                    } else if bytes[at..].starts_with(b"*/") {
                        depth -= 1;
                        at += 2;
                    } else {
                        at += 1;
                    }
                }
            }
            _ => return at,
        }
    }
}

/// Length of the char literal at the start of `s` (which begins with `'`), or
/// `None` when the tick is a lifetime. `'x'` is one scalar between ticks and
/// `'\…'` is an escape (`'\''`, `'\"'`, `'\n'`, `'\u{1F600}'`) closed by the
/// next tick.
fn char_literal_len(s: &[u8]) -> Option<usize> {
    match *s.get(1)? {
        b'\\' => {
            // The escaped byte (position 2) may itself be a tick, so the
            // closing tick is searched for from position 3.
            let close = s.get(3..)?.iter().take(10).position(|&c| c == b'\'')?;
            Some(3 + close + 1)
        }
        b'\'' => None,
        lead => {
            let width = match lead {
                0x00..=0x7f => 1,
                0xc0..=0xdf => 2,
                0xe0..=0xef => 3,
                0xf0..=0xf7 => 4,
                _ => return None,
            };
            (s.get(1 + width) == Some(&b'\'')).then_some(width + 2)
        }
    }
}

/// How many `#` open the raw string whose `r` is at `bytes[at]`, or `None` when
/// that `r` does not start one: it ends an identifier (`bar#"`), or is not
/// followed by `#`* and a quote. A `b` prefix (`br"…"`) is accepted.
fn raw_string_hashes(bytes: &[u8], at: usize) -> Option<usize> {
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let before = match at.checked_sub(1) {
        Some(prev) if bytes[prev] == b'b' => prev.checked_sub(1),
        other => other,
    };
    if before.is_some_and(|p| ident(bytes[p])) {
        return None;
    }
    let hashes = bytes[at + 1..].iter().take_while(|&&c| c == b'#').count();
    (bytes.get(at + 1 + hashes) == Some(&b'"')).then_some(hashes)
}

/// Length of the raw string body in `rest` (the bytes after the opening quote)
/// including its closing `"` and `hashes` hashes, or `None` when the string
/// does not close on this line.
fn raw_string_end(rest: &[u8], hashes: usize) -> Option<usize> {
    (0..rest.len()).find_map(|k| {
        (rest[k] == b'"' && rest.get(k + 1..k + 1 + hashes)?.iter().all(|&c| c == b'#'))
            .then_some(k + 1 + hashes)
    })
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
                    "panic" if self.panic_is_placeholder(node) => {
                        self.placeholder(node, "panic!(\"TODO\")")
                    }
                    // A wrapper whose own name is not a stub, but whose token
                    // tree still shows `todo!` / `unimplemented!`.
                    _ if self.visible_stub_line(node).is_some() => {
                        self.placeholder(node, "wrapper macro")
                    }
                    _ => {}
                }
            }
            // `macro_rules` bodies are token trees, not expanded invocations,
            // so `unimplemented!()` inside one is not a `macro_invocation`.
            // The text is still on an added line; that is the stub.
            "macro_definition" => {
                if let Some(line) = self.visible_stub_line(node) {
                    self.placeholder_finding(
                        node,
                        Severity::Blocking,
                        "added code whose body is a wrapper around `todo!` or `unimplemented!`; the task is not implemented".to_string(),
                    );
                    // The finding is anchored on the definition. Move it to
                    // the line that actually carries the stub when that line
                    // was added, so a marker above the macro covers it.
                    if let Some(found) = self.out.last_mut() {
                        found.line = line;
                    }
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

    /// A panic whose message is a placeholder, including one assembled by
    /// `concat!` from adjacent string literals. A non-literal argument between
    /// two literals ends that run, so `concat!("not ", detail, "implemented")`
    /// is not the phrase `not implemented`.
    fn panic_is_placeholder(&self, node: Node<'_>) -> bool {
        if self.has_placeholder_string(node) {
            return true;
        }
        let mut rest = self.text(node);
        while let Some(at) = rest.find("concat!(") {
            if crate::rigor::concat_placeholder(&rest[at..]).is_some() {
                return true;
            }
            rest = &rest[at + "concat!(".len()..];
        }
        false
    }

    /// An added line of `node` whose code contains `todo!` or `unimplemented!`.
    fn visible_stub_line(&self, node: Node<'_>) -> Option<u32> {
        let start = line_of(node);
        first_visible_stub_line(self.text(node), &|offset| {
            self.added
                .contains(&(start + u32::try_from(offset).unwrap_or(0)))
        })
        .map(|offset| start + u32::try_from(offset).unwrap_or(0))
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
