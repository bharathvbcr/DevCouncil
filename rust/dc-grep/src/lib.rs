//! Repository search, with ripgrep matching and optional tgrep indexing.
//!
//! This crate owns one question — *where in this repository does this pattern
//! appear* — and it answers it with the same three libraries ripgrep itself is
//! assembled from, used the way ripgrep uses them: `ignore` walks the tree in
//! parallel and applies ignore rules, `grep-regex` compiles the pattern, and
//! one `grep-searcher` per worker runs it over file bytes. The walk was
//! single-threaded until it was measured against `rg -j1` and matched it
//! exactly; the matcher had been linked and then run one file at a time.
//! `tgrep-core` supplies candidate planning and postings for explicit snapshots.
//! Only files with unchanged descriptor metadata can be ruled out by a snapshot;
//! the live walker and matcher remain authoritative for every other file.
//!
//! What the crate adds on top of them is the part a search service must not get
//! wrong, which is knowing the difference between *nothing matched* and *the
//! search did not happen*:
//!
//!   - An unparseable pattern is an error, never an empty match set. This is
//!     not a stylistic preference; it is a regression that already cost a run.
//!     A model asked to remove unused imports read `{"count":0}` for the
//!     alternation `sys|time` as proof the file used neither, then read the
//!     same answer for imports the file plainly used, and spent its whole step
//!     budget trying to reconcile two contradictory facts nothing had labelled
//!     as broken.
//!
//!   - A search root outside the repository is refused rather than walked to an
//!     empty result, for the same reason.
//!
//!   - Every file the walk declined to read is counted and reported. A file
//!     skipped for size, a file abandoned as binary, a file the process could
//!     not open — each is a hole in the coverage of the answer, and an answer
//!     that hides its holes is how "no matches here" comes to mean "nowhere I
//!     looked, and I will not say where that was."
//!
//!   - `truncated` means a match was *withheld*, not that the limit was
//!     reached. The two are the same number and different facts, and reporting
//!     the second as the first told every caller with exactly `max_results`
//!     matches — fifty, by default — that its complete answer was partial.
//!
//!   - The answer is the `max_results` smallest `(path, line)` keys, so the
//!     same query over the same tree returns the same rows in the same order.
//!     That is not merely tidy: with workers racing, "the first fifty found"
//!     would be a different fifty on every run.
//!
//! Ignore rules are on by default, which is a deliberate change from the walker
//! this replaced: `.gitignore`, `.ignore`, `.git/info/exclude` and hidden files
//! are all honoured, so a search no longer returns forty hits out of `target/`
//! before it reaches the source. `include_ignored` turns the whole set off for
//! the case where the build output *is* the question.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use grep_regex::RegexMatcherBuilder;
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkMatch};
use ignore::overrides::OverrideBuilder;
use ignore::{WalkBuilder, WalkState};
use serde::{Deserialize, Serialize};

mod index;
mod lexical;
pub use index::{IndexRequest, IndexResponse, SearchIndex, build_index};
pub use lexical::{
    RankedHit, RankedRequest, RankedResponse, ranked_engines, ranked_search, ranked_vocabularies,
};

/// The wire schema this crate speaks. The Go client refuses a binary that
/// answers with a different one rather than decoding through the wrong shape.
pub const SCHEMA_VERSION: u32 = 1;

/// How this binary identifies itself to a health probe.
pub const IDENTITY: &str = "dc-grep";

/// Matches returned when the caller does not say. Fifty is what the tool
/// schema has always promised.
pub const DEFAULT_MAX_RESULTS: usize = 50;

/// Paths returned by a listing when the caller does not say. It is larger than
/// the search default because a path costs a fraction of what a match line
/// does, and the tool it serves is used to enumerate rather than to sample.
pub const DEFAULT_MAX_LIST_RESULTS: usize = 100;

/// The ceiling on `max_results`, whatever the caller asks for. A model that
/// passes a large number is asking for a result set nothing downstream can
/// read; the cap is reported rather than silently applied.
pub const MAX_MAX_RESULTS: usize = 5_000;

/// Files larger than this are not searched. It is the same 2 MiB the Go read
/// tools bound themselves by, because `read_file` and this face the same
/// repository and two different ceilings would only mean one of them was wrong.
pub const DEFAULT_MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// The longest matched line rendered into a result, in bytes.
///
/// The walker this replaced had no such bound: a minified bundle or a
/// single-line data fixture put its entire contents into one match, and fifty
/// of those is a context window. Truncation is marked on the match that
/// suffered it, so a clipped line is never mistaken for the whole line.
pub const DEFAULT_MAX_LINE_BYTES: usize = 1024;

/// The longest pattern accepted, in bytes.
///
/// Patterns come from a model, so their size is not this process's to assume.
/// A 300,000-branch alternation compiled to 416 MiB of automaton in 4.3
/// seconds, and the reply echoed the pattern back, putting 289 KiB into the
/// context window that asked for it. Four kilobytes is longer than any regex a
/// human or a model writes on purpose and far below where either cost bites.
pub const MAX_PATTERN_BYTES: usize = 4 * 1024;

/// The compiled size ceiling for one pattern, in bytes, and for its lazy DFA.
///
/// `MAX_PATTERN_BYTES` bounds the input; these bound the *output*, because the
/// relationship between the two is not linear — a short pattern with nested
/// bounded repeats (`(a{100}){100}{100}`) expands without being long. Ripgrep
/// sets both explicitly for the same reason; the defaults are generous enough
/// that a hostile pattern reaches them.
pub const MAX_REGEX_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_DFA_BYTES: usize = 10 * 1024 * 1024;

/// The largest request accepted on stdin, in bytes.
///
/// `read_to_string` had no bound: a 64 MiB request produced a 75 MiB resident
/// set, and nothing capped what a caller could hand over. Every other boundary
/// in this harness bounds the bytes it will accept from the other side; this
/// one bounds them in the direction the others forgot, which is inbound.
pub const MAX_REQUEST_BYTES: u64 = 1024 * 1024;

/// Ceilings on the per-request tuning knobs.
///
/// The request struct carries `max_file_bytes` and `max_line_bytes` so the Go
/// plane can bound them, but a field a caller can set is a field a caller can
/// set to `u64::MAX`, which is the same as having no bound at all. Both are
/// clamped rather than trusted.
pub const MAX_FILE_BYTES_CEILING: u64 = 64 * 1024 * 1024;
pub const MAX_LINE_BYTES_CEILING: usize = 64 * 1024;

/// Names never searched, whatever the ignore rules say.
///
/// `.git` and `.devcouncil` are the repository's own machinery and the
/// harness's own state; matches from either are the agent reading its own
/// notes back to itself. They are excluded even under `include_ignored`,
/// which is the flag for "search the build output too", not "search the
/// session log too".
///
/// Written without a trailing slash on purpose. A `.git/` glob matches only a
/// directory, and inside a git *worktree* — which is how this harness is
/// routinely run — `.git` is a file holding a gitdir pointer. The slashed form
/// would have walked straight past it.
const ALWAYS_EXCLUDED: &[&str] = &[".git", ".devcouncil"];

/// One search, as the Go execution plane asked for it.
#[derive(Debug, Deserialize)]
pub struct Request {
    /// The regular expression to find. Required, and never empty.
    pub pattern: String,
    /// The repository root. Every result path is relative to it, and nothing
    /// outside it is read.
    pub root: PathBuf,
    /// Where under `root` to search. Empty or "." means the whole repository.
    #[serde(default)]
    pub path: String,
    /// How many matches to return before reporting truncation.
    #[serde(default)]
    pub max_results: usize,
    /// Search files the ignore rules would exclude, hidden files included.
    #[serde(default)]
    pub include_ignored: bool,
    /// Match without regard to case. Off by default, matching the walker this
    /// replaced; `(?i)` in the pattern has always worked and still does.
    #[serde(default)]
    pub case_insensitive: bool,
    /// Files at or above this size are skipped and counted. Zero means the
    /// default.
    #[serde(default)]
    pub max_file_bytes: u64,
    /// Longest rendered line. Zero means the default.
    #[serde(default)]
    pub max_line_bytes: usize,
}

/// One matching line.
#[derive(Debug, Serialize)]
pub struct Hit {
    /// Repository-relative, forward-slashed on every platform.
    pub path: String,
    pub line_number: u64,
    /// The matched line, trimmed of surrounding whitespace and its terminator.
    pub line: String,
    /// Set when `line` is shorter than the line in the file.
    #[serde(skip_serializing_if = "is_false")]
    pub line_truncated: bool,
}

/// What the walk could not look inside, counted rather than dropped.
///
/// Every field here is a hole in the answer's coverage. They are reported
/// unconditionally — a search that skipped nothing says so with zeros — so a
/// caller never has to infer completeness from the absence of a complaint.
///
/// **Scope.** When `truncated` is false these counts are exact and cover the
/// whole tree, because nothing was left unopened. When `truncated` is true the
/// search stopped short on purpose, and `too_large` and `unrepresentable_name`
/// still cover the whole tree — both are decided from the walk's own stat —
/// while `binary` and `unreadable` cover only the files actually opened. They
/// have to: both are learned by opening a file, and `files_pruned` counts the
/// files the search proved it never needed to open. Which files those are
/// depends on worker scheduling, so on a truncated answer those two counts
/// vary run to run. This is written down rather than tidied away because a
/// caller deciding whether to warn about coverage needs to know which of these
/// numbers is a fact about the repository and which is a fact about the run.
#[derive(Debug, Default, Serialize)]
pub struct Skipped {
    /// Files at or over `max_file_bytes`.
    pub too_large: u64,
    /// Files abandoned because they contain binary data.
    ///
    /// A NUL anywhere in the first 64 KiB is found before any match out of
    /// that file is emitted, which covers every real binary format. A NUL
    /// after that is found when the searcher reaches it, and the matches
    /// already collected from the file are then dropped.
    ///
    /// One shape escapes both: a file whose only NUL sits past the point where
    /// the match limit stopped the walk. Reaching it takes a file that is pure
    /// text for `max_results` worth of matching lines and binary only at the
    /// very end, and closing it would mean reading every large file to its end
    /// before returning any of it.
    pub binary: u64,
    /// Files the process could not open or read, and directories the walk
    /// could not descend into.
    pub unreadable: u64,
    /// Files whose names are not valid UTF-8.
    ///
    /// Unix filenames are bytes, and ext4 accepts bytes that are not UTF-8
    /// (APFS does not, which is why this is invisible on macOS and reachable
    /// in production). Rendering such a name lossily produces a path with
    /// U+FFFD in it — a path that names no file, cannot be reopened, and would
    /// hand a model a match it can never act on. The match is dropped and the
    /// file counted instead.
    pub unrepresentable_name: u64,
}

