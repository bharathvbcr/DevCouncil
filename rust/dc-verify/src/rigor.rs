//! Rigor gates: the checks that read the *content* of a diff rather than its
//! shape.
//!
//! Scope classification answers "did this task change files it was allowed to
//! change". These answer the harder question: "is what it wrote actually the
//! work". They are the three gates the Go verifier previously listed in its
//! degraded set, which is why a passing verification could not be trusted to
//! mean very much.
//!
//! Every finding carries a file, a line, and the text that triggered it, so a
//! report is actionable rather than a verdict. And every gate is deliberately
//! conservative about what it flags: a rigor check with a high false-positive
//! rate gets turned off, and a gate nobody runs protects nothing.

use std::collections::{HashMap, HashSet};

use crate::{ChangeStatus, FileDiff, stub_ast};

/// The credential table lives in `dc-redact`, which every consumer shares;
/// these stay reachable here because this is where callers have always found
/// them.
pub use dc_redact::{contains_secret, redact_secrets};

/// How serious a finding is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Blocks the task. Reserved for findings where shipping is clearly wrong.
    Blocking,
    /// Reported and does not block.
    Advisory,
}

/// How a gate knows what it reported.
///
/// [`Severity`] says how much a finding matters. This says how much it can be
/// trusted, which is a different axis and was not on the wire at all until
/// now: `secret_scan`'s prefix-and-length match and a coverage gap read out of
/// an executed profile arrived at a consumer as the same kind of claim, so
/// nothing downstream could tell a measurement from a guess. The verifier
/// already refuses to let a check that *could not run* look like one that ran;
/// this is the same rule one step further in — a guess must not look like a
/// measurement.
///
/// The ladder is deliberately short, and each rung is defined by what would
/// have to be wrong for a finding at that rung to be wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strength {
    /// Follows from the parsed structure of the input. Wrong only if the diff
    /// parser is wrong, which is the one thing every other gate here already
    /// depends on.
    Proven,
    /// Read from an execution artifact the caller supplied. Wrong only if that
    /// artifact does not describe this revision — which the verifier cannot
    /// check and does not claim to.
    Observed,
    /// A textual pattern that correlates with the thing being looked for.
    /// Wrong whenever the pattern matches something else, which is an ordinary
    /// event rather than a defect: these gates are tuned to be conservative,
    /// not to be certain.
    Derived,
}

impl Strength {
    /// The wire spelling. Matched exactly by the Go client, which refuses an
    /// unknown value rather than decoding it as a default — an unrecognised
    /// strength silently read as `proven` would be the precise inversion of
    /// what this type is for.
    pub fn as_str(self) -> &'static str {
        match self {
            Strength::Proven => "proven",
            Strength::Observed => "observed",
            Strength::Derived => "derived",
        }
    }
}

/// One thing a gate found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub gate: &'static str,
    pub severity: Severity,
    /// How the gate knows. See [`Strength`].
    pub strength: Strength,
    pub path: String,
    pub line: u32,
    /// What triggered it. Truncated, and never the full line for a secret
    /// finding — see [`dc_redact::SecretMatch::redacted`].
    pub evidence: String,
    pub message: String,
}

/// Placeholder markers. Matched case-insensitively against added lines only:
/// an existing TODO in untouched code is somebody else's decision, and
/// flagging it would make every diff in a legacy file fail.
pub(crate) const STUB_MARKERS: &[&str] = &[
    "todo",
    "fixme",
    "xxx:",
    "hack:",
    "not implemented",
    "notimplemented",
    "unimplemented",
    "placeholder",
    "for now",
    "stub",
];

/// Bodies that do nothing, in the languages this repo builds.
const EMPTY_BODIES: &[&str] = &[
    "todo!()",
    "unimplemented!()",
    "panic!(\"todo\")",
    "raise notimplementederror",
    "pass  # todo",
];

/// The gate an allow-stub marker moves a finding to. See [`apply_allow_stub`].
pub const GATE_STUB_ALLOWED: &str = "stub_allowed";

/// The marker that declares a stub intentional. It must carry a reason:
/// `// allow-stub: waiting on the v2 API`. A bare marker suppresses nothing.
pub const ALLOW_STUB_MARKER: &str = "allow-stub";

/// Runs the stub and test-rigor checks over a diff with no post-change source.
///
/// Files the diff adds whole are still parsed — their added lines *are* the
/// file. Everything else falls back to the substring checks. The `dcverify`
/// binary uses [`detect_stubs_with`] and reads the working tree.
pub fn detect_stubs(files: &[FileDiff]) -> Vec<Finding> {
    detect_stubs_with(files, &|_| None)
}

/// Runs the stub and test-rigor checks, parsing each changed file's
/// post-change source when `source` can supply it.
///
/// Only constructs on added lines are reported. A gate that also read removed
/// lines would flag a diff for *deleting* a TODO, which is the opposite of
/// what it is for.
///
/// For a file in a language [`stub_ast`] parses, whose source is available and
/// agrees with the diff, the placeholder, empty-body, skipped-test and
/// assert-free-test checks come from the tree and are [`Strength::Proven`]
/// (assert-free: `Derived`). Otherwise the substring checks run, as before, and
/// say they are `Derived`. Either way the comment-marker check is line-based.
///
/// Every finding then passes through [`apply_allow_stub`].
pub fn detect_stubs_with(
    files: &[FileDiff],
    source: &dyn Fn(&FileDiff) -> Option<String>,
) -> Vec<Finding> {
    detect_stubs_report(files, source).findings
}

/// What one stub pass found, and how many files the test-rigor checks ran on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StubReport {
    pub findings: Vec<Finding>,
    /// Files the skipped- and assert-free-test checks ran on: those
    /// [`stub_ast`] parsed, in a language it has those checks for
    /// ([`stub_ast::Lang::has_test_rigor`]). A Java or C# file is parsed for
    /// placeholders and not counted, because no test check reads it, and a
    /// report with zero here ran neither check and must not say it did.
    pub parsed_files: usize,
    /// Files in a language [`stub_ast`] parses, whose source was known, that
    /// the tree did not analyse — a syntax error, a source too large, a parse
    /// past its budget, or one that calls a macro imported from another file
    /// ([`stub_ast::Unparsed::UnresolvedMacroImport`]) and so could hide a
    /// stub the tree cannot see. They took the substring checks, which see
    /// less: a report that lists nothing here and one that lists a file are not
    /// the same report, and the host marks the run degraded for each entry.
    pub unparsed: Vec<UnparsedFile>,
}

