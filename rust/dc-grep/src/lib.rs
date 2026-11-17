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

use std::collections::HashMap;
use std::fs::{self, File};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use grep_regex::RegexMatcherBuilder;
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkMatch};
use ignore::overrides::OverrideBuilder;
use ignore::{WalkBuilder, WalkState};
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

mod git;
mod index;
mod lexical;
pub use index::{IndexRequest, IndexResponse, LeftOut, SearchIndex, build_index};
pub use lexical::{
    RankedHit, RankedRequest, RankedResponse, ranked_engines, ranked_search, ranked_vocabularies,
    wordpiece_ids,
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

/// The ceiling on a *listing's* `max_results`, which is not the search's.
///
/// Searching sample: 5,000 match lines is already more than anything reads.
/// Listing enumerates: it answers "which files are in this repository", and a
/// repository with more than 5,000 files is ordinary — this one crossed it at
/// 5,408. Sharing the sampling ceiling made the enumeration return a prefix
/// flagged `truncated`, which is honest but unhelpful, because the clamp left
/// no larger number to ask for.
///
/// It is `lexical::ingest::MAX_DOCUMENTS`, so the enumeration is never the
/// constraint that binds first: a caller can always list at least as many
/// files as the encoder downstream is willing to accept. Listing beyond that
/// would be listing files nothing can consume.
pub const MAX_LIST_RESULTS: usize = lexical::ingest::MAX_DOCUMENTS;

// Enumerating must never be the constraint that binds before ingesting.
//
// If the listing ceiling fell below what the encoder accepts, a producer could
// be told "that is all the files there are" for a corpus the consumer would
// have taken in full — and the shortfall would surface only as documents
// ranking by the wrong tier. Compile-time, because a test can only fail after
// a binary carrying the wrong ceiling has been built.
const _: () = assert!(MAX_LIST_RESULTS >= lexical::ingest::MAX_DOCUMENTS);
const _: () = assert!(MAX_LIST_RESULTS > MAX_MAX_RESULTS);
const _: () = assert!(DEFAULT_MAX_LIST_RESULTS < MAX_LIST_RESULTS);

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
#[derive(Debug, Default, Deserialize)]
pub struct Request {
    /// The regular expression (or literal string when `fixed_strings` is true)
    /// to find. Required, and never empty.
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
    /// Treat `pattern` as a fixed literal string instead of a regular expression.
    #[serde(default)]
    pub fixed_strings: bool,
    /// Include non-ignored dotfiles and dot-directories in the search.
    /// `.git` and `.devcouncil` remain unconditionally excluded.
    #[serde(default)]
    pub include_hidden: bool,
    /// Preserve leading and interior line whitespace, trimming only the trailing
    /// line terminator (`\n` or `\r\n`).
    #[serde(default)]
    pub preserve_whitespace: bool,
    /// Normalize repository-relative paths to Unicode NFC.
    #[serde(default)]
    pub precompose_unicode: bool,
    /// Files at or above this size are skipped and counted. Zero means the
    /// default.
    #[serde(default)]
    pub max_file_bytes: u64,
    /// Longest rendered line. Zero means the default.
    #[serde(default)]
    pub max_line_bytes: usize,
    /// Include un-ignored `.devcouncil/*` project files while keeping `.git` and
    /// `.devcouncil/dcgrep` index caches excluded.
    #[serde(default)]
    pub include_devcouncil: bool,
    /// Do not read `.ignore` files (so `.ignore` cannot override `.gitignore`).
    #[serde(default)]
    pub ignore_dot_ignore: bool,
    /// Also search regular files tracked in `.git/index` even when `.gitignore`
    /// matches them (ignored if `include_ignored` is already true).
    #[serde(default)]
    pub include_git_tracked: bool,
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
    if request.pattern.is_empty() || (!request.fixed_strings && request.pattern.trim().is_empty()) {
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

    let (root, target) =
        resolve_roots_with(&request.root, &request.path, request.include_devcouncil)?;

    // Compiled before the walk starts, so a bad pattern costs nothing and —
    // far more importantly — reports as a bad pattern rather than as a walk
    // that visited every file and matched none of them.
    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(request.case_insensitive)
        .fixed_strings(request.fixed_strings)
        .multi_line(true)
        .crlf(true)
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
    let walk_opts = WalkOptions {
        apply_ignore_rules,
        include_hidden: request.include_hidden,
        include_devcouncil: request.include_devcouncil,
        ignore_dot_ignore: request.ignore_dot_ignore,
    };
    let has_git_root = root.join(".git").exists();
    let ignore_case = if apply_ignore_rules && has_git_root {
        git::read_ignore_case(&root)
    } else {
        false
    };

    struct TrackedState {
        items: Vec<(String, AtomicBool)>,
        lookup: HashMap<String, usize>,
        ignore_case: bool,
    }

    let tracked_state = if request.include_git_tracked && apply_ignore_rules && has_git_root {
        let target_rel = if target == root {
            String::new()
        } else {
            target
                .strip_prefix(&root)
                .ok()
                .and_then(|rel| slashed(rel, false))
                .unwrap_or_default()
        };
        let tracked = git::read_tracked_regular_paths(&root)?;
        let mut items = Vec::new();
        let mut lookup = HashMap::new();
        for rel in tracked {
            if is_excluded_machinery_rel(&rel, request.include_devcouncil) {
                continue;
            }
            if !target_rel.is_empty() && !rel_is_under_target(&rel, &target_rel, ignore_case) {
                continue;
            }
            let key = tracked_dedup_key(&rel, ignore_case);
            if let std::collections::hash_map::Entry::Vacant(slot) = lookup.entry(key) {
                let idx = items.len();
                slot.insert(idx);
                items.push((rel, AtomicBool::new(false)));
            }
        }
        Some(TrackedState {
            items,
            lookup,
            ignore_case,
        })
    } else {
        None
    };

    let (candidate_index, mut index) = index::Candidates::load(
        &root,
        &request.pattern,
        request.case_insensitive,
        request.fixed_strings,
    );

    let collected = Mutex::new(Collected::default());
    let shared = &collected;
    let matcher = &matcher;
    let root_ref = &root;
    let candidate_index = candidate_index.as_ref();
    let preserve_whitespace = request.preserve_whitespace;
    let precompose_unicode = request.precompose_unicode;
    let tracked_ref = tracked_state.as_ref();

    let skip_nested_walk =
        apply_ignore_rules && has_git_root && target_inside_nested_git(&root, &target);

    if !skip_nested_walk {
        let mut walker = build_walker_with(&root, &target, walk_opts)?;
        // One worker per core, bounded. Everything else in this crate bounds what
        // it will take from the machine it runs on, and threads are no different:
        // a 128-core build server is not a reason to hold 128 files open inside
        // one agent tool call.
        walker.threads(search_threads());
        walker.build_parallel().run(|| {
            // Per worker. A `Searcher` owns a read buffer and is not shareable;
            // the matcher is immutable and is shared by reference.
            let mut searcher = build_line_searcher();

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
                let Ok(rel) = path.strip_prefix(root_ref) else {
                    // Unreachable while `follow_links` is off and `target` is
                    // under `root`, and counted rather than ignored if it ever
                    // stops being.
                    return note(shared, |c| c.skipped.unreadable += 1);
                };
                let Some(raw_rel) = slashed(rel, false) else {
                    return note(shared, |c| c.skipped.unrepresentable_name += 1);
                };
                if let Some(tracked) = tracked_ref
                    && let Some(&idx) = tracked
                        .lookup
                        .get(&tracked_dedup_key(&raw_rel, tracked.ignore_case))
                {
                    tracked.items[idx].1.store(true, Ordering::Relaxed);
                }
                let display_rel = if precompose_unicode && !raw_rel.is_ascii() {
                    raw_rel.nfc().collect::<String>()
                } else {
                    raw_rel.clone()
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
                if skippable(shared, &display_rel, limit) {
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
                search_opened_file(
                    &file,
                    &raw_rel,
                    &display_rel,
                    limit,
                    max_file_bytes,
                    max_line_bytes,
                    preserve_whitespace,
                    candidate_index,
                    matcher,
                    &mut searcher,
                    shared,
                );
                WalkState::Continue
            })
        });
    }

    if let Some(tracked) = tracked_state.as_ref() {
        let missing: Vec<&str> = tracked
            .items
            .iter()
            .filter_map(|(rel, seen)| (!seen.load(Ordering::Relaxed)).then_some(rel.as_str()))
            .collect();
        if !missing.is_empty() {
            let mut searcher = build_line_searcher();
            for raw_rel in missing {
                let file = match open_tracked_regular_no_symlinks(root_ref, raw_rel) {
                    TrackedOpen::File(file) => file,
                    TrackedOpen::Skip => continue,
                    TrackedOpen::Unreadable => {
                        tally(shared, |c| c.skipped.unreadable += 1);
                        continue;
                    }
                };
                let display_rel = if precompose_unicode && !raw_rel.is_ascii() {
                    raw_rel.nfc().collect::<String>()
                } else {
                    raw_rel.to_string()
                };
                let Ok(meta) = file.metadata() else {
                    tally(shared, |c| c.skipped.unreadable += 1);
                    continue;
                };
                if !meta.is_file() {
                    continue;
                }
                if meta.len() >= max_file_bytes {
                    tally(shared, |c| c.skipped.too_large += 1);
                    continue;
                }
                if skippable(shared, &display_rel, limit) {
                    tally(shared, |c| c.files_pruned += 1);
                    continue;
                }
                search_opened_file(
                    &file,
                    raw_rel,
                    &display_rel,
                    limit,
                    max_file_bytes,
                    max_line_bytes,
                    preserve_whitespace,
                    candidate_index,
                    matcher,
                    &mut searcher,
                    shared,
                );
            }
        }
    }

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
#[derive(Debug, Default, Deserialize)]
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
    /// List non-ignored dotfiles and dot-directories.
    /// `.git` and `.devcouncil` remain unconditionally excluded.
    #[serde(default)]
    pub include_hidden: bool,
    /// Include regular files regardless of `max_file_bytes`.
    #[serde(default)]
    pub include_oversized: bool,
    /// Include symbolic links (without following them).
    #[serde(default)]
    pub include_symlinks: bool,
    /// Normalize repository-relative paths to Unicode NFC.
    #[serde(default)]
    pub precompose_unicode: bool,
    /// Files at or above this size are left out and counted, exactly as
    /// `search` leaves them out. Zero means the default — the same default
    /// `search` uses — so the two agree unless a caller overrides one of
    /// them and not the other.
    #[serde(default)]
    pub max_file_bytes: u64,
    /// Include un-ignored `.devcouncil/*` project files while keeping `.git` and
    /// `.devcouncil/dcgrep` index caches excluded.
    #[serde(default)]
    pub include_devcouncil: bool,
    /// Do not read `.ignore` files (so `.ignore` cannot override `.gitignore`).
    #[serde(default)]
    pub ignore_dot_ignore: bool,
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
    let (root, target) =
        resolve_roots_with(&request.root, &request.path, request.include_devcouncil)?;

    let limit = match request.max_results {
        0 => DEFAULT_MAX_LIST_RESULTS,
        n => n.min(MAX_LIST_RESULTS),
    };
    // Clamped identically to `search`, so an absurd override cannot make the
    // listing admit a file the search would refuse.
    let max_file_bytes = match request.max_file_bytes {
        0 => DEFAULT_MAX_FILE_BYTES,
        n => n.min(MAX_FILE_BYTES_CEILING),
    };
    let apply_ignore_rules = !request.include_ignored;
    let has_git_root = root.join(".git").exists();
    if apply_ignore_rules && has_git_root && target_inside_nested_git(&root, &target) {
        return Ok(ListResponse {
            ok: true,
            count: 0,
            paths: Vec::new(),
            truncated: false,
            limit: None,
            skipped: Skipped::default(),
            ignore_rules_applied: apply_ignore_rules,
        });
    }
    let walker = build_walker_with(
        &root,
        &target,
        WalkOptions {
            apply_ignore_rules,
            include_hidden: request.include_hidden,
            include_devcouncil: request.include_devcouncil,
            ignore_dot_ignore: request.ignore_dot_ignore,
        },
    )?;

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
        let Some(file_type) = entry.file_type() else {
            continue;
        };
        let is_symlink = file_type.is_symlink();
        if !file_type.is_file() && !(request.include_symlinks && is_symlink) {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(&root) else {
            skipped.unreadable += 1;
            continue;
        };
        // The walk already stat'ed regular entries, so the size filter the
        // search applies costs nothing to apply here too — and applying it is
        // what makes this list answer the question its caller is really asking.
        // Symlinks (only reachable when `include_symlinks` is set) skip both
        // the size limit and `metadata()` so dangling symlinks stay listed.
        if !is_symlink {
            match entry.metadata() {
                Ok(meta) if !request.include_oversized && meta.len() >= max_file_bytes => {
                    skipped.too_large += 1;
                    continue;
                }
                Ok(_) => {}
                Err(_) => {
                    skipped.unreadable += 1;
                    continue;
                }
            }
        }
        let Some(rel) = slashed(rel, request.precompose_unicode) else {
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
    resolve_roots_with(root, requested, false)
}

fn is_hfs_ignorable(ch: char) -> bool {
    matches!(
        ch,
        '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'
            | '\u{206A}'..='\u{206F}'
            | '\u{FEFF}'
    )
}

fn is_git_metadata_component(part: &str) -> bool {
    let trimmed = part.trim_end_matches(['.', ' ']);
    let base = trimmed.split(':').next().unwrap_or(trimmed);
    if base.eq_ignore_ascii_case(".git") || base.eq_ignore_ascii_case("git~1") {
        return true;
    }
    if base.chars().any(is_hfs_ignorable) {
        let stripped: String = base.chars().filter(|c| !is_hfs_ignorable(*c)).collect();
        let stripped_trimmed = stripped.trim_end_matches(['.', ' ']);
        if stripped_trimmed.eq_ignore_ascii_case(".git")
            || stripped_trimmed.eq_ignore_ascii_case("git~1")
        {
            return true;
        }
    }
    false
}

fn is_devcouncil_dcgrep_pair(prev: &str, curr: &str) -> bool {
    prev.eq_ignore_ascii_case(".devcouncil") && curr.eq_ignore_ascii_case("dcgrep")
}

fn is_excluded_machinery_rel(rel: &str, include_devcouncil: bool) -> bool {
    let mut prev: Option<&str> = None;
    for part in rel.split(['/', '\\']) {
        if is_git_metadata_component(part) {
            return true;
        }
        if !include_devcouncil && part.eq_ignore_ascii_case(".devcouncil") {
            return true;
        }
        if include_devcouncil
            && let Some(prev_part) = prev
            && is_devcouncil_dcgrep_pair(prev_part, part)
        {
            return true;
        }
        prev = Some(part);
    }
    false
}

fn resolve_roots_with(
    root: &Path,
    requested: &str,
    include_devcouncil: bool,
) -> Result<(PathBuf, PathBuf), String> {
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
    // `ALWAYS_EXCLUDED` is applied by the walk as override globs relative to
    // the walk's target, so it can only exclude what lies *under* the target.
    // A target that is itself `.git` (or `src/../.git`, or a symlink to it, or
    // a worktree's `.git` file) was walked from the inside and every byte of it
    // returned. The canonical target is judged here instead, component by
    // component below the root, and case-folded because `.GIT` is `.git` on
    // APFS and NTFS.
    if let Ok(below) = target.strip_prefix(&root) {
        let mut prev: Option<&str> = None;
        for part in below.components() {
            let Some(part) = part.as_os_str().to_str() else {
                prev = None;
                continue;
            };
            if part.eq_ignore_ascii_case(".git") {
                return Err(format!(
                    "path {requested:?} resolves into `.git`, the repository's own machinery or the \
                     harness's state, which this tool never reads"
                ));
            }
            if !include_devcouncil && part.eq_ignore_ascii_case(".devcouncil") {
                return Err(format!(
                    "path {requested:?} resolves into `.devcouncil`, the repository's own machinery or the \
                     harness's state, which this tool never reads"
                ));
            }
            if include_devcouncil
                && let Some(prev_part) = prev
                && is_devcouncil_dcgrep_pair(prev_part, part)
            {
                return Err(format!(
                    "path {requested:?} resolves into `.devcouncil/dcgrep`, the repository's own machinery or the \
                     harness's state, which this tool never reads"
                ));
            }
            prev = Some(part);
        }
    }
    Ok((root, target))
}

#[derive(Clone, Copy)]
pub(crate) struct WalkOptions {
    pub apply_ignore_rules: bool,
    pub include_hidden: bool,
    pub include_devcouncil: bool,
    pub ignore_dot_ignore: bool,
}

fn target_inside_nested_git(root: &Path, target: &Path) -> bool {
    if target == root {
        return false;
    }
    let Ok(rel) = target.strip_prefix(root) else {
        return false;
    };
    let mut cur = root.to_path_buf();
    for comp in rel.components() {
        cur.push(comp);
        if fs::symlink_metadata(cur.join(".git")).is_ok() {
            return true;
        }
    }
    false
}

/// Builds the tree walk both operations use.
///
/// One function, because the alternative already bit: while `devcouncil_grep`
/// honoured ignore rules and `devcouncil_find_files` walked with a hardcoded
/// five-name skip list, `find_files` handed an agent `dist/generated.go` and
/// `grep` would never open it. Two readers of one repository with different
/// assumptions is how "the same tree" drifts apart, and an agent given both
/// answers has no way to tell which one is about the repository it is editing.
pub(crate) fn build_walker(
    root: &Path,
    target: &Path,
    apply_ignore_rules: bool,
    include_hidden: bool,
) -> Result<WalkBuilder, String> {
    build_walker_with(
        root,
        target,
        WalkOptions {
            apply_ignore_rules,
            include_hidden,
            include_devcouncil: false,
            ignore_dot_ignore: false,
        },
    )
}

pub(crate) fn build_walker_with(
    root: &Path,
    target: &Path,
    opts: WalkOptions,
) -> Result<WalkBuilder, String> {
    let mut overrides = OverrideBuilder::new(target);
    overrides
        .case_insensitive(true)
        .map_err(|err| format!("case-insensitive internal exclusions failed: {err}"))?;
    let excluded: &[&str] = if opts.include_devcouncil {
        &[".git"]
    } else {
        ALWAYS_EXCLUDED
    };
    for glob in excluded {
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

    // A standard checkout has a `.git` directory; a linked git worktree has a
    // `.git` file pointing to `$GIT_DIR` / `$GIT_COMMON_DIR`. Enabling
    // `require_git(true)` when `.git` exists at `root` both bounds parent
    // `.gitignore` traversal at `root` and lets `ignore` resolve
    // `$GIT_COMMON_DIR/info/exclude` for linked worktrees.
    let has_git_root = root.join(".git").exists();
    let ignore_case = if opts.apply_ignore_rules && has_git_root {
        git::read_ignore_case(root)
    } else {
        false
    };
    let include_devcouncil = opts.include_devcouncil;
    let prune_nested_git = opts.apply_ignore_rules && has_git_root;

    let mut walker = WalkBuilder::new(target);
    walker
        .overrides(overrides)
        // Never followed. A symlink out of the repository is the whole reason
        // containment is checked at all, and a link that points back inside is
        // reached through its real path anyway.
        .follow_links(false)
        .standard_filters(opts.apply_ignore_rules)
        .require_git(has_git_root)
        .git_ignore(opts.apply_ignore_rules)
        .git_global(opts.apply_ignore_rules)
        .git_exclude(opts.apply_ignore_rules)
        .ignore(opts.apply_ignore_rules && !opts.ignore_dot_ignore)
        .ignore_case_insensitive(ignore_case)
        .parents(apply_ignore_rules_and_git(
            opts.apply_ignore_rules,
            has_git_root,
        ))
        .hidden(opts.apply_ignore_rules && !opts.include_hidden)
        .filter_entry(move |entry| {
            if entry.depth() == 0 {
                return true;
            }
            if let Some(name) = entry.file_name().to_str() {
                if name.eq_ignore_ascii_case(".git") {
                    return false;
                }
                if !include_devcouncil && name.eq_ignore_ascii_case(".devcouncil") {
                    return false;
                }
                if include_devcouncil
                    && name.eq_ignore_ascii_case("dcgrep")
                    && let Some(parent_name) = entry
                        .path()
                        .parent()
                        .and_then(|p| p.file_name())
                        .and_then(|n| n.to_str())
                    && parent_name.eq_ignore_ascii_case(".devcouncil")
                {
                    return false;
                }
            }
            !(prune_nested_git
                && entry.file_type().is_some_and(|ft| ft.is_dir())
                && fs::symlink_metadata(entry.path().join(".git")).is_ok())
        });

    // Non-git trees (or synthetic scratch directories) keep `parents(false)` so
    // an arbitrary `.gitignore` above `root` cannot leak into the walk, while
    // subdirectory walks (`target != root`) still load `.gitignore`/`.ignore`
    // files strictly within `[root, target)`.
    if opts.apply_ignore_rules
        && !has_git_root
        && target != root
        && let Ok(rel) = target.strip_prefix(root)
    {
        let mut cur = root.to_path_buf();
        let mut ancestors = Vec::new();
        let mut components = rel.components().peekable();
        ancestors.push(cur.clone());
        while let Some(comp) = components.next() {
            if components.peek().is_none() {
                break;
            }
            cur.push(comp);
            ancestors.push(cur.clone());
        }
        let ignore_names: &[&str] = if opts.ignore_dot_ignore {
            &[".gitignore"]
        } else {
            &[".gitignore", ".ignore"]
        };
        for dir in ancestors {
            for name in ignore_names {
                let ignore_path = dir.join(name);
                if ignore_path.is_file() {
                    walker.current_dir(&dir);
                    let _ = walker.add_ignore(ignore_path);
                }
            }
        }
    }
    // Always anchor global gitignore matching at `root` rather than letting
    // `WalkBuilder` read the caller process's arbitrary working directory.
    walker.current_dir(root);

    Ok(walker)
}

fn apply_ignore_rules_and_git(apply_ignore_rules: bool, has_git_root: bool) -> bool {
    apply_ignore_rules && has_git_root
}

fn build_line_searcher() -> Searcher {
    SearcherBuilder::new()
        .line_number(true)
        // One line per match. Multi-line would let a single hit carry an
        // unbounded span of the file into the result.
        .multi_line(false)
        // Stop reading a file the moment it looks binary. The matches
        // already collected from it are then discarded — see `binary`.
        .binary_detection(BinaryDetection::quit(b'\x00'))
        .build()
}

fn tracked_dedup_key(rel: &str, ignore_case: bool) -> String {
    let normalized = if rel.is_ascii() {
        rel.to_string()
    } else {
        rel.nfc().collect::<String>()
    };
    if ignore_case {
        normalized.to_lowercase()
    } else {
        normalized
    }
}

fn rel_is_under_target(rel: &str, target_rel: &str, ignore_case: bool) -> bool {
    let rel_norm = tracked_dedup_key(rel, ignore_case);
    let target_norm = tracked_dedup_key(target_rel, ignore_case);
    rel_norm == target_norm
        || rel_norm
            .strip_prefix(&target_norm)
            .is_some_and(|rest| rest.starts_with('/'))
}

enum TrackedOpen {
    File(File),
    Skip,
    Unreadable,
}

fn open_tracked_regular_no_symlinks(root: &Path, raw_rel: &str) -> TrackedOpen {
    let mut cur = root.to_path_buf();
    let mut components = Path::new(raw_rel).components().peekable();
    while let Some(component) = components.next() {
        let is_last = components.peek().is_none();
        match component {
            Component::Normal(part) => cur.push(part),
            _ => return TrackedOpen::Skip,
        }
        let meta = match fs::symlink_metadata(&cur) {
            Ok(meta) => meta,
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) =>
            {
                return TrackedOpen::Skip;
            }
            Err(_) => return TrackedOpen::Unreadable,
        };
        if meta.file_type().is_symlink() {
            return TrackedOpen::Skip;
        }
        if !is_last && !meta.file_type().is_dir() {
            return TrackedOpen::Skip;
        }
        if is_last && !meta.file_type().is_file() {
            return TrackedOpen::Skip;
        }
    }
    match open_for_search(&cur) {
        Ok(file) => TrackedOpen::File(file),
        Err(err)
            if matches!(
                err.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            TrackedOpen::Skip
        }
        Err(_) => TrackedOpen::Unreadable,
    }
}

#[allow(clippy::too_many_arguments)]
fn search_opened_file(
    file: &File,
    raw_rel: &str,
    display_rel: &str,
    limit: usize,
    max_file_bytes: u64,
    max_line_bytes: usize,
    preserve_whitespace: bool,
    candidate_index: Option<&index::Candidates>,
    matcher: &grep_regex::RegexMatcher,
    searcher: &mut Searcher,
    shared: &Mutex<Collected>,
) {
    let Ok(meta) = file.metadata() else {
        tally(shared, |c| c.skipped.unreadable += 1);
        return;
    };
    if !meta.is_file() {
        tally(shared, |c| c.skipped.unreadable += 1);
        return;
    }
    if meta.len() >= max_file_bytes {
        tally(shared, |c| c.skipped.too_large += 1);
        return;
    }

    if let Some(candidates) = candidate_index {
        match candidates.verdict(raw_rel, &meta) {
            index::Candidate::Excluded => {
                tally(shared, |c| c.index_filtered += 1);
                return;
            }
            index::Candidate::Stale => {
                tally(shared, |c| c.index_stale += 1);
            }
            index::Candidate::Search => {}
        }
    }

    let mut local: Vec<Hit> = Vec::new();
    let mut overflowed = false;
    let mut binary = false;

    let outcome = searcher.search_file(
        matcher,
        file,
        Collector {
            path: display_rel,
            out: &mut local,
            limit,
            max_line_bytes,
            preserve_whitespace,
            truncated: &mut overflowed,
            binary: &mut binary,
        },
    );

    if binary {
        tally(shared, |c| c.skipped.binary += 1);
        return;
    }
    if outcome.is_err() {
        tally(shared, |c| c.skipped.unreadable += 1);
        return;
    }

    tally(shared, |c| {
        c.files_searched += 1;
        c.merge(local, overflowed, limit);
    });
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

/// A published path may be joined under the repository only when every
/// component is a normal, relative name.
///
/// This is an allow-list. A prefix check on the string would accept
/// `../outside` and an absolute path, and `O_NOFOLLOW` does not stop either:
/// an absolute path replaces the root, and `..` is an intermediate component.
pub(crate) fn contained_relative(path: &str) -> bool {
    if path.is_empty() || path.as_bytes().contains(&0) || path.contains('\\') {
        return false;
    }
    if path
        .split('/')
        .any(|seg| seg.is_empty() || seg == "." || seg == "..")
    {
        return false;
    }
    let path = Path::new(path);
    if path.is_absolute() {
        return false;
    }
    let mut normal = false;
    for component in path.components() {
        match component {
            Component::Normal(_) => normal = true,
            _ => return false,
        }
    }
    normal
}

/// Opens `path` for a metadata check only when it resolves inside the
/// canonical `root`.
///
/// Absolute paths and parent components are refused before any filesystem
/// call, so a missing `../outside` is `InvalidInput` and not `NotFound`. A
/// lexically contained path is canonicalized next, and a result outside the
/// root is refused before the open. The open is of that canonical path and
/// stays `O_NOFOLLOW` and `O_NONBLOCK`. A matching stamp is not an input.
///
/// The descriptor's own path is not read back: this crate forbids unsafe, and
/// the platform call that would do it is unsafe. The check is the canonical
/// path, then the open of that path.
pub(crate) fn open_inside_root(root: &Path, path: &str) -> io::Result<File> {
    if !contained_relative(path) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path escapes the repository",
        ));
    }
    let root = root.canonicalize()?;
    let joined = root.join(path);
    if !joined.starts_with(&root) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path escapes the repository",
        ));
    }
    let real = joined.canonicalize()?;
    if !real.starts_with(&root) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path escapes the repository",
        ));
    }
    open_for_search(&real)
}