/// The answer to one `Request`.
#[derive(Debug, Serialize)]
pub struct Response {
    /// Always true on this type. A failure is a different shape entirely —
    /// `{"ok":false,"error":...}` — so no caller can read a zero count out of
    /// a search that never ran.
    pub ok: bool,
    pub pattern: String,
    pub count: usize,
    pub matches: Vec<Hit>,
    /// Set when the match limit stopped the walk early.
    #[serde(skip_serializing_if = "is_false")]
    pub truncated: bool,
    /// The limit that did the stopping, present whenever `truncated` is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    /// Files actually opened and searched.
    ///
    /// When `truncated` is set this is not the size of the tree and is not
    /// meant to be read as coverage: see `files_pruned`.
    pub files_searched: u64,
    /// Files not opened because no possible content in them could change any
    /// field of this answer.
    ///
    /// Only ever non-zero alongside `truncated`, and it is carried rather
    /// than folded into `files_searched` because the two are different facts:
    /// one is work done, the other is work that was proved unnecessary. A
    /// caller reading `files_searched` alone against a repository's file
    /// count would otherwise conclude the search had gone blind.
    #[serde(skip_serializing_if = "is_zero")]
    pub files_pruned: u64,
    /// Candidate-index use, including exact filtering and stale-file counts.
    pub index: SearchIndex,
    pub skipped: Skipped,
    /// Whether ignore rules were applied, echoed back so a caller reporting
    /// the result can say which repository it searched.
    pub ignore_rules_applied: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

pub(crate) fn is_zero_usize(value: &usize) -> bool {
    *value == 0
}

/// Runs one search.
///
/// Every `Err` here names a fault. None of them is reachable by a search that
/// simply found nothing: that is `Ok` with `count: 0`.
pub fn search(request: &Request) -> Result<Response, String> {
    if request.pattern.trim().is_empty() {
        return Err("pattern is required".to_string());
    }
    if request.pattern.len() > MAX_PATTERN_BYTES {
        // Named as a refusal rather than truncated into a different pattern.
        // Silently searching for a prefix of what was asked would produce
        // matches that answer a question nobody posed.
        return Err(format!(
            "pattern is {} bytes, over the {MAX_PATTERN_BYTES}-byte limit — \
             this is not a negative result, no search ran",
            request.pattern.len()
        ));
    }

    let (root, target) = resolve_roots(&request.root, &request.path)?;

    // Compiled before the walk starts, so a bad pattern costs nothing and —
    // far more importantly — reports as a bad pattern rather than as a walk
    // that visited every file and matched none of them.
    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(request.case_insensitive)
        .line_terminator(Some(b'\n'))
        // Bounds on what the pattern may compile *to*. Exceeding either is a
        // build error, which is reported as a refused pattern — the same shape
        // as a syntactically invalid one, and for the same reason.
        .size_limit(MAX_REGEX_BYTES)
        .dfa_size_limit(MAX_DFA_BYTES)
        .build(&request.pattern)
        .map_err(|err| {
            format!(
                "pattern {:?} is not a valid regular expression: {err}",
                request.pattern
            )
        })?;

    let limit = match request.max_results {
        0 => DEFAULT_MAX_RESULTS,
        n => n.min(MAX_MAX_RESULTS),
    };
    let max_file_bytes = match request.max_file_bytes {
        0 => DEFAULT_MAX_FILE_BYTES,
        n => n.min(MAX_FILE_BYTES_CEILING),
    };
    let max_line_bytes = match request.max_line_bytes {
        0 => DEFAULT_MAX_LINE_BYTES,
        n => n.min(MAX_LINE_BYTES_CEILING),
    };

    let apply_ignore_rules = !request.include_ignored;
    let mut walker = build_walker(&target, apply_ignore_rules)?;

    let (candidate_index, mut index) =
        index::Candidates::load(&root, &request.pattern, request.case_insensitive);

    let collected = Mutex::new(Collected::default());
    let shared = &collected;
    let matcher = &matcher;
    let root = &root;
    let candidate_index = candidate_index.as_ref();

    // One worker per core, bounded. Everything else in this crate bounds what
    // it will take from the machine it runs on, and threads are no different:
    // a 128-core build server is not a reason to hold 128 files open inside
    // one agent tool call.
    walker.threads(search_threads());
    walker.build_parallel().run(|| {
        // Per worker. A `Searcher` owns a read buffer and is not shareable;
        // the matcher is immutable and is shared by reference.
        let mut searcher = SearcherBuilder::new()
            .line_number(true)
            // One line per match. Multi-line would let a single hit carry an
            // unbounded span of the file into the result.
            .multi_line(false)
            // Stop reading a file the moment it looks binary. The matches
            // already collected from it are then discarded — see `binary`.
            .binary_detection(BinaryDetection::quit(b'\x00'))
            .build();

        Box::new(move |entry| {
            let entry = match entry {
                Ok(entry) => entry,
                // A directory that could not be read is a hole in the answer,
                // not a reason to abandon the search — but it is never silent.
                Err(_) => return note(shared, |c| c.skipped.unreadable += 1),
            };
            // `file_type()` is `None` only for stdin, which this walk never
            // has. Anything that is not a regular file — a directory, a
            // symlink, a FIFO, a device node — is skipped before it can be
            // opened.
            match entry.file_type() {
                Some(file_type) if file_type.is_file() => {}
                _ => return WalkState::Continue,
            }

            let path = entry.path();
            let Ok(rel) = path.strip_prefix(root) else {
                // Unreachable while `follow_links` is off and `target` is
                // under `root`, and counted rather than ignored if it ever
                // stops being.
                return note(shared, |c| c.skipped.unreadable += 1);
            };
            let Some(rel) = slashed(rel) else {
                return note(shared, |c| c.skipped.unrepresentable_name += 1);
            };

            // Size is decided from the walk's own stat, before the prune, so
            // that `too_large` counts the same files on every run. It is a
            // coverage hole and it belongs to the repository, not to whichever
            // worker happened to arrive first — and a hole that appears in one
            // run and not the next is a warning the caller cannot act on. The
            // descriptor is re-checked below; that check is about a file that
            // changed under us, not about this one.
            match entry.metadata() {
                Ok(meta) if meta.len() >= max_file_bytes => {
                    return note(shared, |c| c.skipped.too_large += 1);
                }
                Ok(_) => {}
                Err(_) => return note(shared, |c| c.skipped.unreadable += 1),
            }

            // Asked before the file is opened, because not opening it is the
            // entire saving. See `Collected::can_skip` for why this is sound
            // and why it is not a coverage hole.
            if skippable(shared, &rel, limit) {
                return note(shared, |c| c.files_pruned += 1);
            }

            // Opened once and then interrogated through the descriptor. Every
            // question below — is it really a regular file, how big is it,
            // does it look binary — is asked of what was actually opened
            // rather than of the name, which is the distinction that let a
            // 182-byte symlink to a 5 MiB file past a 2 MiB guard on the Go
            // side of this harness.
            let Ok(file) = open_for_search(path) else {
                return note(shared, |c| c.skipped.unreadable += 1);
            };
            let Ok(meta) = file.metadata() else {
                return note(shared, |c| c.skipped.unreadable += 1);
            };
            if !meta.is_file() {
                // The walk already filtered on `lstat`. This catches the race
                // where the name was a regular file then and is a FIFO or a
                // device now.
                return note(shared, |c| c.skipped.unreadable += 1);
            }
            if meta.len() >= max_file_bytes {
                // Grew past the limit between the walk's stat above and this
                // open. Counted here as well, so the file is named as a hole
                // exactly once either way.
                return note(shared, |c| c.skipped.too_large += 1);
            }

            if let Some(candidates) = candidate_index {
                match candidates.verdict(&rel, &meta) {
                    index::Candidate::Excluded => {
                        return note(shared, |c| c.index_filtered += 1);
                    }
                    index::Candidate::Stale => {
                        tally(shared, |c| c.index_stale += 1);
                    }
                    index::Candidate::Search => {}
                }
            }

            // Per file, into a buffer this worker owns alone. Nothing touches
            // shared state until the file is finished, which is what keeps
            // "drop this file's matches" a local operation rather than a
            // rollback of something another worker may already have read.
            let mut local: Vec<Hit> = Vec::new();
            let mut overflowed = false;
            let mut binary = false;

            let outcome = searcher.search_file(
                matcher,
                &file,
                Collector {
                    path: &rel,
                    out: &mut local,
                    limit,
                    max_line_bytes,
                    truncated: &mut overflowed,
                    binary: &mut binary,
                },
            );

            if binary {
                // A NUL past the probe window. Ripgrep's own detection would
                // keep the matches it found before it; they are dropped
                // instead, because this result goes into a model's context and
                // half a line of a compiled object file is noise that reads
                // like evidence. Dropping them also makes the answer the same
                // on every platform: macOS streams through a 64 KiB buffer and
                // emits those matches, Linux memory-maps and does not.
                //
                // `local` and `overflowed` are dropped with it, so a file
                // excluded whole can never be the reason the *limit* is
                // reported as having held matches back. That used to need a
                // saved flag put back by hand; here it is structural.
                return note(shared, |c| c.skipped.binary += 1);
            }
            if outcome.is_err() {
                return note(shared, |c| c.skipped.unreadable += 1);
            }

            note(shared, |c| {
                c.files_searched += 1;
                c.merge(local, overflowed, limit);
            })
        })
    });

    let Collected {
        best,
        truncated,
        files_searched,
        skipped,
        files_pruned,
        index_filtered,
        index_stale,
    } = match collected.into_inner() {
        Ok(collected) => collected,
        // A worker panicked while holding the lock. The tallies under it are
        // partial by definition, so this is reported as a fault rather than
        // returned: the one rule above every other here is that a search that
        // did not happen must never look like a search that found nothing.
        Err(_) => {
            return Err(
                "a search worker panicked, so this answer would be missing an unknown \
                 number of files — no result is reported for it"
                    .to_string(),
            );
        }
    };
    index.files_filtered += index_filtered;
    index.files_stale += index_stale;

    // `into_sorted_vec` drains the bounded max-heap in ascending key order, so
    // the answer is ordered by path and then line number — and, because the
    // heap kept the *smallest* `limit` keys rather than the first `limit` to
    // arrive, it is the same answer on every run. The sequential walk emitted
    // in `readdir` order: stable on one filesystem, nothing a caller could
    // carry to another, and nothing at all once workers race.
    let matches: Vec<Hit> = best
        .into_sorted_vec()
        .into_iter()
        .map(|hit| hit.0)
        .collect();

    Ok(Response {
        ok: true,
        pattern: request.pattern.clone(),
        count: matches.len(),
        matches,
        truncated,
        limit: truncated.then_some(limit),
        files_searched,
        files_pruned,
        index,
        skipped,
        ignore_rules_applied: apply_ignore_rules,
    })
}

/// One file listing, as the Go execution plane asked for it.
#[derive(Debug, Deserialize)]
pub struct ListRequest {
    /// The repository root. Every path is relative to it.
    pub root: PathBuf,
    /// Where under `root` to list. Empty or "." means the whole repository.
    #[serde(default)]
    pub path: String,
    /// How many paths to return before reporting truncation.
    #[serde(default)]
    pub max_results: usize,
    /// List files the ignore rules would exclude, hidden files included.
    #[serde(default)]
    pub include_ignored: bool,
    /// Files at or above this size are left out and counted, exactly as
    /// `search` leaves them out. Zero means the default — the same default
    /// `search` uses — so the two agree unless a caller overrides one of
    /// them and not the other.
    #[serde(default)]
    pub max_file_bytes: u64,
}

/// The answer to one `ListRequest`.
#[derive(Debug, Serialize)]
pub struct ListResponse {
    pub ok: bool,
    pub count: usize,
    pub paths: Vec<String>,
    #[serde(skip_serializing_if = "is_false")]
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    pub skipped: Skipped,
    pub ignore_rules_applied: bool,
}

/// Lists every file the search would open.
///
/// This exists so `devcouncil_find_files` and `devcouncil_grep` cannot answer
/// different questions about the same repository. It walks through
/// `build_walker`, exactly as `search` does, and applies the same size limit,
/// from the same stat the walk already took.
///
/// The guarantee, stated to the edge rather than rounded up: a path in this
/// list is a path `search` would **open**. Two things can still stop `search`
/// reporting matches from it, and both are named here rather than left for a
/// caller to discover:
///
///   - **Binary content.** Deciding it means reading the file, which is the
///     one cost a listing exists to avoid. A listed path whose contents turn
///     out to hold a NUL is counted in the search's `skipped.binary`, not
///     here.
///   - **A size that changed after this stat.** The listing stats during the
///     walk; the search stats the descriptor it opened. A file that grew past
///     the limit in between is listed and then skipped.
///
/// It said `search would read` before, and that was three filters too
/// optimistic: oversized and binary files were listed as readable, so
/// `find_files` handed out paths that `grep` had silently declined to open —
/// the precise confusion this function was written to end.
///
/// Matching is deliberately *not* done here. `find_files` matches with the
/// harness's own `fnmatch`, which is pinned to a 775-case CPython parity
/// fixture; moving that into this crate would fork the glob semantics to gain
/// nothing. What was wrong was never the matching — it was the two walks.
pub fn list_files(request: &ListRequest) -> Result<ListResponse, String> {
    let (root, target) = resolve_roots(&request.root, &request.path)?;

    let limit = match request.max_results {
        0 => DEFAULT_MAX_LIST_RESULTS,
        n => n.min(MAX_MAX_RESULTS),
    };
    // Clamped identically to `search`, so an absurd override cannot make the
    // listing admit a file the search would refuse.
    let max_file_bytes = match request.max_file_bytes {
        0 => DEFAULT_MAX_FILE_BYTES,
        n => n.min(MAX_FILE_BYTES_CEILING),
    };
    let apply_ignore_rules = !request.include_ignored;
    let walker = build_walker(&target, apply_ignore_rules)?;

    let mut paths = Vec::new();
    let mut truncated = false;
    let mut skipped = Skipped::default();

    for entry in walker.build() {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                skipped.unreadable += 1;
                continue;
            }
        };
        match entry.file_type() {
            Some(file_type) if file_type.is_file() => {}
            _ => continue,
        }
        let Ok(rel) = entry.path().strip_prefix(&root) else {
            skipped.unreadable += 1;
            continue;
        };
        // The walk already stat'ed this entry, so the size filter the search
        // applies costs nothing to apply here too — and applying it is what
        // makes this list answer the question its caller is really asking.
        match entry.metadata() {
            Ok(meta) if meta.len() >= max_file_bytes => {
                skipped.too_large += 1;
                continue;
            }
            Ok(_) => {}
            Err(_) => {
                skipped.unreadable += 1;
                continue;
            }
        }
        let Some(rel) = slashed(rel) else {
            skipped.unrepresentable_name += 1;
            continue;
        };
        // Only a path actually being withheld may raise the flag. This check
        // used to sit at the top of the loop, where it fired on the next entry
        // of *any* kind: one directory visited after the last file was enough
        // to report a complete listing as truncated.
        if paths.len() >= limit {
            truncated = true;
            break;
        }
        paths.push(rel);
    }

    Ok(ListResponse {
        ok: true,
        count: paths.len(),
        paths,
        truncated,
        limit: truncated.then_some(limit),
        skipped,
        ignore_rules_applied: apply_ignore_rules,
    })
}