/// One file the tree could not analyse, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnparsedFile {
    pub path: String,
    pub reason: &'static str,
}

/// [`detect_stubs_with`], also reporting how many files the test-rigor checks
/// ran on.
pub fn detect_stubs_report(
    files: &[FileDiff],
    source: &dyn Fn(&FileDiff) -> Option<String>,
) -> StubReport {
    let mut findings = Vec::new();
    let mut parsed_files = 0;
    let mut unparsed = Vec::new();
    for file in files {
        let lang = stub_ast::Lang::for_path(&file.path);
        let post_image = match post_image(file, source) {
            Ok(text) => Some(text),
            Err(PostImageMiss::TooLarge) => {
                // Not parsed and not read as clean: the substring checks below
                // still run, and the report says the tree never saw the file.
                if lang.is_some() {
                    unparsed.push(UnparsedFile {
                        path: file.path.clone(),
                        reason: stub_ast::Unparsed::TooLarge.as_str(),
                    });
                }
                None
            }
            Err(PostImageMiss::Stale) => {
                // A source that is not the one the diff was taken from cannot be
                // attributed to its lines. The substring checks still run, and
                // the report says the tree never read the file.
                if lang.is_some() {
                    unparsed.push(UnparsedFile {
                        path: file.path.clone(),
                        reason: stub_ast::Unparsed::SourceStale.as_str(),
                    });
                }
                None
            }
            Err(PostImageMiss::Unknown) => None,
        };
        let lines: Option<Vec<&str>> = post_image.as_deref().map(|s| s.lines().collect());
        let ast = match (lang, post_image.as_deref()) {
            (Some(lang), Some(src)) => {
                let added: HashSet<u32> = file.added_lines.iter().map(|(n, _)| *n).collect();
                match stub_ast::analyze_outcome(lang, src, &added) {
                    Ok(found) => Some(found),
                    Err(why) => {
                        unparsed.push(UnparsedFile {
                            path: file.path.clone(),
                            reason: why.as_str(),
                        });
                        None
                    }
                }
            }
            _ => None,
        };
        let mut mine = Vec::new();
        let mut scopes = Vec::new();
        if let Some(ast) = &ast {
            if lang.is_some_and(stub_ast::Lang::has_test_rigor) {
                parsed_files += 1;
            }
            for f in ast {
                let evidence = lines
                    .as_ref()
                    .and_then(|l| l.get(f.line as usize - 1))
                    .copied()
                    .unwrap_or("");
                mine.push(Finding {
                    gate: f.gate,
                    severity: f.severity,
                    strength: f.strength,
                    path: file.path.clone(),
                    line: f.line,
                    evidence: safe_evidence(evidence),
                    message: f.message.clone(),
                });
                scopes.push(Scope::Known(f.scope_line));
            }
        }
        let line_based = substring_findings(file, ast.is_none());
        // A parsed file knows each placeholder's function; on the substring
        // path it has to be inferred from the lines.
        let unparsed = if ast.is_none() {
            Scope::Infer
        } else {
            Scope::Known(None)
        };
        scopes.extend(line_based.iter().map(|_| unparsed));
        mine.extend(line_based);
        apply_allow_stub(&mut mine, &scopes, file, lines.as_deref());
        findings.extend(mine);
    }
    StubReport {
        findings,
        parsed_files,
        unparsed,
    }
}

/// Why the post-change file is not known.
enum PostImageMiss {
    /// The source is past [`stub_ast::MAX_SOURCE_BYTES`]. It is known, and it is
    /// not read, which a report must say: nothing was checked.
    TooLarge,
    /// No source was given. The caller knows why and reports it; this crate
    /// cannot.
    Unknown,
    /// A source was given and does not agree with the diff.
    Stale,
}

/// The file as it stands after the change, or why it cannot be known.
///
/// A file the diff adds whole is its added lines. Anything else comes from
/// `source`, and is believed only if every added line of the diff is at its
/// stated line number in it: a working tree that moved on since the diff was
/// taken would otherwise attribute findings to the wrong lines.
fn post_image(
    file: &FileDiff,
    source: &dyn Fn(&FileDiff) -> Option<String>,
) -> Result<String, PostImageMiss> {
    let whole_file = file.status == ChangeStatus::Added
        && file
            .added_lines
            .iter()
            .enumerate()
            .all(|(i, (n, _))| *n as usize == i + 1);
    let text = if whole_file {
        let mut s = String::new();
        for (_, line) in &file.added_lines {
            s.push_str(line);
            s.push('\n');
        }
        s
    } else {
        source(file).ok_or(PostImageMiss::Unknown)?
    };
    if text.len() > stub_ast::MAX_SOURCE_BYTES {
        return Err(PostImageMiss::TooLarge);
    }
    let lines: Vec<&str> = text.lines().collect();
    let agrees = file.added_lines.iter().all(|(n, content)| {
        lines
            .get((*n as usize).wrapping_sub(1))
            .is_some_and(|l| l.trim_end_matches('\r') == content.trim_end_matches('\r'))
    });
    if agrees {
        Ok(text)
    } else {
        Err(PostImageMiss::Stale)
    }
}

