//! Documentation drift: Markdown that names code which is gone or has moved.
//!
//! Deterministic, no model. A document is read, its references are extracted
//! by a pure scanner ([`extract_references`]), and each one is resolved against
//! the latest DevMap generation and the working tree
//! ([`doc_drift_report`], reached through `StoreQueryEngine::doc_drift`).
//!
//! # What counts as a reference
//!
//! Precision over recall: a false "drift" is noise that trains people to ignore
//! the report, so anything ambiguous is counted under `skipped`, with a reason,
//! and never checked. Fenced code blocks (``` and ~~~, an unterminated fence
//! runs to the end of the file as CommonMark says), indented code blocks and
//! HTML comments are not scanned.
//!
//! 1. **Links** `[text](target)`, images, and reference definitions
//!    `[x]: target` whose target is repo-relative. Schemes (`http:`,
//!    `mailto:`), a bare `#anchor` and an absolute `/path` are skipped. The
//!    target resolves against the document's directory and is normalised;
//!    anything that climbs out of the repository root is `outside_repo` and is
//!    not followed. A missing target is `missing_file`; `#L10` / `#L10-L20`
//!    past the end of the file is `line_out_of_range`; `#slug` on a Markdown
//!    target that no heading produces (GitHub slugs, duplicates `-1`, `-2`) is
//!    `missing_anchor`; other anchors are skipped. Symbol-shaped code spans in
//!    the link text must be defined in a code target: defined elsewhere is
//!    `symbol_moved` (with the current files), defined nowhere
//!    `missing_symbol`.
//! 2. **Path spans**: a code span holding a `/`, no whitespace and none of
//!    `* ? < > { } $ |`, naming an existing path or ending in a known source or
//!    document extension, with an optional `:N` / `:N-M` suffix. Tried against
//!    the repo root, then the document's directory, then as the tail of an
//!    indexed path (`db/query.rs` for `rust/devmap-store/src/db/query.rs`).
//!    Missing is `missing_file`; a line past the end is `line_out_of_range`.
//! 3. **Symbol spans**: one identifier or a qualified identifier that is
//!    symbol-shaped: a name with an internal case change (`VerifyTask`,
//!    `runRigorGates`), a trailing `()`, or a `::`-qualified name. A `.`
//!    qualified name is checked when its last segment starts upper-case, or it
//!    ends in `()`. Plain lowercase words, ALL_CAPS, flags, snake_case without
//!    `()` or `::`, bare file names and well-known external qualifiers
//!    (`std::`, `fmt.`) are `ambiguous_identifier` / `bare_filename` /
//!    `external_qualifier` / `known_term` skips.
//!
//! # Symbol matching rule
//!
//! A span `Q.L` (qualifier `Q`, last segment `L`; `::` and `.` are the same
//! separator) matches a stored symbol when the symbol's name is `L` **and**
//! `Q` is empty, **or** `Q`'s segments are the tail of the symbol's qualified
//! name once the leading `path::` is removed (`Store::latest_file` against
//! `rust/.../query.rs::Store::latest_file`), **or** the last segment of `Q`
//! equals (case-insensitively, `-` read as `_`) a directory name or the file
//! stem of the symbol's path (`store.Requirements` for a Go package `store`,
//! `devmap_store::Store` for `rust/devmap-store/`). A name that exists only
//! under another qualifier is still `missing_symbol`, with the places it does
//! exist in `current_locations`.
//!
//! # Honesty
//!
//! Every cap carries both numbers and sets `walk_incomplete`: documents over
//! the cap, documents that could not be read, lines too long to scan,
//! references past the per-document cap, targets that could not be read.
//! `findings: []` therefore only means "clean" when `walk_incomplete` is
//! absent. No persisted generation answers `available: false`, not an empty
//! list. Documents the index never read (over the size limit, ignored) are
//! not scanned. Secret paths are never opened, as documents or as targets: a
//! referenced secret path is checked for existence only.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;

use devmap_store::{Store, StoredSymbol};
use serde::{Deserialize, Serialize};

use crate::cancel::Cancel;

#[cfg(test)]
#[path = "doc_drift_tests.rs"]
mod tests;

/// Lines longer than this are not scanned (counted under `line_too_long`). The
/// inline scanner is bounded per line, so a hostile one-line document cannot
/// turn into quadratic work.
pub const MAX_LINE_BYTES: usize = 4096;
/// Most references one document yields before the scan stops.
pub const MAX_REFERENCES_PER_DOC: usize = 2000;
/// Most documents one call scans.
pub const MAX_DOCS: usize = 5000;
/// Most findings one call keeps. `total` still counts the rest.
pub const MAX_FINDINGS: usize = 10_000;
/// How far the scanner looks for the end of a link's text or destination.
const LINK_LOOKAHEAD: usize = 512;
/// Target facts kept between references.
const TARGET_CACHE_CAP: usize = 512;
/// Locations named on a moved-symbol finding.
const MAX_LOCATIONS: usize = 5;

/// Languages whose files declare no code, so a link to one is not "a code file".
const NON_CODE_LANGUAGES: [&str; 7] = [
    "markdown", "json", "yaml", "toml", "config", "text", "notebook",
];

/// Extensions that make a slash-bearing code span a path even when nothing is
/// there: source files (the query matcher's list) and the documents and
/// manifests a README links.
const DOC_EXTENSIONS: [&str; 12] = [
    "md", "mdx", "toml", "json", "yaml", "yml", "sh", "sql", "proto", "html", "css", "txt",
];

/// Qualifiers that name somebody else's code.
const EXTERNAL_QUALIFIERS: [&str; 38] = [
    "std", "core", "alloc", "fmt", "os", "io", "fs", "sys", "strings", "strconv", "errors", "time",
    "context", "http", "json", "sync", "sort", "bytes", "regexp", "filepath", "log", "flag",
    "self", "Self", "this", "crate", "super", "console", "process", "window", "document", "Math",
    "JSON", "Object", "Array", "Vec", "Option", "Result",
];

/// Product and language names that are PascalCase without being symbols.
const KNOWN_TERMS: [&str; 30] = [
    "GitHub",
    "GitLab",
    "JavaScript",
    "TypeScript",
    "PostgreSQL",
    "MySQL",
    "SQLite",
    "MongoDB",
    "OAuth",
    "macOS",
    "iOS",
    "iPadOS",
    "tvOS",
    "watchOS",
    "WebAssembly",
    "OpenAI",
    "YouTube",
    "LinkedIn",
    "ClickHouse",
    "FreeBSD",
    "OpenBSD",
    "NetBSD",
    "VSCode",
    "IntelliJ",
    "PyTorch",
    "TensorFlow",
    "NumPy",
    "GraphQL",
    "WebSocket",
    "DevCouncil",
];