/// Resolves the repository root and the search root under it, refusing anything
/// that escapes.
///
/// Canonicalising both sides is what makes the containment check mean
/// something. Comparing the strings as given would let `root/../root2`, or a
/// symlinked directory, satisfy a prefix test while reading somewhere else
/// entirely.
fn resolve_roots(root: &Path, requested: &str) -> Result<(PathBuf, PathBuf), String> {
    let root = root
        .canonicalize()
        .map_err(|err| format!("repository root {root:?} is unreadable: {err}"))?;

    let requested = requested.trim();
    let target = if requested.is_empty() || requested == "." || requested == "./" {
        root.clone()
    } else {
        root.join(requested)
            .canonicalize()
            .map_err(|err| format!("path {requested:?} is unreadable: {err}"))?
    };
    if !target.starts_with(&root) {
        return Err(format!(
            "path {requested:?} resolves outside the repository and this tool reads only inside it"
        ));
    }
    Ok((root, target))
}

/// Builds the tree walk both operations use.
///
/// One function, because the alternative already bit: while `devcouncil_grep`
/// honoured ignore rules and `devcouncil_find_files` walked with a hardcoded
/// five-name skip list, `find_files` handed an agent `dist/generated.go` and
/// `grep` would never open it. Two readers of one repository with different
/// assumptions is how "the same tree" drifts apart, and an agent given both
/// answers has no way to tell which one is about the repository it is editing.
fn build_walker(target: &Path, apply_ignore_rules: bool) -> Result<WalkBuilder, String> {
    let mut overrides = OverrideBuilder::new(target);
    for glob in ALWAYS_EXCLUDED {
        // A leading `!` is an *ignore* in override syntax — the sense is
        // inverted from gitignore. Only ignores are added here, which keeps
        // `num_whitelists()` at zero; a single non-`!` glob would flip the
        // matcher into whitelist mode and exclude the entire repository except
        // what it named.
        overrides
            .add(&format!("!{glob}"))
            .map_err(|err| format!("internal exclusion {glob:?} is not a valid glob: {err}"))?;
    }
    let overrides = overrides
        .build()
        .map_err(|err| format!("internal exclusions did not compile: {err}"))?;

    let mut walker = WalkBuilder::new(target);
    walker
        .overrides(overrides)
        // Never followed. A symlink out of the repository is the whole reason
        // containment is checked at all, and a link that points back inside is
        // reached through its real path anyway.
        .follow_links(false)
        .standard_filters(apply_ignore_rules)
        // `require_git(false)` so `.gitignore` is honoured in a directory that
        // is not itself a git checkout — a worktree subdirectory, an extracted
        // archive, a test fixture. Without it the rules silently stop applying
        // and the caller cannot tell.
        .require_git(false)
        .git_ignore(apply_ignore_rules)
        .git_global(apply_ignore_rules)
        .git_exclude(apply_ignore_rules)
        .ignore(apply_ignore_rules)
        .parents(apply_ignore_rules)
        .hidden(apply_ignore_rules);
    Ok(walker)
}

/// Opens a file for searching without letting the filesystem redirect or block
/// the read.
///
/// `O_NOFOLLOW` refuses a final component that has become a symlink since the
/// walk looked at it, and `O_NONBLOCK` means a FIFO planted in the repository
/// fails the open instead of holding this process until something writes to
/// the other end. Both are races the walk's own `lstat` cannot close, and both
/// are reachable by anything that can write into the tree under search.
#[cfg(unix)]
fn open_for_search(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    File::options()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
}