/// The line-based checks: placeholder bodies (only when the file was not
/// parsed) and comment markers (always).
fn substring_findings(file: &FileDiff, include_bodies: bool) -> Vec<Finding> {
    let mut findings = Vec::new();
    {
        for (index, (line_no, content)) in file.added_lines.iter().enumerate() {
            let lowered = content.to_ascii_lowercase();
            let trimmed = lowered.trim();
            if trimmed.contains(ALLOW_STUB_MARKER) {
                // The declaration itself, not a stub. Its reason is recorded
                // on the finding it covers.
                continue;
            }

            if include_bodies
                && is_c_family_source(&file.path)
                && let Some((severity, message)) =
                    c_family_placeholder(&c_family_statement(&file.added_lines, index))
            {
                findings.push(Finding {
                    gate: "stub_detection",
                    severity,
                    strength: Strength::Derived,
                    path: file.path.clone(),
                    line: *line_no,
                    evidence: safe_evidence(content),
                    message,
                });
                continue;
            }

            if let Some(marker) = EMPTY_BODIES
                .iter()
                .find(|m| include_bodies && trimmed.contains(**m))
            {
                findings.push(Finding {
                    gate: "stub_detection",
                    severity: Severity::Blocking,
                    // Blocking and yet only `Derived`, which is exactly the
                    // pair this axis exists to express. `todo!()` is
                    // unambiguous as a language construct, but this is a
                    // substring test over one line of text: the same bytes
                    // inside a string literal, a doc example or a macro that
                    // quotes its input match identically. Shipping it is
                    // clearly wrong when the match is real, so it blocks — and
                    // a consumer weighing an appeal deserves to know the
                    // verifier pattern-matched rather than parsed.
                    strength: Strength::Derived,
                    path: file.path.clone(),
                    line: *line_no,
                    evidence: safe_evidence(content),
                    message: format!(
                        "added code whose body is `{marker}`; the task is not implemented"
                    ),
                });
                continue;
            }

            if include_bodies
                && let Some((severity, message)) =
                    placeholder_call(&file.path, &c_family_statement(&file.added_lines, index))
            {
                findings.push(Finding {
                    gate: "stub_detection",
                    severity,
                    strength: Strength::Derived,
                    path: file.path.clone(),
                    line: *line_no,
                    evidence: safe_evidence(content),
                    message,
                });
                continue;
            }

            // Comment markers are advisory. A TODO in a new file is often a
            // legitimate note about future work, and blocking on it would make
            // the gate something people route around.
            if let Some(marker) = STUB_MARKERS.iter().find(|m| trimmed.contains(**m)) {
                if !is_comment_or_string(trimmed) {
                    continue;
                }
                findings.push(Finding {
                    gate: "stub_detection",
                    severity: Severity::Advisory,
                    // The weakest thing this file reports: a word, in a
                    // comment, that often but not always means unfinished
                    // work. Advisory and derived agree here, which is the
                    // uninteresting case — the axis earns its keep on the
                    // blocking findings above and below.
                    strength: Strength::Derived,
                    path: file.path.clone(),
                    line: *line_no,
                    evidence: safe_evidence(content),
                    message: format!("added a `{marker}` marker"),
                });
            }
        }
    }
    findings
}

/// Languages whose placeholder is `throw new T(message)` and which have no
/// grammar here, or whose grammar is the fallback's second chance: the
/// statement is judged by [`thrown_placeholder`], the verdict the tree-based
/// check shares, so the two readings agree on the wording.
fn throws_new_source(path: &str) -> bool {
    let ext = path.rsplit_once('.').map_or("", |(_, e)| e);
    matches!(
        ext,
        "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "mts" | "cts" | "php" | "kt" | "kts"
    )
}

/// The placeholder a lowercased statement is, in the languages the fallback
/// reads: a `todo!(…)` / `unimplemented!(…)` macro with arguments (the exact
/// no-argument spellings are `EMPTY_BODIES`), a Go or Rust `panic` whose message
/// opens with a placeholder phrase, Kotlin's `TODO(…)`, and a thrown exception
/// saying the code is unfinished.
///
/// Code position only: the match must not be inside a string literal or after
/// a `//` comment opener, so a log line quoting `todo!("x")` is not a stub.
fn placeholder_call(path: &str, statement: &str) -> Option<(Severity, String)> {
    let word_start = |at: usize| {
        !statement[..at]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.')
    };
    for name in ["todo!(", "unimplemented!("] {
        if statement
            .match_indices(name)
            .any(|(at, _)| is_code_at(statement, at) && word_start(at))
        {
            return Some((
                Severity::Blocking,
                format!("added code whose body is `{name}…)`; the task is not implemented"),
            ));
        }
    }
    for name in ["panic!(", "panic("] {
        for (at, _) in statement.match_indices(name) {
            if !is_code_at(statement, at) || !word_start(at) {
                continue;
            }
            let args = statement[at + name.len()..].trim_start();
            if let Some(message) = args.strip_prefix('"') {
                if let Some(phrase) = PLACEHOLDER_MESSAGES.iter().find(|p| {
                    message.starts_with(**p)
                        && !message[p.len()..]
                            .chars()
                            .next()
                            .is_some_and(char::is_alphanumeric)
                }) {
                    return Some((
                        Severity::Blocking,
                        format!("added code panics saying `{phrase}`; the task is not implemented"),
                    ));
                }
            } else if let Some(phrase) = concat_placeholder(args) {
                // panic!(concat!("not ", "implemented")) is one placeholder
                // split across literals. Each piece alone is not the phrase.
                return Some((
                    Severity::Blocking,
                    format!("added code panics saying `{phrase}`; the task is not implemented"),
                ));
            }
        }
    }
    let kotlin = path.ends_with(".kt") || path.ends_with(".kts");
    if kotlin
        && statement
            .match_indices("todo(")
            .any(|(at, _)| is_code_at(statement, at) && word_start(at))
    {
        return Some((
            Severity::Blocking,
            "added code whose body is Kotlin's `TODO(…)`; the task is not implemented".to_string(),
        ));
    }
    if throws_new_source(path) {
        return c_family_placeholder(statement);
    }
    None
}

/// The placeholder phrase assembled by `concat!(...)` at the start of a panic's
/// arguments, if a contiguous run of string-literal arguments joins into one.
///
/// Pieces are taken only from the outermost `concat!` call, and only from
/// arguments that are nothing but a string literal. A non-literal between two
/// literals ends the run: `concat!("not ", detail, "implemented")` is not
/// `concat!("not ", "implemented")`. A real
/// `panic!("queue invariant violated: len {}", len)` has no `concat!` and
/// stays clean.
pub(crate) fn concat_placeholder(args: &str) -> Option<&'static str> {
    let rest = args.trim_start();
    let body = rest.strip_prefix("concat!(")?;
    let mut run = String::new();
    let mut pieces = 0usize;
    let mut found: Option<&'static str> = None;
    for arg in concat_args(body) {
        if arg.trim().is_empty() {
            continue;
        }
        match sole_string_literal(arg) {
            Some(content) => {
                run.push_str(&content);
                pieces += 1;
            }
            None => note_concat_run(&mut run, &mut pieces, &mut found),
        }
    }
    note_concat_run(&mut run, &mut pieces, &mut found);
    found
}