/// What a reference asks about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefSpec {
    /// `[text](target)`, an image, or a `[x]: target` definition. `target` is
    /// as written (anchor included); `symbols` are the symbol-shaped code spans
    /// in the link text.
    Link {
        target: String,
        symbols: Vec<String>,
    },
    /// A code span naming a path. `lines` is the `:N` / `:N-M` suffix.
    PathSpan {
        path: String,
        lines: Option<(u32, u32)>,
        /// The last segment has a known source or document extension.
        has_known_extension: bool,
    },
    /// A symbol-shaped code span: `qualifier` segments, then `name`.
    Symbol {
        qualifier: Vec<String>,
        name: String,
        written: String,
    },
}

/// One reference and the line (1-based) it was written on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocReference {
    pub line: u32,
    pub spec: RefSpec,
}

/// What [`extract_references`] found in one document.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Extraction {
    pub references: Vec<DocReference>,
    /// Candidates recognised but not checkable, by reason.
    pub skipped: BTreeMap<&'static str, u32>,
    /// The per-document reference cap cut the scan.
    pub truncated: bool,
}

impl Extraction {
    /// Every candidate the scan recognised: the checkable ones and the skipped.
    pub fn candidates(&self) -> u32 {
        let skipped: u32 = self.skipped.values().sum();
        u32::try_from(self.references.len())
            .unwrap_or(u32::MAX)
            .saturating_add(skipped)
    }

    fn skip(&mut self, reason: &'static str) {
        *self.skipped.entry(reason).or_insert(0) += 1;
    }
}

/// A line outside code, with HTML comments blanked.
struct Line {
    no: u32,
    text: String,
}

fn is_blank(text: &str) -> bool {
    text.trim().is_empty()
}

fn leading_indent(text: &str) -> usize {
    let mut width = 0;
    for byte in text.bytes() {
        match byte {
            b' ' => width += 1,
            b'\t' => width += 4,
            _ => break,
        }
    }
    width
}

fn starts_list_item(trimmed: &str) -> bool {
    let bytes = trimmed.as_bytes();
    match bytes.first() {
        Some(b'-' | b'*' | b'+') => matches!(bytes.get(1), Some(b' ' | b'\t')),
        Some(b'0'..=b'9') => {
            let digits = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
            digits <= 9
                && matches!(bytes.get(digits), Some(b'.' | b')'))
                && matches!(bytes.get(digits + 1), Some(b' ' | b'\t'))
        }
        _ => false,
    }
}

/// The fence a line opens: its character and length.
fn fence_open(text: &str) -> Option<(u8, usize)> {
    if leading_indent(text) > 3 {
        return None;
    }
    let trimmed = text.trim_start();
    let first = *trimmed.as_bytes().first()?;
    if first != b'`' && first != b'~' {
        return None;
    }
    let run = trimmed.bytes().take_while(|b| *b == first).count();
    if run < 3 {
        return None;
    }
    // A backtick fence's info string cannot hold a backtick: that is a code
    // span written at the start of a line.
    if first == b'`' && trimmed[run..].contains('`') {
        return None;
    }
    Some((first, run))
}

fn fence_closes(text: &str, fence: (u8, usize)) -> bool {
    if leading_indent(text) > 3 {
        return false;
    }
    let trimmed = text.trim();
    let run = trimmed.bytes().take_while(|b| *b == fence.0).count();
    run >= fence.1 && run == trimmed.len()
}

/// `text` with HTML comments removed. `in_comment` carries an open comment
/// across lines. A `<!--` inside an inline code span is not a comment.
fn strip_comments(text: &str, in_comment: &mut bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        if *in_comment {
            match rest.find("-->") {
                Some(end) => {
                    *in_comment = false;
                    rest = &rest[end + 3..];
                }
                None => return out,
            }
            continue;
        }
        let consumed = text.len() - rest.len();
        let mut from = 0;
        let start = loop {
            match rest[from..].find("<!--") {
                None => break None,
                Some(offset) => {
                    let at = from + offset;
                    let before = &text[..consumed + at];
                    if before.bytes().filter(|b| *b == b'`').count() % 2 == 1 {
                        from = at + 4;
                    } else {
                        break Some(at);
                    }
                }
            }
        };
        match start {
            None => {
                out.push_str(rest);
                return out;
            }
            Some(at) => {
                out.push_str(&rest[..at]);
                *in_comment = true;
                rest = &rest[at + 4..];
            }
        }
    }
}

/// Every line of `text` that is prose: outside fences, indented code and HTML
/// comments.
fn prose_lines(text: &str) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut fence: Option<(u8, usize)> = None;
    let mut in_comment = false;
    let mut previous_blank = true;
    let mut previous_indented_code = false;
    for (index, raw) in text.lines().enumerate() {
        let no = u32::try_from(index + 1).unwrap_or(u32::MAX);
        if let Some(open) = fence {
            if fence_closes(raw, open) {
                fence = None;
            }
            previous_blank = false;
            previous_indented_code = false;
            continue;
        }
        let visible = if in_comment || raw.contains("<!--") {
            strip_comments(raw, &mut in_comment)
        } else {
            raw.to_string()
        };
        if let Some(open) = fence_open(&visible) {
            fence = Some(open);
            previous_blank = false;
            previous_indented_code = false;
            continue;
        }
        let blank = is_blank(&visible);
        let indented_code = !blank
            && leading_indent(&visible) >= 4
            && (previous_blank || previous_indented_code)
            && !starts_list_item(visible.trim_start());
        previous_blank = blank;
        previous_indented_code = indented_code || (blank && previous_indented_code);
        if indented_code {
            continue;
        }
        lines.push(Line { no, text: visible });
    }
    lines
}

fn percent_decode(text: &str) -> String {
    if !text.contains('%') {
        return text.to_string();
    }
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let pair = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(byte) = pair.and_then(|pair| u8::from_str_radix(pair, 16).ok()) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| text.to_string())
}

