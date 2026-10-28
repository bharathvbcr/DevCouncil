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

use std::collections::{HashMap, HashSet};

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
    /// A macro this file uses is imported from another file, so the tree it
    /// parsed cannot say whether that macro is a stub.
    UnresolvedMacroImport,
    /// A macro imported from another file, or an alias of a std stub macro, is
    /// called inside the token tree of another macro, which the tree does not
    /// parse into invocations.
    AliasInTokenTree,
    /// The alias walk was cut off at its cap, so the aliases it found are not
    /// all of them.
    AliasWalkBudget,
    /// No working-tree root was given, so the file's source was never looked for.
    NoRoot,
    /// The path leaves the working tree or runs through a symlink, so the source
    /// is not read.
    OutsideRoot,
    /// The file the diff changed is not in the working tree.
    SourceMissing,
    /// The path is not a regular file (a directory, a FIFO, a device).
    NotRegular,
    /// The file exists and could not be read.
    ReadError,
    /// The source does not hold the added lines where the diff says they are, so
    /// the working tree has moved on since the diff was taken.
    SourceStale,
}

impl Unparsed {
    /// The reason as it is reported to the host.
    pub fn as_str(self) -> &'static str {
        match self {
            Unparsed::TooLarge => "source too large to parse",
            Unparsed::NoTree => "parser produced no tree",
            Unparsed::OverBudget => "parse exceeded its time budget",
            Unparsed::SyntaxError => "syntax error",
            Unparsed::UnresolvedMacroImport => {
                "macro imported from another file could not be resolved"
            }
            Unparsed::AliasInTokenTree => "alias-in-token-tree",
            Unparsed::AliasWalkBudget => "alias-walk-budget",
            Unparsed::NoRoot => "source not read: no working-tree root",
            Unparsed::OutsideRoot => "source not read: path outside the working tree",
            Unparsed::SourceMissing => "source not read: file missing",
            Unparsed::NotRegular => "source not read: not a regular file",
            Unparsed::ReadError => "source not read: unreadable",
            Unparsed::SourceStale => "source does not match the diff",
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
    analyze_capped(
        lang,
        source,
        added,
        budget,
        MAX_SCOPE_CHECKS,
        &mut Steps::default(),
    )
}

/// What one analysis spent on the two bounded pre-passes, for a test to assert
/// on in place of a wall clock: a count is the same on a loaded machine and an
/// idle one. Never part of any output.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Steps {
    /// Nodes and leaves the alias walk visited (see `resolve_aliases`).
    alias_visits: usize,
    /// Scopes examined answering "is this macro defined here" (see `defined_at`).
    scope_checks: usize,
    /// A scope-check question found the cap spent.
    scope_checks_exhausted: bool,
}

/// [`analyze_within`] with the per-file scope-check cap as a parameter, so a
/// test can lower it, and `steps` filled in with what the pre-passes spent.
fn analyze_capped(
    lang: Lang,
    source: &str,
    added: &HashSet<u32>,
    budget: std::time::Duration,
    scope_checks: usize,
    steps: &mut Steps,
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
    // The imports are read before the walk: the walk visits a call site before
    // a `use` that comes after it, and a `use` in a function or an inline
    // module names the macro for the whole file as far as this scan goes.
    let imports = if lang == Lang::Rust {
        RustImports::collect(root, source.as_bytes(), added, scope_checks)
    } else {
        RustImports::default()
    };
    let mut walker = Walker {
        lang,
        src: source.as_bytes(),
        added,
        imports: &imports,
        unresolved: false,
        token_tree: false,
        out: Vec::new(),
    };
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        walker.visit(node);
        let mut cursor = node.walk();
        stack.extend(node.named_children(&mut cursor));
    }
    *steps = Steps {
        alias_visits: imports.alias_visits,
        scope_checks: imports.scope_checks_spent.get(),
        scope_checks_exhausted: imports.scope_checks_exhausted.get(),
    };
    // What the tree proved wins. Only a file with nothing blocking in it is
    // degraded for a macro it could not read, so a degraded file is never also
    // one the scan already refused.
    let blocked = walker.out.iter().any(|f| f.severity == Severity::Blocking);
    if !blocked {
        if imports.over_budget || imports.scope_checks_exhausted.get() {
            return Err(Unparsed::AliasWalkBudget);
        }
        if walker.unresolved || imports.unresolved {
            return Err(Unparsed::UnresolvedMacroImport);
        }
        if walker.token_tree {
            return Err(Unparsed::AliasInTokenTree);
        }
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
    imports: &'a RustImports,
    /// An added invocation named a macro this file imports from another file.
    unresolved: bool,
    /// An added line calls such a macro from inside another macro's token tree.
    token_tree: bool,
    out: Vec<AstFinding>,
}

/// The crates whose `todo!`, `unimplemented!` and `panic!` are the stubs.
const STD_ROOTS: [&str; 3] = ["std", "core", "alloc"];
/// The macros an alias can stand for.
const STUB_MACROS: [&str; 3] = ["todo", "unimplemented", "panic"];
/// Bounds on one `use` tree, so a hostile file cannot make the pre-pass cost
/// more than the parse did. A tree past either is degraded, never half read.
const MAX_USE_DEPTH: usize = 32;
const MAX_USE_SEGMENTS: usize = 256;

/// A name as it is written, without the `r#` of a raw identifier.
fn strip_raw(name: &str) -> &str {
    name.strip_prefix("r#").unwrap_or(name)
}

fn node_name(node: Node<'_>, src: &[u8]) -> String {
    strip_raw(node.utf8_text(src).unwrap_or("")).to_string()
}