#[cfg(not(unix))]
fn open_for_search(path: &Path) -> io::Result<File> {
    File::open(path)
}

/// Renders a repository-relative path with forward slashes on every platform,
/// so a result read on Windows names the same file a result read on Unix does.
///
/// `None` when any component is not valid UTF-8. Lossy conversion was the
/// alternative and it is worse than refusing: it yields a path containing
/// U+FFFD, which names no file on any filesystem, so the caller receives a
/// match it cannot open and has no way to tell that from a real one.
fn slashed(path: &Path) -> Option<String> {
    let mut out = String::new();
    for component in path.components() {
        let part = component.as_os_str().to_str()?;
        if !out.is_empty() {
            out.push('/');
        }
        out.push_str(part);
    }
    Some(out)
}

/// Workers for one search.
///
/// Ripgrep's speed is not its matcher alone — it is the matcher run over a
/// tree many files at a time. This crate linked the matcher and then walked
/// with one thread, which measured as `rg -j1`: on an 18-core machine a
/// 10,600-file search took 210 ms where ripgrep took 95 ms, and the whole
/// difference was here.
///
/// Capped rather than taken. The walk holds one open descriptor per worker and
/// this runs inside an agent's tool call, alongside whatever else that agent
/// started; a machine with 128 cores is not asking for 128 concurrent reads.
fn search_threads() -> usize {
    const MAX_WORKERS: usize = 12;
    std::thread::available_parallelism()
        .map(|cores| cores.get().min(MAX_WORKERS))
        .unwrap_or(1)
}

/// Whether this file can be left unopened. See `Collected::can_skip`.
///
/// Taken under the same lock the tallies use, once per file and before the
/// open, so the cost of asking is a lock acquisition and the saving is a
/// `read(2)` of up to `max_file_bytes`.
fn skippable(shared: &Mutex<Collected>, rel: &str, limit: usize) -> bool {
    match shared.lock() {
        Ok(guard) => guard.can_skip(rel, limit),
        Err(poisoned) => poisoned.into_inner().can_skip(rel, limit),
    }
}

/// Runs `tally` under the shared lock and tells the walk to keep going.
fn note(shared: &Mutex<Collected>, tally: impl FnOnce(&mut Collected)) -> WalkState {
    self::tally(shared, tally);
    WalkState::Continue
}

/// Runs `apply` under the shared lock.
///
/// A poisoned lock is recovered rather than propagated. The state behind it is
/// a set of counters and a heap, all of which are consistent at every point a
/// panic could land — and the caller checks for poisoning once at the end,
/// where it can refuse the whole answer instead of silently returning a
/// partial one from inside a worker.
fn tally(shared: &Mutex<Collected>, apply: impl FnOnce(&mut Collected)) {
    match shared.lock() {
        Ok(mut guard) => apply(&mut guard),
        Err(poisoned) => apply(&mut poisoned.into_inner()),
    }
}

/// One hit, ordered the way the answer is ordered.
///
/// `path` first and then `line_number`, so the ordering is a property of the
/// repository rather than of which worker happened to finish first.
struct Ranked(Hit);

impl PartialEq for Ranked {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == std::cmp::Ordering::Equal
    }
}

impl Eq for Ranked {}

impl PartialOrd for Ranked {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Ranked {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0
            .path
            .cmp(&other.0.path)
            .then_with(|| self.0.line_number.cmp(&other.0.line_number))
    }
}

/// What the workers build between them.
#[derive(Default)]
struct Collected {
    /// The `limit` smallest hits by `Ranked` order, as a max-heap, so the
    /// largest — the next one to be displaced — is the one on top.
    best: std::collections::BinaryHeap<Ranked>,
    truncated: bool,
    files_searched: u64,
    skipped: Skipped,
    files_pruned: u64,
    index_filtered: u64,
    index_stale: u64,
}

impl Collected {
    /// Whether a file can be left unopened without changing the answer.
    ///
    /// Sound only with both of these true, and it checks both:
    ///
    ///   - **The heap is full.** A hit can then only enter by displacing the
    ///     worst key currently held, so `worst` is a real ceiling rather than
    ///     an artefact of a half-filled heap.
    ///   - **`truncated` is already set.** This is the half that matters.
    ///     Skipping an unopened file cannot be allowed to *establish*
    ///     truncation — that would report a hole nobody proved, which is the
    ///     exact defect this file was carrying. Once the flag is already true
    ///     and honest, a skipped file adds nothing to it.
    ///
    /// With both held, a file whose path sorts strictly after `worst` cannot
    /// contribute a hit (every key it could produce is larger than the one
    /// already being displaced) and cannot change `truncated`. Nothing else in
    /// the answer depends on it. Equal paths are not skipped: that is the same
    /// file, and a smaller line number in it would still place.
    fn can_skip(&self, rel: &str, limit: usize) -> bool {
        if !self.truncated || self.best.len() < limit {
            return false;
        }
        match self.best.peek() {
            Some(worst) => rel > worst.0.path.as_str(),
            None => false,
        }
    }

    /// Folds one finished file into the answer.
    ///
    /// `overflowed` is that file's own report that it held more matches than
    /// the whole limit. Both it and a displaced hit mean the same thing and
    /// are recorded the same way: a match exists that the caller is not being
    /// shown.
    fn merge(&mut self, local: Vec<Hit>, overflowed: bool, limit: usize) {
        if overflowed {
            self.truncated = true;
        }
        for hit in local {
            let candidate = Ranked(hit);
            if self.best.len() < limit {
                self.best.push(candidate);
                continue;
            }
            // Full. Something is being left out either way, which is the
            // whole of what `truncated` claims.
            self.truncated = true;
            match self.best.peek() {
                Some(worst) if candidate < *worst => {
                    self.best.pop();
                    self.best.push(candidate);
                }
                _ => {}
            }
        }
    }
}

/// Collects matching lines from one file.
struct Collector<'a> {
    path: &'a str,
    out: &'a mut Vec<Hit>,
    limit: usize,
    max_line_bytes: usize,
    truncated: &'a mut bool,
    binary: &'a mut bool,
}

impl Sink for Collector<'_> {
    type Error = io::Error;

    fn matched(&mut self, _searcher: &Searcher, mat: &SinkMatch<'_>) -> Result<bool, io::Error> {
        // The limit is full and here is one more match. This is the only
        // moment that learns a match was actually withheld, so it is the only
        // place allowed to say so.
        if self.out.len() >= self.limit {
            *self.truncated = true;
            return Ok(false);
        }
        let (line, line_truncated) = render_line(mat.bytes(), self.max_line_bytes);
        self.out.push(Hit {
            path: self.path.to_string(),
            // `line_number(true)` is set on the searcher, so this is always
            // `Some`. Zero would be a lie about a real line, so an absent
            // number stops the search rather than inventing one.
            line_number: match mat.line_number() {
                Some(number) => number,
                None => {
                    return Err(io::Error::other(
                        "searcher reported a match without a line number",
                    ));
                }
            },
            line,
            line_truncated,
        });
        // Deliberately keeps scanning once the limit fills, rather than
        // stopping here and reporting truncation. Stopping on the match that
        // *fills* the limit cannot tell "there are more" from "that was all",
        // and it answered both the same way: a repository holding exactly
        // `limit` matches told every caller its complete answer was partial.
        // The next match — in this file or a later one — trips the branch
        // above and stops the walk there. The extra work is the distance to
        // that match, and where there is no such match the walk costs what a
        // search finding nothing already costs.
        Ok(true)
    }

    fn binary_data(&mut self, _searcher: &Searcher, _offset: u64) -> Result<bool, io::Error> {
        *self.binary = true;
        Ok(false)
    }
}