/// The GitHub slug of one heading's text.
fn slugify(heading: &str) -> String {
    let mut plain = String::with_capacity(heading.len());
    let mut chars = heading.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            // `[text](url)` keeps the text.
            ']' if chars.peek() == Some(&'(') => {
                let mut depth = 0u32;
                for next in chars.by_ref() {
                    match next {
                        '(' => depth += 1,
                        ')' => {
                            depth = depth.saturating_sub(1);
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
            }
            '<' => {
                // An inline HTML tag contributes nothing.
                let mut tag = String::new();
                let mut closed = false;
                for next in chars.by_ref() {
                    if next == '>' {
                        closed = true;
                        break;
                    }
                    tag.push(next);
                    if tag.len() > 256 {
                        break;
                    }
                }
                if !closed {
                    plain.push('<');
                    plain.push_str(&tag);
                }
            }
            _ => plain.push(ch),
        }
    }
    let mut slug = String::with_capacity(plain.len());
    for ch in plain.trim().chars() {
        if ch == ' ' {
            slug.push('-');
        } else if ch == '-' || ch == '_' || ch.is_alphanumeric() {
            slug.extend(ch.to_lowercase());
        }
    }
    slug
}

fn atx_heading(text: &str) -> Option<&str> {
    if leading_indent(text) > 3 {
        return None;
    }
    let trimmed = text.trim_start();
    let hashes = trimmed.bytes().take_while(|b| *b == b'#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &trimmed[hashes..];
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
        return None;
    }
    let rest = rest.trim();
    // A closing run of `#` is not part of the heading.
    let without_closing = rest.trim_end_matches('#');
    Some(
        if without_closing.len() < rest.len()
            && (without_closing.is_empty() || without_closing.ends_with([' ', '\t']))
        {
            without_closing.trim_end()
        } else {
            rest
        },
    )
}

fn setext_underline(text: &str) -> bool {
    if leading_indent(text) > 3 {
        return false;
    }
    let trimmed = text.trim();
    !trimmed.is_empty()
        && (trimmed.bytes().all(|b| b == b'=') || trimmed.bytes().all(|b| b == b'-'))
}

/// Every anchor the headings of `text` produce, GitHub style: lowercase, no
/// punctuation but `-` and `_`, spaces as `-`, and the second heading with the
/// same slug `slug-1`, the third `slug-2`.
pub fn heading_slugs(text: &str) -> BTreeSet<String> {
    let mut slugs = BTreeSet::new();
    let mut seen: HashMap<String, u32> = HashMap::new();
    let mut add = |heading: &str| {
        let base = slugify(heading);
        let count = seen.entry(base.clone()).or_insert(0);
        let slug = if *count == 0 {
            base
        } else {
            format!("{base}-{count}")
        };
        *count += 1;
        slugs.insert(slug);
    };
    let mut previous: Option<(u32, &str)> = None;
    let lines = prose_lines(text);
    let mut paragraph: Option<String> = None;
    for line in &lines {
        if line.text.len() > MAX_LINE_BYTES {
            paragraph = None;
            previous = None;
            continue;
        }
        if let Some(heading) = atx_heading(&line.text) {
            add(heading);
            paragraph = None;
            previous = None;
            continue;
        }
        let consecutive = previous.is_some_and(|(no, _)| no + 1 == line.no);
        if setext_underline(&line.text) && consecutive {
            if let Some(heading) = paragraph.take() {
                add(&heading);
                previous = None;
                continue;
            }
        }
        let trimmed = line.text.trim();
        let plain_paragraph = !trimmed.is_empty()
            && leading_indent(&line.text) <= 3
            && !starts_list_item(trimmed)
            && !trimmed.starts_with(['>', '|']);
        paragraph = plain_paragraph.then(|| trimmed.to_string());
        previous = Some((line.no, ""));
    }
    slugs
}

/// `target` read from the document at `doc`, as a normalised repo-relative
/// path. `None` when it climbs out of the repository root.
pub fn resolve_relative(doc: &str, target: &str) -> Option<String> {
    let mut parts: Vec<&str> = match doc.rfind('/') {
        Some(at) => doc[..at]
            .split('/')
            .filter(|part| !part.is_empty())
            .collect(),
        None => Vec::new(),
    };
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    Some(parts.join("/"))
}

/// `target` read from the repo root.
fn normalise_from_root(target: &str) -> Option<String> {
    resolve_relative("", target)
}

fn has_scheme(target: &str) -> bool {
    let bytes = target.as_bytes();
    if !bytes.first().is_some_and(u8::is_ascii_alphabetic) {
        return false;
    }
    let end = bytes
        .iter()
        .position(|b| !(b.is_ascii_alphanumeric() || matches!(b, b'+' | b'.' | b'-')));
    matches!(end, Some(at) if bytes[at] == b':')
}

fn is_identifier(segment: &str) -> bool {
    let mut chars = segment.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// An internal case change: lower or digit then upper (`runRigor`, `Verify` +
/// `Task`), or an acronym leading into a word (`HTTPServer`).
fn has_case_change(segment: &str) -> bool {
    let chars: Vec<char> = segment.chars().collect();
    chars.windows(2).any(|pair| {
        (pair[0].is_ascii_lowercase() || pair[0].is_ascii_digit()) && pair[1].is_ascii_uppercase()
    }) || chars.windows(3).any(|triple| {
        triple[0].is_ascii_uppercase()
            && triple[1].is_ascii_uppercase()
            && triple[2].is_ascii_lowercase()
    })
}

fn has_known_extension(last_segment: &str) -> bool {
    let Some((stem, extension)) = last_segment.rsplit_once('.') else {
        return false;
    };
    !stem.is_empty()
        && (crate::query_match::has_source_extension(last_segment)
            || DOC_EXTENSIONS.contains(&extension))
}

fn domain_like(first_segment: &str) -> bool {
    if first_segment.starts_with('.') {
        return false;
    }
    match first_segment.rsplit_once('.') {
        Some((host, tld)) if !host.is_empty() => matches!(
            tld,
            "com" | "org" | "net" | "io" | "dev" | "ai" | "app" | "edu" | "gov" | "co"
        ),
        _ => false,
    }
}

/// `:N` or `:N-M`.
fn split_line_suffix(span: &str) -> (&str, Option<(u32, u32)>) {
    let Some((base, suffix)) = span.rsplit_once(':') else {
        return (span, None);
    };
    if base.is_empty() || base.ends_with(':') {
        return (span, None);
    }
    let (first, last) = match suffix.split_once('-') {
        Some((a, b)) => (a, b),
        None => (suffix, suffix),
    };
    match (first.parse::<u32>(), last.parse::<u32>()) {
        (Ok(a), Ok(b)) if !first.is_empty() && !last.is_empty() && a.max(b) > 0 => {
            (base, Some((a.min(b), a.max(b))))
        }
        _ => (span, None),
    }
}

enum Classified {
    Ignore,
    Skip(&'static str),
    Reference(RefSpec),
}

/// What a code span is: a path, a symbol, or neither.
fn classify_span(content: &str) -> Classified {
    let span = content.trim();
    if span.is_empty() || span.contains(char::is_whitespace) || span.starts_with('-') {
        return Classified::Ignore;
    }
    let (base, lines) = split_line_suffix(span);
    if base.contains('/') {
        return classify_path(base, lines);
    }
    classify_symbol(span)
}

fn classify_path(base: &str, lines: Option<(u32, u32)>) -> Classified {
    if base.contains([
        '*', '?', '<', '>', '{', '}', '$', '|', '\\', '(', ')', '=', ',', ';', '@',
    ]) || base.contains("::")
    {
        return Classified::Ignore;
    }
    if base.contains("://") {
        return Classified::Skip("external_reference");
    }
    if base.starts_with('/') {
        return Classified::Skip("absolute_path");
    }
    if base.starts_with('~') {
        return Classified::Skip("home_path");
    }
    let trimmed = base.trim_start_matches("./").trim_end_matches('/');
    if trimmed.is_empty() {
        return Classified::Ignore;
    }
    if domain_like(trimmed.split('/').next().unwrap_or("")) {
        return Classified::Skip("external_reference");
    }
    if trimmed.starts_with("path/to/") || trimmed.contains("/path/to/") {
        return Classified::Skip("placeholder_path");
    }
    let last = trimmed.rsplit('/').next().unwrap_or("");
    Classified::Reference(RefSpec::PathSpan {
        path: trimmed.to_string(),
        lines,
        has_known_extension: has_known_extension(last),
    })
}

fn classify_symbol(span: &str) -> Classified {
    let (body, called) = match span.strip_suffix("()") {
        Some(body) => (body, true),
        None => (span, false),
    };
    let separators_colon = body.contains("::");
    let segments: Vec<&str> = if separators_colon {
        body.split("::").flat_map(|part| part.split('.')).collect()
    } else {
        body.split('.').collect()
    };
    if segments.is_empty() || !segments.iter().all(|segment| is_identifier(segment)) {
        // `Cargo.toml` is not an identifier-shaped file name check, but a
        // bare file name is worth a named skip.
        return if body.contains('.')
            && !body.contains(['(', ')', '<', '>'])
            && has_known_extension(body.rsplit('/').next().unwrap_or(body))
        {
            Classified::Skip("bare_filename")
        } else {
            Classified::Ignore
        };
    }
    let name = segments[segments.len() - 1];
    if segments.len() > 1 && !separators_colon && has_known_extension(body) {
        return Classified::Skip("bare_filename");
    }
    let qualifier: Vec<String> = segments[..segments.len() - 1]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    if segments.len() > 1 && EXTERNAL_QUALIFIERS.contains(&segments[0]) {
        return Classified::Skip("external_qualifier");
    }
    if KNOWN_TERMS.contains(&body) {
        return Classified::Skip("known_term");
    }
    let qualified = segments.len() > 1;
    let all_caps = !name.chars().any(|c| c.is_ascii_lowercase());
    let shaped = if called {
        true
    } else if all_caps {
        false
    } else if qualified {
        separators_colon
            || name.chars().next().is_some_and(|c| c.is_ascii_uppercase())
            || has_case_change(name)
    } else {
        has_case_change(name)
    };
    if !shaped {
        return Classified::Skip("ambiguous_identifier");
    }
    Classified::Reference(RefSpec::Symbol {
        qualifier,
        name: name.to_string(),
        written: span.to_string(),
    })
}

/// Index of the backtick run of length `n` that closes a span opened before
/// `from`.
fn find_closing(bytes: &[u8], from: usize, n: usize) -> Option<usize> {
    let mut j = from;
    while j < bytes.len() {
        if bytes[j] == b'`' {
            let run = bytes[j..].iter().take_while(|b| **b == b'`').count();
            if run == n {
                return Some(j);
            }
            j += run;
        } else {
            j += 1;
        }
    }
    None
}

/// A code span's content: one space of padding on both sides is not content.
fn span_content(inner: &str) -> &str {
    if inner.len() >= 2
        && inner.starts_with(' ')
        && inner.ends_with(' ')
        && !inner.trim().is_empty()
    {
        &inner[1..inner.len() - 1]
    } else {
        inner
    }
}

/// The index of the `]` matching the `[` at `open`, looking at most
/// [`LINK_LOOKAHEAD`] bytes ahead and stepping over code spans and escapes.
fn matching_bracket(bytes: &[u8], open: usize) -> Option<usize> {
    let limit = bytes.len().min(open + LINK_LOOKAHEAD);
    let mut depth = 0usize;
    let mut i = open;
    while i < limit {
        match bytes[i] {
            b'\\' => i += 1,
            b'`' => {
                let run = bytes[i..].iter().take_while(|b| **b == b'`').count();
                if let Some(close) = find_closing(&bytes[..limit], i + run, run) {
                    i = close + run - 1;
                } else {
                    i += run - 1;
                }
            }
            b'[' => depth += 1,
            b']' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// The destination of the link whose `(` is at `open`, and the index after the
/// closing `)`.
fn link_destination(line: &str, open: usize) -> Option<(&str, usize)> {
    let bytes = line.as_bytes();
    let limit = bytes.len().min(open + LINK_LOOKAHEAD);
    let mut i = open + 1;
    while i < limit && matches!(bytes[i], b' ' | b'\t') {
        i += 1;
    }
    let (destination, mut after) = if bytes.get(i) == Some(&b'<') {
        let end = (i + 1..limit).find(|j| bytes[*j] == b'>')?;
        (&line[i + 1..end], end + 1)
    } else {
        let start = i;
        let mut depth = 0usize;
        while i < limit {
            match bytes[i] {
                b'\\' => i += 1,
                b'(' => depth += 1,
                b')' if depth == 0 => break,
                b')' => depth -= 1,
                b' ' | b'\t' => break,
                _ => {}
            }
            i += 1;
        }
        // Ran out of room before the destination ended.
        if i >= limit {
            return None;
        }
        (&line[start..i], i)
    };
    // An optional title, then the closing parenthesis.
    while after < limit && bytes[after] != b')' {
        after += 1;
    }
    (bytes.get(after) == Some(&b')')).then_some((destination, after + 1))
}

fn symbol_spans_in(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            let run = bytes[i..].iter().take_while(|b| **b == b'`').count();
            if let Some(close) = find_closing(bytes, i + run, run) {
                if let Classified::Reference(RefSpec::Symbol { written, .. }) =
                    classify_span(span_content(&text[i + run..close]))
                {
                    found.push(written);
                }
                i = close + run;
                continue;
            }
            i += run;
        } else {
            i += 1;
        }
    }
    found
}

struct Scan<'a> {
    out: &'a mut Extraction,
    line: u32,
    /// Backtick run lengths with no closing run later on this line. A run that
    /// found none means no later run of that length can, so each length is
    /// searched for failure at most once and the scan stays linear.
    unclosed: u64,
}

impl Scan<'_> {
    fn push(&mut self, spec: RefSpec) {
        if self.out.references.len() >= MAX_REFERENCES_PER_DOC {
            self.out.truncated = true;
            return;
        }
        self.out.references.push(DocReference {
            line: self.line,
            spec,
        });
    }

    fn link(&mut self, destination: &str, symbols: Vec<String>) {
        let target = destination.trim();
        if target.is_empty() {
            self.out.skip("empty_link");
        } else if has_scheme(target) || target.starts_with("//") {
            self.out.skip("external_link");
        } else if target.starts_with('#') {
            self.out.skip("same_document_anchor");
        } else if target.starts_with('/') {
            self.out.skip("absolute_link");
        } else if target.contains(['{', '}', '<', '>', '$', '*']) {
            self.out.skip("placeholder");
        } else {
            self.push(RefSpec::Link {
                target: target.to_string(),
                symbols,
            });
        }
    }

    fn span(&mut self, content: &str) {
        match classify_span(span_content(content)) {
            Classified::Ignore => {}
            Classified::Skip(reason) => self.out.skip(reason),
            Classified::Reference(spec) => self.push(spec),
        }
    }

    /// Scan `line`. With `links_only` set, code spans are passed over: the
    /// text of a link is scanned for nested links (an image inside a link)
    /// without its code spans being counted a second time.
    fn scan(&mut self, line: &str, links_only: bool) {
        let bytes = line.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'\\' => i += 2,
                b'`' => {
                    let run = bytes[i..].iter().take_while(|b| **b == b'`').count();
                    let closed = if run < 64 && self.unclosed & (1 << run) != 0 {
                        None
                    } else {
                        find_closing(bytes, i + run, run)
                    };
                    match closed {
                        Some(close) => {
                            if !links_only {
                                self.span(&line[i + run..close]);
                            }
                            i = close + run;
                        }
                        None => {
                            if run < 64 {
                                self.unclosed |= 1 << run;
                            }
                            i += run;
                        }
                    }
                }
                b'[' => {
                    let end = matching_bracket(bytes, i).and_then(|close| {
                        (bytes.get(close + 1) == Some(&b'('))
                            .then(|| link_destination(line, close + 1).map(|d| (close, d)))
                            .flatten()
                    });
                    match end {
                        Some((close, (destination, after))) => {
                            let text = &line[i + 1..close];
                            let symbols = if links_only {
                                Vec::new()
                            } else {
                                symbol_spans_in(text)
                            };
                            self.link(destination, symbols);
                            if text.contains("](") {
                                self.scan(text, true);
                            }
                            i = after;
                        }
                        None => i += 1,
                    }
                }
                _ => i += 1,
            }
        }
    }
}

