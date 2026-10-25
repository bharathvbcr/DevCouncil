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
//! # What the report cannot see, and says so
//!
//! Each of these is a counted skip, never a finding:
//!
//! * **A name that survives in a comment or a string suppresses
//!   `missing_symbol`.** The index models functions, types and methods, not
//!   enum variants, fields, config keys or other people's libraries, so a name
//!   it does not define is reported only when it also appears as a word in no
//!   indexed non-Markdown file. A deleted function still named in a comment
//!   therefore goes unreported. Counted as `defined_outside_symbol_index`.
//! * **`qualifier_unmatched` hides references to a renamed owner.**
//!   `OldType::method`, where `method` still exists under another type, is not
//!   reported: a stale qualifier cannot be told from a spelling the extractor
//!   does not share. For a link, a symbol defined in the target file under
//!   another owner is the same skip.
//! * **`external_qualifier`**: a qualified name whose first segment is no
//!   symbol, directory or file stem of this repository (`serde_json::Value`,
//!   `tokio::spawn`, `React.Component`) names somebody else's code. Bare
//!   well-known external names cannot be told apart and stay checked.
//! * **Spans and links differ on a missing top-level directory.** A link is
//!   resolved against its document, so a missing target is certainly broken:
//!   `[main](oldpkg/main.go)` is `missing_file`. A span's root is ambiguous, so
//!   `` `oldpkg/main.go` `` whose first directory exists nowhere in the tree
//!   may belong to another repository and is counted as `unrooted_path`. A
//!   deleted top-level directory is therefore reported through its links and
//!   missed through its spans. Likewise `generated_path` (`tmp/`, `target/`,
//!   tool dot-directories, `*.log`), `partial_path`, `not_a_path`,
//!   `outside_repo`.
//! * **Links through a symlink that leaves the repository** are `outside_repo`
//!   and never stat past the link, so the report is no existence oracle for the
//!   host. Secret paths are judged after links are resolved: an alias of
//!   `.env` is `.env`.
//! * **Opt-outs**: a whole-line comment outside a fence,
//!   `<!-- devmap:doc-drift ignore-next-line -->` or
//!   `<!-- devmap:doc-drift ignore-file -->`, drops the references on the next
//!   line or in the file. What they hid is `skipped.ignored_by_comment`.
//! * **`docs/archive/` is excluded by default** (`default_excludes`,
//!   `docs_excluded_by_default` say so), lifted by naming a path inside it or
//!   by `--no-default-excludes` / `default_excludes: false`.
//!
//! # Anchors
//!
//! Slugs follow github-slugger: the rendered heading text (inline code, link
//! text, image alt, emphasis markers dropped, entities decoded), lowercased,
//! everything but letters, marks, numbers, `-` and `_` removed, each space a
//! `-`. A repeated slug takes the first free `-N`, skipping suffixes already
//! taken. `id` and `name` attributes of HTML tags are anchors; headings inside
//! blockquotes and list items count; `file.md#` and `#top` are the top of the
//! page. A heading line too long to scan makes the target's anchors incomplete:
//! a missing anchor there is `anchor_target_unscanned`, not a finding.
//!
//! # Bounds
//!
//! Extraction is linear in the document (comment stripping is one pass, bracket
//! lookahead is budgeted per line) and checks for cancellation every few
//! hundred lines. The unindexed-name word search reads at most 64 MiB of
//! source, honours cancellation per file, and says when it was cut. The
//! finding cap keeps the first 10 000 findings by (document, line), whichever
//! pass found them, and reports both numbers.
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

use crate::cancel::{Cancel, QueryCancelled};