/// The segments of a path node, leading `::` dropped and `r#` stripped:
/// `::core::r#todo` is `["core", "todo"]`.
fn path_segments(mut node: Node<'_>, src: &[u8]) -> Vec<String> {
    let mut reversed = Vec::new();
    loop {
        if node.kind() == "scoped_identifier" {
            if let Some(name) = node.child_by_field_name("name") {
                reversed.push(node_name(name, src));
            }
            match node.child_by_field_name("path") {
                Some(path) => node = path,
                None => break,
            }
        } else {
            reversed.push(node_name(node, src));
            break;
        }
    }
    reversed.reverse();
    reversed
}

/// One leaf of a `use` tree, or an `extern crate x as y;`: the path it imports
/// and the name it binds.
struct UseRecord {
    path: Vec<String>,
    alias: String,
    added: bool,
    public: bool,
}

impl UseRecord {
    /// The path with a leading `self` dropped: `use self::t as u` and
    /// `use t as u` name the same item.
    fn relative(&self) -> &[String] {
        match self.path.split_first() {
            Some((first, rest)) if first == "self" => rest,
            _ => &self.path,
        }
    }
}

/// The `macro_rules!` of one file, indexed for the question "is a macro called
/// `name` in textual scope at byte `at`".
///
/// A macro is visible from its end to the end of the block, module body or file
/// it is written in. Those scopes nest or are disjoint, so the distinct ones
/// form a tree; each scope keeps, per name, the end of its earliest definition
/// (the earliest is the only one that can matter). A question is then answered
/// by walking the scopes that hold `at` from the inside out, which is as deep
/// as the nesting and not as many as the definitions.
#[derive(Default)]
struct MacroScopes {
    /// Distinct scopes, by start ascending then end descending.
    scopes: Vec<std::ops::Range<usize>>,
    /// The scope each one is nested in.
    parent: Vec<Option<usize>>,
    /// Name, then scope, to the end of the earliest definition in that scope.
    earliest_end: HashMap<String, HashMap<usize, usize>>,
}

impl MacroScopes {
    fn build(defs: Vec<(String, std::ops::Range<usize>, usize)>) -> Self {
        let mut scopes: Vec<std::ops::Range<usize>> =
            defs.iter().map(|(_, scope, _)| scope.clone()).collect();
        scopes.sort_by(|a, b| a.start.cmp(&b.start).then(b.end.cmp(&a.end)));
        scopes.dedup();
        let mut parent = Vec::with_capacity(scopes.len());
        let mut open: Vec<usize> = Vec::new();
        for (index, scope) in scopes.iter().enumerate() {
            while open.last().is_some_and(|top| scopes[*top].end < scope.end) {
                open.pop();
            }
            parent.push(open.last().copied());
            open.push(index);
        }
        let mut earliest_end: HashMap<String, HashMap<usize, usize>> = HashMap::new();
        for (name, scope, end) in defs {
            // Every definition's scope was indexed above, so this always
            // finds it; a definition it could not find would only leave its
            // alias in force.
            let Ok(index) =
                scopes.binary_search_by(|s| s.start.cmp(&scope.start).then(scope.end.cmp(&s.end)))
            else {
                continue;
            };
            let slot = earliest_end
                .entry(name)
                .or_default()
                .entry(index)
                .or_insert(end);
            *slot = (*slot).min(end);
        }
        Self {
            scopes,
            parent,
            earliest_end,
        }
    }

    fn defines(&self, name: &str) -> bool {
        self.earliest_end.contains_key(name)
    }

    /// Where the walk outward from `at` starts: the last scope that starts at
    /// or before `at`. If it ends before `at` the scope holding `at` (if any)
    /// is one it is nested in, so the walk reaches it through `parent`. `None`
    /// when no indexed scope starts that early.
    fn innermost_candidate(&self, at: usize) -> Option<usize> {
        self.scopes
            .partition_point(|s| s.start <= at)
            .checked_sub(1)
    }
}

/// A cap on the `use` leaves one file is resolved from, and on the nodes the
/// resolution visits. A file past either is degraded, never half resolved.
const MAX_USE_RECORDS: usize = 50_000;
const MAX_ALIAS_VISITS: usize = 500_000;
/// A cap on the scopes examined answering "is this macro defined here" over one
/// file, summed over every invocation. A file past it is degraded, and the
/// question that hit the cap is answered "not defined", so an alias stays in
/// force: a hostile file can cost this scan at most this many checks.
const MAX_SCOPE_CHECKS: usize = 1_000_000;

/// What the `use` declarations, `extern crate`s, `mod x;` and `macro_rules!` of
/// one Rust file say about the macros its invocations name.
#[derive(Default)]
struct RustImports {
    /// An alias of a std stub macro: the name it binds, and the macro it is.
    aliases: HashMap<String, &'static str>,
    /// `std`, `core`, `alloc` and every name `use` or `extern crate` binds to
    /// one of them (`use std as s;`).
    roots: HashSet<String>,
    /// Lowercased names of the aliases of `todo!` and `unimplemented!`, which
    /// the token-tree lexer looks for beside the macros' own names.
    token_names: HashSet<String>,
    /// Names imported from `crate`, `super`, `self` or a module this file
    /// declares with `mod x;`, which is another file.
    local_names: HashSet<String>,
    local_mods: HashSet<String>,
    /// Macros this file defines, by name. A macro's body is scanned where it
    /// stands, so a call it resolves to is not another file's.
    macro_defs: MacroScopes,
    /// An added aliased `pub use` of std's `todo!` or `unimplemented!` makes
    /// the alias callable from files this scan is not reading, or a `use` tree
    /// was too large to read in full.
    unresolved: bool,
    /// The alias walk was cut off at a cap, so the aliases it found are not all.
    over_budget: bool,
    /// The scope-check cap for this file, and how much of it the invocations
    /// have spent. A query that finds the cap spent sets `scope_checks_exhausted`.
    scope_check_cap: usize,
    scope_checks_spent: std::cell::Cell<usize>,
    /// Visits the alias walk made, for [`Steps`].
    alias_visits: usize,
    scope_checks_exhausted: std::cell::Cell<bool>,
}