/// A `[label]: destination` definition, as the destination.
fn reference_definition(text: &str) -> Option<&str> {
    if leading_indent(text) > 3 {
        return None;
    }
    let trimmed = text.trim_start();
    let rest = trimmed.strip_prefix('[')?;
    let close = rest.find("]:")?;
    let label = &rest[..close];
    if label.is_empty() || label.starts_with('^') || label.contains('[') || label.contains(']') {
        return None;
    }
    let after = rest[close + 2..].trim_start();
    let destination = match after.strip_prefix('<') {
        Some(inner) => &inner[..inner.find('>')?],
        None => after.split_whitespace().next()?,
    };
    Some(destination)
}

/// Every reference in `text`. Pure: no store, no file system.
pub fn extract_references(text: &str) -> Extraction {
    let mut out = Extraction::default();
    for line in prose_lines(text) {
        if line.text.len() > MAX_LINE_BYTES {
            out.skip("line_too_long");
            continue;
        }
        let mut scan = Scan {
            out: &mut out,
            line: line.no,
            unclosed: 0,
        };
        if let Some(destination) = reference_definition(&line.text) {
            scan.link(destination, Vec::new());
            continue;
        }
        scan.scan(&line.text, false);
        if scan.out.truncated {
            break;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Resolution
// ---------------------------------------------------------------------------

/// What a reference found wrong.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum DriftKind {
    MissingFile,
    LineOutOfRange,
    MissingAnchor,
    SymbolMoved,
    MissingSymbol,
}

impl DriftKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MissingFile => "missing_file",
            Self::LineOutOfRange => "line_out_of_range",
            Self::MissingAnchor => "missing_anchor",
            Self::SymbolMoved => "symbol_moved",
            Self::MissingSymbol => "missing_symbol",
        }
    }
}