#[cfg(test)]
#[path = "doc_drift_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "doc_drift_hardening_tests.rs"]
mod hardening_tests;

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
/// Source bytes the word search for unindexed names reads at most.
const MAX_WORD_SEARCH_BYTES: u64 = 64 * 1024 * 1024;
/// Directory prefixes left out unless asked for by name: archived documents
/// are historical records of what was true then.
pub const DEFAULT_EXCLUDES: [&str; 1] = ["docs/archive"];
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
const EXTERNAL_QUALIFIERS: [&str; 41] = [
    "std", "core", "alloc", "fmt", "os", "io", "fs", "sys", "strings", "strconv", "errors", "time",
    "context", "http", "json", "sync", "sort", "bytes", "regexp", "filepath", "log", "flag",
    "self", "Self", "this", "crate", "super", "console", "process", "window", "document", "Math",
    "JSON", "Object", "Array", "Vec", "Option", "Result", "clippy", "rustc", "rustfmt",
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
    /// Named by an `ignore-next-line` comment.
    ignored: bool,
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
///
/// One pass: the backtick parity is carried forward from where the last
/// marker was looked at, so a line of `<!--` after a stray backtick costs one
/// visit per byte, not one recount of the line per marker.
fn strip_comments(text: &str, in_comment: &mut bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pos = 0;
    let mut counted_to = 0;
    let mut odd = false;
    loop {
        if *in_comment {
            match text[pos..].find("-->") {
                Some(end) => {
                    *in_comment = false;
                    pos += end + 3;
                }
                None => return out,
            }
            continue;
        }
        let mut from = pos;
        let start = loop {
            match text[from..].find("<!--") {
                None => break None,
                Some(offset) => {
                    let at = from + offset;
                    if counted_to < at {
                        let ticks = text.as_bytes()[counted_to..at]
                            .iter()
                            .filter(|b| **b == b'`')
                            .count();
                        odd ^= ticks % 2 == 1;
                        counted_to = at;
                    }
                    if odd {
                        from = at + 4;
                    } else {
                        break Some(at);
                    }
                }
            }
        };
        match start {
            None => {
                out.push_str(&text[pos..]);
                return out;
            }
            Some(at) => {
                out.push_str(&text[pos..at]);
                *in_comment = true;
                pos = at + 4;
            }
        }
    }
}

/// Every line of `text` that is prose: outside fences, indented code and HTML
/// comments.
fn prose_lines(text: &str, cancel: Option<&Cancel>) -> Result<(Vec<Line>, bool), QueryCancelled> {
    let mut lines = Vec::new();
    let mut fence: Option<(u8, usize)> = None;
    let mut in_comment = false;
    let mut previous_blank = true;
    let mut previous_indented_code = false;
    let mut ignore_line: Option<u32> = None;
    let mut ignore_file = false;
    for (index, raw) in text.lines().enumerate() {
        if let Some(cancel) = cancel {
            cancel.check_every(index)?;
        }
        let no = u32::try_from(index + 1).unwrap_or(u32::MAX);
        if fence.is_none() {
            match suppression(raw) {
                Some(Suppression::NextLine) => ignore_line = Some(no.saturating_add(1)),
                Some(Suppression::File) => ignore_file = true,
                None => {}
            }
        }
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
        lines.push(Line {
            no,
            text: visible,
            ignored: ignore_line == Some(no),
        });
    }
    Ok((lines, ignore_file))
}

/// The two deliberate opt-outs, each a whole-line HTML comment:
/// `<!-- devmap:doc-drift ignore-next-line -->` and
/// `<!-- devmap:doc-drift ignore-file -->`.
enum Suppression {
    NextLine,
    File,
}

fn suppression(raw: &str) -> Option<Suppression> {
    let trimmed = raw.trim();
    let inner = trimmed.strip_prefix("<!--")?.strip_suffix("-->")?.trim();
    match inner {
        "devmap:doc-drift ignore-next-line" => Some(Suppression::NextLine),
        "devmap:doc-drift ignore-file" => Some(Suppression::File),
        _ => None,
    }
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

/// A character entity (`&amp;`, `&#38;`, `&#x26;`) at the start of `rest`, as
/// the character and the bytes it took. Only the handful a heading uses.
fn entity(rest: &[char]) -> Option<(char, usize)> {
    if rest.first() != Some(&'&') {
        return None;
    }
    let end = rest.iter().take(12).position(|c| *c == ';')?;
    let body: String = rest[1..end].iter().collect();
    let decoded = match body.as_str() {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        _ => {
            let digits = body.strip_prefix('#')?;
            let code = match digits.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => digits.parse().ok()?,
            };
            char::from_u32(code)?
        }
    };
    Some((decoded, end + 1))
}

/// The text a heading renders, which is what GitHub slugs: inline code keeps
/// its content, a link or image keeps its text, emphasis markers and HTML tags
/// go, entities decode, a backslash escape is its character. `_` is an
/// emphasis delimiter only when a flanking pair matches (`_Emphasis_`, the
/// `__` of `__init__`); inside a word (`snake_case`) it stays.
fn heading_text(heading: &str) -> String {
    let src: Vec<char> = heading.trim().chars().collect();
    let mut out: Vec<char> = Vec::with_capacity(src.len());
    // (index into `out`, can open, can close)
    let mut underscores: Vec<(usize, bool, bool)> = Vec::new();
    let mut unclosed: u64 = 0;
    let mut i = 0;
    while i < src.len() {
        let ch = src[i];
        match ch {
            '\\' if src.get(i + 1).is_some_and(char::is_ascii_punctuation) => {
                out.push(src[i + 1]);
                i += 2;
            }
            '`' => {
                let run = src[i..].iter().take_while(|c| **c == '`').count();
                let close = if run < 64 && unclosed & (1 << run) != 0 {
                    None
                } else {
                    let mut j = i + run;
                    let mut found = None;
                    while j < src.len() {
                        if src[j] == '`' {
                            let n = src[j..].iter().take_while(|c| **c == '`').count();
                            if n == run {
                                found = Some(j);
                                break;
                            }
                            j += n;
                        } else {
                            j += 1;
                        }
                    }
                    found
                };
                match close {
                    Some(j) => {
                        out.extend(&src[i + run..j]);
                        i = j + run;
                    }
                    None => {
                        if run < 64 {
                            unclosed |= 1 << run;
                        }
                        i += run;
                    }
                }
            }
            ']' if matches!(src.get(i + 1), Some('(' | '[')) => {
                // `](destination)` and `][label]` are not text.
                let (open, shut) = if src[i + 1] == '(' {
                    ('(', ')')
                } else {
                    ('[', ']')
                };
                let mut depth = 0u32;
                let mut j = i + 1;
                let mut end = None;
                while j < src.len() {
                    if src[j] == '\\' {
                        j += 2;
                        continue;
                    }
                    if src[j] == open {
                        depth += 1;
                    } else if src[j] == shut {
                        depth -= 1;
                        if depth == 0 {
                            end = Some(j);
                            break;
                        }
                    }
                    j += 1;
                }
                match end {
                    Some(j) => i = j + 1,
                    None => {
                        out.push(ch);
                        i += 1;
                    }
                }
            }
            '<' => {
                // An inline HTML tag contributes nothing.
                match src[i..].iter().take(256).position(|c| *c == '>') {
                    Some(offset)
                        if src
                            .get(i + 1)
                            .is_some_and(|c| c.is_alphabetic() || *c == '/') =>
                    {
                        i += offset + 1;
                    }
                    _ => {
                        out.push(ch);
                        i += 1;
                    }
                }
            }
            '&' => match entity(&src[i..]) {
                Some((decoded, used)) => {
                    out.push(decoded);
                    i += used;
                }
                None => {
                    out.push(ch);
                    i += 1;
                }
            },
            '_' => {
                let before = i.checked_sub(1).map(|j| src[j]);
                let after = src.get(i + 1).copied();
                let before_space = before.is_none_or(char::is_whitespace);
                let after_space = after.is_none_or(char::is_whitespace);
                let before_word = before.is_some_and(char::is_alphanumeric);
                let after_word = after.is_some_and(char::is_alphanumeric);
                underscores.push((
                    out.len(),
                    !after_space && !before_word,
                    !before_space && !after_word,
                ));
                out.push('_');
                i += 1;
            }
            _ => {
                out.push(ch);
                i += 1;
            }
        }
    }
    let mut dropped: HashSet<usize> = HashSet::new();
    let mut open: Vec<usize> = Vec::new();
    for (at, can_open, can_close) in underscores {
        if can_close && !open.is_empty() {
            if let Some(opener) = open.pop() {
                dropped.insert(opener);
                dropped.insert(at);
            }
        } else if can_open {
            open.push(at);
        }
    }
    out.iter()
        .enumerate()
        .filter(|(at, _)| !dropped.contains(at))
        .map(|(_, c)| *c)
        .collect()
}

/// Combining marks, which github-slugger keeps (`\p{M}`).
fn is_mark(ch: char) -> bool {
    matches!(
        ch as u32,
        0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F
    )
}

/// The GitHub slug of one heading's source text (github-slugger): the
/// rendered text lowercased, everything but letters, marks, numbers, `-` and
/// `_` removed, and each ASCII space a `-`. Nothing is collapsed or trimmed:
/// `Tom & Jerry` is `tom--jerry`.
fn slugify(heading: &str) -> String {
    let mut slug = String::new();
    for ch in heading_text(heading).chars() {
        if ch == ' ' {
            slug.push('-');
        } else if ch == '-' || ch == '_' || ch.is_alphanumeric() || is_mark(ch) {
            slug.extend(ch.to_lowercase());
        }
    }
    slug
}

/// The anchors named by `id="…"` and `name="…"` attributes of the HTML tags on
/// `line` (`<a name="top"></a>`), lowercased.
fn tag_anchors(line: &str, into: &mut BTreeSet<String>) {
    if !line.contains('<') {
        return;
    }
    let bytes = line.as_bytes();
    let mut at = 0;
    let mut failures = 0;
    while let Some(offset) = line[at..].find('<') {
        let start = at + offset;
        let window = &bytes[start..bytes.len().min(start + 1024)];
        let Some(len) = window.iter().position(|b| *b == b'>') else {
            failures += 1;
            if failures > 64 {
                return;
            }
            at = start + 1;
            continue;
        };
        let tag = &line[start..start + len];
        let lower = tag.to_ascii_lowercase();
        for attribute in [" id=", " name="] {
            let Some(found) = lower.find(attribute) else {
                continue;
            };
            let value = &tag[found + attribute.len()..];
            let value = match value.chars().next() {
                Some(quote @ ('"' | '\'')) => {
                    let inner = &value[1..];
                    inner.find(quote).map(|end| &inner[..end])
                }
                Some(_) => Some(value.split_whitespace().next().unwrap_or("")),
                None => None,
            };
            if let Some(value) = value.filter(|v| !v.is_empty()) {
                into.insert(value.to_lowercase());
            }
        }
        at = start + len + 1;
    }
}

/// `text` with block prefixes (blockquote `>`, list markers) peeled, so a
/// heading inside either is seen as the heading it is.
fn peel_block_prefixes(text: &str) -> &str {
    let mut rest = text;
    for _ in 0..8 {
        let trimmed = rest.trim_start();
        if let Some(after) = trimmed.strip_prefix('>') {
            rest = after;
        } else if starts_list_item(trimmed) {
            let skip = trimmed
                .find([' ', '\t'])
                .map(|at| at + 1)
                .unwrap_or(trimmed.len());
            rest = &trimmed[skip..];
        } else {
            return rest;
        }
    }
    rest
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

/// The anchors of one document.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Anchors {
    pub slugs: BTreeSet<String>,
    /// A line too long to scan could have been a heading: the set may be
    /// missing an anchor, so an absent one is not proof.
    pub incomplete: bool,
}

/// Every anchor the headings of `text` produce, as github-slugger does, plus
/// the `id` and `name` of HTML tags. Headings count inside blockquotes and list
/// items, ATX and setext, with inline code, links and emphasis reduced to
/// their text. A repeated slug takes the first free `-N`: `# Foo`, `# Foo-1`,
/// `# Foo`, `# Foo` give `foo`, `foo-1`, `foo-2`, `foo-3`, because a suffix
/// already taken is skipped, exactly as the slugger's occurrence table does.
pub fn heading_anchors(text: &str, cancel: Option<&Cancel>) -> Result<Anchors, QueryCancelled> {
    let mut anchors = Anchors::default();
    let mut occurrences: HashMap<String, u32> = HashMap::new();
    let mut add = |heading: &str, slugs: &mut BTreeSet<String>| {
        let original = slugify(heading);
        let mut slug = original.clone();
        while occurrences.contains_key(&slug) {
            let count = occurrences.entry(original.clone()).or_insert(0);
            *count += 1;
            slug = format!("{original}-{count}");
        }
        occurrences.insert(slug.clone(), 0);
        slugs.insert(slug);
    };
    let (lines, _) = prose_lines(text, cancel)?;
    let mut paragraph: Option<String> = None;
    let mut previous: Option<(u32, bool)> = None;
    for line in &lines {
        let long = line.text.len() > MAX_LINE_BYTES;
        let peeled = peel_block_prefixes(&line.text);
        if long {
            // A long line is only a problem if it is, or may become, a heading.
            if peeled.trim_start().starts_with('#') {
                anchors.incomplete = true;
            }
            paragraph = None;
            previous = Some((line.no, true));
            continue;
        }
        tag_anchors(&line.text, &mut anchors.slugs);
        if let Some(heading) = atx_heading(peeled) {
            add(heading, &mut anchors.slugs);
            paragraph = None;
            previous = None;
            continue;
        }
        let consecutive = previous.filter(|(no, _)| no + 1 == line.no);
        if setext_underline(&line.text) {
            if let Some((_, true)) = consecutive {
                anchors.incomplete = true;
            }
            if consecutive.is_some() {
                if let Some(heading) = paragraph.take() {
                    add(&heading, &mut anchors.slugs);
                    previous = None;
                    continue;
                }
            }
        }
        let trimmed = line.text.trim();
        let plain_paragraph = !trimmed.is_empty()
            && leading_indent(&line.text) <= 3
            && !starts_list_item(trimmed)
            && !trimmed.starts_with(['>', '|']);
        paragraph = plain_paragraph.then(|| trimmed.to_string());
        previous = Some((line.no, false));
    }
    Ok(anchors)
}

/// The slugs of `text`'s anchors, with no cancellation. See [`heading_anchors`].
pub fn heading_slugs(text: &str) -> BTreeSet<String> {
    heading_anchors(text, None)
        .map(|anchors| anchors.slugs)
        .unwrap_or_default()
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
        // `a::b` is a path. `pkg.Func` is a package-qualified name; a dotted
        // chain that is deeper (`Microsoft.Extensions.Foo`) is a namespace,
        // and one ending lower-case (`receive.denyCurrentBranch`,
        // `Habit.statusLabel`) is a config key or a property.
        separators_colon
            || (qualifier.len() == 1 && name.chars().next().is_some_and(|c| c.is_ascii_uppercase()))
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
///
/// `budget` is the line's remaining scan allowance, charged a unit per byte
/// looked at: a line of unmatched `[` would otherwise cost the lookahead once
/// per bracket.
fn matching_bracket(bytes: &[u8], open: usize, budget: &mut usize) -> Option<usize> {
    let limit = bytes.len().min(open + LINK_LOOKAHEAD);
    let mut depth = 0usize;
    let mut i = open;
    let mut unclosed: u64 = 0;
    while i < limit {
        if *budget == 0 {
            return None;
        }
        *budget -= 1;
        match bytes[i] {
            b'\\' => i += 1,
            b'`' => {
                let run = bytes[i..].iter().take_while(|b| **b == b'`').count();
                let closing = if run < 64 && unclosed & (1 << run) != 0 {
                    None
                } else {
                    find_closing(&bytes[..limit], i + run, run)
                };
                if let Some(close) = closing {
                    *budget = budget.saturating_sub(close - i);
                    i = close + run - 1;
                } else {
                    if run < 64 {
                        unclosed |= 1 << run;
                    }
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
fn link_destination<'a>(
    line: &'a str,
    open: usize,
    budget: &mut usize,
) -> Option<(&'a str, usize)> {
    let bytes = line.as_bytes();
    let limit = bytes.len().min(open + LINK_LOOKAHEAD);
    // Charged up front for the whole window it may read.
    let cost = limit.saturating_sub(open);
    if *budget < cost {
        *budget = 0;
        return None;
    }
    *budget -= cost;
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
    /// Bytes of bracket and destination lookahead this line may still spend.
    budget: usize,
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
                    let budget = &mut self.budget;
                    let end = matching_bracket(bytes, i, budget).and_then(|close| {
                        (bytes.get(close + 1) == Some(&b'('))
                            .then(|| link_destination(line, close + 1, budget).map(|d| (close, d)))
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
    extract_references_cancellable(text, None).unwrap_or_default()
}

/// [`extract_references`], consulting `cancel` every few hundred lines so an
/// abandoned scan of a large document stops promptly. Everything in it is
/// linear in the document: comment stripping, fence tracking and the inline
/// scan, whose bracket lookahead is budgeted per line.
///
/// Two deliberate opt-outs, each a whole-line HTML comment outside a fence:
/// `<!-- devmap:doc-drift ignore-next-line -->` drops every reference on the
/// next line, `<!-- devmap:doc-drift ignore-file -->` every reference in the
/// document. What they hide is counted under `skipped.ignored_by_comment`, so
/// the opt-out stays visible.
pub fn extract_references_cancellable(
    text: &str,
    cancel: Option<&Cancel>,
) -> Result<Extraction, QueryCancelled> {
    let mut out = Extraction::default();
    let (lines, ignore_file) = prose_lines(text, cancel)?;
    let mut hidden = 0u32;
    for line in lines {
        if line.text.len() > MAX_LINE_BYTES {
            out.skip("line_too_long");
            continue;
        }
        // An ignored line is scanned into a scratch extraction so that what the
        // comment hid is counted rather than lost.
        let mut scratch = Extraction::default();
        let target = if line.ignored { &mut scratch } else { &mut out };
        let mut scan = Scan {
            out: target,
            line: line.no,
            unclosed: 0,
            budget: line.text.len() * 32 + 4096,
        };
        match reference_definition(&line.text) {
            Some(destination) => scan.link(destination, Vec::new()),
            None => scan.scan(&line.text, false),
        }
        let stop = scan.out.truncated && !line.ignored;
        if line.ignored {
            hidden += scratch.candidates();
        }
        if stop {
            break;
        }
    }
    if ignore_file {
        hidden += out.candidates();
        out = Extraction::default();
    }
    if hidden > 0 {
        *out.skipped.entry("ignored_by_comment").or_insert(0) += hidden;
    }
    Ok(out)
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
    /// The default exclusions that left documents out of this scan (empty when
    /// none did, or when the call lifted them). They are not in `docs_total`.
    #[serde(default)]
    pub default_excludes: Vec<String>,
    /// How many documents those exclusions left out.
    #[serde(default)]
    pub docs_excluded_by_default: u32,
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
        if trimmed.starts_with('/') {
            anyhow::bail!("{what} {trimmed:?} is an absolute path; give a repo-relative one");
        }
        if trimmed.contains('\\') {
            anyhow::bail!("{what} {trimmed:?} contains a backslash; separate directories with `/`");
        }
        if trimmed.split('/').any(|part| part == "..") {
            anyhow::bail!("{what} {trimmed:?} contains `..`, which could leave the repository");
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
    anchors: Option<Anchors>,
}

enum PathState {
    Missing,
    /// Some component is a link whose real location is outside the repository
    /// (or cannot be resolved). Never stat beyond it: whether a path past the
    /// link exists is a fact about the host, not about this repository.
    Outside,
    Dir,
    File,
}

impl PathState {
    fn exists(&self) -> bool {
        matches!(self, Self::Dir | Self::File)
    }
}

/// The bounds one call works inside. Tests lower them; callers get
/// [`Limits::default`].
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Findings kept; the first ones by (document, line).
    pub max_findings: usize,
    /// Source bytes the word search for unindexed names reads.
    pub max_word_search_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_findings: MAX_FINDINGS,
            max_word_search_bytes: MAX_WORD_SEARCH_BYTES,
        }
    }
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
    /// Kept findings, ordered by (document, line, emission): the cap drops the
    /// last ones in that order, whichever pass found them.
    findings: BTreeMap<(String, u32, u64), DocDriftFinding>,
    sequence: u64,
    total_findings: u32,
    unreadable_targets: u32,
    /// Targets whose heading scan skipped a line that may have been a heading.
    unscanned_anchor_targets: u32,
    limits: Limits,
    /// Names a document could qualify a symbol with: every symbol name and every
    /// directory and file stem, normalised. A qualifier outside it names
    /// somebody else's code.
    known_qualifiers: HashSet<String>,
    /// Symbols the index does not define, awaiting the word search.
    pending: Vec<PendingSymbol>,
}

struct PendingSymbol {
    doc: String,
    line: u32,
    written: String,
    name: String,
    detail: String,
}

/// The identifier-shaped words of `text`.
fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|word| !word.is_empty())
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
        self.sequence += 1;
        self.findings.insert(
            (doc.to_string(), line, self.sequence),
            DocDriftFinding {
                doc: doc.to_string(),
                line,
                kind,
                reference: reference.to_string(),
                detail,
                current_locations,
            },
        );
        while self.findings.len() > self.limits.max_findings {
            self.findings.pop_last();
        }
    }

    /// What `rel` is on disk, walking it one component at a time so a link
    /// that leaves the repository is refused before anything past it is looked
    /// at. `rel` is normalised and relative to the root.
    fn state(&self, rel: &str) -> PathState {
        let mut current = self.root.to_path_buf();
        let mut parts = rel.split('/').filter(|part| !part.is_empty()).peekable();
        while let Some(part) = parts.next() {
            current.push(part);
            let Ok(meta) = std::fs::symlink_metadata(&current) else {
                return PathState::Missing;
            };
            let meta = if meta.file_type().is_symlink() {
                match current.canonicalize() {
                    Ok(real) if real.starts_with(self.root) => match std::fs::metadata(&real) {
                        Ok(meta) => {
                            current = real;
                            meta
                        }
                        Err(_) => return PathState::Missing,
                    },
                    // Out of the tree, dangling, or a loop: not followed.
                    _ => return PathState::Outside,
                }
            } else {
                meta
            };
            if parts.peek().is_none() {
                return if meta.is_dir() {
                    PathState::Dir
                } else {
                    PathState::File
                };
            }
            if !meta.is_dir() {
                return PathState::Missing;
            }
        }
        PathState::Dir
    }

    /// Whether `rel` is a secret path as written or once its links are
    /// resolved: `docs/alias -> ../.env` is `.env`.
    fn is_secret(&self, rel: &str) -> bool {
        if devmap_extract::secret::is_secret_path(rel) {
            return true;
        }
        std::fs::canonicalize(self.root.join(rel))
            .ok()
            .and_then(|real| {
                real.strip_prefix(self.root)
                    .ok()
                    .map(|inside| inside.to_string_lossy().replace('\\', "/"))
            })
            .is_some_and(|real| devmap_extract::secret::is_secret_path(&real))
    }

    /// Line count and heading slugs of `rel`. `None` when the file cannot be
    /// read (too large, not text, refused): a check that did not run.
    fn target_facts(&mut self, rel: &str) -> anyhow::Result<Option<std::rc::Rc<TargetFacts>>> {
        if let Some(cached) = self.facts.get(rel) {
            return Ok(cached.clone());
        }
        self.cancel.check()?;
        let text = devmap_extract::safe_fs::read_repo_source(
            Some(self.root),
            rel,
            devmap_extract::MAX_SOURCE_BYTES,
        )
        .ok();
        let facts = match text {
            None => None,
            Some(text) => Some(std::rc::Rc::new(TargetFacts {
                lines: text.lines().count(),
                anchors: if is_markdown_path(rel) {
                    Some(heading_anchors(&text, Some(self.cancel))?)
                } else {
                    None
                },
            })),
        };
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
        if self.is_secret(rel) {
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
        // `serde_json::Value`, `tokio::spawn`: a qualifier that is no type,
        // module, crate, package or directory of this repository names
        // somebody else's code, whose absence from the index is no drift.
        if let Some(first) = qualifier.first() {
            if !self.known_qualifiers.contains(&normalise_segment(first)) {
                self.skip("external_qualifier");
                return;
            }
        }
        self.check("symbol");
        if !self.matches(qualifier, name).is_empty() {
            return;
        }
        if self.by_name.contains_key(name) {
            // The name exists, under other qualifiers. Whether the document's
            // qualifier is stale or only spelled differently from the
            // extractor's cannot be told from here: not a finding.
            self.skip("qualifier_unmatched");
            return;
        }
        self.pending.push(PendingSymbol {
            doc: doc.to_string(),
            line,
            written: written.to_string(),
            name: name.to_string(),
            detail: format!("no symbol named {name} is defined in the index"),
        });
    }

    /// Settle the symbols the index does not define. The index models
    /// functions, types and methods; it does not model enum variants, fields,
    /// config keys or other people's libraries. A name is therefore missing
    /// only when it appears as a word in no indexed source file either: one
    /// pass over the non-Markdown files, early out once every wanted name is
    /// seen.
    fn settle_pending(&mut self, files: &[(String, String)]) -> anyhow::Result<Option<String>> {
        if self.pending.is_empty() {
            return Ok(None);
        }
        let mut wanted: HashSet<String> = self.pending.iter().map(|p| p.name.clone()).collect();
        let mut remaining = wanted.len();
        let mut seen: HashSet<String> = HashSet::new();
        let mut bytes_read = 0u64;
        let mut unreadable = 0u32;
        let mut cut: Option<String> = None;
        for (path, language) in files {
            if remaining == 0 {
                break;
            }
            if language == "markdown" || self.is_secret(path) {
                continue;
            }
            if bytes_read >= self.limits.max_word_search_bytes {
                cut = Some(format!(
                    "the word search over source files stopped after {bytes_read} bytes (its bound is {}); \
                     names it had not found by then were left unchecked, not reported",
                    self.limits.max_word_search_bytes
                ));
                break;
            }
            self.cancel.check()?;
            let Ok(text) = devmap_extract::safe_fs::read_repo_source(
                Some(self.root),
                path,
                devmap_extract::MAX_SOURCE_BYTES,
            ) else {
                unreadable += 1;
                continue;
            };
            bytes_read += text.len() as u64;
            for word in words(&text) {
                if wanted.contains(word) {
                    wanted.remove(word);
                    seen.insert(word.to_string());
                    remaining -= 1;
                    if remaining == 0 {
                        break;
                    }
                }
            }
        }
        let pending = std::mem::take(&mut self.pending);
        for item in pending {
            if seen.contains(&item.name) {
                self.skip("defined_outside_symbol_index");
            } else if cut.is_some() && wanted.contains(&item.name) {
                self.skip("word_search_incomplete");
            } else {
                self.find(
                    &item.doc,
                    item.line,
                    DriftKind::MissingSymbol,
                    &item.written,
                    item.detail,
                    Vec::new(),
                );
            }
        }
        Ok(match (cut, unreadable) {
            (Some(reason), _) => Some(reason),
            (None, 0) => None,
            (None, n) => Some(format!(
                "{n} source file(s) could not be read for the word search; a name reported missing may appear in one of them"
            )),
        })
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
        // `file.md#` is the top of the page, and GitHub reads `#top` the same.
        let anchor = anchor.filter(|a| !a.is_empty() && !a.eq_ignore_ascii_case("top"));
        let state = self.state(&rel);
        if matches!(state, PathState::Outside) {
            // Through a link that leaves the repository: not followed, not
            // judged.
            self.skip("outside_repo");
            return Ok(());
        }
        self.check("link");
        match state {
            PathState::Outside => {}
            PathState::Missing => {
                if is_generated_path(&rel) {
                    // A link to a log or a build output that a clean checkout
                    // does not carry.
                    self.skip("generated_path");
                    return Ok(());
                }
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
                    .filter(|symbol| symbol.path == rel)
                    .copied()
                    .collect();
                if !here.is_empty() {
                    if qualifier.is_empty() || here.iter().any(|s| qualifier_matches(s, &qualifier))
                    {
                        continue;
                    }
                    // Defined in the file, under another owner than the link
                    // text names: renamed type or different spelling, unknowable.
                    self.skip("qualifier_unmatched");
                    continue;
                }
                // The index has no such symbol in the file. That is not yet
                // "moved": enum variants, fields and the like are not indexed.
                // Only a name that appears nowhere in the file may be reported.
                if self.is_secret(&rel) {
                    self.skip("secret_path");
                    continue;
                }
                match self.file_mentions(&rel, &name)? {
                    None => {
                        self.unreadable_targets += 1;
                        self.skip("unreadable_target");
                        continue;
                    }
                    Some(true) => {
                        self.skip("defined_outside_symbol_index");
                        continue;
                    }
                    Some(false) => {}
                }
                let elsewhere: Vec<&StoredSymbol> = all
                    .iter()
                    .filter(|symbol| symbol.path != rel)
                    .copied()
                    .collect();
                if elsewhere.is_empty() {
                    self.pending.push(PendingSymbol {
                        doc: doc.to_string(),
                        line,
                        written: written.clone(),
                        name: name.clone(),
                        detail: format!(
                            "{name} is not defined in {rel} or anywhere else in the index"
                        ),
                    });
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

    /// Whether `name` appears as a word anywhere in `rel`. `None` when the file
    /// cannot be read.
    fn file_mentions(&self, rel: &str, name: &str) -> anyhow::Result<Option<bool>> {
        self.cancel.check()?;
        Ok(devmap_extract::safe_fs::read_repo_source(
            Some(self.root),
            rel,
            devmap_extract::MAX_SOURCE_BYTES,
        )
        .ok()
        .map(|text| words(&text).any(|word| word == name)))
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
        if self.is_secret(rel) {
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
            .anchors
            .as_ref()
            .is_some_and(|anchors| anchors.slugs.contains(&slug));
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
                // A line too long to scan could have held the heading.
                if facts.anchors.as_ref().is_some_and(|a| a.incomplete) {
                    self.unscanned_anchor_targets += 1;
                    self.skip("anchor_target_unscanned");
                    return Ok(());
                }
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
        let states: Vec<PathState> = candidates.iter().map(|c| self.state(c)).collect();
        let existing = states
            .iter()
            .position(PathState::exists)
            .map(|at| candidates[at].clone());
        let Some(existing) = existing else {
            // Through a link that leaves the repository, nothing was looked
            // at and nothing is claimed: neither "exists" nor "is missing".
            if states.iter().any(|s| matches!(s, PathState::Outside)) {
                self.skip("outside_repo");
                return Ok(());
            }
            // `db/query.rs` for a file the index knows by a longer path.
            if candidates.iter().any(|c| self.suffixes.contains(c)) {
                self.skip("partial_path");
                return Ok(());
            }
            if !has_known_extension {
                self.skip("not_a_path");
                return Ok(());
            }
            if is_generated_path(path) {
                self.skip("generated_path");
                return Ok(());
            }
            // A path whose first directory is nowhere in this tree is written
            // against some other root: a sibling repository, a crate directory,
            // an example. Not checkable, not drift.
            let first = path.split('/').next().unwrap_or("");
            let rooted = self.state(first).exists()
                || resolve_relative(doc, first).is_some_and(|rel| self.state(&rel).exists());
            if !rooted {
                self.skip("unrooted_path");
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

/// Paths a build, a scratch run or a tool's per-user setup creates: absent from
/// a clean checkout without being stale.
fn is_generated_path(path: &str) -> bool {
    let first = path.split('/').next().unwrap_or("");
    matches!(first, "tmp" | "target" | "node_modules" | "dist" | "build")
        || (first.starts_with('.')
            && first.len() > 1
            && !matches!(
                first,
                ".github" | ".gitlab" | ".circleci" | ".vscode" | ".cargo" | ".husky" | ".."
            ))
        || path.ends_with(".log")
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
    doc_drift_report_with(
        store,
        cancel,
        &DocDriftOptions {
            paths,
            exclude,
            token_budget,
            default_excludes: true,
            limits: Limits::default(),
        },
    )
}

/// What one call asks. `default_excludes` leaves out the [`DEFAULT_EXCLUDES`]
/// unless a path names something inside one.
pub struct DocDriftOptions<'a> {
    pub paths: &'a [String],
    pub exclude: &'a [String],
    pub token_budget: u32,
    pub default_excludes: bool,
    pub limits: Limits,
}

/// [`doc_drift_report`] with every knob.
pub fn doc_drift_report_with(
    store: &Store,
    cancel: &Cancel,
    options: &DocDriftOptions<'_>,
) -> anyhow::Result<DocDriftReport> {
    let token_budget = options.token_budget;
    let include = clean_prefixes(options.paths, "path")?;
    let exclude = clean_prefixes(options.exclude, "exclude")?;
    // A default exclusion is lifted by naming a path inside it.
    let defaults: Vec<&str> = if options.default_excludes {
        DEFAULT_EXCLUDES
            .iter()
            .filter(|prefix| !include.iter().any(|p| under_prefix(p, prefix)))
            .copied()
            .collect()
    } else {
        Vec::new()
    };
    let unavailable = |reason: &str| DocDriftReport {
        default_excludes: Vec::new(),
        docs_excluded_by_default: 0,
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
    let mut excluded_by_default = 0u32;
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
        if defaults.iter().any(|p| under_prefix(path, p)) {
            excluded_by_default += 1;
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
    let mut known_qualifiers: HashSet<String> =
        by_name.keys().map(|name| normalise_segment(name)).collect();
    for (path, _) in &files {
        for part in path.split('/') {
            known_qualifiers.insert(normalise_segment(part));
            if let Some((stem, _)) = part.rsplit_once('.') {
                known_qualifiers.insert(normalise_segment(stem));
            }
        }
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
        findings: BTreeMap::new(),
        sequence: 0,
        total_findings: 0,
        unreadable_targets: 0,
        unscanned_anchor_targets: 0,
        limits: options.limits,
        known_qualifiers,
        pending: Vec::new(),
    };

    let mut docs_scanned = 0u32;
    let mut references = 0u32;
    let mut unread: Vec<String> = Vec::new();
    let mut withheld = 0u32;
    let mut capped_docs = 0u32;
    for doc in docs {
        cancel.check()?;
        if resolver.is_secret(doc) {
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
        let extraction = extract_references_cancellable(&text, Some(cancel))?;
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
    if let Some(reason) = resolver.settle_pending(&files)? {
        push_reason(&mut reasons, reason);
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
    if resolver.unscanned_anchor_targets > 0 {
        push_reason(
            &mut reasons,
            format!(
                "{} anchor reference(s) were not judged because the target has a line over {MAX_LINE_BYTES} bytes that may hold the heading",
                resolver.unscanned_anchor_targets
            ),
        );
    }
    let total = resolver.total_findings;
    if usize::try_from(total).unwrap_or(usize::MAX) > resolver.findings.len() {
        push_reason(
            &mut reasons,
            format!(
                "finding cap of {} cut the list; {total} references drifted and the first {} by document and line were kept",
                options.limits.max_findings,
                resolver.findings.len()
            ),
        );
    }

    let findings = std::mem::take(&mut resolver.findings);
    let mut shown_findings = Vec::new();
    let mut tokens_used = 0u32;
    for finding in findings.into_values() {
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
        default_excludes: if excluded_by_default > 0 {
            defaults.iter().map(|p| (*p).to_string()).collect()
        } else {
            Vec::new()
        },
        docs_excluded_by_default: excluded_by_default,
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