/// What a name the resolver has learnt stands for.
enum Learnt {
    /// `std`, `core`, `alloc`, or a name bound to one of them.
    Root,
    /// A name bound to a std stub macro.
    Stub(&'static str),
}

/// How far up a name's binding may move: `panic` is the weakest stub, and
/// `todo` and `unimplemented` are equal, so the first of them to bind a name
/// keeps it.
fn stub_rank(stub: &str) -> u8 {
    if stub == "panic" { 1 } else { 2 }
}

/// The macro among [`STUB_MACROS`] that `name` is.
fn stub_macro(name: &str) -> Option<&'static str> {
    STUB_MACROS.iter().copied().find(|m| *m == name)
}

impl RustImports {
    /// Reads the file's imports in one pass over the tree and one indexed walk
    /// over the `use` leaves, each leaf visited once per name that triggers it.
    fn collect(root: Node<'_>, src: &[u8], added: &HashSet<u32>, scope_check_cap: usize) -> Self {
        let mut records = Vec::new();
        let mut defs = Vec::new();
        let mut out = RustImports {
            scope_check_cap,
            ..RustImports::default()
        };
        // A walk over the named nodes, one at a time: every kind matched below
        // is a named node, so the anonymous punctuation need not be collected.
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            if records.len() > MAX_USE_RECORDS {
                out.over_budget = true;
                return out;
            }
            match node.kind() {
                "use_declaration" => {
                    let mut cursor = node.walk();
                    let public = node
                        .named_children(&mut cursor)
                        .any(|c| c.kind() == "visibility_modifier");
                    if let Some(argument) = node.child_by_field_name("argument") {
                        out.unresolved |= collect_use(argument, public, src, added, &mut records);
                    }
                }
                "extern_crate_declaration" => {
                    if let (Some(name), Some(alias)) = (
                        node.child_by_field_name("name"),
                        node.child_by_field_name("alias"),
                    ) {
                        records.push(UseRecord {
                            path: vec![node_name(name, src)],
                            alias: node_name(alias, src),
                            added: added.contains(&line_of(node)),
                            public: false,
                        });
                    }
                }
                "macro_definition" if !is_cfg_gated(node, src) => {
                    if let Some(name) = node.child_by_field_name("name") {
                        defs.push((node_name(name, src), scope_of(node), node.end_byte()));
                    }
                }
                "mod_item" if node.child_by_field_name("body").is_none() => {
                    if let Some(name) = node.child_by_field_name("name") {
                        out.local_mods.insert(node_name(name, src));
                    }
                }
                _ => {}
            }
            let mut cursor = node.walk();
            stack.extend(node.named_children(&mut cursor));
        }
        out.macro_defs = MacroScopes::build(defs);
        if records.len() > MAX_USE_RECORDS {
            out.over_budget = true;
            return out;
        }
        out.alias_visits = out.resolve_aliases(&records, MAX_ALIAS_VISITS);
        for record in &records {
            let local = record.path.first().is_some_and(|root| {
                matches!(root.as_str(), "crate" | "super" | "self") || out.local_mods.contains(root)
            });
            if local && record.alias != "_" {
                out.local_names.insert(record.alias.clone());
            }
            if record.public
                && record.added
                && record.alias != "_"
                && matches!(out.stub_of(record), Some("todo" | "unimplemented"))
                && record.path.last() != Some(&record.alias)
            {
                out.unresolved = true;
            }
        }
        out.token_names = out
            .aliases
            .iter()
            .filter(|(_, stub)| **stub != "panic")
            .map(|(name, _)| name.clone())
            .collect();
        out
    }

    /// Binds every name that comes to stand for a std stub macro.
    ///
    /// The leaves are indexed by the first segment of their path, and a name is
    /// looked up once per thing it is learnt to be: `use std as s; use s::todo
    /// as t; use self::t as u;` is three lookups however the declarations are
    /// ordered, where a pass over all leaves per round was quadratic in a
    /// reverse chain.
    ///
    /// A name's binding only moves up ([`stub_rank`]: none, then `panic`, then
    /// `todo` or `unimplemented`, where a second stub keeps the first), so it
    /// changes at most twice and each `(name, what it is)` is queued once. A
    /// binding that could flip back and forth re-queued the name on every flip,
    /// and every re-queue visited every leaf under it. What is counted against
    /// `max_visits` is each name taken from the queue and each leaf looked at,
    /// so the cap bounds the work itself and not just the number of pops. The
    /// number of visits made is returned.
    fn resolve_aliases(&mut self, records: &[UseRecord], max_visits: usize) -> usize {
        let mut by_first: HashMap<&str, Vec<&UseRecord>> = HashMap::new();
        for record in records {
            if let Some(first) = record.relative().first() {
                by_first.entry(first.as_str()).or_default().push(record);
            }
        }
        self.roots = STD_ROOTS.iter().map(|r| (*r).to_string()).collect();
        let mut queued: HashSet<(String, Option<&'static str>)> = HashSet::new();
        let mut queue: Vec<(String, Learnt)> = Vec::new();
        for root in STD_ROOTS {
            if queued.insert(((*root).to_string(), None)) {
                queue.push(((*root).to_string(), Learnt::Root));
            }
        }
        let mut visits = 0usize;
        while let Some((name, learnt)) = queue.pop() {
            visits += 1;
            if visits > max_visits {
                self.over_budget = true;
                return visits;
            }
            let Some(leaves) = by_first.get(name.as_str()) else {
                continue;
            };
            for record in leaves {
                visits += 1;
                if visits > max_visits {
                    self.over_budget = true;
                    return visits;
                }
                if record.alias == "_" {
                    continue;
                }
                let relative = record.relative();
                let bound = match learnt {
                    Learnt::Root if relative.len() == 1 => {
                        if self.roots.insert(record.alias.clone())
                            && queued.insert((record.alias.clone(), None))
                        {
                            queue.push((record.alias.clone(), Learnt::Root));
                        }
                        None
                    }
                    Learnt::Root => relative.last().and_then(|last| stub_macro(last)),
                    Learnt::Stub(stub) if relative.len() == 1 => Some(stub),
                    Learnt::Stub(_) => None,
                };
                if let Some(stub) = bound
                    && self
                        .aliases
                        .get(&record.alias)
                        .is_none_or(|have| stub_rank(stub) > stub_rank(have))
                {
                    self.aliases.insert(record.alias.clone(), stub);
                    if queued.insert((record.alias.clone(), Some(stub))) {
                        queue.push((record.alias.clone(), Learnt::Stub(stub)));
                    }
                }
            }
        }
        visits
    }

    /// The std macro a leaf re-exports, however it reaches it: a std path, a
    /// name already bound to one, or such a name through `crate::` or `super::`.
    fn stub_of(&self, record: &UseRecord) -> Option<&'static str> {
        match record.relative() {
            [only] => self.aliases.get(only).copied(),
            [first, last] if matches!(first.as_str(), "crate" | "super") => {
                self.aliases.get(last).copied()
            }
            [first, .., last] if self.roots.contains(first) => stub_macro(last),
            _ => None,
        }
    }

    /// Whether a `macro_rules!` called `name` is visible at byte `at`: written
    /// in a block, module or file that holds `at`, and before it. Textual scope
    /// ends with the module, so a macro in `mod tests` hides nothing outside it.
    ///
    /// Each scope examined spends one of the file's [`MAX_SCOPE_CHECKS`]. When
    /// they are spent this answers `false` (the alias stays in force) and
    /// records that the file is to be degraded.
    fn defined_at(&self, name: &str, at: usize) -> bool {
        let Some(by_scope) = self.macro_defs.earliest_end.get(name) else {
            return false;
        };
        let mut scope = self.macro_defs.innermost_candidate(at);
        while let Some(index) = scope {
            if !self.spend_scope_check() {
                return false;
            }
            if self.macro_defs.scopes[index].end > at
                && by_scope.get(&index).is_some_and(|end| *end <= at)
            {
                return true;
            }
            scope = self.macro_defs.parent[index];
        }
        false
    }

    /// Spends one scope check, or reports that none are left.
    fn spend_scope_check(&self) -> bool {
        let spent = self.scope_checks_spent.get();
        if spent >= self.scope_check_cap {
            self.scope_checks_exhausted.set(true);
            return false;
        }
        self.scope_checks_spent.set(spent + 1);
        true
    }

    /// The std macro the name written at byte `at` is an alias of, unless a
    /// `macro_rules!` of that name is in scope there.
    fn alias_at(&self, name: &str, at: usize) -> Option<&'static str> {
        self.aliases
            .get(name)
            .copied()
            .filter(|_| !self.defined_at(name, at))
    }

    /// Whether the invocation named by `segments` at byte `at` is a macro of
    /// another file of the crate: a name imported from `crate`, `super`, `self`
    /// or a `mod x;` module, or a path through one. A macro this file defines is
    /// read from its own body and is not that.
    fn is_from_elsewhere(&self, segments: &[String], at: usize) -> bool {
        match segments {
            [only] => self.local_names.contains(only) && !self.defined_at(only, at),
            [first, .., last] => {
                (matches!(first.as_str(), "crate" | "super" | "self")
                    || self.local_mods.contains(first))
                    && !self.macro_defs.defines(last)
            }
            [] => false,
        }
    }
}