fn note_concat_run(run: &mut String, pieces: &mut usize, found: &mut Option<&'static str>) {
    if *pieces >= 2 && found.is_none() {
        let lowered = run.to_ascii_lowercase();
        *found = PLACEHOLDER_MESSAGES
            .iter()
            .copied()
            .find(|phrase| lowered.contains(phrase));
    }
    run.clear();
    *pieces = 0;
}

/// Top-level arguments of a `concat!(...)` body, split on commas that are
/// not inside a nested group, a string, or a comment. `body` starts after
/// the opening `(`.
fn concat_args(body: &str) -> Vec<&str> {
    let bytes = body.as_bytes();
    let mut args = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    let mut depth: i32 = 1;
    while i < bytes.len() && depth > 0 {
        if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'/') {
            i += 2;
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
            continue;
        }
        if let Some(quote) = rust_string_at(bytes, i) {
            i = skip_rust_string(bytes, quote);
            continue;
        }
        match bytes[i] {
            b'(' | b'[' | b'{' => {
                depth += 1;
                i += 1;
            }
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    args.push(&body[start..i]);
                    break;
                }
                i += 1;
            }
            b',' if depth == 1 => {
                args.push(&body[start..i]);
                i += 1;
                start = i;
            }
            _ => i += 1,
        }
    }
    args
}

/// Index of the opening quote if `bytes[at..]` is a Rust string literal,
/// including `b`/`c`/`r` prefixes and raw hashes.
fn rust_string_at(bytes: &[u8], at: usize) -> Option<usize> {
    let mut j = at;
    if matches!(bytes.get(j), Some(b'b' | b'c')) {
        match bytes.get(j + 1) {
            Some(b'"' | b'r') => j += 1,
            _ => return None,
        }
    }
    if bytes.get(j) == Some(&b'r') {
        j += 1;
        while bytes.get(j) == Some(&b'#') {
            j += 1;
        }
    }
    if bytes.get(j) == Some(&b'"') {
        Some(j)
    } else {
        None
    }
}

fn skip_rust_string(bytes: &[u8], quote: usize) -> usize {
    let hashes = raw_hashes(bytes, quote);
    let mut i = quote + 1;
    if hashes == 0 {
        while i < bytes.len() && bytes[i] != b'"' {
            if bytes[i] == b'\\' && i + 1 < bytes.len() {
                i += 2;
                continue;
            }
            i += 1;
        }
        return (i + 1).min(bytes.len());
    }
    while i < bytes.len() {
        if bytes[i] == b'"'
            && i + 1 + hashes <= bytes.len()
            && bytes[i + 1..i + 1 + hashes].iter().all(|b| *b == b'#')
        {
            return i + 1 + hashes;
        }
        i += 1;
    }
    bytes.len()
}

fn raw_hashes(bytes: &[u8], quote: usize) -> usize {
    let mut hashes = 0usize;
    let mut k = quote;
    while k > 0 && bytes[k - 1] == b'#' {
        hashes += 1;
        k -= 1;
    }
    hashes
}

/// The contents of `arg` when the argument is one string literal and nothing
/// else. Anything else — an identifier, an operator, a second token — is not
/// a piece that can be joined with its neighbours.
fn sole_string_literal(arg: &str) -> Option<String> {
    let s = trim_leading_ws_comments(arg);
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return None;
    }
    let quote = rust_string_at(bytes, 0)?;
    let content_start = quote + 1;
    let end = skip_rust_string(bytes, quote);
    if !trim_leading_ws_comments(&s[end..]).is_empty() {
        return None;
    }
    let inner_end = if raw_hashes(bytes, quote) == 0 {
        end.saturating_sub(1).max(content_start)
    } else {
        let hashes = raw_hashes(bytes, quote);
        end.saturating_sub(1 + hashes).max(content_start)
    };
    Some(s[content_start..inner_end].to_string())
}

fn trim_leading_ws_comments(mut s: &str) -> &str {
    loop {
        s = s.trim_start();
        if let Some(rest) = s.strip_prefix("/*") {
            let Some(end) = rest.find("*/") else {
                return s;
            };
            s = &rest[end + 2..];
            continue;
        }
        if let Some(rest) = s.strip_prefix("//") {
            s = rest.split_once('\n').map(|(_, right)| right).unwrap_or("");
            continue;
        }
        return s;
    }
}

/// Whether `path` is Java or C#: languages whose placeholders are a thrown
/// exception and whose methods are declared with no keyword to anchor on.
/// [`stub_ast`] parses them when the post-change source is known; the line
/// checks here are their fallback when it is not.
fn is_c_family_source(path: &str) -> bool {
    path.ends_with(".java") || path.ends_with(".cs")
}

/// Most added lines a Java or C# statement is followed across.
const MAX_STATEMENT_LINES: usize = 8;

/// The lowercased statement starting at `added[index]`: that line, and while
/// its parentheses stay open, the added lines that directly follow it — so
/// `throw new UnsupportedOperationException(` with its message on the next
/// line is judged by the message. Bounded, and stops at a gap in numbering,
/// where the diff no longer shows what came next.
fn c_family_statement(added: &[(u32, String)], index: usize) -> String {
    let mut statement = String::new();
    let mut depth: i32 = 0;
    for (offset, (n, line)) in added[index..].iter().enumerate().take(MAX_STATEMENT_LINES) {
        if offset > 0 && (depth <= 0 || *n != added[index].0 + offset as u32) {
            break;
        }
        let lowered = line.to_ascii_lowercase();
        depth += paren_balance(&lowered);
        if offset > 0 {
            statement.push(' ');
        }
        statement.push_str(lowered.trim());
    }
    statement
}