/// One reference that no longer holds.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DocDriftFinding {
    pub doc: String,
    pub line: u32,
    pub kind: DriftKind,
    /// The reference as the document wrote it.
    pub reference: String,
    pub detail: String,
    /// Where the symbol is defined now.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub current_locations: Vec<String>,
}

/// The documentation drift report.
///
/// `shown + hidden == total`. `walk_incomplete` is set when anything was not
/// checked for a reason other than the deliberate `skipped` ones; an empty
/// `findings` list with `walk_incomplete` absent and `available` true is the
/// only clean answer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DocDriftReport {
    /// False when no generation is persisted: nothing was checked.
    pub available: bool,
    pub docs_scanned: u32,
    /// Markdown documents the request matched, before the document cap.
    pub docs_total: u32,
    /// Candidates found: checked, skipped and those that held.
    pub references: u32,
    /// Checks made, by kind: `link`, `path`, `line`, `anchor`, `symbol`.
    pub checked: BTreeMap<String, u32>,
    /// Candidates and sub-checks left unchecked, by reason.
    pub skipped: BTreeMap<String, u32>,
    pub findings: Vec<DocDriftFinding>,
    pub shown: u32,
    pub hidden: u32,
    pub total: u32,
    pub truncated: bool,
    pub tokens_used: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub walk_incomplete: Option<String>,
}

fn finding_tokens(finding: &DocDriftFinding) -> u32 {
    let bytes = finding.doc.len()
        + finding.reference.len()
        + finding.detail.len()
        + finding
            .current_locations
            .iter()
            .map(String::len)
            .sum::<usize>()
        + finding.kind.as_str().len()
        + 16;
    u32::try_from(bytes / 4).unwrap_or(u32::MAX).max(1)
}

/// Repo-relative prefixes from the request, refused when they could name
/// something outside the repository.
fn clean_prefixes(values: &[String], what: &str) -> anyhow::Result<Vec<String>> {
    let mut out = Vec::with_capacity(values.len());
    for value in values {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            anyhow::bail!("{what} entries must not be empty");
        }
        if trimmed.starts_with('/')
            || trimmed.contains('\\')
            || trimmed.split('/').any(|part| part == "..")
        {
            anyhow::bail!("{what} {trimmed:?} must be a repo-relative path without `..`");
        }
        let cleaned = trimmed
            .trim_start_matches("./")
            .trim_end_matches('/')
            .to_string();
        if cleaned.is_empty() || cleaned == "." {
            anyhow::bail!("{what} {trimmed:?} names the whole repository; omit it instead");
        }
        out.push(cleaned);
    }
    Ok(out)
}

fn under_prefix(path: &str, prefix: &str) -> bool {
    path == prefix
        || (path.len() > prefix.len()
            && path.starts_with(prefix)
            && path.as_bytes()[prefix.len()] == b'/')
}

fn is_markdown_path(path: &str) -> bool {
    path.rsplit_once('.').is_some_and(|(stem, extension)| {
        !stem.is_empty() && matches!(extension, "md" | "markdown" | "mdx")
    })
}

/// Facts about a file that a reference into it needs.
struct TargetFacts {
    lines: usize,
    slugs: Option<BTreeSet<String>>,
}

enum PathState {
    Missing,
    Dir,
    File,
}

struct Resolver<'a> {
    root: &'a Path,
    cancel: &'a Cancel,
    by_name: HashMap<&'a str, Vec<&'a StoredSymbol>>,
    languages: HashMap<&'a str, &'a str>,
    suffixes: HashSet<String>,
    facts: HashMap<String, Option<std::rc::Rc<TargetFacts>>>,
    checked: BTreeMap<String, u32>,
    skipped: BTreeMap<String, u32>,
    findings: Vec<DocDriftFinding>,
    total_findings: u32,
    unreadable_targets: u32,
}

impl<'a> Resolver<'a> {
    fn check(&mut self, kind: &str) {
        *self.checked.entry(kind.to_string()).or_insert(0) += 1;
    }