/// Whether a `#[cfg(..)]` or `#[cfg_attr(..)]` stands on `node`. The attributes
/// are preceding siblings of the item, with doc comments allowed between them.
///
/// Such a `macro_rules!` may be compiled out, and then the name is the std alias.
/// Registering it would let a definition that no build contains shadow that
/// alias, so it is not registered: the alias stays in force, and some build
/// compiles the stub it names.
fn is_cfg_gated(node: Node<'_>, src: &[u8]) -> bool {
    let mut at = node.prev_named_sibling();
    while let Some(prev) = at {
        if prev.kind() == "attribute_item" {
            let name = prev
                .named_child(0)
                .and_then(|attribute| attribute.named_child(0))
                .map(|first| node_name(first, src));
            if matches!(name.as_deref(), Some("cfg" | "cfg_attr")) {
                return true;
            }
        } else if !is_comment(prev) {
            return false;
        }
        at = prev.prev_named_sibling();
    }
    false
}

/// The byte range of the block, module body or file that `node` is written in.
fn scope_of(node: Node<'_>) -> std::ops::Range<usize> {
    let mut at = node.parent();
    while let Some(parent) = at {
        if matches!(parent.kind(), "declaration_list" | "block" | "source_file") {
            return parent.start_byte()..parent.end_byte();
        }
        at = parent.parent();
    }
    0..usize::MAX
}