/// Opening minus closing parentheses outside string literals and `//`
/// comments.
fn paren_balance(line: &str) -> i32 {
    let mut depth = 0;
    let mut in_string = false;
    let mut escaped = false;
    let bytes = line.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if in_string {
            match b {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'/' if bytes.get(i + 1) == Some(&b'/') => break,
            b'(' => depth += 1,
            b')' => depth -= 1,
            _ => {}
        }
    }
    depth
}

/// Whether byte `at` of `line` is code: not inside a string literal, and not
/// after a `//` comment opener. `log("throw new NotImplementedException()")`
/// mentions a throw; it does not throw.
fn is_code_at(line: &str, at: usize) -> bool {
    let mut in_string = false;
    let mut escaped = false;
    let bytes = line.as_bytes();
    for (i, &b) in bytes[..at].iter().enumerate() {
        if in_string {
            match b {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'/' if bytes.get(i + 1) == Some(&b'/') => return false,
            _ => {}
        }
    }
    !in_string
}

/// Phrases that make a thrown exception's message a placeholder, whatever the
/// exception type. "not supported yet" is the body NetBeans generates.
const PLACEHOLDER_MESSAGES: &[&str] = &[
    "not implemented",
    "not yet implemented",
    "unimplemented",
    "implement me",
    "not supported yet",
    "todo",
];

/// The placeholder a lowercased Java or C# line throws, if any: the line
/// checks' reading of a `throw new T(args)`, graded by [`thrown_placeholder`].
fn c_family_placeholder(lowered: &str) -> Option<(Severity, String)> {
    let at = lowered
        .match_indices("throw new ")
        .map(|(at, _)| at)
        .find(|at| is_code_at(lowered, *at))?;
    let thrown = &lowered[at + "throw new ".len()..];
    let open = thrown.find('(')?;
    let ty = thrown[..open].trim();
    let args = &thrown[open + 1..];
    let args = args[..args.rfind(')').unwrap_or(args.len())].trim();
    thrown_placeholder(ty, args, args.is_empty())
}

/// Whether throwing a Java or C# exception is a placeholder, and how much it
/// matters. The one verdict both readings of a throw share: [`stub_ast`] from
/// the tree, [`c_family_placeholder`] from the line.
///
/// `ty` is the exception's type as written, `message` the text of its
/// arguments the caller could read (the tree passes its string literals only),
/// and `bare` whether it was constructed with no argument at all. All three
/// lowercased.
///
/// `NotImplementedException` (C#'s generated stub body, and Apache Commons')
/// has no other use, so throwing it blocks; so does any exception whose message
/// says the code is unfinished. A *bare* `UnsupportedOperationException` is
/// only advisory: it is the commonest Java placeholder and also how an
/// immutable collection refuses a mutator, and nothing in the code tells them
/// apart. One with any other message is a deliberate refusal and is not
/// reported. `NotSupportedException` is never reported — it is how a read-only
/// stream refuses a write.
pub(crate) fn thrown_placeholder(
    ty: &str,
    message: &str,
    bare: bool,
) -> Option<(Severity, String)> {
    let ty = ty.rsplit('.').next().unwrap_or(ty).trim();
    if ty == "notimplementedexception" {
        return Some((
            Severity::Blocking,
            "added code whose body throws `NotImplementedException`; the task is not implemented"
                .to_string(),
        ));
    }
    if let Some(phrase) = PLACEHOLDER_MESSAGES
        .iter()
        .find(|p| contains_word(message, p))
    {
        return Some((
            Severity::Blocking,
            format!(
                "added code throws an exception saying `{phrase}`; the task is not implemented"
            ),
        ));
    }
    (ty == "unsupportedoperationexception" && bare).then(|| {
        (
            Severity::Advisory,
            "added code throws a bare `UnsupportedOperationException`: a placeholder, or a \
             deliberate refusal such as an immutable collection's mutator — nothing in the \
             code tells which"
                .to_string(),
        )
    })
}

/// Whether `phrase` occurs in `text` with no letter or digit on either side,
/// so `todo` matches `"TODO: wire it"` and not `"todoList is empty"`.
fn contains_word(text: &str, phrase: &str) -> bool {
    text.match_indices(phrase).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + phrase.len()..].chars().next();
        !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
    })
}