    fn skip(&mut self, reason: &str) {
        *self.skipped.entry(reason.to_string()).or_insert(0) += 1;
    }

    fn find(
        &mut self,
        doc: &str,
        line: u32,
        kind: DriftKind,
        reference: &str,
        detail: String,
        current_locations: Vec<String>,
    ) {
        self.total_findings = self.total_findings.saturating_add(1);
        if self.findings.len() < MAX_FINDINGS {
            self.findings.push(DocDriftFinding {
                doc: doc.to_string(),
                line,
                kind,
                reference: reference.to_string(),
                detail,
                current_locations,
            });
        }
    }

    fn state(&self, rel: &str) -> PathState {
        if rel.is_empty() {
            return PathState::Dir;
        }
        match std::fs::metadata(self.root.join(rel)) {
            Ok(meta) if meta.is_dir() => PathState::Dir,
            Ok(_) => PathState::File,
            Err(_) => PathState::Missing,
        }
    }

    /// Line count and heading slugs of `rel`. `None` when the file cannot be
    /// read (too large, not text, refused): a check that did not run.
    fn target_facts(&mut self, rel: &str) -> anyhow::Result<Option<std::rc::Rc<TargetFacts>>> {
        if let Some(cached) = self.facts.get(rel) {
            return Ok(cached.clone());
        }
        self.cancel.check()?;
        let facts = devmap_extract::safe_fs::read_repo_source(
            Some(self.root),
            rel,
            devmap_extract::MAX_SOURCE_BYTES,
        )
        .ok()
        .map(|text| {
            std::rc::Rc::new(TargetFacts {
                lines: text.lines().count(),
                slugs: is_markdown_path(rel).then(|| heading_slugs(&text)),
            })
        });
        if self.facts.len() < TARGET_CACHE_CAP {
            self.facts.insert(rel.to_string(), facts.clone());
        }
        Ok(facts)
    }

    /// Check a line suffix or `#L` anchor against `rel`'s length.
    fn check_lines(
        &mut self,
        doc: &str,
        line: u32,
        reference: &str,
        rel: &str,
        wanted: (u32, u32),
    ) -> anyhow::Result<()> {
        if devmap_extract::secret::is_secret_path(rel) {
            self.skip("secret_path");
            return Ok(());
        }
        let Some(facts) = self.target_facts(rel)? else {
            self.unreadable_targets += 1;
            self.skip("unreadable_target");
            return Ok(());
        };
        self.check("line");
        if usize::try_from(wanted.1).unwrap_or(usize::MAX) > facts.lines {
            self.find(
                doc,
                line,
                DriftKind::LineOutOfRange,
                reference,
                format!(
                    "{rel} has {} lines; the reference reaches line {}",
                    facts.lines, wanted.1
                ),
                Vec::new(),
            );
        }
        Ok(())
    }

    fn is_code_file(&self, rel: &str) -> bool {
        self.languages
            .get(rel)
            .is_some_and(|language| !NON_CODE_LANGUAGES.contains(language))
    }

    /// Stored symbols named `name` whose qualifier agrees with `qualifier`.
    fn matches(&self, qualifier: &[String], name: &str) -> Vec<&'a StoredSymbol> {
        let Some(candidates) = self.by_name.get(name) else {
            return Vec::new();
        };
        candidates
            .iter()
            .filter(|symbol| qualifier_matches(symbol, qualifier))
            .copied()
            .collect()
    }

    fn locations(symbols: &[&StoredSymbol]) -> Vec<String> {
        let paths: BTreeSet<&str> = symbols.iter().map(|s| s.path.as_str()).collect();
        paths
            .into_iter()
            .take(MAX_LOCATIONS)
            .map(str::to_string)
            .collect()
    }

    fn check_symbol(
        &mut self,
        doc: &str,
        line: u32,
        qualifier: &[String],
        name: &str,
        written: &str,
    ) {
        self.check("symbol");
        if !self.matches(qualifier, name).is_empty() {
            return;
        }
        let elsewhere: Vec<&StoredSymbol> = self
            .by_name
            .get(name)
            .map(|all| all.to_vec())
            .unwrap_or_default();
        if elsewhere.is_empty() {
            self.find(
                doc,
                line,
                DriftKind::MissingSymbol,
                written,
                format!("no symbol named {name} is defined in the index"),
                Vec::new(),
            );
        } else {
            self.find(
                doc,
                line,
                DriftKind::MissingSymbol,
                written,
                format!(
                    "{name} is defined, but not under the qualifier {}",
                    qualifier.join("::")
                ),
                Self::locations(&elsewhere),
            );
        }
    }

    fn check_link(
        &mut self,
        doc: &str,
        line: u32,
        target: &str,
        symbols: &[String],
    ) -> anyhow::Result<()> {
        let decoded = percent_decode(target);
        let (path_part, anchor) = match decoded.split_once('#') {
            Some((path, anchor)) => (path.to_string(), Some(anchor.to_string())),
            None => (decoded.clone(), None),
        };
        let path_part = path_part
            .split_once('?')
            .map_or(path_part.as_str(), |(path, _)| path)
            .to_string();
        if path_part.is_empty() {
            self.skip("same_document_anchor");
            return Ok(());
        }
        let Some(rel) = resolve_relative(doc, &path_part) else {
            self.skip("outside_repo");
            return Ok(());
        };
        self.check("link");
        match self.state(&rel) {
            PathState::Missing => {
                self.find(
                    doc,
                    line,
                    DriftKind::MissingFile,
                    target,
                    format!("{rel} does not exist"),
                    Vec::new(),
                );
                return Ok(());
            }
            PathState::Dir => {
                if anchor.is_some() {
                    self.skip("anchor_on_directory");
                }
                self.standalone_symbols(doc, line, symbols);
                return Ok(());
            }
            PathState::File => {}
        }
        if let Some(anchor) = anchor.as_deref() {
            self.check_anchor(doc, line, target, &rel, anchor)?;
        }
        if symbols.is_empty() {
            return Ok(());
        }
        if self.is_code_file(&rel) {
            for written in symbols {
                let (qualifier, name) = split_written(written);
                self.check("symbol");
                let all = self.by_name.get(name.as_str()).cloned().unwrap_or_default();
                let here: Vec<&StoredSymbol> = all
                    .iter()
                    .filter(|symbol| symbol.path == rel && qualifier_matches(symbol, &qualifier))
                    .copied()
                    .collect();
                if !here.is_empty() {
                    continue;
                }
                let elsewhere: Vec<&StoredSymbol> = all
                    .iter()
                    .filter(|symbol| symbol.path != rel)
                    .copied()
                    .collect();
                if elsewhere.is_empty() {
                    self.find(
                        doc,
                        line,
                        DriftKind::MissingSymbol,
                        written,
                        format!("{name} is not defined in {rel} or anywhere else in the index"),
                        Vec::new(),
                    );
                } else {
                    self.find(
                        doc,
                        line,
                        DriftKind::SymbolMoved,
                        written,
                        format!("{name} is no longer defined in {rel}"),
                        Self::locations(&elsewhere),
                    );
                }
            }
        } else {
            self.standalone_symbols(doc, line, symbols);
        }
        Ok(())
    }

    /// Link-text symbols whose target is not a code file stand on their own.
    fn standalone_symbols(&mut self, doc: &str, line: u32, symbols: &[String]) {
        for written in symbols {
            let (qualifier, name) = split_written(written);
            self.check_symbol(doc, line, &qualifier, &name, written);
        }
    }

    fn check_anchor(
        &mut self,
        doc: &str,
        line: u32,
        reference: &str,
        rel: &str,
        anchor: &str,
    ) -> anyhow::Result<()> {
        if devmap_extract::secret::is_secret_path(rel) {
            self.skip("secret_path");
            return Ok(());
        }
        let wanted = parse_line_anchor(anchor);
        let markdown = is_markdown_path(rel);
        if wanted.is_none() && !markdown {
            self.skip("non_markdown_anchor");
            return Ok(());
        }
        let Some(facts) = self.target_facts(rel)? else {
            self.unreadable_targets += 1;
            self.skip("unreadable_target");
            return Ok(());
        };
        let slug = percent_decode(anchor).to_lowercase();
        let heading_exists = facts
            .slugs
            .as_ref()
            .is_some_and(|slugs| slugs.contains(&slug));
        if heading_exists {
            self.check("anchor");
            return Ok(());
        }
        match wanted {
            Some((_, last)) => {
                self.check("line");
                if usize::try_from(last).unwrap_or(usize::MAX) > facts.lines {
                    self.find(
                        doc,
                        line,
                        DriftKind::LineOutOfRange,
                        reference,
                        format!(
                            "{rel} has {} lines; the anchor reaches line {last}",
                            facts.lines
                        ),
                        Vec::new(),
                    );
                }
            }
            None => {
                self.check("anchor");
                self.find(
                    doc,
                    line,
                    DriftKind::MissingAnchor,
                    reference,
                    format!("{rel} has no heading with the anchor #{anchor}"),
                    Vec::new(),
                );
            }
        }
        Ok(())
    }

    fn check_path_span(
        &mut self,
        doc: &str,
        line: u32,
        path: &str,
        lines: Option<(u32, u32)>,
        has_known_extension: bool,
    ) -> anyhow::Result<()> {
        let from_root = normalise_from_root(path);
        let from_doc = resolve_relative(doc, path);
        if from_root.is_none() && from_doc.is_none() {
            self.skip("outside_repo");
            return Ok(());
        }
        let mut candidates: Vec<String> = Vec::new();
        for candidate in [from_root, from_doc].into_iter().flatten() {
            if !candidates.contains(&candidate) {
                candidates.push(candidate);
            }
        }
        let written = match lines {
            Some((first, last)) if first == last => format!("{path}:{first}"),
            Some((first, last)) => format!("{path}:{first}-{last}"),
            None => path.to_string(),
        };
        let existing = candidates
            .iter()
            .find(|candidate| !matches!(self.state(candidate), PathState::Missing))
            .cloned();
        let Some(existing) = existing else {
            // `db/query.rs` for a file the index knows by a longer path.
            if candidates.iter().any(|c| self.suffixes.contains(c)) {
                self.skip("partial_path");
                return Ok(());
            }
            if !has_known_extension {
                self.skip("not_a_path");
                return Ok(());
            }
            self.check("path");
            self.find(
                doc,
                line,
                DriftKind::MissingFile,
                &written,
                format!("{} does not exist", candidates[0]),
                Vec::new(),
            );
            return Ok(());
        };
        self.check("path");
        if let Some(range) = lines {
            if matches!(self.state(&existing), PathState::File) {
                self.check_lines(doc, line, &written, &existing, range)?;
            } else {
                self.skip("line_on_directory");
            }
        }
        Ok(())
    }
}