/// Flattens one `use` argument into leaves. Returns whether the tree was too
/// large to read in full.
fn collect_use(
    argument: Node<'_>,
    public: bool,
    src: &[u8],
    added: &HashSet<u32>,
    out: &mut Vec<UseRecord>,
) -> bool {
    let mut too_large = false;
    let mut stack = vec![(argument, Vec::<String>::new(), 0usize)];
    while let Some((node, prefix, depth)) = stack.pop() {
        if depth > MAX_USE_DEPTH {
            too_large = true;
            continue;
        }
        // A leaf's own path is `self` for `use a::{self as b}`, which names the
        // prefix, not a segment called `self`.
        let leaf_path = |path: Node<'_>| {
            let mut full = prefix.clone();
            if path.kind() != "self" {
                full.extend(path_segments(path, src));
            }
            full
        };
        let mut record = |path: Vec<String>, alias: String| {
            out.push(UseRecord {
                path,
                alias,
                added: added.contains(&line_of(node)),
                public,
            });
        };
        match node.kind() {
            "use_as_clause" => {
                let (Some(path), Some(alias)) = (
                    node.child_by_field_name("path"),
                    node.child_by_field_name("alias"),
                ) else {
                    continue;
                };
                let full = leaf_path(path);
                if full.len() > MAX_USE_SEGMENTS {
                    too_large = true;
                    continue;
                }
                record(full, node_name(alias, src));
            }
            "scoped_use_list" | "use_list" => {
                let mut next = prefix.clone();
                if let Some(path) = node.child_by_field_name("path") {
                    next.extend(path_segments(path, src));
                }
                if next.len() > MAX_USE_SEGMENTS {
                    too_large = true;
                    continue;
                }
                let list = node.child_by_field_name("list").unwrap_or(node);
                let mut cursor = list.walk();
                for child in list.named_children(&mut cursor) {
                    stack.push((child, next.clone(), depth + 1));
                }
            }
            "identifier" | "crate" | "super" | "self" | "scoped_identifier" | "metavariable" => {
                let full = leaf_path(node);
                if full.len() > MAX_USE_SEGMENTS {
                    too_large = true;
                    continue;
                }
                if let Some(last) = full.last().cloned() {
                    record(full, last);
                }
            }
            _ => {}
        }
    }
    too_large
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

/// One macro invocation the lexer saw in a token tree: the path it names and
/// where it starts.
struct MacroCall {
    /// The segments of the path, leading `::` dropped and `r#` stripped.
    segments: Vec<String>,
    /// Byte offset of the path's first segment within the text.
    offset: usize,
    /// Line of it, counted from 0 within the text.
    line: usize,
}

/// The longest path the lexer reads for one invocation. A longer one is not
/// read, so a hostile `a::a::a::…` costs no more than the text it is written in.
const MAX_CALL_PATH_SEGMENTS: usize = 64;

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80
}

/// Whether the identifier at `at` is the tail of a path: `::` comes before it,
/// and an identifier before that. A leading `::` is not.
fn follows_path_separator(bytes: &[u8], at: usize) -> bool {
    let back = |mut i: usize| {
        while i > 0 && bytes[i - 1].is_ascii_whitespace() {
            i -= 1;
        }
        i
    };
    let colons = back(at);
    if colons < 2 || &bytes[colons - 2..colons] != b"::" {
        return false;
    }
    let before = back(colons - 2);
    before > 0 && is_ident_byte(bytes[before - 1])
}

/// The invocation whose path begins at `start`, or `None` when what follows is
/// not a path and then `!` and an opening delimiter.
fn read_macro_call(bytes: &[u8], start: usize) -> Option<(Vec<String>, usize)> {
    let mut segments = Vec::new();
    let mut at = start;
    loop {
        let mut end = at;
        while bytes.get(end).is_some_and(|&b| is_ident_byte(b)) {
            end += 1;
        }
        if end == at || segments.len() >= MAX_CALL_PATH_SEGMENTS {
            return None;
        }
        segments.push(String::from_utf8_lossy(&bytes[at..end]).into_owned());
        let next = skip_trivia(bytes, end);
        if bytes[next..].starts_with(b"::") {
            at = skip_trivia(bytes, next + 2);
            if bytes[at..].starts_with(b"r#") {
                at += 2;
            }
            continue;
        }
        if bytes.get(next) != Some(&b'!') {
            return None;
        }
        let open = skip_trivia(bytes, next + 1);
        return matches!(bytes.get(open), Some(b'(' | b'[' | b'{')).then_some((segments, start));
    }
}