/// Renders a repository-relative path with forward slashes on every platform,
/// so a result read on Windows names the same file a result read on Unix does.
///
/// `None` when any component is not valid UTF-8. Lossy conversion was the
/// alternative and it is worse than refusing: it yields a path containing
/// U+FFFD, which names no file on any filesystem, so the caller receives a
/// match it cannot open and has no way to tell that from a real one.
fn slashed(path: &Path, precompose_unicode: bool) -> Option<String> {
    let mut out = String::new();
    for component in path.components() {
        let part = component.as_os_str().to_str()?;
        if !out.is_empty() {
            out.push('/');
        }
        out.push_str(part);
    }
    if precompose_unicode && !out.is_ascii() {
        Some(out.nfc().collect())
    } else {
        Some(out)
    }
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
    preserve_whitespace: bool,
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
        let (line, line_truncated) =
            render_line(mat.bytes(), self.max_line_bytes, self.preserve_whitespace);
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
fn render_line(bytes: &[u8], max_line_bytes: usize, preserve_whitespace: bool) -> (String, bool) {
    let text = String::from_utf8_lossy(bytes);
    let trimmed = if preserve_whitespace {
        let s = text.as_ref().strip_suffix('\n').unwrap_or(text.as_ref());
        s.strip_suffix('\r').unwrap_or(s)
    } else {
        text.trim()
    };
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
                ..Default::default()
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

    #[test]
    fn an_escaping_published_path_is_refused_before_it_is_opened() {
        let scratch = Scratch::new();
        scratch.write("kept.rs", b"fn kept() {}\n");
        let absolute = open_inside_root(&scratch.path, "/etc/passwd").unwrap_err();
        assert_eq!(absolute.kind(), io::ErrorKind::InvalidInput);
        let parent = open_inside_root(&scratch.path, "../no-such-wave3-canary").unwrap_err();
        assert_eq!(
            parent.kind(),
            io::ErrorKind::InvalidInput,
            "a missing parent path is refused before open, not reported NotFound"
        );
        let file = open_inside_root(&scratch.path, "kept.rs").expect("a real file inside opens");
        assert!(file.metadata().unwrap().is_file());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_directory_does_not_open_outside_the_root() {
        let outside = Scratch::new();
        outside.write("secret.rs", b"fn secret() {}\n");
        let scratch = Scratch::new();
        std::os::unix::fs::symlink(&outside.path, scratch.path.join("link")).unwrap();
        let err = open_inside_root(&scratch.path, "link/secret.rs").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput, "{err}");
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
            include_ignored,
            ..Default::default()
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
            ..Default::default()
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
            max_results: 4,
            ..Default::default()
        })
        .expect("listing runs");
        assert_eq!(response.count, 4);
        assert!(response.truncated);
        assert_eq!(response.limit, Some(4));
    }

    /// A listing is used to enumerate, so its ceiling cannot be the search's.
    ///
    /// `MAX_MAX_RESULTS` bounds a *sample* a model reads, and 5,000 match lines
    /// is more than anything downstream wants. A listing answers "which files
    /// are in this repository", and for that question 5,000 is not a large
    /// number — this repository passed it during the writing of this test, at
    /// 5,408 files. Clamping the enumeration to the sampling ceiling made
    /// `dcgrep files` return a prefix under a flag no caller was obliged to
    /// read, and the callers that did read it were told to "pass a larger
    /// max_results" — advice that could not work, because the clamp was the
    /// thing refusing them.
    ///
    /// What made it costly rather than merely wrong: `dc-sparse-encode`
    /// asks this question to decide which files to encode. A prefix meant the
    /// files past the cut got no model weights and silently fell back to BM25
    /// inside an index reported as learned.
    #[test]
    fn a_listing_asked_for_everything_is_not_clamped_to_the_search_ceiling() {
        let scratch = Scratch::new();
        let count = MAX_MAX_RESULTS + 200;
        for i in 0..count {
            scratch.write(&format!("d{:03}/f{:05}.txt", i / 100, i), b"x\n");
        }
        let response = list_files(&ListRequest {
            root: scratch.path.clone(),
            max_results: MAX_MAX_RESULTS + 1_000,
            ..Default::default()
        })
        .expect("listing runs");
        assert!(
            !response.truncated,
            "a tree of {count} files truncated at {:?} although the caller \
             asked for {}",
            response.limit,
            MAX_MAX_RESULTS + 1_000
        );
        assert_eq!(response.count, count);
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
                    max_results: limit,
                    ..Default::default()
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
            include_ignored: true,
            ..Default::default()
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
                slashed(&bad, false),
                None,
                "a lossy path names no file and must never reach a caller"
            );
        }
        assert_eq!(
            slashed(Path::new("src/nested/a.rs"), false),
            Some("src/nested/a.rs".to_string())
        );
    }

    #[test]
    fn max_max_results_is_the_number_the_go_plane_mirrors() {
        // manvi/dc/dcgrep declares MaxListResults against the *listing*
        // ceiling so a caller asking for "everything" is not silently clamped
        // to less than it believes it asked for. The two are asserted equal
        // across the boundary in the Go tests; this pins the values they agree
        // on. MaxListResults mirrored MAX_MAX_RESULTS until a listing stopped
        // being a sample: 5,000 is the right bound on match lines and the
        // wrong one on file names.
        assert_eq!(MAX_MAX_RESULTS, 5_000);
        assert_eq!(MAX_LIST_RESULTS, 200_000);
    }

    // The three relationships between the listing ceiling, the search ceiling
    // and the ingest bound were asserted here. They are facts about constants,
    // so they now sit beside `MAX_LIST_RESULTS` as `const` assertions and fail
    // the build rather than a test run. What remains a test is the behaviour:
    // `a_listing_asked_for_everything_is_not_clamped_to_the_search_ceiling`
    // proves the walk honours them.

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
                    include_ignored,
                    ..Default::default()
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

    #[test]
    fn fixed_strings_matches_metacharacters_and_whitespace() {
        let scratch = Scratch::new();
        scratch.write("src/expr.txt", b"match (a+)+ and [ and -dash\n   \n");

        for literal in ["(a+)+", "[", "-dash", "   "] {
            let mut request = scratch.request(literal);
            request.fixed_strings = true;
            let response = search(&request).expect("fixed-string search runs");
            assert_eq!(
                response.count, 1,
                "expected 1 literal match for {literal:?}: {response:?}"
            );
            assert_eq!(response.matches[0].path, "src/expr.txt");
        }

        // Default regex mode still rejects `[` as invalid regex and `"   "` as empty.
        assert!(search(&scratch.request("[")).is_err());
        assert!(search(&scratch.request("   ")).is_err());
    }

    #[test]
    fn include_hidden_admits_dotfiles_but_keeps_gitignore_and_internal_exclusions() {
        let scratch = Scratch::new();
        scratch.write(".gitignore", b"*.ignored\n.hidden-ignored/\n");
        scratch.write(".github/ci.yml", b"needle\n");
        scratch.write(".hidden-ignored/secret.txt", b"needle\n");
        scratch.write("skip.ignored", b"needle\n");
        scratch.write(".git/HEAD", b"needle\n");
        scratch.write(".devcouncil/state.json", b"needle\n");
        scratch.write("visible.txt", b"needle\n");

        let mut request = scratch.request("needle");
        request.include_hidden = true;
        let mut matched: Vec<String> = search(&request)
            .expect("search runs")
            .matches
            .into_iter()
            .map(|m| m.path)
            .collect();
        matched.sort();
        assert_eq!(matched, vec![".github/ci.yml", "visible.txt"]);

        let mut listed = list_files(&ListRequest {
            root: scratch.path.clone(),
            include_hidden: true,
            ..Default::default()
        })
        .expect("listing runs")
        .paths;
        listed.sort();
        assert_eq!(listed, vec![".github/ci.yml", ".gitignore", "visible.txt"]);
    }

    #[test]
    fn walker_honors_linked_worktree_exclude_and_stops_above_non_git_root() {
        // 1. Linked worktree `.git` file pointing at gitdir + commondir with `info/exclude`.
        let outer = Scratch::new();
        outer.write("main-repo/.git/HEAD", b"ref: refs/heads/main\n");
        outer.write("main-repo/.git/info/exclude", b"*.wt_excluded\n");
        outer.write("main-repo/.git/worktrees/wt1/commondir", b"../..\n");
        let gitdir = outer.path.join("main-repo/.git/worktrees/wt1");
        outer.write(
            "wt1/.git",
            format!("gitdir: {}\n", gitdir.display()).as_bytes(),
        );
        outer.write("wt1/keep.txt", b"needle\n");
        outer.write("wt1/drop.wt_excluded", b"needle\n");

        let mut wt_req = Request {
            pattern: "needle".to_string(),
            root: outer.path.join("wt1"),
            ..Default::default()
        };
        wt_req.include_hidden = true;
        let wt_res = search(&wt_req).expect("worktree search runs");
        assert_eq!(paths(&wt_res), vec!["keep.txt"]);

        let wt_list = list_files(&ListRequest {
            root: outer.path.join("wt1"),
            include_hidden: true,
            ..Default::default()
        })
        .expect("worktree list runs");
        assert_eq!(wt_list.paths, vec!["keep.txt"]);

        // 2. Non-git tree does not inherit parent directory's `.gitignore` above `root`,
        //    yet walking a subdirectory (`path: "sub"`) still honors `root/.gitignore`.
        outer.write(".gitignore", b"keep.txt\n");
        outer.write("nongit/.gitignore", b"sub/skip.txt\n");
        outer.write("nongit/keep.txt", b"needle\n");
        outer.write("nongit/sub/ok.txt", b"needle\n");
        outer.write("nongit/sub/skip.txt", b"needle\n");

        let root_res = search(&Request {
            pattern: "needle".to_string(),
            root: outer.path.join("nongit"),
            ..Default::default()
        })
        .expect("nongit root search runs");
        let mut root_paths: Vec<_> = root_res.matches.into_iter().map(|m| m.path).collect();
        root_paths.sort();
        assert_eq!(root_paths, vec!["keep.txt", "sub/ok.txt"]);

        let sub_res = search(&Request {
            pattern: "needle".to_string(),
            root: outer.path.join("nongit"),
            path: "sub".to_string(),
            ..Default::default()
        })
        .expect("nongit sub search runs");
        assert_eq!(paths(&sub_res), vec!["sub/ok.txt"]);
    }

    #[test]
    fn preserve_whitespace_keeps_indentation_and_strips_crlf_or_lf() {
        let scratch = Scratch::new();
        scratch.write("indent.txt", b"  \tneedle\t \r\n    needle\n");

        let mut req = scratch.request("needle");
        req.preserve_whitespace = true;
        let res = search(&req).expect("search runs");
        assert_eq!(res.count, 2);
        assert_eq!(res.matches[0].line, "  \tneedle\t ");
        assert_eq!(res.matches[1].line, "    needle");

        let default_res = search(&scratch.request("needle")).expect("default search runs");
        assert_eq!(default_res.matches[0].line, "needle");
        assert_eq!(default_res.matches[1].line, "needle");
    }

    #[test]
    fn list_files_supports_oversized_symlinks_and_unicode_precomposition() {
        let scratch = Scratch::new();
        scratch.write("small.txt", b"hi\n");
        scratch.write("big.txt", &[b'a'; 128]);
        scratch.write("cafe\u{0301}.txt", b"hi\n");

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink("small.txt", scratch.path.join("link.txt")).expect("create valid symlink");
            symlink("missing.txt", scratch.path.join("dangling.txt"))
                .expect("create dangling symlink");
        }

        let listed = list_files(&ListRequest {
            root: scratch.path.clone(),
            max_file_bytes: 32,
            include_oversized: true,
            include_symlinks: true,
            precompose_unicode: true,
            ..Default::default()
        })
        .expect("listing runs");

        assert!(listed.paths.contains(&"big.txt".to_string()));
        assert!(listed.paths.contains(&"caf\u{00e9}.txt".to_string()));
        #[cfg(unix)]
        {
            assert!(listed.paths.contains(&"link.txt".to_string()));
            assert!(listed.paths.contains(&"dangling.txt".to_string()));
        }
        assert_eq!(listed.skipped.too_large, 0);
        assert_eq!(listed.skipped.unreadable, 0);

        // Also assert `precompose_unicode` on `slashed` directly (independent of OS normalization).
        assert_eq!(
            slashed(Path::new("dir/cafe\u{0301}.txt"), true),
            Some("dir/caf\u{00e9}.txt".to_string())
        );

        let mut search_req = scratch.request("hi");
        search_req.precompose_unicode = true;
        let search_res = search(&search_req).expect("search with precompose runs");
        assert!(
            search_res
                .matches
                .iter()
                .any(|m| m.path == "caf\u{00e9}.txt"),
            "expected precomposed search match path: {search_res:?}"
        );
    }

    fn run_git_helper(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("spawn git");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn include_devcouncil_admits_project_files_while_excluding_dcgrep_and_preserving_defaults() {
        let scratch = Scratch::new();
        scratch.write(
            ".gitignore",
            b".devcouncil/*\n!.devcouncil/config.yaml\n!.devcouncil/claude-plugin/\n",
        );
        scratch.write(".devcouncil/config.yaml", b"mode: needle\n");
        scratch.write(
            ".devcouncil/claude-plugin/devcouncil/agents/a.md",
            b"agent needle\n",
        );
        scratch.write(".devcouncil/ledger.sqlite", b"ignored needle\n");
        scratch.write(".devcouncil/dcgrep/meta.json", b"cache needle\n");
        scratch.write("src/main.rs", b"code needle\n");

        // Default caller (Manvi / Go client): `.devcouncil` stays excluded.
        let mut default_req = scratch.request("needle");
        default_req.include_hidden = true;
        let default_res = search(&default_req).expect("default search runs");
        assert_eq!(paths(&default_res), vec!["src/main.rs"]);
        assert!(
            search(&Request {
                pattern: "needle".into(),
                root: scratch.path.clone(),
                path: ".devcouncil".into(),
                ..Default::default()
            })
            .is_err()
        );

        // Explorer caller (`include_devcouncil: true`): admits un-ignored `.devcouncil/*`,
        // keeps `.devcouncil/ledger.sqlite` ignored and `.devcouncil/dcgrep` excluded even if
        // `include_ignored: true` or targeted directly.
        let mut explorer_req = scratch.request("needle");
        explorer_req.include_hidden = true;
        explorer_req.include_devcouncil = true;
        let mut matched: Vec<String> = search(&explorer_req)
            .expect("explorer search runs")
            .matches
            .into_iter()
            .map(|m| m.path)
            .collect();
        matched.sort();
        assert_eq!(
            matched,
            vec![
                ".devcouncil/claude-plugin/devcouncil/agents/a.md",
                ".devcouncil/config.yaml",
                "src/main.rs",
            ]
        );

        let mut listed = list_files(&ListRequest {
            root: scratch.path.clone(),
            include_hidden: true,
            include_devcouncil: true,
            ..Default::default()
        })
        .expect("explorer list runs")
        .paths;
        listed.sort();
        assert_eq!(
            listed,
            vec![
                ".devcouncil/claude-plugin/devcouncil/agents/a.md",
                ".devcouncil/config.yaml",
                ".gitignore",
                "src/main.rs",
            ]
        );

        // Even with `include_ignored: true` + `include_devcouncil: true`, `.devcouncil/dcgrep`
        // index blobs remain excluded and cannot be targeted directly.
        let mut ignored_req = scratch.request("needle");
        ignored_req.include_hidden = true;
        ignored_req.include_devcouncil = true;
        ignored_req.include_ignored = true;
        let ignored_paths: Vec<String> = search(&ignored_req)
            .expect("ignored explorer search runs")
            .matches
            .into_iter()
            .map(|m| m.path)
            .collect();
        assert!(
            ignored_paths
                .iter()
                .all(|p| !p.starts_with(".devcouncil/dcgrep")),
            "dcgrep cache leaked: {ignored_paths:?}"
        );
        assert!(ignored_paths.contains(&".devcouncil/ledger.sqlite".to_string()));

        assert!(
            search(&Request {
                pattern: "needle".into(),
                root: scratch.path.clone(),
                path: ".devcouncil".into(),
                include_devcouncil: true,
                include_hidden: true,
                ..Default::default()
            })
            .is_ok()
        );
        assert!(
            search(&Request {
                pattern: "needle".into(),
                root: scratch.path.clone(),
                path: ".devcouncil/dcgrep".into(),
                include_devcouncil: true,
                include_hidden: true,
                ..Default::default()
            })
            .is_err()
        );
    }

    #[test]
    fn ignore_dot_ignore_prevents_dot_ignore_from_overriding_gitignore() {
        let scratch = Scratch::new();
        scratch.write(".gitignore", b"*.ignored\nsecret/\n");
        scratch.write(".ignore", b"!keep.ignored\n!secret/\n");
        scratch.write("keep.ignored", b"needle\n");
        scratch.write("secret/data.txt", b"needle\n");
        scratch.write("src/ok.txt", b"needle\n");
        // Non-git nested subdirectory `.ignore` must also be suppressed when requested.
        scratch.write("sub/.ignore", b"!nested.ignored\n");
        scratch.write("sub/nested.ignored", b"needle\n");
        scratch.write("sub/ok.txt", b"needle\n");

        // Default `.ignore` behavior stays unchanged for existing callers.
        let default_res = search(&scratch.request("needle")).expect("default search runs");
        assert!(
            paths(&default_res).contains(&"keep.ignored"),
            "default dc-grep honors .ignore: {:?}",
            default_res.matches
        );

        let mut git_only_req = scratch.request("needle");
        git_only_req.ignore_dot_ignore = true;
        let git_only_res = search(&git_only_req).expect("ignore_dot_ignore search runs");
        assert_eq!(paths(&git_only_res), vec!["src/ok.txt", "sub/ok.txt"]);

        let mut sub_req = scratch.request("needle");
        sub_req.path = "sub".to_string();
        sub_req.ignore_dot_ignore = true;
        let sub_res = search(&sub_req).expect("sub search runs");
        assert_eq!(paths(&sub_res), vec!["sub/ok.txt"]);

        let mut listed = list_files(&ListRequest {
            root: scratch.path.clone(),
            ignore_dot_ignore: true,
            ..Default::default()
        })
        .expect("list runs")
        .paths;
        listed.sort();
        assert_eq!(listed, vec!["src/ok.txt", "sub/ok.txt"]);
    }

    #[test]
    fn untracked_nested_git_repos_and_worktrees_are_not_descended_inside_git_root() {
        let scratch = Scratch::new();
        scratch.write(".git/HEAD", b"ref: refs/heads/main\n");
        scratch.write("keep.txt", b"needle\n");
        scratch.write(".worktrees/wt1/.git", b"gitdir: /elsewhere/wt1\n");
        scratch.write(".worktrees/wt1/secret.rs", b"needle\n");
        scratch.write("nested_repo/.git/HEAD", b"ref: refs/heads/main\n");
        scratch.write("nested_repo/inside.rs", b"needle\n");

        let mut req = scratch.request("needle");
        req.include_hidden = true;
        let res = search(&req).expect("search runs");
        assert_eq!(paths(&res), vec!["keep.txt"]);

        let listed = list_files(&ListRequest {
            root: scratch.path.clone(),
            include_hidden: true,
            ..Default::default()
        })
        .expect("list runs");
        assert_eq!(listed.paths, vec!["keep.txt"]);

        // Direct subdir target inside an untracked nested git repo is also empty.
        let mut sub_req = scratch.request("needle");
        sub_req.path = "nested_repo".to_string();
        sub_req.include_hidden = true;
        let sub_res = search(&sub_req).expect("sub search runs");
        assert!(sub_res.matches.is_empty());
    }

    #[test]
    fn case_insensitive_machinery_core_ignorecase_and_crlf_anchors() {
        let scratch = Scratch::new();
        scratch.write(".git/HEAD", b"ref: refs/heads/main\n");
        // Verify comment-before-backslash and `]` inside quoted subsection name do not break `core.ignorecase`.
        scratch.write(
            ".git/config",
            b"[branch \"fix]1\"]\n\tremote = origin # comment \\\n[core]\n\tignorecase = true\n",
        );
        scratch.write(".gitignore", b"BUILD/\n");
        scratch.write("build/generated.txt", b"needle\n");
        scratch.write("sub/.GIT/secret.txt", b"needle\n");
        scratch.write("sub/.DEVCouncil/secret.txt", b"needle\n");
        scratch.write("crlf.txt", b"alpha needle\r\nneedle beta\r\n");

        let mut req = scratch.request("needle$");
        req.include_hidden = true;
        let res = search(&req).expect("crlf end-anchor search runs");
        assert_eq!(res.count, 1, "only CRLF end-of-line should match: {res:?}");
        assert_eq!(res.matches[0].path, "crlf.txt");
        assert_eq!(res.matches[0].line_number, 1);

        let mut all_req = scratch.request("needle");
        all_req.include_hidden = true;
        let all_res = search(&all_req).expect("all search runs");
        assert_eq!(paths(&all_res), vec!["crlf.txt", "crlf.txt"]);

        // `include_ignored: true` + `include_hidden: true` must still exclude `.GIT` and `.DEVCouncil`.
        let mut inc_ignored = scratch.request("needle");
        inc_ignored.include_hidden = true;
        inc_ignored.include_ignored = true;
        let inc_res = search(&inc_ignored).expect("ignored search runs");
        assert!(
            inc_res
                .matches
                .iter()
                .all(|m| !m.path.contains(".GIT") && !m.path.contains(".DEVCouncil")),
            "case-insensitive machinery leaked: {:?}",
            inc_res.matches
        );

        // With `core.ignorecase = false`, `BUILD/` must NOT ignore lowercase `build/`.
        scratch.write(".git/config", b"[core]\n\tignorecase = false\n");
        let case_sensitive_res = search(&all_req).expect("case-sensitive ignore search runs");
        let cs_paths = paths(&case_sensitive_res);
        assert!(
            cs_paths.contains(&"build/generated.txt"),
            "expected build/generated.txt when core.ignorecase = false: {cs_paths:?}"
        );
    }

    #[test]
    fn include_git_tracked_searches_tracked_gitignored_files_in_v3_and_v4_indexes() {
        for index_version in [3, 4] {
            let scratch = Scratch::new();
            run_git_helper(&scratch.path, &["init", "-q"]);
            run_git_helper(
                &scratch.path,
                &["config", "index.version", &index_version.to_string()],
            );
            scratch.write(".gitignore", b"AGENTS.md\nCLAUDE.md\nignored.txt\n");
            scratch.write("AGENTS.md", b"guide needle\n");
            scratch.write("CLAUDE.md", b"claude needle\n");
            scratch.write("ignored.txt", b"untracked needle\n");
            scratch.write("src/lib.rs", b"code needle\n");
            scratch.write("tracked_deleted.md", b"deleted needle\n");
            run_git_helper(
                &scratch.path,
                &["add", ".gitignore", "src/lib.rs", "tracked_deleted.md"],
            );
            run_git_helper(&scratch.path, &["add", "-f", "AGENTS.md", "CLAUDE.md"]);
            fs::remove_file(scratch.path.join("tracked_deleted.md")).unwrap();

            #[cfg(unix)]
            let outside = Scratch::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::symlink;
                outside.write("outside_target.txt", b"symlink needle\n");
                // Ignored + tracked-as-regular: replace on-disk file with symlink -> must NOT follow.
                fs::remove_file(scratch.path.join("CLAUDE.md")).unwrap();
                symlink(
                    outside.path.join("outside_target.txt"),
                    scratch.path.join("CLAUDE.md"),
                )
                .unwrap();
            }

            let mut req = scratch.request("needle");
            req.include_hidden = true;
            req.include_git_tracked = true;
            req.include_devcouncil = true;
            req.ignore_dot_ignore = true;
            let res = search(&req).expect("git tracked search runs");
            let mut matched = paths(&res);
            matched.sort();
            #[cfg(unix)]
            let expected = vec!["AGENTS.md", "src/lib.rs"];
            #[cfg(not(unix))]
            let expected = vec!["AGENTS.md", "CLAUDE.md", "src/lib.rs"];
            assert_eq!(matched, expected, "v{index_version} matches: {res:?}");
            assert_eq!(res.skipped.unreadable, 0);
        }
    }
}