/// Honours `allow-stub: <reason>` declarations.
///
/// A finding is covered when the marker is on its own line or in the run of
/// comment, attribute and decorator lines directly above it. A covered finding
/// with a non-empty reason moves to [`GATE_STUB_ALLOWED`] as an advisory
/// finding carrying the reason and what it would have been, so the report
/// records the decision rather than losing the stub. A marker with no reason
/// is not honoured; the finding stays and says so.
///
/// `lines` is the post-change file when known; otherwise the diff's added
/// lines stand in for it, so only a marker the diff added can cover a finding.
///
/// Then the declaration audit: every marker the diff adds that covers no
/// finding is reported on its own, advisory under [`GATE_STUB_ALLOWED`]. The
/// author has said the code beneath it is a stub, which is the strongest
/// evidence of one there is, and a body the gate does not recognise — a
/// constant return, a no-op — is exactly where nothing else would say so.
fn apply_allow_stub(
    findings: &mut Vec<Finding>,
    scopes: &[Scope],
    file: &FileDiff,
    lines: Option<&[&str]>,
) {
    let added: HashMap<u32, &str> = file
        .added_lines
        .iter()
        .map(|(n, s)| (*n, s.as_str()))
        .collect();
    let line_at = |n: u32| -> Option<&str> {
        match lines {
            Some(l) => l.get((n as usize).checked_sub(1)?).copied(),
            None => added.get(&n).copied(),
        }
    };
    let c_family = is_c_family_source(&file.path);
    // The marker on `start`, or in the annotation lines directly above it,
    // with the line it is on.
    let declared_at = |start: u32| -> Option<(u32, String)> {
        let mut n = start;
        let mut first = true;
        while let Some(text) = line_at(n) {
            let trimmed = text.trim();
            if !first && !is_annotation_line(trimmed, c_family) {
                return None;
            }
            if let Some(r) = allow_stub_reason(trimmed) {
                return Some((n, r));
            }
            first = false;
            n = n.checked_sub(1).filter(|m| *m >= 1)?;
        }
        None
    };
    let mut covering: HashSet<u32> = HashSet::new();
    for (finding, scope) in findings.iter_mut().zip(scopes) {
        if finding.gate == "secret_scan" {
            continue;
        }
        // A marker on the function covers a placeholder in its body.
        let enclosing = match scope {
            Scope::Known(line) => *line,
            Scope::Infer => inferred_function_line(finding.line, &line_at, c_family),
        };
        let declared = declared_at(finding.line).or_else(|| enclosing.and_then(declared_at));
        let reason = declared.map(|(marker_line, r)| {
            covering.insert(marker_line);
            r
        });
        match reason {
            Some(r) if !r.is_empty() => {
                finding.message = format!("stub allowed: {r} (was: {})", finding.message);
                finding.gate = GATE_STUB_ALLOWED;
                finding.severity = Severity::Advisory;
            }
            Some(_) => {
                finding.message = format!(
                    "{} (an {ALLOW_STUB_MARKER} marker covers this line but gives no reason, \
                     so it is not honoured)",
                    finding.message
                );
            }
            None => {}
        }
    }

    // Prose is where a marker is written about, not used.
    if [".md", ".rst", ".txt"]
        .iter()
        .any(|ext| file.path.ends_with(ext))
    {
        return;
    }
    for (line_no, content) in &file.added_lines {
        if covering.contains(line_no) {
            continue;
        }
        let Some(reason) = declared_allow_stub(content) else {
            continue;
        };
        let message = if reason.is_empty() {
            format!(
                "added an {ALLOW_STUB_MARKER} declaration with no reason over code no stub check \
                 recognised; review what it declares"
            )
        } else {
            format!(
                "stub declared: {reason} (an {ALLOW_STUB_MARKER} declaration over code no stub \
                 check recognised; review what it declares)"
            )
        };
        findings.push(Finding {
            gate: GATE_STUB_ALLOWED,
            severity: Severity::Advisory,
            strength: Strength::Derived,
            path: file.path.clone(),
            line: *line_no,
            evidence: safe_evidence(content),
            message,
        });
    }
}

/// The reason an allow-stub marker written as a comment gives, or `None` for a
/// line where the marker is not in a comment — the constant that spells it, a
/// test string quoting it.
fn declared_allow_stub(line: &str) -> Option<String> {
    let at = line.to_ascii_lowercase().find(ALLOW_STUB_MARKER)?;
    let before = line[..at].trim_end();
    ["//", "#", "/*", "*", "--", "<!--"]
        .iter()
        .any(|opener| before.ends_with(opener))
        .then(|| allow_stub_reason(line.trim()))
        .flatten()
}

/// Where a finding's enclosing function is, for [`apply_allow_stub`].
#[derive(Debug, Clone, Copy)]
enum Scope {
    /// From the parse: the declaration line, or `None` for a finding that is
    /// already anchored on one.
    Known(Option<u32>),
    /// Substring path: infer it with [`inferred_function_line`].
    Infer,
}

/// How far above a finding the substring path looks for its function.
const MAX_SCOPE_SEARCH_LINES: u32 = 400;

/// The nearest line above `line` that declares a function and is indented
/// less than `line` itself, on the substring path where there is no tree.
///
/// The indentation rule is what stops a marker on one function from covering
/// code that follows it: a placeholder at the function's own depth is not in
/// its body. A line that cannot be read ends the search, so with no working
/// tree only the diff's own contiguous added lines are considered.
///
/// `c_family` also recognises Java and C# method declarations, which have no
/// keyword; see [`declares_c_family_method`].
fn inferred_function_line<'a>(
    line: u32,
    line_at: &dyn Fn(u32) -> Option<&'a str>,
    c_family: bool,
) -> Option<u32> {
    let indent = |s: &str| s.len() - s.trim_start().len();
    let own = indent(line_at(line)?);
    let mut n = line;
    for _ in 0..MAX_SCOPE_SEARCH_LINES {
        n = n.checked_sub(1).filter(|m| *m >= 1)?;
        let text = line_at(n)?;
        if text.trim().is_empty() {
            continue;
        }
        let trimmed = text.trim();
        if indent(text) < own
            && (declares_function(trimmed) || c_family && declares_c_family_method(trimmed))
        {
            return Some(n);
        }
    }
    None
}

/// Whether a trimmed line opens a function in one of the languages whose
/// keyword makes that recognisable from the line alone.
fn declares_function(trimmed: &str) -> bool {
    const MODIFIERS: &[&str] = &[
        "pub",
        "pub(crate)",
        "pub(super)",
        "async",
        "unsafe",
        "const",
        "extern",
        "export",
        "default",
        "static",
        "public",
        "private",
        "protected",
    ];
    let mut words = trimmed.split_whitespace().peekable();
    while words.peek().is_some_and(|w| MODIFIERS.contains(w)) {
        words.next();
    }
    matches!(words.next(), Some("fn" | "def" | "func" | "function"))
}