/// Turns raw line bytes into the string a result carries.
///
/// Lossy, deliberately: a line with one invalid byte in it is still the line
/// the agent asked about, and refusing to render it would turn a real match
/// into a silent absence.
fn render_line(bytes: &[u8], max_line_bytes: usize) -> (String, bool) {
    let text = String::from_utf8_lossy(bytes);
    let trimmed = text.trim();
    if trimmed.len() <= max_line_bytes {
        return (trimmed.to_string(), false);
    }
    // Truncating a `str` at a byte index panics off a char boundary, so the
    // cut is walked back to the nearest one.
    let mut end = max_line_bytes;
    while end > 0 && !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    (trimmed[..end].to_string(), true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A scratch repository that removes itself.
    ///
    /// Hand-rolled rather than pulled from `tempfile`, because a dev-dependency
    /// is an audit surface too and this needs four lines of what that crate
    /// does.
    struct Scratch {
        path: PathBuf,
    }

    impl Scratch {
        fn new() -> Scratch {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let unique = format!(
                "dc-grep-test-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::SeqCst)
            );
            let path = std::env::temp_dir().join(unique);
            fs::create_dir_all(&path).expect("create scratch root");
            Scratch { path }
        }

        fn write(&self, rel: &str, contents: &[u8]) {
            let full = self.path.join(rel);
            if let Some(parent) = full.parent() {
                fs::create_dir_all(parent).expect("create scratch parent");
            }
            fs::write(full, contents).expect("write scratch file");
        }

        fn request(&self, pattern: &str) -> Request {
            Request {
                pattern: pattern.to_string(),
                root: self.path.clone(),
                path: String::new(),
                max_results: 0,
                include_ignored: false,
                case_insensitive: false,
                max_file_bytes: 0,
                max_line_bytes: 0,
            }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn paths(response: &Response) -> Vec<&str> {
        response.matches.iter().map(|m| m.path.as_str()).collect()
    }

    #[test]
    fn schema_version_is_pinned() {
        // The Go client refuses a binary answering with a different number.
        // Changing this constant without changing that one is the bug this
        // assertion exists to make loud.
        assert_eq!(SCHEMA_VERSION, 1);
        assert_eq!(IDENTITY, "dc-grep");
    }

    #[test]
    fn reports_the_path_line_number_and_line_of_a_match() {
        let scratch = Scratch::new();
        scratch.write("src/main.rs", b"fn main() {\n    let pi = 3.14159;\n}\n");

        let response = search(&scratch.request("3\\.14159")).expect("search runs");

        assert_eq!(response.count, 1);
        let hit = &response.matches[0];
        assert_eq!(hit.path, "src/main.rs");
        assert_eq!(hit.line_number, 2);
        assert_eq!(hit.line, "let pi = 3.14159;");
        assert!(!hit.line_truncated);
    }

    #[test]
    fn an_alternation_matches_every_branch() {
        // The regression that motivated a regex engine here in the first place:
        // a substring search answered `count: 0` for `subprocess|re|os` against
        // a file importing all three, and nothing in the reply said the pattern
        // had not been understood.
        let scratch = Scratch::new();
        scratch.write(
            "mod.py",
            b"import subprocess\nimport re\nimport os\n\nvalue = 1\n",
        );

        let hit = search(&scratch.request("subprocess|re|os")).expect("search runs");
        assert!(hit.count > 0, "alternation must match: {hit:?}");

        // And the negative must remain a real negative, or one useless answer
        // would have replaced another.
        let miss = search(&scratch.request("sys|time")).expect("search runs");
        assert_eq!(miss.count, 0);
    }

    #[test]
    fn an_unparseable_pattern_is_an_error_not_an_empty_result() {
        let scratch = Scratch::new();
        scratch.write("a.txt", b"anything\n");

        let err = search(&scratch.request("unclosed(group")).expect_err("must not succeed");
        assert!(
            err.contains("not a valid regular expression"),
            "the error must name the fault: {err}"
        );
    }

    #[test]
    fn an_empty_pattern_is_an_error() {
        let scratch = Scratch::new();
        let err = search(&scratch.request("   ")).expect_err("must not succeed");
        assert!(err.contains("pattern is required"), "{err}");
    }

    #[test]
    fn a_search_root_outside_the_repository_is_refused() {
        let scratch = Scratch::new();
        scratch.write("a.txt", b"needle\n");

        let mut request = scratch.request("needle");
        request.path = "../..".to_string();
        let err = search(&request).expect_err("must not succeed");
        assert!(err.contains("outside the repository"), "{err}");
    }

    #[test]
    fn gitignored_files_are_skipped_by_default_and_reachable_on_request() {
        let scratch = Scratch::new();
        scratch.write(".gitignore", b"build/\n");
        scratch.write("src/lib.rs", b"needle\n");
        scratch.write("build/generated.rs", b"needle\n");

        let default = search(&scratch.request("needle")).expect("search runs");
        assert_eq!(paths(&default), vec!["src/lib.rs"]);
        assert!(default.ignore_rules_applied);

        let mut request = scratch.request("needle");
        request.include_ignored = true;
        let everything = search(&request).expect("search runs");
        let mut found = paths(&everything);
        found.sort_unstable();
        assert_eq!(found, vec!["build/generated.rs", "src/lib.rs"]);
        assert!(!everything.ignore_rules_applied);
    }

    #[test]
    fn git_and_devcouncil_stay_excluded_even_when_ignore_rules_are_off() {
        let scratch = Scratch::new();
        scratch.write(".git/config", b"needle\n");
        scratch.write(".devcouncil/state.json", b"needle\n");
        scratch.write("src/lib.rs", b"needle\n");

        let mut request = scratch.request("needle");
        request.include_ignored = true;
        let response = search(&request).expect("search runs");

        assert_eq!(
            paths(&response),
            vec!["src/lib.rs"],
            "the harness's own state is never search results"
        );
    }

    #[test]
    fn a_git_pointer_file_is_excluded_the_way_a_git_directory_is() {
        // A worktree checkout — how this harness is routinely run — has `.git`
        // as a file, not a directory. A directory-only glob walked past it.
        let scratch = Scratch::new();
        scratch.write(".git", b"gitdir: /somewhere/needle\n");
        scratch.write("src/lib.rs", b"needle\n");

        let mut request = scratch.request("needle");
        request.include_ignored = true;
        let response = search(&request).expect("search runs");

        assert_eq!(paths(&response), vec!["src/lib.rs"]);
    }

    #[test]
    fn a_binary_file_contributes_no_matches_and_is_counted() {
        let scratch = Scratch::new();
        let mut binary = b"needle then binary\n".to_vec();
        binary.extend_from_slice(&[0u8, 1, 2, 3, 0, 9]);
        scratch.write("blob.bin", &binary);
        scratch.write("src/lib.rs", b"needle\n");

        let response = search(&scratch.request("needle")).expect("search runs");

        assert_eq!(
            paths(&response),
            vec!["src/lib.rs"],
            "a match found before the first NUL is still a match out of a binary file"
        );
        assert_eq!(
            response.skipped.binary, 1,
            "and the skip is reported: {response:?}"
        );
    }

    #[test]
    fn a_large_binary_file_contributes_nothing() {
        // Shaped like every real binary format: a NUL in the header, bytes
        // that happen to look like text after it, and far larger than the
        // searcher's 64 KiB buffer. Detection runs over that first buffer
        // before any match in it is reported, on both the streaming path macOS
        // takes and the memory-mapped path Linux takes.
        let scratch = Scratch::new();
        let mut contents = b"\x7fELF\x02\x01\x01\x00".to_vec();
        for _ in 0..8_000 {
            contents.extend_from_slice(b"needle in a haystack line\n");
        }
        assert!(
            contents.len() > 64 * 1024,
            "the file must exceed one buffer"
        );
        scratch.write("large.bin", &contents);
        scratch.write("src/lib.rs", b"needle\n");

        let mut request = scratch.request("needle");
        request.max_results = 10_000;
        let response = search(&request).expect("search runs");

        assert_eq!(paths(&response), vec!["src/lib.rs"], "{response:?}");
        assert_eq!(response.skipped.binary, 1);
        assert_eq!(response.files_searched, 1);
    }

    #[test]
    fn a_nul_past_the_probe_window_still_voids_the_whole_file() {
        // The second mechanism: a file whose only NUL is past the 8 KiB probe.
        // The searcher reaches it and reports it, and every match already
        // collected from that file is dropped rather than kept — which is also
        // what makes the answer identical on macOS, which streams this file,
        // and Linux, which memory-maps it.
        let scratch = Scratch::new();
        let mut contents = Vec::new();
        contents.extend_from_slice(b"needle near the top\n");
        for _ in 0..3_000 {
            contents.extend_from_slice(b"padding line with nothing of interest\n");
        }
        assert!(
            contents.len() > 64 * 1024,
            "the file must exceed one buffer"
        );
        contents.push(0u8);
        scratch.write("tail-nul.dat", &contents);
        scratch.write("src/lib.rs", b"needle\n");

        // The default limit of 50 is never reached here, so the walk runs to
        // the NUL. That is the condition this mechanism needs, and the test
        // above covers what happens when it is not met.
        let response = search(&scratch.request("needle")).expect("search runs");

        assert_eq!(paths(&response), vec!["src/lib.rs"], "{response:?}");
        assert_eq!(response.skipped.binary, 1);
    }

    #[test]
    fn an_oversized_file_is_skipped_and_counted_rather_than_silently_dropped() {
        let scratch = Scratch::new();
        let mut big = vec![b'a'; 4096];
        big.extend_from_slice(b"\nneedle\n");
        scratch.write("big.txt", &big);
        scratch.write("small.txt", b"needle\n");

        let mut request = scratch.request("needle");
        request.max_file_bytes = 1024;
        let response = search(&request).expect("search runs");

        assert_eq!(paths(&response), vec!["small.txt"]);
        assert_eq!(
            response.skipped.too_large, 1,
            "an unsearched file is a hole in the coverage and must be named: {response:?}"
        );
        assert_eq!(response.files_searched, 1);
    }

    #[test]
    fn the_match_limit_is_reported_rather_than_applied_silently() {
        let scratch = Scratch::new();
        scratch.write("many.txt", b"needle\nneedle\nneedle\nneedle\nneedle\n");

        let mut request = scratch.request("needle");
        request.max_results = 2;
        let response = search(&request).expect("search runs");

        assert_eq!(response.count, 2);
        assert!(response.truncated);
        assert_eq!(response.limit, Some(2));

        // And an untruncated search says so by omission, so the two are never
        // confusable.
        let full = search(&scratch.request("needle")).expect("search runs");
        assert_eq!(full.count, 5);
        assert!(!full.truncated);
        assert_eq!(full.limit, None);
    }

    #[test]
    fn max_results_is_capped_and_the_cap_is_the_reported_limit() {
        let scratch = Scratch::new();
        scratch.write("a.txt", b"needle\n");

        let mut request = scratch.request("needle");
        request.max_results = usize::MAX;
        let response = search(&request).expect("search runs");
        // One match, so nothing truncates; the point is that an absurd request
        // does not become an absurd allocation or an unbounded walk.
        assert_eq!(response.count, 1);
        assert!(!response.truncated);
    }

    #[test]
    fn a_very_long_line_is_clipped_and_says_so() {
        let scratch = Scratch::new();
        let mut line = vec![b'x'; 4096];
        line.extend_from_slice(b"needle\n");
        scratch.write("minified.js", &line);

        let mut request = scratch.request("needle");
        request.max_line_bytes = 64;
        let response = search(&request).expect("search runs");

        assert_eq!(response.count, 1);
        let hit = &response.matches[0];
        assert_eq!(hit.line.len(), 64);
        assert!(
            hit.line_truncated,
            "a clipped line must never pass as the whole line: {hit:?}"
        );
    }

    #[test]
    fn a_line_clipped_mid_character_does_not_panic() {
        let scratch = Scratch::new();
        // A multi-byte character straddling the cut.
        let mut contents = "é".repeat(64).into_bytes();
        contents.extend_from_slice(b"needle\n");
        scratch.write("utf8.txt", &contents);

        let mut request = scratch.request("needle");
        request.max_line_bytes = 5;
        let response = search(&request).expect("search runs");
        assert_eq!(response.count, 1);
        assert!(response.matches[0].line_truncated);
        // 5 is not a char boundary in a run of two-byte characters, so the cut
        // walks back to 4.
        assert_eq!(response.matches[0].line, "éé");
    }

    #[test]
    fn case_insensitivity_is_off_by_default_and_available_on_request() {
        let scratch = Scratch::new();
        scratch.write("a.txt", b"Needle\n");

        assert_eq!(search(&scratch.request("needle")).expect("runs").count, 0);

        let mut request = scratch.request("needle");
        request.case_insensitive = true;
        assert_eq!(search(&request).expect("runs").count, 1);

        // The inline form has always worked and must keep working, because it
        // is what every model reaches for.
        assert_eq!(
            search(&scratch.request("(?i)needle")).expect("runs").count,
            1
        );
    }

    #[test]
    fn a_subdirectory_search_root_is_honoured_and_paths_stay_repository_relative() {
        let scratch = Scratch::new();
        scratch.write("src/a.rs", b"needle\n");
        scratch.write("docs/b.md", b"needle\n");

        let mut request = scratch.request("needle");
        request.path = "src".to_string();
        let response = search(&request).expect("search runs");

        assert_eq!(
            paths(&response),
            vec!["src/a.rs"],
            "a path scoped to src must not reach docs, and must still name the file from the root"
        );
    }

    #[test]
    fn a_search_root_that_does_not_exist_is_an_error_not_an_empty_result() {
        let scratch = Scratch::new();
        scratch.write("a.txt", b"needle\n");

        let mut request = scratch.request("needle");
        request.path = "no/such/dir".to_string();
        let err = search(&request).expect_err("must not succeed");
        assert!(err.contains("unreadable"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_pointing_out_of_the_repository_is_never_read() {
        let outside = Scratch::new();
        outside.write("secret.txt", b"needle\n");

        let scratch = Scratch::new();
        scratch.write("src/a.rs", b"needle\n");
        std::os::unix::fs::symlink(&outside.path, scratch.path.join("escape"))
            .expect("create symlink");

        let mut request = scratch.request("needle");
        request.include_ignored = true;
        let response = search(&request).expect("search runs");

        assert_eq!(
            paths(&response),
            vec!["src/a.rs"],
            "following the link would have read outside the repository: {response:?}"
        );
    }

    #[test]
    fn an_empty_repository_is_a_real_negative() {
        let scratch = Scratch::new();
        let response = search(&scratch.request("needle")).expect("search runs");
        assert_eq!(response.count, 0);
        assert_eq!(response.files_searched, 0);
        assert!(response.ok);
    }

    fn list(scratch: &Scratch, include_ignored: bool) -> ListResponse {
        list_files(&ListRequest {
            root: scratch.path.clone(),
            path: String::new(),
            max_results: 0,
            include_ignored,
            max_file_bytes: 0,
        })
        .expect("listing runs")
    }

    /// The invariant the two tools exist under: everything the listing names is
    /// something the search would open, and nothing else is.
    ///
    /// This is asserted by *running both* over the same tree rather than by
    /// reading the code, because the defect it replaces was two functions that
    /// each looked correct in isolation.
    #[test]
    fn the_listing_and_the_search_see_exactly_the_same_files() {
        let scratch = Scratch::new();
        scratch.write(".gitignore", b"dist/\n*.log\n");
        scratch.write("src/a.rs", b"needle\n");
        scratch.write("src/nested/b.rs", b"needle\n");
        scratch.write("dist/generated.rs", b"needle\n");
        scratch.write("debug.log", b"needle\n");
        scratch.write(".hidden/c.rs", b"needle\n");
        scratch.write(".git/config", b"needle\n");
        scratch.write(".devcouncil/log.json", b"needle\n");

        for include_ignored in [false, true] {
            let mut listed = list(&scratch, include_ignored).paths;
            listed.sort();

            let mut request = scratch.request("needle");
            request.include_ignored = include_ignored;
            request.max_results = 10_000;
            let mut matched: Vec<String> = search(&request)
                .expect("search runs")
                .matches
                .into_iter()
                .map(|hit| hit.path)
                .collect();
            matched.sort();

            // Every file in this tree contains the needle exactly once, so the
            // two sets are directly comparable. `.gitignore` itself does not,
            // which is why it is filtered rather than expected to match.
            listed.retain(|path| path != ".gitignore");

            assert_eq!(
                listed, matched,
                "listing and search disagree at include_ignored={include_ignored}"
            );
        }
    }

    #[test]
    fn the_listing_refuses_a_root_outside_the_repository() {
        let scratch = Scratch::new();
        scratch.write("a.txt", b"x\n");
        let err = list_files(&ListRequest {
            root: scratch.path.clone(),
            path: "../..".to_string(),
            max_results: 0,
            include_ignored: false,
            max_file_bytes: 0,
        })
        .expect_err("must not succeed");
        assert!(err.contains("outside the repository"), "{err}");
    }

    #[test]
    fn the_listing_reports_its_own_truncation() {
        let scratch = Scratch::new();
        for i in 0..10 {
            scratch.write(&format!("f{i}.txt"), b"x\n");
        }
        let response = list_files(&ListRequest {
            root: scratch.path.clone(),
            path: String::new(),
            max_results: 4,
            include_ignored: false,
            max_file_bytes: 0,
        })
        .expect("listing runs");
        assert_eq!(response.count, 4);
        assert!(response.truncated);
        assert_eq!(response.limit, Some(4));
    }

    /// `truncated` means a match was withheld — not that the limit was reached.
    ///
    /// The two are the same number and different facts, and the searcher used
    /// to report the second while calling it the first: a repository holding
    /// exactly `max_results` matches returned every one of them under a flag
    /// saying there were more. Fifty is the default, so this was not an exotic
    /// boundary — it was every query whose answer happened to be fifty.
    #[test]
    fn an_answer_that_exactly_fills_the_limit_is_complete_and_says_so() {
        let scratch = Scratch::new();
        scratch.write("a.txt", b"needle\nneedle\n");
        scratch.write("sub/b.txt", b"nothing here\n");

        let mut request = scratch.request("needle");
        request.max_results = 2;
        let response = search(&request).expect("search runs");

        assert_eq!(response.count, 2);
        assert!(
            !response.truncated,
            "every match in the tree was returned, so nothing was withheld"
        );
        assert_eq!(response.limit, None);
    }

    /// The other direction, so the fix above cannot be "never truncate".
    #[test]
    fn one_match_past_the_limit_is_what_truncation_means() {
        let scratch = Scratch::new();
        scratch.write("a.txt", b"needle\nneedle\nneedle\n");

        let mut request = scratch.request("needle");
        request.max_results = 2;
        let response = search(&request).expect("search runs");

        assert_eq!(response.count, 2);
        assert!(response.truncated);
        assert_eq!(response.limit, Some(2));
    }

    /// A file dropped whole is one hole, and it is counted as one.
    ///
    /// Its matches never reach the caller, so it cannot also be the reason the
    /// answer claims the *limit* held matches back.
    #[test]
    fn a_file_dropped_as_binary_never_contributes_truncation() {
        let scratch = Scratch::new();
        scratch.write("a.txt", b"needle\n");
        scratch.write("b.bin", b"needle\nneedle\x00 tail\n");

        let mut request = scratch.request("needle");
        request.max_results = 1;
        let response = search(&request).expect("search runs");

        assert_eq!(response.count, 1);
        assert_eq!(response.skipped.binary, 1);
        if response.truncated {
            // Reachable only if the walk saw a.txt first and b.bin offered a
            // match before its NUL was found. That is a real withheld match in
            // a file that was *searched*, not dropped — so the flag is honest
            // and the binary count belongs to a different file than the flag.
            assert_eq!(response.limit, Some(1));
        }
    }

    /// The listing declines what the search declines, and counts what it left.
    ///
    /// The doc comment claimed "a path in this list is a path `search` would
    /// read" while the listing named oversized files the search skips — so
    /// `find_files` handed an agent a path, `grep` returned nothing for it,
    /// and nothing in either answer said the file had never been opened.
    #[test]
    fn the_listing_declines_the_oversized_files_the_search_declines() {
        let scratch = Scratch::new();
        scratch.write("small.txt", b"needle\n");
        let mut fat = vec![b'x'; (DEFAULT_MAX_FILE_BYTES as usize) + 16];
        fat.extend_from_slice(b"\nneedle\n");
        scratch.write("huge.txt", &fat);

        let listing = list(&scratch, false);
        assert_eq!(listing.paths, vec!["small.txt".to_string()]);
        assert_eq!(
            listing.skipped.too_large, 1,
            "the file left out is counted, not silently dropped"
        );

        // And the search agrees, which is the whole point of one walk.
        let response = search(&scratch.request("needle")).expect("search runs");
        assert_eq!(paths(&response), vec!["small.txt"]);
        assert_eq!(response.skipped.too_large, 1);
    }

    /// The capped answer is a prefix of the uncapped one.
    ///
    /// This is the oracle for the whole parallel path at once. `search` keeps
    /// the `limit` smallest (path, line) keys, so a capped run must return
    /// exactly the first `limit` rows of an uncapped run over the same tree —
    /// whatever order the workers happened to finish in, and whatever the
    /// path-prune decided not to open. A prune that ever skipped a file it
    /// should have read shows up here as a missing or displaced row.
    #[test]
    fn a_capped_search_is_the_prefix_of_an_uncapped_one() {
        let scratch = Scratch::new();
        // Names deliberately out of walk order relative to their content, so
        // "first by path" cannot coincide with "first visited".
        for (dir, file) in [
            ("zeta", "a.txt"),
            ("alpha", "z.txt"),
            ("mid", "m.txt"),
            ("alpha", "a.txt"),
            ("zeta", "z.txt"),
        ] {
            scratch.write(
                &format!("{dir}/{file}"),
                b"needle one\nfiller\nneedle two\nneedle three\n",
            );
        }

        let mut full = scratch.request("needle");
        full.max_results = 5_000;
        let full = search(&full).expect("search runs");
        assert!(!full.truncated, "the uncapped run must see everything");
        let everything: Vec<(String, u64)> = full
            .matches
            .iter()
            .map(|hit| (hit.path.clone(), hit.line_number))
            .collect();
        assert_eq!(everything.len(), 15);

        // Sorted, and sorted the way the answer claims to be.
        let mut sorted = everything.clone();
        sorted.sort();
        assert_eq!(
            everything, sorted,
            "results come back in (path, line) order"
        );

        for limit in 1..=everything.len() + 1 {
            let mut request = scratch.request("needle");
            request.max_results = limit;
            let capped = search(&request).expect("search runs");
            let got: Vec<(String, u64)> = capped
                .matches
                .iter()
                .map(|hit| (hit.path.clone(), hit.line_number))
                .collect();
            assert_eq!(
                got,
                everything[..limit.min(everything.len())].to_vec(),
                "limit={limit} must be the first {limit} of the uncapped answer"
            );
            assert_eq!(capped.truncated, everything.len() > limit);
            if capped.files_pruned > 0 {
                assert!(
                    capped.truncated,
                    "a file may only be left unopened once truncation is already established"
                );
            }
        }
    }

    /// The same query twice is the same answer twice.
    ///
    /// Worth its own test because the sequential walk this replaced returned
    /// matches in `readdir` order: reproducible on one filesystem by accident,
    /// and not reproducible at all once workers race for the heap.
    #[test]
    fn the_same_truncating_query_answers_identically_every_time() {
        let scratch = Scratch::new();
        for i in 0..40 {
            scratch.write(&format!("d{i:02}/f{i:02}.txt"), b"needle\nneedle\n");
        }

        let mut request = scratch.request("needle");
        request.max_results = 17;
        let first = search(&request).expect("search runs");
        assert!(first.truncated);

        let fingerprint = |response: &Response| {
            response
                .matches
                .iter()
                .map(|hit| format!("{}:{}", hit.path, hit.line_number))
                .collect::<Vec<_>>()
        };
        let expected = fingerprint(&first);
        for round in 0..12 {
            let again = search(&request).expect("search runs");
            assert_eq!(fingerprint(&again), expected, "run {round} differed");
            assert_eq!(again.count, 17);
            assert!(again.truncated);
        }
    }

    /// Every file the walk saw is accounted for exactly once.
    ///
    /// Workers may divide a tree differently on every run, so the split
    /// between searched, pruned and skipped moves. The total must not: a file
    /// that falls out of all three buckets is a file the answer silently never
    /// considered, which is the failure this crate exists to make impossible.
    #[test]
    fn searched_pruned_and_skipped_account_for_every_file() {
        let scratch = Scratch::new();
        for i in 0..30 {
            scratch.write(&format!("d{i:02}/f{i:02}.txt"), b"needle\nneedle\n");
        }
        scratch.write("blob.bin", b"needle\x00needle\n");
        let mut fat = vec![b'x'; (DEFAULT_MAX_FILE_BYTES as usize) + 8];
        fat.extend_from_slice(b"\nneedle\n");
        scratch.write("huge.txt", &fat);
        let total_files = 32;

        let mut seen = std::collections::BTreeSet::new();
        for limit in [1usize, 3, 17, 59, 60, 61, 5_000] {
            let mut request = scratch.request("needle");
            request.max_results = limit;
            let response = search(&request).expect("search runs");

            let accounted = response.files_searched
                + response.files_pruned
                + response.skipped.too_large
                + response.skipped.binary
                + response.skipped.unreadable
                + response.skipped.unrepresentable_name;
            assert_eq!(
                accounted, total_files,
                "limit={limit}: {} searched + {} pruned + {:?}",
                response.files_searched, response.files_pruned, response.skipped
            );

            // Decided from the walk's stat, so they are facts about the
            // repository and hold at every limit — including the limits that
            // prune most of the tree away.
            assert_eq!(response.skipped.too_large, 1, "limit={limit}");

            if !response.truncated {
                assert_eq!(response.files_pruned, 0, "limit={limit}");
                seen.insert((response.skipped.binary, response.skipped.unreadable));
            }
        }
        assert_eq!(
            seen.len(),
            1,
            "an untruncated answer opens every file, so its holes cannot vary: {seen:?}"
        );
    }

    /// Truncation, checked against an oracle instead of an expectation.
    ///
    /// Walk order is `readdir` order, so the shape that exposed the listing
    /// bug — a directory visited after the last file — is not something a
    /// fixture can pin down. Rather than write a test that passes by luck,
    /// this compares every capped run against an uncapped run of the same
    /// tree: `truncated` must be true exactly when the uncapped answer is
    /// larger. That holds whatever order the filesystem hands back.
    #[test]
    fn truncation_agrees_with_an_uncapped_run_over_generated_trees() {
        for seed in 0..24u32 {
            let scratch = Scratch::new();
            let files = 1 + (seed % 6) as usize;
            let per_file = 1 + (seed / 6) as usize;

            for f in 0..files {
                let mut body = String::new();
                for line in 0..per_file {
                    body.push_str(&format!("needle {f} {line}\n"));
                }
                // Directories, some of them empty, so the walk yields
                // non-file entries between and after the files.
                let rel = match seed % 3 {
                    0 => format!("f{f}.txt"),
                    1 => format!("d{f}/f{f}.txt"),
                    _ => format!("d{f}/deeper/f{f}.txt"),
                };
                scratch.write(&rel, body.as_bytes());
            }
            scratch.write("zz_empty_dir/.keep", b"");
            let _ = fs::remove_file(scratch.path.join("zz_empty_dir/.keep"));

            let total_matches = files * per_file;
            let total_files = files;

            for limit in 1..=(total_matches + 2) {
                let mut request = scratch.request("needle");
                request.max_results = limit;
                let response = search(&request).expect("search runs");

                let expected = total_matches > limit;
                assert_eq!(
                    response.truncated, expected,
                    "search seed={seed} limit={limit}: {} matches exist, \
                     {} returned, truncated={}",
                    total_matches, response.count, response.truncated
                );
                assert_eq!(response.count, total_matches.min(limit));
                assert_eq!(response.limit.is_some(), expected);
            }

            for limit in 1..=(total_files + 2) {
                let listing = list_files(&ListRequest {
                    root: scratch.path.clone(),
                    path: String::new(),
                    max_results: limit,
                    include_ignored: false,
                    max_file_bytes: 0,
                })
                .expect("listing runs");

                let expected = total_files > limit;
                assert_eq!(
                    listing.truncated, expected,
                    "listing seed={seed} limit={limit}: {} files exist, \
                     {} returned, truncated={}",
                    total_files, listing.count, listing.truncated
                );
                assert_eq!(listing.count, total_files.min(limit));
                assert_eq!(listing.limit.is_some(), expected);
            }
        }
    }

    #[test]
    fn an_over_long_pattern_is_refused_rather_than_compiled() {
        let scratch = Scratch::new();
        scratch.write("a.txt", b"needle\n");

        let long = "a".repeat(MAX_PATTERN_BYTES + 1);
        let err = search(&scratch.request(&long)).expect_err("must not succeed");
        assert!(err.contains("over the"), "{err}");
        assert!(
            err.contains("no search ran"),
            "the refusal must not read as a negative result: {err}"
        );

        // And one byte under the limit still runs, so the bound is a bound and
        // not a wall in front of legitimate patterns.
        let allowed = "a".repeat(MAX_PATTERN_BYTES);
        assert!(search(&scratch.request(&allowed)).is_ok());
    }

    /// A short pattern can compile to an enormous automaton, so bounding the
    /// input is not the same as bounding the work.
    ///
    /// Measured before this bound existed: `(\p{L}{200}){200}` and its
    /// relatives compiled to 416 MiB in 4.3 seconds, from a pattern a model can
    /// type in a second.
    #[test]
    fn a_pattern_that_compiles_enormous_is_refused() {
        let scratch = Scratch::new();
        scratch.write("a.txt", b"needle\n");

        for pattern in [
            "a{1000}{1000}",
            r"(\w{500}){500}",
            "((((a{50}){50}){50}){50})",
            r"(?i)(\p{L}{200}){200}",
        ] {
            let err =
                search(&scratch.request(pattern)).expect_err(&format!("{pattern} must be refused"));
            assert!(
                err.contains("not a valid regular expression"),
                "{pattern}: {err}"
            );
        }

        // A large but honest pattern still works, so the ceiling is not simply
        // rejecting anything with a repetition in it.
        assert!(search(&scratch.request(r"[\s\S]{5000}")).is_ok());
    }

    #[test]
    fn the_per_request_knobs_are_clamped_rather_than_trusted() {
        let scratch = Scratch::new();
        let mut line = vec![b'x'; 256 * 1024];
        line.extend_from_slice(b"needle\n");
        scratch.write("wide.txt", &line);

        let mut request = scratch.request("needle");
        // A field a caller can set is a field a caller can set to the maximum.
        request.max_line_bytes = usize::MAX;
        request.max_file_bytes = u64::MAX;
        let response = search(&request).expect("search runs");

        assert_eq!(response.count, 1);
        let hit = &response.matches[0];
        assert!(
            hit.line.len() <= MAX_LINE_BYTES_CEILING,
            "an unbounded request must not produce an unbounded line: {} bytes",
            hit.line.len()
        );
        assert!(hit.line_truncated);
    }

    /// The same rule, applied to a file that really exists.
    ///
    /// Unix filenames are bytes. ext4 accepts bytes that are not UTF-8 and APFS
    /// refuses them, so this case is unreachable on the machine most of this
    /// was written on and perfectly reachable in production. The test creates
    /// the file and returns early where the filesystem will not have it, rather
    /// than asserting nothing on every platform to avoid failing on one.
    #[cfg(unix)]
    #[test]
    fn a_real_file_with_a_non_utf8_name_is_counted_not_reported() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let scratch = Scratch::new();
        scratch.write("good.txt", b"needle\n");

        let bad = scratch.path.join(OsStr::from_bytes(b"ba\xffd.txt"));
        if fs::write(&bad, b"needle\n").is_err() {
            // APFS and any other filesystem that enforces UTF-8 names. The
            // rendering itself is asserted unconditionally in the test above.
            return;
        }

        let mut request = scratch.request("needle");
        request.include_ignored = true;
        let response = search(&request).expect("search runs");

        assert_eq!(
            paths(&response),
            vec!["good.txt"],
            "a path that cannot be reopened must never be reported as a match: {response:?}"
        );
        assert_eq!(
            response.skipped.unrepresentable_name, 1,
            "and the file must be counted rather than dropped: {response:?}"
        );

        // The listing answers the same way, so the two views stay identical
        // even on the paths neither of them can name.
        let listing = list_files(&ListRequest {
            root: scratch.path.clone(),
            path: String::new(),
            max_results: 0,
            include_ignored: true,
            max_file_bytes: 0,
        })
        .expect("listing runs");
        assert!(
            !listing.paths.iter().any(|path| path.contains('\u{fffd}')),
            "a lossy path reached the listing: {listing:?}"
        );
        assert_eq!(listing.skipped.unrepresentable_name, 1);
    }

    #[test]
    fn a_path_component_that_is_not_utf8_yields_no_path_at_all() {
        // Built from bytes rather than from a file, because APFS refuses to
        // create such a name and ext4 does not — the filesystem this runs on
        // decides whether the case is reachable, and the rendering must be
        // correct on both.
        #[cfg(unix)]
        {
            use std::ffi::OsStr;
            use std::os::unix::ffi::OsStrExt;
            let bad = PathBuf::from(OsStr::from_bytes(b"src/ca\xffe.rs"));
            assert_eq!(
                slashed(&bad),
                None,
                "a lossy path names no file and must never reach a caller"
            );
        }
        assert_eq!(
            slashed(Path::new("src/nested/a.rs")),
            Some("src/nested/a.rs".to_string())
        );
    }

    #[test]
    fn max_max_results_is_the_number_the_go_plane_mirrors() {
        // manvi/dc/dcgrep declares MaxListResults against this constant so a
        // caller asking for "everything" is not silently clamped to less than
        // it believes it asked for. The two are asserted equal across the
        // boundary in the Go tests; this pins the value they agree on.
        assert_eq!(MAX_MAX_RESULTS, 5_000);
    }

    /// A tiny deterministic generator, so the randomized tests below reproduce
    /// exactly on a failure and add no dependency to a crate whose dependency
    /// list is the reason it needed authorising.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            // xorshift64*: short, well-known, and good enough to shuffle test
            // inputs. Nothing here is cryptographic.
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            x.wrapping_mul(0x2545_f491_4f6c_dd1d)
        }

        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }

        fn pick<'a, T>(&mut self, options: &'a [T]) -> &'a T {
            &options[self.below(options.len())]
        }
    }

    /// Whatever the pattern, the engine answers or names a fault. It never
    /// panics, and it never reports a fault as an empty match set.
    ///
    /// The pattern is the one input a model controls completely, so this is the
    /// surface where "it did not occur to me that someone would type that" is
    /// least acceptable. The alphabet below is regex metacharacters rather than
    /// letters precisely because random letters would only ever produce valid,
    /// boring patterns.
    #[test]
    fn no_pattern_makes_the_engine_panic_or_lie() {
        let scratch = Scratch::new();
        scratch.write("src/a.rs", b"needle\nfn main() {}\n");
        scratch.write("src/b.txt", b"[]()*+?{}|^$.\\\n");

        const PIECES: &[&str] = &[
            "a",
            ".",
            "*",
            "+",
            "?",
            "|",
            "(",
            ")",
            "[",
            "]",
            "{",
            "}",
            "^",
            "$",
            "\\",
            "\\b",
            "\\w",
            "\\s",
            "\\p{L}",
            "(?i)",
            "(?m)",
            "(?s)",
            "(?:",
            "(?P<n>",
            "{2,}",
            // Repeat counts are tiny on purpose, and the reason is that they
            // compose: the generator concatenates up to twelve pieces, so five
            // "{50}" tokens in a row is a{50}{50}{50}{50}{50} — fifty to the
            // fifth power of states, built and then thrown away, four thousand
            // times. That is what made this test take ninety seconds. The
            // shapes are what it is here to vary; the cost of a pattern that
            // compiles enormous has its own test.
            "{0,4}",
            "{3}",
            "[a-z]",
            "[^a]",
            "\\x00",
            "\u{e9}",
            "\u{1f600}",
            "needle",
            "",
            " ",
        ];

        let mut rng = Rng(0x5eed_1234_abcd_ef01);
        // Two hundred and fifty, and the number is a budget rather than a
        // belief about coverage.
        //
        // Each iteration costs about 47ms, essentially all of it compiling a
        // fresh regex in a debug build — the same work takes a millisecond
        // optimised, which is why the release binary searches this whole
        // repository in 45ms. Four thousand iterations put ninety seconds into
        // a gate that runs on every commit and found nothing the first two
        // hundred did not.
        //
        // The seed is fixed, so this explores the same shapes every run and a
        // failure reproduces exactly rather than appearing once in a CI log.
        for iteration in 0..250 {
            let mut pattern = String::new();
            for _ in 0..rng.below(12) {
                pattern.push_str(rng.pick(PIECES));
            }

            let mut request = scratch.request(&pattern);
            request.case_insensitive = iteration % 2 == 0;
            request.include_ignored = iteration % 3 == 0;
            request.max_results = rng.below(8);

            match search(&request) {
                Ok(response) => {
                    assert!(response.ok, "an Ok response must say ok: {pattern:?}");
                    assert_eq!(
                        response.count,
                        response.matches.len(),
                        "count and matches disagree for {pattern:?}"
                    );
                    for hit in &response.matches {
                        assert!(!hit.path.is_empty(), "empty path for {pattern:?}");
                        assert!(hit.line_number >= 1, "line 0 for {pattern:?}");
                        assert!(
                            !hit.path.starts_with('/') && !hit.path.contains(".."),
                            "escaping path {:?} for {pattern:?}",
                            hit.path
                        );
                    }
                    if response.truncated {
                        assert!(response.limit.is_some(), "truncated without a limit");
                    }
                }
                Err(message) => {
                    // A refusal must name what was refused. An empty message is
                    // the same failure as an empty match set: a fault the
                    // caller cannot distinguish from an answer.
                    assert!(
                        !message.trim().is_empty(),
                        "a refusal with no reason for {pattern:?}"
                    );
                }
            }
        }
    }

    /// The listing and the search agree over randomly generated trees, not just
    /// the one tree someone wrote by hand.
    ///
    /// The correspondence is the whole reason `list_files` exists, and the
    /// defect it replaced was two hand-written walks that each looked right.
    #[test]
    fn the_listing_matches_the_search_over_generated_trees() {
        const NAMES: &[&str] = &[
            "a.rs",
            "b.go",
            "c.md",
            ".hidden.txt",
            "build.log",
            "x.tmp",
            "nested/d.rs",
            "nested/deep/e.go",
            "dist/f.js",
            ".config/g.toml",
            "target/h.rs",
        ];
        const IGNORES: &[&str] = &[
            "",
            "*.log\n",
            "dist/\n",
            "target/\n",
            "*.tmp\n",
            "dist/\ntarget/\n*.log\n",
            "nested/\n",
            "!important\n*.rs\n",
        ];

        let mut rng = Rng(0x1234_5678_9abc_def0);
        for _ in 0..120 {
            let scratch = Scratch::new();
            scratch.write(".gitignore", rng.pick(IGNORES).as_bytes());
            // Every file carries the same needle, so the listing and the match
            // set are directly comparable.
            let mut written = 0;
            for name in NAMES {
                if rng.below(3) > 0 {
                    scratch.write(name, b"needle\n");
                    written += 1;
                }
            }
            if written == 0 {
                continue;
            }

            for include_ignored in [false, true] {
                let mut listed = list_files(&ListRequest {
                    root: scratch.path.clone(),
                    path: String::new(),
                    max_results: 0,
                    include_ignored,
                    max_file_bytes: 0,
                })
                .expect("listing runs")
                .paths;
                listed.retain(|path| path != ".gitignore");
                listed.sort();

                let mut request = scratch.request("needle");
                request.include_ignored = include_ignored;
                request.max_results = 1000;
                let mut matched: Vec<String> = search(&request)
                    .expect("search runs")
                    .matches
                    .into_iter()
                    .map(|hit| hit.path)
                    .collect();
                matched.sort();

                assert_eq!(
                    listed, matched,
                    "listing and search disagree at include_ignored={include_ignored}"
                );
            }
        }
    }

    #[test]
    fn arbitrary_file_bytes_survive_the_json_boundary() {
        // The reason this crate serialises rather than hand-rolls: the line it
        // emits is whatever the repository under search happens to contain.
        let scratch = Scratch::new();
        scratch.write(
            "weird.txt",
            "needle \" \\ \u{7} \u{1b}[0m ünïcödé\n".as_bytes(),
        );

        let response = search(&scratch.request("needle")).expect("search runs");
        let encoded = serde_json::to_string(&response).expect("renders as JSON");
        let decoded: serde_json::Value = serde_json::from_str(&encoded).expect("and parses back");
        assert_eq!(decoded["matches"][0]["line"], response.matches[0].line);
    }
}