/// `L10` or `L10-L20` as a line range.
fn parse_line_anchor(anchor: &str) -> Option<(u32, u32)> {
    let rest = anchor.strip_prefix('L')?;
    let (first, last) = match rest.split_once('-') {
        Some((a, b)) => (a, b.strip_prefix('L').unwrap_or(b)),
        None => (rest, rest),
    };
    let first: u32 = first.parse().ok()?;
    let last: u32 = last.parse().ok()?;
    (first.max(last) > 0).then_some((first.min(last), first.max(last)))
}

/// `Store::latest_file` as `(["Store"], "latest_file")`.
fn split_written(written: &str) -> (Vec<String>, String) {
    let body = written.strip_suffix("()").unwrap_or(written);
    let mut segments: Vec<String> = body
        .split("::")
        .flat_map(|part| part.split('.'))
        .map(str::to_string)
        .collect();
    let name = segments.pop().unwrap_or_default();
    (segments, name)
}

fn normalise_segment(segment: &str) -> String {
    segment.replace('-', "_").to_lowercase()
}

/// The module doc's matching rule.
fn qualifier_matches(symbol: &StoredSymbol, qualifier: &[String]) -> bool {
    if qualifier.is_empty() {
        return true;
    }
    let tail = symbol
        .qualified_name
        .strip_prefix(&symbol.path)
        .and_then(|rest| rest.strip_prefix("::"))
        .unwrap_or(&symbol.qualified_name);
    let owners: Vec<&str> = tail.split("::").flat_map(|part| part.split('.')).collect();
    let owners = &owners[..owners.len().saturating_sub(1)];
    if owners.len() >= qualifier.len()
        && owners[owners.len() - qualifier.len()..]
            .iter()
            .zip(qualifier)
            .all(|(have, want)| have == want)
    {
        return true;
    }
    let Some(last) = qualifier.last() else {
        return true;
    };
    let wanted = normalise_segment(last);
    let mut parts: Vec<&str> = symbol.path.split('/').collect();
    if let Some(file) = parts.pop() {
        let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
        if normalise_segment(stem) == wanted {
            return true;
        }
    }
    parts.iter().any(|dir| normalise_segment(dir) == wanted)
}

fn path_suffixes(paths: impl Iterator<Item = impl AsRef<str>>) -> HashSet<String> {
    let mut suffixes = HashSet::new();
    for path in paths {
        let path = path.as_ref();
        let mut at = 0;
        while let Some(offset) = path[at..].find('/') {
            at += offset + 1;
            suffixes.insert(path[at..].to_string());
        }
    }
    suffixes
}

fn push_reason(reasons: &mut Vec<String>, reason: String) {
    reasons.push(reason);
}