/// Whether a trimmed Java or C# line opens a method or constructor.
///
/// These declare a method as `[annotations] [modifiers] [type] name(params)`,
/// so the shape is: something before a `(`, ending in a bare identifier, with
/// at least one word before that identifier, and not a statement. A statement
/// gives itself away by its first word (`return`, `throw`, `if`, `new`, …),
/// by an `=` before the parenthesis, by a qualified callee (`log.info(`), or
/// by ending in `;` — which also excludes an abstract or interface method,
/// which has no body to hold a placeholder.
fn declares_c_family_method(trimmed: &str) -> bool {
    const STATEMENT_WORDS: &[&str] = &[
        "if",
        "else",
        "for",
        "foreach",
        "while",
        "do",
        "switch",
        "case",
        "catch",
        "try",
        "finally",
        "using",
        "lock",
        "return",
        "throw",
        "new",
        "await",
        "yield",
        "var",
        "fixed",
        "checked",
        "unchecked",
        "synchronized",
        "assert",
        "goto",
        "break",
        "continue",
        "when",
        "typeof",
        "sizeof",
        "nameof",
        "default",
        "}",
    ];
    if trimmed.ends_with(';') {
        return false;
    }
    let Some(open) = trimmed.find('(') else {
        return false;
    };
    let head = &trimmed[..open];
    if head.contains('=') {
        return false;
    }
    // Annotations (`@Override`) and attributes (`[HttpGet]`) on the same line
    // precede the declaration rather than being part of it.
    let words: Vec<&str> = head
        .split_whitespace()
        .filter(|w| !w.starts_with('@') && !w.starts_with('['))
        .collect();
    let is_identifier = |w: &str| {
        w.starts_with(|c: char| c.is_alphabetic() || c == '_')
            && w.chars().all(|c| c.is_alphanumeric() || c == '_')
    };
    match words[..] {
        [first, .., name] => is_identifier(name) && !STATEMENT_WORDS.contains(&first),
        // A constructor with no modifier, `Pricing(Catalog c) {`: one
        // capitalised name whose parameter list closes on the line. A call
        // statement of that shape would end in `;`, refused above.
        [name] => {
            is_identifier(name)
                && name.starts_with(char::is_uppercase)
                && trimmed.trim_end_matches('{').trim_end().ends_with(')')
        }
        [] => false,
    }
}

/// A line that sits between a declaration and what it annotates.
///
/// In Java and C# that includes a C# attribute, `[HttpGet]`. Only there: in
/// most other languages a line opening with `[` is a list or an index, and
/// treating it as an annotation would let a marker reach across code.
fn is_annotation_line(trimmed: &str, c_family: bool) -> bool {
    (c_family && trimmed.starts_with('['))
        || trimmed.starts_with("//")
        || trimmed.starts_with('#')
        || trimmed.starts_with("/*")
        || trimmed.starts_with('*')
        || trimmed.starts_with('@')
        || trimmed.starts_with("--")
}

/// The reason an allow-stub marker on this line gives, `Some("")` for a
/// marker with none, or `None` when there is no marker.
fn allow_stub_reason(line: &str) -> Option<String> {
    let at = line.to_ascii_lowercase().find(ALLOW_STUB_MARKER)?;
    let rest = &line[at + ALLOW_STUB_MARKER.len()..];
    let reason = rest
        .trim_start_matches([':', '(', ' ', '\t', '='])
        .trim_end_matches(['*', '/', ')', '>', '-', ' ', '\t'])
        .trim();
    Some(reason.to_string())
}

/// Reports whether a line looks like a comment or a string literal, which is
/// where a placeholder marker means something.
///
/// The alternative — flagging the marker anywhere — makes an identifier such as
/// `todoItems` or a legitimate `stubServer` into a finding, and a gate that
/// fires on ordinary names is a gate that gets disabled.
fn is_comment_or_string(trimmed: &str) -> bool {
    trimmed.starts_with("//")
        || trimmed.starts_with('#')
        || trimmed.starts_with("/*")
        || trimmed.starts_with('*')
        || trimmed.starts_with("--")
        || trimmed.starts_with("<!--")
        || trimmed.contains("\" todo")
        || trimmed.contains("// todo")
}

/// Scans added lines for credential shapes.
///
/// This is the gate whose *absence* mattered most. The write gate refuses to
/// write to `.env`, but nothing stopped a key from being pasted into an
/// ordinary source file — and a credential committed to history is compromised
/// even after it is deleted, because the object stays in the repository.
pub fn scan_secrets(files: &[FileDiff]) -> Vec<Finding> {
    let mut findings = Vec::new();
    for file in files {
        for (line_no, content) in &file.added_lines {
            if let Some(secret) = dc_redact::find_secret(content) {
                findings.push(Finding {
                    gate: "secret_scan",
                    severity: Severity::Blocking,
                    // A vendor prefix and a length floor, or a literal in a
                    // credential's context — a named key, a URL's userinfo, an
                    // authorization header. That identifies the *shape* of a
                    // credential; it is
                    // not a proof that the token authenticates anything, and a
                    // fixture key, a rotated key and a live key are
                    // indistinguishable here. It blocks anyway — the cost of
                    // being wrong is one redaction, and the cost of being
                    // right and silent is a key in the history forever — but
                    // the report says which of the two kinds of certainty this
                    // is.
                    strength: Strength::Derived,
                    path: file.path.clone(),
                    line: *line_no,
                    // The finding names the shape and shows only the prefix.
                    // A report that quotes the key in full copies the secret
                    // into the evidence trail, the terminal, and the session
                    // log — which is the leak this gate exists to prevent.
                    evidence: secret.redacted,
                    message: format!(
                        "added line contains what looks like a {}; a credential in a commit is \
                         compromised even after it is deleted, because the object stays in history",
                        secret.name
                    ),
                });
            }
        }
    }
    findings
}

/// Builds the evidence text a non-secret finding may quote from an added line.
///
/// Every evidence field outside `scan_secrets` is built through here, so a
/// line carrying both a stub marker and a credential cannot leak the
/// credential through a gate that does not itself look for credentials — which
/// is exactly how `// TODO remove before merge sk-ant-…` once reached the
/// report verbatim while the secret gate beside it showed only `sk-ant-…`.
fn safe_evidence(line: &str) -> String {
    let trimmed = line.trim();
    match dc_redact::find_secret(trimmed) {
        Some(secret) => format!("<contains a {}; quoted text withheld>", secret.name),
        None => truncate(trimmed, 120),
    }
}

/// A file's test coverage, as a set of covered line numbers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileCoverage {
    pub path: String,
    pub covered_lines: Vec<u32>,
}

/// What the coverage intersection found for one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageGap {
    pub path: String,
    /// Added lines no test executed.
    pub uncovered_lines: Vec<u32>,
    pub added_lines: usize,
}