/// Every macro invocation in code position in `text`.
///
/// A small lexer, not a parser, and it reads the whole text so that what spans
/// lines (a string, a raw string, a block comment) is skipped as one thing. It
/// skips everything that is not code: string literals; raw strings
/// (`r#"…"#`); char literals, so a `'"'` does not open a string that hides what
/// follows it; line comments; and block comments, which nest and are skipped
/// before a quote inside one is looked at. A tick that is not a char literal is
/// a lifetime and is skipped alone. The macro name may be separated from its
/// `!` by whitespace or comments (`todo !()`), and must be followed by a
/// delimiter, so `todo != x` is not an invocation. Something left unterminated
/// is not code.
///
/// It is what reads a token tree, which tree-sitter does not parse into
/// invocations: the arguments of `dbg!(…)` or the body of a `macro_rules!`.
fn visible_macro_calls(text: &str) -> Vec<MacroCall> {
    let bytes = text.as_bytes();
    let newlines: Vec<usize> = bytes
        .iter()
        .enumerate()
        .filter_map(|(at, &b)| (b == b'\n').then_some(at))
        .collect();
    let line_at = |at: usize| newlines.partition_point(|&n| n < at);
    let mut calls = Vec::new();
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
                    None => return calls,
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
                let boundary = i == 0 || !is_ident_byte(bytes[i - 1]);
                if boundary
                    && is_ident_byte(b)
                    && !b.is_ascii_digit()
                    && !follows_path_separator(bytes, i)
                    && let Some((segments, offset)) = read_macro_call(bytes, i)
                {
                    calls.push(MacroCall {
                        segments,
                        offset,
                        line: line_at(offset),
                    });
                }
            }
        }
        i += 1;
    }
    calls
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
                let segments = node
                    .child_by_field_name("macro")
                    .map(|m| path_segments(m, self.src))
                    .unwrap_or_default();
                let written = segments.last().map_or("", String::as_str);
                // The name a `use` gave a std stub macro is that macro, unless
                // a `macro_rules!` of that name is in scope where it is called.
                let name = self
                    .imports
                    .alias_at(written, node.start_byte())
                    .unwrap_or(written);
                let calls = visible_macro_calls(self.text(node));
                match name {
                    "todo" | "unimplemented" => self.placeholder(node, &format!("{name}!()")),
                    "panic" if self.panic_is_placeholder(node) => {
                        self.placeholder(node, "panic!(\"TODO\")")
                    }
                    // A wrapper whose own name is not a stub, but whose token
                    // tree still shows `todo!` / `unimplemented!`.
                    _ if self.stub_line_in(node, &calls).is_some() => {
                        self.placeholder(node, "wrapper macro")
                    }
                    _ => {
                        if self.imports.is_from_elsewhere(&segments, node.start_byte()) {
                            self.unresolved = true;
                        }
                        // The invocation's own path is the call at offset 0.
                        self.note_token_tree(node, calls.iter().filter(|c| c.offset > 0));
                    }
                }
            }
            // `macro_rules` bodies are token trees, not expanded invocations,
            // so `unimplemented!()` inside one is not a `macro_invocation`.
            // The text is still on an added line; that is the stub.
            "macro_definition" => {
                let calls = visible_macro_calls(self.text(node));
                self.note_token_tree(node, calls.iter());
                if let Some(line) = self.stub_line_in(node, &calls) {
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

    /// The first added line of `node` that calls `todo!` or `unimplemented!`, or
    /// the name a `use` of this file gave one of them, in code position.
    fn stub_line_in(&self, node: Node<'_>, calls: &[MacroCall]) -> Option<u32> {
        let start = line_of(node);
        calls
            .iter()
            .filter(|call| {
                call.segments.last().is_some_and(|name| {
                    let at = node.start_byte() + call.offset;
                    (name.eq_ignore_ascii_case("todo")
                        || name.eq_ignore_ascii_case("unimplemented"))
                        || (self.imports.token_names.contains(name)
                            && !self.imports.defined_at(name, at))
                })
            })
            .map(|call| start + u32::try_from(call.line).unwrap_or(0))
            .find(|line| self.added.contains(line))
    }

    /// Records that a token tree on an added line calls a macro imported from
    /// another file of the crate. Tree-sitter does not parse a token tree into
    /// invocations, so the walker never sees such a call, and what it resolves
    /// to is not readable here.
    fn note_token_tree<'c>(&mut self, node: Node<'_>, calls: impl Iterator<Item = &'c MacroCall>) {
        let start = line_of(node);
        for call in calls {
            let line = start + u32::try_from(call.line).unwrap_or(0);
            if self.added.contains(&line)
                && self
                    .imports
                    .is_from_elsewhere(&call.segments, node.start_byte() + call.offset)
            {
                self.token_tree = true;
            }
        }
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

#[cfg(test)]
mod scope_check_cap_tests {
    use super::*;

    const BUDGET: std::time::Duration = std::time::Duration::from_secs(10);

    /// A block that defines a macro of the name `use` imports, then `calls`
    /// invocations the definition covers. Every line is added.
    fn covered_calls(import: &str, calls: usize) -> (String, HashSet<u32>) {
        let mut body = format!("{import}\npub fn f() {{\n    macro_rules! t {{ () => {{}}; }}\n");
        for _ in 0..calls {
            body.push_str("    t!();\n");
        }
        body.push_str("}\n");
        let lines = body.lines().count() as u32;
        (body, (1..=lines).collect())
    }

    fn analyze(body: &str, added: &HashSet<u32>, cap: usize) -> Result<Vec<AstFinding>, Unparsed> {
        analyze_capped(Lang::Rust, body, added, BUDGET, cap, &mut Steps::default())
    }

    /// The smallest cap under which the file is read to the end, clean, found
    /// by search: the outcome never gets worse as the cap rises.
    fn smallest_resolving_cap(body: &str, added: &HashSet<u32>) -> usize {
        let (mut low, mut high) = (0usize, MAX_SCOPE_CHECKS);
        let clean = |cap| matches!(analyze(body, added, cap), Ok(f) if f.is_empty());
        assert!(clean(high));
        while low < high {
            let mid = low + (high - low) / 2;
            if clean(mid) {
                high = mid;
            } else {
                low = mid + 1;
            }
        }
        low
    }

    #[test]
    fn a_cap_one_short_degrades_and_the_cap_itself_resolves() {
        // `crate::inner::t` is another file's macro unless the block's own
        // `macro_rules!` is in scope, so a file that cannot say is degraded.
        let (body, added) = covered_calls("use crate::inner::t;", 40);
        let needed = smallest_resolving_cap(&body, &added);
        assert!(
            needed >= 40,
            "each call spends at least one check: {needed}"
        );
        assert_eq!(analyze(&body, &added, needed).unwrap(), Vec::new());
        assert_eq!(
            analyze(&body, &added, needed - 1).unwrap_err(),
            Unparsed::AliasWalkBudget
        );
    }

    #[test]
    fn spent_cap_leaves_the_alias_in_force() {
        // With no check left the macro is not seen as defined, so the stub
        // alias is read as the stub it is: blocked, not degraded.
        let (body, added) = covered_calls("use std::todo as t;", 3);
        let findings = analyze(&body, &added, 0);
        assert!(
            matches!(&findings, Ok(f) if f.iter().any(|f| f.severity == Severity::Blocking)),
            "{findings:?}"
        );
    }
}

#[cfg(test)]
mod alias_walk_tests {
    use super::*;

    fn record(path: &[&str], alias: &str) -> UseRecord {
        UseRecord {
            path: path.iter().map(|s| (*s).to_string()).collect(),
            alias: alias.to_string(),
            added: true,
            public: false,
        }
    }

    /// The shape that made one pop cost every leaf under its name: `t` is bound
    /// to `todo` and `unimplemented` in turn, and `t` and `u` hand each other
    /// the flip.
    fn pingpong(k: usize, m: usize) -> Vec<UseRecord> {
        let mut records = Vec::new();
        for _ in 0..k {
            records.push(record(&["std", "todo"], "t"));
            records.push(record(&["std", "unimplemented"], "t"));
        }
        for _ in 0..m {
            records.push(record(&["t"], "u"));
        }
        for _ in 0..m {
            records.push(record(&["u"], "t"));
        }
        records
    }

    #[test]
    fn visits_stay_linear_in_the_records_however_the_binding_flips() {
        let records = pingpong(300, 300);
        let mut imports = RustImports::default();
        // A cap the quadratic walk passed by orders of magnitude (it made
        // ~k*m leaf visits per round): the monotone walk needs a few per leaf.
        let visits = imports.resolve_aliases(&records, 4 * records.len() + 16);
        assert!(
            !imports.over_budget,
            "visited {visits} for {} records",
            records.len()
        );
        assert!(visits <= 4 * records.len() + 16, "{visits}");
        assert_eq!(imports.aliases.get("t"), Some(&"todo"));
        assert_eq!(imports.aliases.get("u"), Some(&"todo"));
    }

    #[test]
    fn a_lowered_cap_counts_leaf_visits_and_degrades() {
        let records = pingpong(50, 50);
        let mut imports = RustImports::default();
        let visits = imports.resolve_aliases(&records, 40);
        assert!(imports.over_budget);
        assert_eq!(visits, 41, "the walk stops at the first visit past the cap");
    }

    #[test]
    fn one_first_segment_with_many_names_is_visited_once_per_binding() {
        // `use std::todo as a0; ...` under one root: every leaf hangs off
        // `std`, which is learnt once.
        let records: Vec<UseRecord> = (0..2_000)
            .map(|i| record(&["std", "todo"], &format!("a{i}")))
            .collect();
        let mut imports = RustImports::default();
        let visits = imports.resolve_aliases(&records, 4 * records.len() + 16);
        assert!(!imports.over_budget);
        assert!(visits <= 4 * records.len() + 16, "{visits}");
        assert_eq!(imports.aliases.len(), 2_000);
    }

    #[test]
    fn a_stronger_stub_replaces_panic_and_never_the_reverse() {
        let records = vec![
            record(&["std", "panic"], "t"),
            record(&["std", "todo"], "t"),
            record(&["std", "panic"], "t"),
            record(&["t"], "u"),
        ];
        let mut imports = RustImports::default();
        imports.resolve_aliases(&records, MAX_ALIAS_VISITS);
        assert_eq!(imports.aliases.get("t"), Some(&"todo"));
        assert_eq!(imports.aliases.get("u"), Some(&"todo"));
    }
}

/// The cost bounds of the alias pre-passes, asserted as step counts.
///
/// The integration tests that carried a 2 s wall clock for these shapes measured
/// the machine as much as the code: under load the same code passed and failed.
/// A count of the visits the walk made and the scopes the queries examined is
/// the bound itself, and the same on any machine. The integration tests keep
/// their verdict assertions and a loose wall-clock backstop.
#[cfg(test)]
mod step_count_tests {
    use super::*;

    const CALL: &str = "pub fn f() -> u32 {\n    t!()\n}\n";

    fn steps_of(body: &str, cap: usize) -> (Result<Vec<AstFinding>, Unparsed>, Steps) {
        let added: HashSet<u32> = (1..=body.lines().count() as u32).collect();
        let mut steps = Steps::default();
        let result = analyze_capped(
            Lang::Rust,
            body,
            &added,
            std::time::Duration::from_secs(120),
            cap,
            &mut steps,
        );
        (result, steps)
    }

    fn blocked(result: &Result<Vec<AstFinding>, Unparsed>) -> bool {
        matches!(result, Ok(f) if f.iter().any(|f| f.severity == Severity::Blocking))
    }

    /// A walk visits each leaf a bounded number of times, however the shape
    /// flips the bindings: at most four visits per `use` leaf.
    fn assert_linear(label: &str, body: &str, leaves: usize) -> Steps {
        let (result, steps) = steps_of(body, MAX_SCOPE_CHECKS);
        assert!(
            steps.alias_visits <= 4 * leaves,
            "{label}: {} visits for {leaves} leaves",
            steps.alias_visits
        );
        assert!(!matches!(result, Err(Unparsed::AliasWalkBudget)), "{label}");
        assert!(steps.alias_visits > 0, "{label}: the walk never ran");
        steps
    }

    fn pingpong(k: usize, m: usize) -> String {
        let mut body = String::new();
        for _ in 0..k {
            body.push_str("use std::todo as t;\nuse std::unimplemented as t;\n");
        }
        for _ in 0..m {
            body.push_str("use t as u;\n");
        }
        for _ in 0..m {
            body.push_str("use u as t;\n");
        }
        body.push_str(CALL);
        body
    }

    // ---- residue2_alias.rs, attack2_round2.rs, attack_verify_rigor.rs ----------

    #[test]
    fn the_67_second_pingpong_is_linear_in_its_leaves_and_blocks() {
        let body = pingpong(6_000, 12_000);
        assert_linear("pingpong 6000/12000", &body, 2 * 6_000 + 2 * 12_000);
        assert!(blocked(&steps_of(&body, MAX_SCOPE_CHECKS).0));
    }

    #[test]
    fn a_smaller_pingpong_is_linear_too() {
        assert_linear(
            "pingpong 2000/2000",
            &pingpong(2_000, 2_000),
            2 * 2_000 + 2 * 2_000,
        );
    }

    #[test]
    fn ten_times_the_attack_stops_at_the_record_cap_before_the_walk_starts() {
        let (result, steps) = steps_of(&pingpong(20_000, 20_000), MAX_SCOPE_CHECKS);
        assert_eq!(
            steps.alias_visits, 0,
            "the walk started past the record cap"
        );
        assert!(
            matches!(result, Err(Unparsed::AliasWalkBudget)),
            "{result:?}"
        );
    }

    #[test]
    fn a_use_walk_past_the_record_cap_never_starts() {
        let mut body = String::new();
        for i in 0..50_100 {
            body.push_str(&format!("use a{i} as b{i};\n"));
        }
        body.push_str("pub fn f() -> u32 {\n    1\n}\n");
        let (result, steps) = steps_of(&body, MAX_SCOPE_CHECKS);
        assert_eq!(steps.alias_visits, 0);
        assert!(
            matches!(result, Err(Unparsed::AliasWalkBudget)),
            "{result:?}"
        );
    }

    #[test]
    fn flips_through_panic_are_linear_and_block() {
        let mut body = String::new();
        for _ in 0..3_000 {
            body.push_str("use std::panic as t;\nuse std::todo as t;\n");
        }
        for _ in 0..3_000 {
            body.push_str("use t as u;\nuse u as t;\n");
        }
        body.push_str(CALL);
        assert_linear("panic flips", &body, 2 * 3_000 + 2 * 3_000);
        assert!(blocked(&steps_of(&body, MAX_SCOPE_CHECKS).0));
    }

    #[test]
    fn one_first_segment_with_many_names_is_linear_and_blocks() {
        let mut body = String::new();
        for i in 0..20_000 {
            body.push_str(&format!("use std::todo as a{i};\n"));
        }
        body.push_str("use a19999 as t;\n");
        body.push_str(CALL);
        assert_linear("many names", &body, 20_001);
        assert!(blocked(&steps_of(&body, MAX_SCOPE_CHECKS).0));
    }

    #[test]
    fn a_long_alias_chain_is_linear_in_either_order() {
        for n in [2_000usize, 8_000] {
            for forward in [false, true] {
                let order: Vec<usize> = if forward {
                    (1..n).collect()
                } else {
                    (1..n).rev().collect()
                };
                let mut body = String::new();
                for i in order {
                    body.push_str(&format!("use t{} as t{};\n", i - 1, i));
                }
                body.push_str(&format!(
                    "use std::todo as t0;\npub fn f() -> u32 {{\n    t{}!()\n}}\n",
                    n - 1
                ));
                assert_linear(&format!("chain n={n} forward={forward}"), &body, n);
                assert!(blocked(&steps_of(&body, MAX_SCOPE_CHECKS).0));
            }
        }
    }

    #[test]
    fn many_aliases_times_many_invocations_spend_no_scope_checks_and_few_visits() {
        for (aliases, calls) in [(500usize, 2_000usize), (2_000, 8_000)] {
            let mut body = String::new();
            for i in 0..aliases {
                body.push_str(&format!("use std::todo as a{i};\n"));
            }
            body.push_str("pub fn f() {\n");
            for _ in 0..calls {
                body.push_str("    w!(x);\n");
            }
            body.push_str("}\n");
            let (_, steps) = steps_of(&body, MAX_SCOPE_CHECKS);
            assert_eq!(
                steps.scope_checks, 0,
                "no macro_rules is defined, so no scope is examined"
            );
            assert!(
                steps.alias_visits <= 4 * aliases,
                "{} visits for {aliases} aliases",
                steps.alias_visits
            );
        }
    }

    // ---- attack3_round3.rs: defs x calls ---------------------------------------

    /// The probe: 25,000 `macro_rules! t` each in its own block, then 60,000
    /// calls outside every one of them. Each call examines exactly the one scope
    /// the search lands on and finds it closed: the count is the number of
    /// calls, not defs x calls (1.5 x 10^9), and it is far under the cap.
    #[test]
    fn the_defs_by_calls_probe_examines_one_scope_per_call() {
        let mut body = String::from("use std::todo as t;\n");
        for _ in 0..25_000 {
            body.push_str("{ macro_rules! t { () => {}; } }\n");
        }
        body.push_str("pub fn f() {\n");
        for _ in 0..60_000 {
            body.push_str("    t!();\n");
        }
        body.push_str("}\n");
        let (result, steps) = steps_of(&body, MAX_SCOPE_CHECKS);
        eprintln!("probe steps: {steps:?}");
        assert_eq!(steps.scope_checks, 60_000, "{steps:?}");
        assert!(!steps.scope_checks_exhausted);
        assert!(blocked(&result), "the alias is in force at every call");
        // The cap is exact: the count that was spent resolves, one fewer is
        // exhausted, and an exhausted query leaves the alias in force.
        let spent = steps.scope_checks;
        let (at_cap, at_cap_steps) = steps_of(&body, spent);
        assert!(!at_cap_steps.scope_checks_exhausted, "{at_cap_steps:?}");
        assert!(blocked(&at_cap));
        let (under, under_steps) = steps_of(&body, spent - 1);
        assert!(under_steps.scope_checks_exhausted, "{under_steps:?}");
        assert_eq!(under_steps.scope_checks, spent - 1);
        assert!(blocked(&under));
    }
}