/// Scan the Markdown documents of the latest generation and report the
/// references that no longer hold. See the module docs for the rules.
pub fn doc_drift_report(
    store: &Store,
    cancel: &Cancel,
    paths: &[String],
    exclude: &[String],
    token_budget: u32,
) -> anyhow::Result<DocDriftReport> {
    let include = clean_prefixes(paths, "path")?;
    let exclude = clean_prefixes(exclude, "exclude")?;
    let unavailable = |reason: &str| DocDriftReport {
        available: false,
        docs_scanned: 0,
        docs_total: 0,
        references: 0,
        checked: BTreeMap::new(),
        skipped: BTreeMap::new(),
        findings: Vec::new(),
        shown: 0,
        hidden: 0,
        total: 0,
        truncated: false,
        tokens_used: 0,
        walk_incomplete: Some(reason.to_string()),
    };
    let Some((page, files)) = store.all_symbols_page_with_files()? else {
        return Ok(unavailable("no persisted generation is available"));
    };
    let root = match page.repo_root.as_deref() {
        Some(root) => std::path::PathBuf::from(root),
        None => std::env::current_dir()?,
    };
    let root = match root.canonicalize() {
        Ok(root) => root,
        Err(error) => {
            return Ok(unavailable(&format!(
                "the repository root {} cannot be read: {error}",
                root.display()
            )))
        }
    };

    let mut reasons: Vec<String> = Vec::new();
    let mut docs: Vec<&str> = Vec::new();
    for (path, language) in &files {
        if language != "markdown" {
            continue;
        }
        if !include.is_empty() && !include.iter().any(|p| under_prefix(path, p)) {
            continue;
        }
        if exclude.iter().any(|p| under_prefix(path, p)) {
            continue;
        }
        docs.push(path);
    }
    let unmatched: Vec<&String> = include
        .iter()
        .filter(|p| !files.iter().any(|(path, _)| under_prefix(path, p)))
        .collect();
    if !unmatched.is_empty() {
        push_reason(
            &mut reasons,
            format!(
                "{} requested path(s) match no indexed file and were not scanned: {}",
                unmatched.len(),
                unmatched
                    .iter()
                    .take(5)
                    .map(|p| p.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        );
    }
    let docs_total = u32::try_from(docs.len()).unwrap_or(u32::MAX);
    if docs.len() > MAX_DOCS {
        push_reason(
            &mut reasons,
            format!(
                "document cap of {MAX_DOCS} cut the scan; {} documents match and {MAX_DOCS} were scanned",
                docs.len()
            ),
        );
        docs.truncate(MAX_DOCS);
    }

    let mut by_name: HashMap<&str, Vec<&StoredSymbol>> = HashMap::new();
    let language_of: HashMap<&str, &str> = files
        .iter()
        .map(|(path, language)| (path.as_str(), language.as_str()))
        .collect();
    for symbol in &page.rows {
        // A document defines nothing a document can drift from.
        if language_of.get(symbol.path.as_str()) == Some(&"markdown") {
            continue;
        }
        by_name
            .entry(symbol.name.as_str())
            .or_default()
            .push(symbol);
    }
    let mut resolver = Resolver {
        root: &root,
        cancel,
        by_name,
        languages: language_of,
        suffixes: path_suffixes(files.iter().map(|(path, _)| path.as_str())),
        facts: HashMap::new(),
        checked: ["link", "path", "line", "anchor", "symbol"]
            .into_iter()
            .map(|kind| (kind.to_string(), 0))
            .collect(),
        skipped: BTreeMap::new(),
        findings: Vec::new(),
        total_findings: 0,
        unreadable_targets: 0,
    };

    let mut docs_scanned = 0u32;
    let mut references = 0u32;
    let mut unread: Vec<String> = Vec::new();
    let mut withheld = 0u32;
    let mut capped_docs = 0u32;
    for doc in docs {
        cancel.check()?;
        if devmap_extract::secret::is_secret_path(doc) {
            withheld += 1;
            continue;
        }
        let text = match devmap_extract::safe_fs::read_repo_source(
            Some(&root),
            doc,
            devmap_extract::MAX_SOURCE_BYTES,
        ) {
            Ok(text) => text,
            Err(error) => {
                unread.push(format!("{doc}: {error}"));
                continue;
            }
        };
        docs_scanned += 1;
        let extraction = extract_references(&text);
        references = references.saturating_add(extraction.candidates());
        if extraction.truncated {
            capped_docs += 1;
        }
        for (reason, count) in &extraction.skipped {
            *resolver.skipped.entry((*reason).to_string()).or_insert(0) += count;
        }
        for reference in &extraction.references {
            match &reference.spec {
                RefSpec::Link { target, symbols } => {
                    resolver.check_link(doc, reference.line, target, symbols)?;
                }
                RefSpec::PathSpan {
                    path,
                    lines,
                    has_known_extension,
                } => resolver.check_path_span(
                    doc,
                    reference.line,
                    path,
                    *lines,
                    *has_known_extension,
                )?,
                RefSpec::Symbol {
                    qualifier,
                    name,
                    written,
                } => resolver.check_symbol(doc, reference.line, qualifier, name, written),
            }
        }
    }
    if !unread.is_empty() {
        push_reason(
            &mut reasons,
            format!(
                "{} document(s) could not be read and were not scanned (first: {})",
                unread.len(),
                unread[0]
            ),
        );
    }
    if withheld > 0 {
        push_reason(
            &mut reasons,
            format!("{withheld} document(s) at secret paths were withheld, not scanned"),
        );
    }
    if capped_docs > 0 {
        push_reason(
            &mut reasons,
            format!(
                "{capped_docs} document(s) hit the cap of {MAX_REFERENCES_PER_DOC} references and were scanned only that far"
            ),
        );
    }
    if let Some(long) = resolver.skipped.get("line_too_long") {
        push_reason(
            &mut reasons,
            format!("{long} line(s) over {MAX_LINE_BYTES} bytes were not scanned"),
        );
    }
    if resolver.unreadable_targets > 0 {
        push_reason(
            &mut reasons,
            format!(
                "{} reference(s) could not be fully checked because the target could not be read",
                resolver.unreadable_targets
            ),
        );
    }
    let total = resolver.total_findings;
    if usize::try_from(total).unwrap_or(usize::MAX) > resolver.findings.len() {
        push_reason(
            &mut reasons,
            format!(
                "finding cap of {MAX_FINDINGS} cut the list; {total} references drifted and {} were kept",
                resolver.findings.len()
            ),
        );
    }

    let mut findings = std::mem::take(&mut resolver.findings);
    findings.sort_by(|a, b| (&a.doc, a.line).cmp(&(&b.doc, b.line)));
    let mut shown_findings = Vec::new();
    let mut tokens_used = 0u32;
    for finding in findings {
        let cost = finding_tokens(&finding);
        if cost > token_budget.saturating_sub(tokens_used) {
            break;
        }
        tokens_used += cost;
        shown_findings.push(finding);
    }
    let shown = u32::try_from(shown_findings.len()).unwrap_or(u32::MAX);
    let hidden = total.saturating_sub(shown);
    Ok(DocDriftReport {
        available: true,
        docs_scanned,
        docs_total,
        references,
        checked: resolver.checked,
        skipped: resolver.skipped,
        findings: shown_findings,
        shown,
        hidden,
        total,
        truncated: hidden > 0,
        tokens_used,
        walk_incomplete: (!reasons.is_empty()).then(|| reasons.join("; ")),
    })
}