/// The result of intersecting a diff with coverage data.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CoverageReport {
    pub gaps: Vec<CoverageGap>,
    /// Files that changed but for which no coverage data was supplied at all.
    ///
    /// This list is the reason the whole report exists in this shape. A file
    /// with no coverage data and a file with full coverage both produce zero
    /// gaps, and reporting them the same way is how "diff coverage passed"
    /// comes to mean "coverage was never measured".
    pub unmeasured: Vec<String>,
    /// Files the intersection did not ask a coverage question about, because
    /// their extension is not one this gate knows how to measure.
    ///
    /// The third bucket exists for the same reason as the second. A skipped
    /// file used to leave no trace at all — not a gap, not unmeasured — so a
    /// diff that touched only files outside the allowlist produced an
    /// all-clear that was indistinguishable from one whose every line was
    /// executed. It is reported rather than blocking: "coverage is not a
    /// question about this file" is a real answer, and it is only a safe one
    /// while it is written down.
    pub skipped_by_type: Vec<String>,
}

impl CoverageReport {
    /// Reports whether every added line was measured and executed.
    ///
    /// `skipped_by_type` is deliberately not part of this. A skipped file is
    /// not a failure of the change; it is a limit of the gate, and the report
    /// carries it so a reader can see which one they are looking at.
    pub fn is_clean(&self) -> bool {
        self.gaps.is_empty() && self.unmeasured.is_empty()
    }
}

/// Intersects added lines with covered lines.
///
/// Files whose extension marks them as non-executable — documentation, data,
/// configuration — are not asked the coverage question, because "coverage" is
/// not a question about a Markdown file. They are *recorded* in
/// `skipped_by_type` rather than dropped: a file that left the report with no
/// trace at all was reported exactly like one that was measured and clean.
pub fn intersect_coverage(files: &[FileDiff], coverage: &[FileCoverage]) -> CoverageReport {
    let mut report = CoverageReport::default();
    for file in files {
        if file.status == crate::ChangeStatus::Deleted {
            continue;
        }
        let added = file.added_line_numbers();
        if added.is_empty() {
            continue;
        }
        if !is_executable_source(&file.path) {
            report.skipped_by_type.push(file.path.clone());
            continue;
        }
        let Some(entry) = coverage.iter().find(|c| c.path == file.path) else {
            report.unmeasured.push(file.path.clone());
            continue;
        };
        // Binary search rather than a linear scan of every covered line for
        // every added line: `finish` in the coverage parser hands back sorted,
        // deduplicated lines, and the quadratic version turned a large profile
        // into a second source of the wall-clock blowup the parser's bounds
        // now prevent. A caller that built a FileCoverage by hand may not have
        // sorted it, so that is checked rather than assumed — a wrong answer
        // from a binary search over unsorted data would be a silently missed
        // coverage gap.
        let sorted: std::borrow::Cow<'_, [u32]> = if entry.covered_lines.is_sorted() {
            std::borrow::Cow::Borrowed(&entry.covered_lines)
        } else {
            let mut owned = entry.covered_lines.clone();
            owned.sort_unstable();
            std::borrow::Cow::Owned(owned)
        };
        let uncovered: Vec<u32> = added
            .iter()
            .copied()
            .filter(|line| sorted.binary_search(line).is_err())
            .collect();
        if !uncovered.is_empty() {
            report.gaps.push(CoverageGap {
                path: file.path.clone(),
                uncovered_lines: uncovered,
                added_lines: added.len(),
            });
        }
    }
    report.unmeasured.sort();
    report.skipped_by_type.sort();
    report.gaps.sort_by(|a, b| a.path.cmp(&b.path));
    report
}

/// Extensions whose files execute, and therefore can be covered.
///
/// The list is an allowlist and it is kept long on purpose. Every extension
/// missing from it used to be dropped from the report entirely, so a diff
/// adding `rm -rf "$TARGET"` to a `.sh` file, or an ES module to a `.mjs` one,
/// passed the coverage gate without being counted — while the identical
/// CommonJS `.js` file was measured. Shell, Kotlin, Swift, PHP and C# were in
/// the same hole. Nothing here is measured by every ecosystem's tooling, and a
/// file with no coverage data lands in `unmeasured`, which is the honest
/// answer: it executed, and nobody showed evidence that it ran.
fn is_executable_source(path: &str) -> bool {
    const EXECUTABLE: &[&str] = &[
        ".go", ".rs", ".py", ".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".mts", ".cts", ".java",
        ".kt", ".kts", ".scala", ".swift", ".rb", ".php", ".cs", ".c", ".cc", ".cpp", ".h", ".hpp",
        ".sh", ".bash", ".zsh", ".ps1", ".pl", ".lua",
    ];
    // A test file's own lines are not the thing coverage is asking about.
    if dc_glob::is_test_path(path) {
        return false;
    }
    EXECUTABLE.iter().any(|ext| path.ends_with(ext))
}

fn truncate(text: &str, n: usize) -> String {
    if text.chars().count() <= n {
        return text.to_string();
    }
    let cut: String = text.chars().take(n).collect();
    format!("{cut}…")
}

#[cfg(test)]
mod test_path_conformance {
    use super::is_executable_source;

    /// A test file's own lines are not what coverage asks about, and "is a test
    /// file" is the shared rule: a row of the fixture that executes is
    /// measurable exactly when the fixture says it is not a test.
    #[test]
    fn executable_test_files_are_exempt_exactly_as_the_shared_fixture_says() {
        let rows =
            dc_glob::parse_test_path_rows(include_str!("../../../protocols/test-paths/v1.json"))
                .expect("fixture");
        let mut checked = 0;
        let mut wrong = Vec::new();
        for (path, is_test) in rows {
            // The fixture's rows without an executable extension say nothing
            // about this predicate.
            let probe = if path.contains('.') {
                path.clone()
            } else {
                continue;
            };
            let executable = [
                ".go", ".py", ".ts", ".js", ".rb", ".java", ".rs", ".cs", ".scala",
            ]
            .iter()
            .any(|ext| probe.ends_with(ext));
            if !executable {
                continue;
            }
            checked += 1;
            if is_executable_source(&probe) == is_test {
                wrong.push(format!("{probe} (test={is_test})"));
            }
        }
        assert!(checked >= 15, "only {checked} rows reached the predicate");
        assert!(wrong.is_empty(), "{wrong:?}");
    }
}
