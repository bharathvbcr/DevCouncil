//! Optional, bounded tgrep snapshots. The live walker still owns membership,
//! and a file is excluded only when its opened descriptor matches the version
//! read into the index. Missing evidence always means running the matcher.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File, Metadata, TryLockError};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tgrep_core::PostingEntry;
use tgrep_core::meta::{FileEvidence, file_version};
use tgrep_core::query::{build_query_plan, execute_plan};
use tgrep_core::reader::IndexReader;

use crate::lexical;
use crate::{DEFAULT_MAX_FILE_BYTES, build_walker, open_for_search, resolve_roots, slashed};

// Bumped from 1 when `lexical.bin` joined the slot. A published slot is
// identified by this number, and an older one is refused rather than read with
// a file missing from its integrity set — the check that would otherwise fail
// with "lexical.bin changed after publication" about a file that was never
// written.
//
// Bumped to 3 with `lexical::store::SCHEMA`, and it has to move whenever that
// does. The two numbers guard the same slot at different depths, and only this
// one is checked early enough to say something useful: an operator who upgrades
// the binary over an existing cache otherwise reaches `Reader::open` and is
// told "lexical index is schema 2, this build reads 3" in the middle of a
// query, where the answer they need is "rebuild with dcgrep index" — which is
// exactly what refusing here says.
//
// Bumped to 4 with `lexical::store::SCHEMA`, which gained the coverage fields
// `rank` reports. The pointer is checked first, so a cache written by the
// previous build is refused here with the remedy, not by `Reader::open`.
const CACHE_SCHEMA: u32 = 4;

// The slot's number must never fall behind the ranked index's.
//
// They guard the same published slot at different depths, and only
// `CACHE_SCHEMA` is read early enough to answer "rebuild with dcgrep index".
// If `lexical::store::SCHEMA` moves alone, an operator who upgrades over an
// existing cache gets `Reader::open`'s raw complaint in the middle of a query
// instead — a correct refusal that tells them nothing they can act on.
//
// Not equality: `CACHE_SCHEMA` has its own reasons to move, such as a seventh
// file joining `INDEX_FILES`. Only the direction that loses a usable error
// message is a fault.
//
// Checked at compile time rather than by a test, because a test can only fail
// after a binary with the wrong pairing has already been built.
const _: () = assert!(
    CACHE_SCHEMA >= crate::lexical::store::SCHEMA,
    "lexical::store::SCHEMA is ahead of CACHE_SCHEMA; bump CACHE_SCHEMA so a \
     stale slot is refused where the message is useful"
);

const MAX_INDEX_FILES: usize = 50_000;
const MAX_INDEX_INPUT_BYTES: u64 = 128 * 1024 * 1024;
const MAX_INDEX_DURATION: Duration = Duration::from_secs(30);
const MAX_CACHE_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// Bytes one posting takes in `index.bin`: `file_id` (4) + `loc_mask` (1) +
/// `next_mask` (1). tgrep keeps its own constant crate-private, so this one is
/// pinned to `PostingEntry::encode()` by a test rather than trusted.
const TRIGRAM_POSTING_BYTES: u64 = 6;
/// Bytes one entry takes in `lookup.bin`: `hash` (4) + `offset` (8) +
/// `length` (4). One per *distinct* trigram, which a posting list of one entry
/// can make equal to the number of postings, so for sizing this file's bound is
/// the postings' bound too.
const TRIGRAM_LOOKUP_BYTES: u64 = 16;

/// Postings the trigram index may hold, derived from the one bound that is not
/// ours to choose: a cache file larger than `MAX_CACHE_FILE_BYTES` is refused
/// by `Published::open`, and the build's own integrity pass fails on it first.
///
/// Both `index.bin` (6 bytes per posting) and `lookup.bin` (16 bytes per
/// distinct trigram, and distinct trigrams never exceed postings) must fit, so
/// the ceiling is the larger entry's quotient: 4,194,304. That is the most this
/// can hold with no input able to produce a file the reader will then refuse.
/// `index.bin` alone would allow 11,184,810, and a corpus of mostly distinct
/// trigrams would fail the build outright past the smaller figure.
///
/// This was 2,000,000, picked by eye, and it is the number that ended the walk
/// at 1,139 of this repository's 2,507 files.
const MAX_INDEX_POSTINGS: usize = {
    let entry = if TRIGRAM_POSTING_BYTES > TRIGRAM_LOOKUP_BYTES {
        TRIGRAM_POSTING_BYTES
    } else {
        TRIGRAM_LOOKUP_BYTES
    };
    (MAX_CACHE_FILE_BYTES / entry) as usize
};
const INDEX_FILES: &[&str] = &[
    "index.bin",
    "lookup.bin",
    "files.bin",
    "meta.json",
    "filestamps.json",
    // The ranked index rides in the same slot, under the same lock and the
    // same integrity stamps. A second cache directory with its own lifetime
    // would mean two things to invalidate and one of them eventually stale.
    "lexical.bin",
];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexRequest {
    pub root: PathBuf,
    /// Optional smaller file budget. Zero uses the built-in ceiling.
    #[serde(default)]
    pub max_files: usize,
    /// Output of a learned sparse encoder, run offline against this
    /// repository. Absent means the ranked index is built from this crate's
    /// own tokeniser and BM25, which needs no model and no Python.
    ///
    /// The two are alternatives, not layers: one `lexical.bin` is published
    /// either way, and it names which term space it speaks so a query can
    /// never be scored in the other one.
    #[serde(default)]
    pub sparse: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
pub struct IndexResponse {
    pub ok: bool,
    pub engine: &'static str,
    /// Files the walk reached. The trigram index's ceiling does not end the
    /// walk — the ranked index goes on — so this counts past it; only the
    /// walk's own ceilings (`files`, `duration`, `input_bytes`) stop the count.
    pub files_seen: usize,
    /// Files in the trigram index.
    pub files_indexed: usize,
    /// Files seen that are not in the trigram index, for any reason, the
    /// trigram ceiling included. A search still reads them.
    pub files_unindexed: usize,
    pub walk_errors: usize,
    /// No ceiling stopped the trigram index; this does not assert every file
    /// was indexable. It says nothing of the ranked index, which has its own
    /// `lexical_limit_reason` — the two ceilings are separate and either can
    /// bite alone.
    pub traversal_complete: bool,
    /// The ceiling that stopped the trigram index, if one did.
    pub limit_reason: Option<&'static str>,
    pub input_bytes: u64,
    pub postings: usize,
    pub cache_warning: Option<String>,
    /// Documents in the ranked index. Not bounded by `files_indexed`: the
    /// trigram index can fill first and the ranked one carries on. Lower than
    /// the files read whenever a file's text was too repetitive to tokenise
    /// whole.
    pub lexical_files: usize,
    pub lexical_postings: usize,
    /// Files the walk reached that are text this engine admits and that no
    /// document in the ranked index stands for: not openable, not readable, over
    /// the read bound and text by its first 8 KiB, changed while being read, a name that is not UTF-8, a
    /// document tokenised only in part (which would rank by a length it does not
    /// have, so it is counted rather than indexed short), or — with an imported
    /// encoding — a file the encoding did not describe. Binary and non-UTF-8
    /// files are not counted: they are not rankable by kind. Any nonzero value
    /// makes a `rank` answer `complete: false`.
    pub lexical_unindexed: usize,
    /// `lexical_unindexed` by cause, with the first paths of each, so a
    /// ranking that says `complete: false` can be told why without a second
    /// tool. Counts add up to `lexical_unindexed`; the sample is bounded.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub lexical_unindexed_by_class: BTreeMap<&'static str, LeftOut>,
    /// The ceiling that stopped the ranked build, if one did: its own
    /// (`lexical_postings`) or the walk's (`files`, `duration`, `input_bytes`).
    /// The same value a `rank` answer reports, because it is read back from the
    /// file that answer comes from.
    pub lexical_limit_reason: Option<&'static str>,
    /// Which term space the ranked index speaks.
    pub lexical_vocabulary: &'static str,
    /// The encoder that produced the ranked weights, when they were imported
    /// rather than computed here. Reported so a rebuild is attributable to a
    /// model; the ranking itself does not consult it, because both halves of
    /// a learned ranking are published in one file and cannot be mixed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lexical_model: Option<String>,
    /// Files the encoder described that this walk never admitted — deleted
    /// since the encoding ran, ignored, too large, or outside the repository.
    /// They are dropped, not indexed. A large number here means the encoding
    /// is stale, which is otherwise invisible: the build succeeds and the
    /// ranking is simply missing files nobody asked about.
    #[serde(skip_serializing_if = "is_zero")]
    pub lexical_unmatched: usize,
}

fn is_zero(value: &usize) -> bool {
    *value == 0
}

/// Most paths kept per cause in `lexical_unindexed_by_class`. A repository can
/// leave out thousands of files for one reason, and the response is a report,
/// not a listing; `count` is always the whole number.
const LEFT_OUT_SAMPLE: usize = 50;

/// One cause for leaving an admissible file out of the ranked index.
#[derive(Debug, Default, Serialize)]
pub struct LeftOut {
    pub count: usize,
    pub sample: Vec<String>,
}

impl IndexResponse {
    /// Records an admissible file the ranked index does not hold, and why.
    ///
    /// The one place `lexical_unindexed` is incremented, so the total and the
    /// per-cause counts cannot disagree.
    fn left_out(&mut self, class: &'static str, path: &str) {
        self.lexical_unindexed += 1;
        let entry = self.lexical_unindexed_by_class.entry(class).or_default();
        entry.count += 1;
        if entry.sample.len() < LEFT_OUT_SAMPLE {
            entry.sample.push(path.to_string());
        }
    }
}

#[derive(Debug, Serialize)]
pub struct SearchIndex {
    pub status: &'static str,
    pub reason: Option<String>,
    pub files_indexed: usize,
    pub files_filtered: u64,
    pub files_stale: u64,
}

impl SearchIndex {
    fn scan(reason: impl Into<String>) -> Self {
        Self {
            status: "scan",
            reason: Some(reason.into()),
            files_indexed: 0,
            files_filtered: 0,
            files_stale: 0,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Current {
    schema: u32,
    root: PathBuf,
    slot: String,
    files_indexed: usize,
}

/// The reader lock keeps an inactive slot from being recycled while a search
/// uses it. Locks are nonblocking: contention costs acceleration, never search.
pub(crate) struct Candidates {
    _lock: File,
    evidence: FileEvidence,
    absent: HashSet<String>,
}

/// One published slot, opened and proved.
///
/// Both readers of this cache — the trigram planner and the ranked index —
/// need the same four things established before they may believe a byte of
/// it: the shared lock, the pointer's schema and root, the slot directory,
/// and an integrity stamp per file matching what is on disk. They used to be
/// established in one of them, which meant the other either repeated them or
/// trusted them. Repeating them is how two readers come to disagree about
/// whether a cache is valid.
/// Why a published index could not be opened.
///
/// The two variants have opposite remedies, and collapsing them into one
/// string is how `rank` came to tell callers that a repository holding a
/// complete index had none — advising a build while a build was the thing
/// holding the lock. Typed rather than sniffed from the message, because the
/// caller that has to tell them apart is the one that phrases them.
pub(crate) enum OpenFault {
    /// A builder holds the lock. Nothing is wrong and nothing needs doing; the
    /// condition clears when that build publishes.
    Busy(String),
    /// No index, a schema that this build cannot read, or an artifact that
    /// failed its integrity check. None of these clear on their own.
    Unusable(String),
}

impl OpenFault {
    pub(crate) fn message(self) -> String {
        match self {
            OpenFault::Busy(message) | OpenFault::Unusable(message) => message,
        }
    }
}

pub(crate) struct Published {
    /// Held for as long as the caller reads. Dropping it republishes nothing;
    /// it only stops a build from recycling the slot mid-read.
    pub(crate) lock: File,
    pub(crate) slot: PathBuf,
    pub(crate) files_indexed: usize,
}

impl Published {
    pub(crate) fn open(root: &Path) -> Result<Self, OpenFault> {
        let cache = cache_directory(root, false).map_err(OpenFault::Unusable)?;
        let lock = open_for_search(&cache.join("cache.lock"))
            .map_err(|err| OpenFault::Unusable(error(err)))?;
        acquire_reader_lock(&lock)?;
        let current: Current =
            read_json(&cache.join("current.json"), 4096).map_err(OpenFault::Unusable)?;
        if current.schema != CACHE_SCHEMA
            || current.root != root
            || !matches!(current.slot.as_str(), "slot-a" | "slot-b")
        {
            return Err(OpenFault::Unusable(
                "cache identity or schema differs; rebuild with dcgrep index".into(),
            ));
        }
        let slot = cache.join(&current.slot);
        require_directory(&slot).map_err(OpenFault::Unusable)?;
        require_directory(&slot.join("integrity")).map_err(OpenFault::Unusable)?;
        bounded_regular_file(&slot.join("integrity/filestamps.json"), 16 * 1024)
            .map_err(OpenFault::Unusable)?;
        let integrity = tgrep_core::meta::read_file_evidence(&slot.join("integrity"))
            .map_err(|err| OpenFault::Unusable(error(err)))?;
        // Detect replaced, truncated, or edited artifacts before interpreting
        // their postings. The snapshots themselves are immutable while locked.
        for name in INDEX_FILES {
            let metadata = bounded_regular_file(&slot.join(name), MAX_CACHE_FILE_BYTES)
                .map_err(OpenFault::Unusable)?;
            if integrity.version(name) != Some(&file_version(&metadata)) {
                return Err(OpenFault::Unusable(format!(
                    "{name} changed after publication; rebuild with dcgrep index"
                )));
            }
        }
        Ok(Published {
            lock,
            slot,
            files_indexed: current.files_indexed,
        })
    }
}

impl Candidates {
    pub(crate) fn load(
        root: &Path,
        pattern: &str,
        case_insensitive: bool,
    ) -> (Option<Self>, SearchIndex) {
        // Unix ctime + inode catches rewrites with restored mtime. Other
        // platforms retain live search until equivalent evidence is available.
        if !cfg!(unix) {
            return (None, SearchIndex::scan("precise_file_versions_unavailable"));
        }
        // tgrep's HIR adapter and ripgrep's byte matcher differ for byte-mode
        // flags and Unicode case folding. Conservatively scan those patterns;
        // ordinary case-sensitive regexes still get trigram planning.
        if case_insensitive || pattern.contains("(?") {
            return (None, SearchIndex::scan("pattern_requires_live_scan"));
        }
        let plan = match build_query_plan(pattern, false) {
            Ok(plan) if !plan.is_match_all() => plan,
            _ => return (None, SearchIndex::scan("pattern_has_no_safe_trigrams")),
        };
        match Self::open(root, &plan) {
            Ok((candidates, count)) => (
                Some(candidates),
                SearchIndex {
                    status: "used",
                    reason: None,
                    files_indexed: count,
                    files_filtered: 0,
                    files_stale: 0,
                },
            ),
            Err(err) => (None, SearchIndex::scan(format!("index unavailable: {err}"))),
        }
    }

    fn open(root: &Path, plan: &tgrep_core::query::QueryPlan) -> Result<(Self, usize), String> {
        let Published {
            lock,
            slot,
            files_indexed,
        } = Published::open(root).map_err(OpenFault::message)?;
        let reader = IndexReader::open(&slot).map_err(error)?;
        reader.validate_lookup()?;
        if reader.num_files() != files_indexed {
            return Err("index file count disagrees with its manifest".into());
        }
        let mut evidence = tgrep_core::meta::read_file_evidence(&slot).map_err(error)?;
        // The slot's stamps also cover files only the ranked index reached,
        // which `rank` needs to call them stale. A search is about the trigram
        // snapshot, and for a file outside it "changed since the snapshot read
        // it" is not a claim the snapshot can make — so those stamps are not
        // shown to the planner, and `verdict` answers `Search` for them exactly
        // as it did before they existed. Skipped in the ordinary case, where
        // the two sets are the same size.
        if evidence.stamps.len() > reader.num_files() {
            let indexed: HashSet<&str> = reader.all_paths().iter().map(String::as_str).collect();
            evidence
                .stamps
                .retain(|path, _| indexed.contains(path.as_str()));
            evidence
                .versions
                .retain(|path, _| indexed.contains(path.as_str()));
            evidence
                .content_ids
                .retain(|path, _| indexed.contains(path.as_str()));
        }
        let matching: HashSet<u32> = execute_plan(plan, &|hash| reader.lookup_trigram(hash))
            .into_iter()
            .collect();
        let mut absent = HashSet::new();
        for (id, path) in reader.all_paths().iter().enumerate() {
            let id = u32::try_from(id).map_err(error)?;
            if !matching.contains(&id) && evidence.version(path).is_some() {
                absent.insert(path.clone());
            }
        }
        Ok((
            Self {
                _lock: lock,
                evidence,
                absent,
            },
            reader.num_files(),
        ))
    }

    pub(crate) fn verdict(&self, path: &str, metadata: &Metadata) -> Candidate {
        let Some(version) = self.evidence.version(path) else {
            return Candidate::Search;
        };
        if *version != file_version(metadata) {
            return Candidate::Stale;
        }
        if self.absent.contains(path) {
            return Candidate::Excluded;
        }
        Candidate::Search
    }
}

/// What the snapshot has to say about one file.
///
/// Returned rather than written through an `&mut SearchIndex` out-parameter,
/// because the callers that tally these now run concurrently and a shared
/// counter passed down into the decision is a lock held across it. The
/// decision is per-file and pure; only the tally is shared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Candidate {
    /// The snapshot proves this file cannot contain the pattern.
    Excluded,
    /// The file changed since the snapshot read it, so the snapshot has
    /// nothing to say and the matcher decides.
    Stale,
    /// No evidence either way. The matcher decides.
    Search,
}

/// Build a bounded snapshot with tgrep's extractor and on-disk writer. Using
/// the existing safe walker/open boundary avoids introducing a second file
/// admission policy or a library read that follows replacement symlinks.
pub fn build_index(request: &IndexRequest) -> Result<IndexResponse, String> {
    if request.max_files > MAX_INDEX_FILES {
        return Err(format!(
            "max_files exceeds the {MAX_INDEX_FILES}-file index ceiling"
        ));
    }
    let max_files = if request.max_files == 0 {
        MAX_INDEX_FILES
    } else {
        request.max_files
    };
    build_with_limits(
        request,
        max_files,
        MAX_INDEX_POSTINGS,
        lexical::store::MAX_POSTINGS,
        lexical::store::MAX_FILE_BYTES,
        MAX_INDEX_INPUT_BYTES,
        MAX_INDEX_DURATION,
    )
}

/// How much of an oversized file is read to tell text from a binary asset.
const OVERSIZED_PROBE_BYTES: u64 = 8 * 1024;

/// The admission rule for text: no NUL byte, no byte-order mark, valid UTF-8.
/// The one owner of it, applied to a whole file on the admitted path and to a
/// prefix of an oversized one (`truncated`), where a character cut off by the
/// end of the prefix is not evidence of anything.
fn is_admitted_text(bytes: &[u8], truncated: bool) -> bool {
    if bytes.contains(&0) || bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        return false;
    }
    match std::str::from_utf8(bytes) {
        Ok(_) => true,
        // Only a cut inside a multi-byte character is forgiven, and only when
        // the bytes are a prefix: the rest of that character is past the cut.
        Err(err) => truncated && err.error_len().is_none(),
    }
}

/// Whether an oversized file is text this engine would have admitted had it
/// been small enough, judged by its first `OVERSIZED_PROBE_BYTES`.
///
/// A file that cannot be probed counts as text: leaving it out of a ranking is
/// then reported as a shortfall, which is the safe direction, instead of being
/// assumed to be a binary asset nobody wanted ranked.
fn oversized_is_text(file: &File) -> bool {
    let mut prefix = Vec::new();
    match file.take(OVERSIZED_PROBE_BYTES).read_to_end(&mut prefix) {
        Ok(_) => is_admitted_text(&prefix, true),
        Err(_) => true,
    }
}

/// Records that the walk ended, and what ended it.
///
/// The ceiling is the trigram index's own too unless that index already filled
/// and stopped growing: `limit_reason` names what ended *it*, and after
/// `postings` nothing the walk meets later changes that.
fn walk_ended(
    limit: lexical::store::Limit,
    trigram_full: bool,
    response: &mut IndexResponse,
    walk_stop: &mut Option<lexical::store::Limit>,
) {
    *walk_stop = Some(limit);
    if !trigram_full {
        response.limit_reason = Some(limit.as_str());
    }
}

/// The ceilings are parameters so a test can make each one bite on a handful of
/// files. `max_postings` bounds the trigram index; `lexical_postings` and
/// `lexical_bytes` bound the ranked one, by posting count and by the size of the
/// file it would write. None of the three ends another index, and only
/// `max_files`, `max_bytes` and `max_duration` end the walk.
fn build_with_limits(
    request: &IndexRequest,
    max_files: usize,
    max_postings: usize,
    lexical_postings: usize,
    lexical_bytes: u64,
    max_bytes: u64,
    max_duration: Duration,
) -> Result<IndexResponse, String> {
    let (root, _) = resolve_roots(&request.root, "")?;
    if !root.is_dir() {
        return Err("index root must be a directory".into());
    }
    // Read and validate the encoding before the lock is taken and before a
    // single file is walked. A malformed encoding is the operator's mistake,
    // and finding it after a full-repository read would cost them the walk and
    // tell them nothing they could not have been told in milliseconds.
    let mut encoded = match &request.sparse {
        Some(path) => Some(lexical::ingest::read(path)?),
        None => None,
    };
    let cache = cache_directory(&root, true)?;
    let lock = open_cache_writer(&cache.join("cache.lock"), true)?;
    acquire_writer_lock(&lock)?;
    let pointer = cache.join("current.json");
    let (previous, cache_warning) = if pointer.try_exists().map_err(error)? {
        match read_json::<Current>(&pointer, 4096) {
            Ok(current) => (Some(current), None),
            Err(err) => (
                None,
                Some(format!("replaced unreadable cache pointer: {err}")),
            ),
        }
    } else {
        (None, None)
    };
    let slot_name = if previous.as_ref().is_some_and(|p| p.slot == "slot-a") {
        "slot-b"
    } else {
        "slot-a"
    };
    let slot = cache.join(slot_name);
    if slot.try_exists().map_err(error)? {
        require_directory(&slot)?;
        // Fixed private cache slots, never a caller-supplied deletion target.
        // The other slot remains published throughout this rebuild.
        fs::remove_dir_all(&slot).map_err(error)?;
    }
    fs::create_dir(&slot).map_err(error)?;

    let started = Instant::now();
    let mut paths = Vec::new();
    let mut inverted: HashMap<u32, Vec<PostingEntry>> = HashMap::new();
    let mut evidence = FileEvidence::default();
    let mut response = IndexResponse {
        ok: true,
        engine: "tgrep-core",
        files_seen: 0,
        files_indexed: 0,
        files_unindexed: 0,
        walk_errors: 0,
        traversal_complete: true,
        limit_reason: None,
        input_bytes: 0,
        postings: 0,
        cache_warning,
        lexical_files: 0,
        lexical_postings: 0,
        lexical_unindexed: 0,
        lexical_unindexed_by_class: BTreeMap::new(),
        lexical_limit_reason: None,
        lexical_vocabulary: match &encoded {
            Some(encoded) => encoded.vocabulary.as_str(),
            None => lexical::store::Vocabulary::CodeV1.as_str(),
        },
        lexical_model: encoded.as_ref().map(|encoded| encoded.model.clone()),
        lexical_unmatched: 0,
    };
    // Built from the same bytes, on the same pass. A separate walk would read
    // every file twice and — the part that actually bites — could disagree
    // with this one about which files exist, which is how two indexes of one
    // repository come to answer different questions about it.
    let mut lexical = lexical::store::Builder::with_ceiling(
        match &encoded {
            Some(encoded) => encoded.vocabulary,
            None => lexical::store::Vocabulary::CodeV1,
        },
        lexical_postings,
        lexical_bytes,
    );
    // The query half, stored before the first document. Without it the index
    // carries document weights in a learned term space and no way to weigh a
    // query in that space, which `Reader::open` refuses rather than letting it
    // publish. It is also a block of the file, so its bytes are reserved against
    // the budget before any document can spend them.
    if let Some(encoded) = encoded.as_mut() {
        lexical
            .set_query_side(
                std::mem::take(&mut encoded.vocab),
                std::mem::take(&mut encoded.query_weights),
            )
            .map_err(|err| format!("the encoder's query side cannot be stored: {err}"))?;
    }
    // The trigram index filled and stopped growing. The walk goes on for the
    // ranked index, which has a ceiling of its own and is not this one's to
    // end: the two used to share a `break`, and the ranked index stopped at
    // whatever file the trigram postings ran out on.
    let mut trigram_full = false;
    // What ended the walk, if anything did. Kept apart from `limit_reason`,
    // which names what ended the trigram index and keeps saying `postings`
    // after that index filled, whatever the walk meets later.
    let mut walk_stop: Option<lexical::store::Limit> = None;
    let mut lexical_terms: HashMap<u32, f32> = HashMap::new();
    // Which of the encoder's documents this walk actually reached. The
    // difference between this and the encoding's own count is how stale it is,
    // and reporting it is the only way that fact reaches anyone: a ranking
    // over a subset does not look different from a ranking over the whole.
    let mut lexical_used: usize = 0;
    for entry in build_walker(&root, true)?.build() {
        if started.elapsed() >= max_duration {
            walk_ended(
                lexical::store::Limit::Duration,
                trigram_full,
                &mut response,
                &mut walk_stop,
            );
            break;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                response.walk_errors += 1;
                continue;
            }
        };
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        response.files_seen += 1;
        // Nothing left to build. Counted so `files_seen` is the size of the
        // tree the walk admits, which is what a partial ranking is partial of;
        // not read, so the cost of that number is a directory walk.
        if trigram_full && lexical.is_full() {
            continue;
        }
        // Every `continue` below that is not a file kind this engine never
        // ranks is a file the ranking is missing, and is counted in
        // `lexical_unindexed` so `complete` cannot be true over it. The ones that
        // are not counted say why where they are: a file with a NUL byte or that
        // is not UTF-8 is not rankable, so leaving it out is not a shortfall.
        let Some(path) = entry.path().strip_prefix(&root).ok().and_then(slashed) else {
            // A name that is not UTF-8 cannot be written into the index.
            response.left_out("unrepresentable_name", &entry.path().to_string_lossy());
            continue;
        };
        let file = match open_for_search(entry.path()) {
            Ok(file) => file,
            Err(_) => {
                // Permission denied, vanished, a loop: the file exists and could
                // not be opened, which is not the same as having nothing to say.
                response.left_out("unopenable", &path);
                continue;
            }
        };
        let metadata = match file.metadata() {
            Ok(metadata) if metadata.is_file() && metadata.len() < DEFAULT_MAX_FILE_BYTES => {
                metadata
            }
            Ok(metadata) if metadata.is_file() => {
                // Over the read bound, so in neither index, as before. The one
                // question left is whether that leaves the ranking short of
                // text it should hold, and a large binary asset does not: it
                // is judged by its first bytes through the same rule the
                // admitted path applies.
                if oversized_is_text(&file) {
                    response.left_out("oversized_text", &path);
                }
                continue;
            }
            _ => {
                // No longer a regular file, or no metadata.
                response.left_out("not_a_regular_file", &path);
                continue;
            }
        };
        if response.input_bytes.saturating_add(metadata.len()) > max_bytes {
            walk_ended(
                lexical::store::Limit::InputBytes,
                trigram_full,
                &mut response,
                &mut walk_stop,
            );
            break;
        }
        let before = file_version(&metadata);
        if !before.is_trusted() {
            response.left_out("untrusted_version", &path);
            continue;
        }
        let mut bytes = Vec::new();
        let read = (&file).take(DEFAULT_MAX_FILE_BYTES).read_to_end(&mut bytes);
        response.input_bytes += bytes.len() as u64;
        if bytes.len() as u64 >= DEFAULT_MAX_FILE_BYTES || response.input_bytes > max_bytes {
            walk_ended(
                lexical::store::Limit::InputBytes,
                trigram_full,
                &mut response,
                &mut walk_stop,
            );
            break;
        }
        if read.is_err() {
            response.left_out("unreadable", &path);
            continue;
        }
        // Only unchanged plain UTF-8 is eligible for negative evidence. Other
        // encodings, NULs, and BOMs remain the live searcher's responsibility.
        // Not a shortfall: these are not text this engine ranks, and counting
        // every binary file would make `complete` unreachable. (A UTF-8 byte
        // order mark is caught here too, though it is text; that is a gap in
        // the admission rule, and `complete` is documented as relative to it.)
        if !is_admitted_text(&bytes, false) {
            continue;
        }
        // The file ceiling is checked where a file would be admitted, not for
        // every file the walk meets: a binary file that follows the last
        // admissible one is not a file the budget turned away, and calling it
        // one made a ranking of every rankable file report `files`. Documents
        // in either index count, so the budget bounds the ranked index once the
        // trigram one has stopped growing.
        if paths.len().max(lexical.documents()) >= max_files {
            walk_ended(
                lexical::store::Limit::Files,
                trigram_full,
                &mut response,
                &mut walk_stop,
            );
            break;
        }
        // Not extracted once that index is full: nothing would read them.
        let trigrams = (!trigram_full).then(|| tgrep_core::trigram::extract_merged_masks(&bytes));
        let after = match file.metadata() {
            Ok(meta) => file_version(&meta),
            Err(_) => {
                response.left_out("no_metadata_after_read", &path);
                continue;
            }
        };
        if before != after {
            // Changed while it was read, so the bytes in hand are not any
            // version of the file; the next build will see it.
            response.left_out("changed_while_read", &path);
            continue;
        }
        // Checked above: this byte slice is plain UTF-8 with no NUL and no
        // BOM, which is the same admission rule the trigram side applies.
        let mut ranked = false;
        if let Ok(text) = std::str::from_utf8(&bytes)
            && !lexical.is_full()
        {
            match &encoded {
                // Imported: the weights for this file were computed offline by
                // the model. The walk still decides *whether* it is indexed —
                // the encoding names paths, and a name in a file is not
                // permission to put a path in the index.
                Some(encoded) => match encoded.documents.get(&path) {
                    Some((total_terms, weights)) => {
                        lexical_used += 1;
                        ranked = lexical.add(path.clone(), *total_terms, weights);
                    }
                    // Reached by the walk, absent from the encoding: the file
                    // is newer than the model run. It is searchable and simply
                    // unranked, which `lexical_unindexed` already means.
                    None => response.left_out("not_in_encoding", &path),
                },
                // Computed here, from these bytes, on this pass.
                None => {
                    lexical_terms.clear();
                    let mut total_terms: u32 = 0;
                    let whole = lexical::token::terms_of(text, |term| {
                        *lexical_terms
                            .entry(lexical::token::term_id(term))
                            .or_insert(0.0) += 1.0;
                        total_terms = total_terms.saturating_add(1);
                    });
                    if whole {
                        let weights: Vec<(u32, f32)> =
                            lexical_terms.iter().map(|(id, tf)| (*id, *tf)).collect();
                        ranked = lexical.add(path.clone(), total_terms, &weights);
                    } else {
                        // The tokeniser stopped at its per-document ceiling, so
                        // this document's length is not its length. Ranking it
                        // against documents that were measured whole would put
                        // it wherever the ceiling happened to fall.
                        response.left_out("term_cap", &path);
                    }
                }
            }
        }
        // Outside the trigram index when it is full already, or when this file
        // is the one that fills it. Ranked files still get their version
        // stamped: `rank` calls a hit stale by comparing it, and a file with no
        // stamp is one it cannot say anything about. `Candidates::open` keeps
        // these stamps from the planner.
        let trigrams = match trigrams {
            Some(trigrams) if response.postings.saturating_add(trigrams.len()) <= max_postings => {
                trigrams
            }
            overflow => {
                if overflow.is_some() {
                    response.limit_reason = Some("postings");
                    trigram_full = true;
                }
                if ranked {
                    evidence.insert_verified(path, before.stamp().clone(), None, Some(before));
                }
                continue;
            }
        };
        let file_id = u32::try_from(paths.len()).map_err(error)?;
        response.postings += trigrams.len();
        for (hash, masks) in trigrams {
            inverted.entry(hash).or_default().push(PostingEntry {
                file_id,
                loc_mask: masks.loc_mask,
                next_mask: masks.next_mask,
            });
        }
        evidence.insert_verified(path.clone(), before.stamp().clone(), None, Some(before));
        paths.push(path);
    }
    response.files_indexed = paths.len();
    response.files_unindexed = response.files_seen - paths.len();
    response.traversal_complete = response.limit_reason.is_none();
    tgrep_core::builder::write_index_from_snapshot(
        &root,
        &slot,
        &paths,
        &inverted,
        response.traversal_complete && response.files_unindexed == 0 && response.walk_errors == 0,
    )
    .map_err(error)?;
    tgrep_core::meta::write_file_evidence(&evidence, &slot).map_err(error)?;
    let parity = match encoded {
        Some(encoded) => {
            response.lexical_unmatched = encoded.documents.len().saturating_sub(lexical_used);
            encoded.parity
        }
        None => Vec::new(),
    };
    response.lexical_files = lexical.documents();
    response.lexical_postings = lexical.postings();
    // Written into the file, so `rank` reports what this build reports and the
    // two cannot be read from different places.
    lexical.record_walk(
        response.files_seen,
        walk_stop,
        response.walk_errors,
        response.lexical_unindexed,
    );
    response.lexical_limit_reason = lexical.stopped_by().map(lexical::store::Limit::as_str);
    let rendered = lexical.finish();
    // Read back through the same validator a search will use, before anything
    // is published. A slot that fails to open later is a slot the searcher
    // must refuse, and refusing at search time means the failure surfaces to
    // whoever is mid-query rather than to whoever ran the build.
    let opened = lexical::store::Reader::open(rendered.clone())
        .map_err(|err| format!("the ranked index this build produced does not open: {err}"))?;
    // The gate ran once in `ingest` against the encoder's own header, which
    // proves this build's tokeniser matches the model. This runs it again
    // against the bytes about to be written, which proves the vocabulary
    // survived serialisation — a dropped or reordered token would resolve
    // every query to the wrong ids, and nothing else in the pipeline would
    // notice, because wrong ids return an empty ranking rather than an error.
    lexical::wordpiece::check_parity(&parity, &opened)?;
    drop(opened);
    let mut lexical_file = File::create_new(slot.join("lexical.bin")).map_err(error)?;
    lexical_file.write_all(&rendered).map_err(error)?;
    lexical_file.sync_all().map_err(error)?;
    drop(lexical_file);
    drop(rendered);
    drop(inverted);
    let check = IndexReader::open(&slot).map_err(error)?;
    check.validate_lookup()?;
    if check.num_files() != paths.len() {
        return Err("written index lost file entries".into());
    }
    drop(check);
    let mut integrity = FileEvidence::default();
    for name in INDEX_FILES {
        let file = open_cache_writer(&slot.join(name), false)?;
        file.sync_all().map_err(error)?;
        let version = file_version(&bounded_regular_file(
            &slot.join(name),
            MAX_CACHE_FILE_BYTES,
        )?);
        integrity.insert_verified((*name).into(), version.stamp().clone(), None, Some(version));
    }
    fs::create_dir(slot.join("integrity")).map_err(error)?;
    tgrep_core::meta::write_file_evidence(&integrity, &slot.join("integrity")).map_err(error)?;
    open_cache_writer(&slot.join("integrity/filestamps.json"), false)?
        .sync_all()
        .map_err(error)?;
    let pending = cache.join("current.tmp");
    match fs::remove_file(&pending) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(error(err)),
    }
    let mut publish = File::create_new(&pending).map_err(error)?;
    let current = Current {
        schema: CACHE_SCHEMA,
        root,
        slot: slot_name.into(),
        files_indexed: paths.len(),
    };
    publish
        .write_all(&serde_json::to_vec(&current).map_err(error)?)
        .map_err(error)?;
    publish.sync_all().map_err(error)?;
    drop(publish);
    fs::rename(pending, pointer).map_err(error)?;
    Ok(response)
}

/// Reads a whole file, refusing anything over `limit` rather than truncating.
///
/// The bound is checked by taking one byte past it: a read stopped exactly at
/// the limit cannot tell a file that fit from one that was cut, and a cut
/// index is very likely still structurally valid with fewer postings in it —
/// which would rank a repository by part of itself and say nothing.
pub(crate) fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let file =
        open_for_search(path).map_err(|err| format!("{} is unreadable: {err}", path.display()))?;
    let mut bytes = Vec::new();
    let read = (&file)
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|err| format!("{} could not be read: {err}", path.display()))?;
    if read as u64 > limit {
        return Err(format!(
            "{} is larger than the {limit}-byte limit",
            path.display()
        ));
    }
    Ok(bytes)
}

/// How long to keep trying for the writer lock before reporting it busy.
///
/// This absorbs a lock that is held by nobody. An flock belongs to the open
/// file description, so a process that holds this lock and then spawns a child
/// leaks a duplicate of the descriptor into it, and the lock outlives its
/// owner's `close()` until that child execs. `dcgrep` itself is single-shot
/// and never spawns anything, but `build_index` is public API: a host that
/// links it and spawns subprocesses on other threads sees a lock held by a
/// process that has already released it. Measured in this repository's own
/// test harness, which is exactly such a host: 1.7–2.5ms.
///
/// A real concurrent build holds the lock for the length of a build, which is
/// orders of magnitude past this, so waiting this long never queues behind one
/// — a genuine builder is still reported busy, just as before.
const WRITER_LOCK_SETTLES: Duration = Duration::from_millis(250);

/// Takes the writer lock, distinguishing contention from a lock that is broken.
///
/// Only `WouldBlock` is retried. An I/O error means locking is unavailable
/// here — a filesystem without it, a bad descriptor — and no amount of waiting
/// changes that, so it is reported at once and carries its own cause:
/// `TryLockError`'s own `Display` renders the I/O variant as the bare words
/// "lock acquisition failed due to I/O error" and drops the `io::Error` inside
/// it, which is the one thing a reader of that message needs.
fn acquire_writer_lock(lock: &File) -> Result<(), String> {
    let deadline = Instant::now() + WRITER_LOCK_SETTLES;
    loop {
        match lock.try_lock() {
            Ok(()) => return Ok(()),
            Err(TryLockError::Error(err)) => {
                return Err(format!("index busy or locking unavailable: {err}"));
            }
            Err(err @ TryLockError::WouldBlock) => {
                if Instant::now() >= deadline {
                    return Err(format!("index busy or locking unavailable: {err}"));
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }
}

/// Takes the shared read lock, distinguishing a running build from a broken
/// lock — and waiting briefly, because most contention is the publish itself.
///
/// A build holds this lock for the whole build, so waiting cannot outlast a
/// real one; what it does outlast is the slot flip, which is the window a
/// reader is overwhelmingly likely to land in. Stressing `rank` against
/// repeated republishes, 29% of reads were refused without this wait.
///
/// `WouldBlock` is the only thing retried. An I/O error means locking is
/// unavailable here and no amount of waiting changes it — and it must not be
/// reported as a running build, because that would tell a caller to wait for
/// something that is never going to happen.
fn acquire_reader_lock(lock: &File) -> Result<(), OpenFault> {
    let deadline = Instant::now() + WRITER_LOCK_SETTLES;
    loop {
        match lock.try_lock_shared() {
            Ok(()) => return Ok(()),
            Err(TryLockError::Error(err)) => {
                return Err(OpenFault::Unusable(format!(
                    "the index lock could not be taken: {err}"
                )));
            }
            Err(TryLockError::WouldBlock) => {
                if Instant::now() >= deadline {
                    return Err(OpenFault::Busy(
                        "a build is publishing this repository's index; \
                         retry the query in a moment"
                            .into(),
                    ));
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }
}

fn error(err: impl std::fmt::Display) -> String {
    err.to_string()
}

fn require_directory(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(error)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(format!(
            "cache path must be a real directory: {}",
            path.display()
        ));
    }
    Ok(())
}

fn cache_directory(root: &Path, create: bool) -> Result<PathBuf, String> {
    let mut path = root.to_path_buf();
    for part in [".devcouncil", "dcgrep"] {
        path.push(part);
        if create {
            match fs::create_dir(&path) {
                Ok(()) => {}
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
                Err(err) => return Err(error(err)),
            }
        }
        require_directory(&path)?;
    }
    Ok(path)
}

fn open_cache_writer(path: &Path, create: bool) -> Result<File, String> {
    let mut options = File::options();
    options
        .read(true)
        .write(true)
        .create(create)
        .truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path).map_err(error)?;
    if !file.metadata().map_err(error)?.is_file() {
        return Err("cache output is not a regular file".into());
    }
    Ok(file)
}

fn bounded_regular_file(path: &Path, limit: u64) -> Result<Metadata, String> {
    let metadata = open_for_search(path)
        .map_err(error)?
        .metadata()
        .map_err(error)?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err(format!(
            "cache file is not regular or exceeds {limit} bytes: {}",
            path.display()
        ));
    }
    Ok(metadata)
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path, limit: u64) -> Result<T, String> {
    bounded_regular_file(path, limit)?;
    let mut bytes = Vec::new();
    open_for_search(path)
        .map_err(error)?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(error)?;
    if bytes.len() as u64 > limit {
        return Err("cache JSON exceeded its read bound".into());
    }
    serde_json::from_slice(&bytes).map_err(error)
}

#[cfg(test)]
mod tests {
    use super::{IndexRequest, MAX_INDEX_DURATION, build_with_limits};
    use crate::lexical::store::MAX_FILE_BYTES as LEXICAL_BYTES;
    use crate::lexical::store::MAX_POSTINGS as LEXICAL_POSTINGS;

    // The CACHE_SCHEMA/store::SCHEMA ordering was asserted here. It is a fact
    // about two constants, so it now sits beside them as a `const` assertion
    // and fails the build rather than a test run.
    use std::fs;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    #[test]
    fn resource_caps_publish_partial_evidence_without_hiding_unindexed_matches() {
        let root = std::env::temp_dir().join(format!(
            "dcgrep-budget-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("hit.rs"), "needle\n").unwrap();
        let request = IndexRequest {
            root: root.clone(),
            max_files: 0,
            sparse: None,
        };
        for (postings, bytes, duration, reason) in [
            (0, 100, MAX_INDEX_DURATION, "postings"),
            (100, 0, MAX_INDEX_DURATION, "input_bytes"),
            (100, 100, Duration::ZERO, "duration"),
        ] {
            let built = build_with_limits(
                &request,
                100,
                postings,
                LEXICAL_POSTINGS,
                LEXICAL_BYTES,
                bytes,
                duration,
            )
            .unwrap();
            assert_eq!(built.files_indexed, 0);
            assert_eq!(built.limit_reason, Some(reason));
            assert!(!built.traversal_complete);
            let search = serde_json::from_value(serde_json::json!({
                "root": root, "pattern": "needle"
            }))
            .unwrap();
            assert_eq!(crate::search(&search).unwrap().count, 1, "{reason}");
        }
        fs::remove_dir_all(root).unwrap();
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "dcgrep-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        root
    }

    /// One word per file, shared with no other, so a query for it can only be
    /// answered by that file.
    const WORDS: [&str; 12] = [
        "alpaca", "badger", "caribou", "dingo", "echidna", "ferret", "gazelle", "hamster", "ibex",
        "jackal", "kestrel", "lemming",
    ];

    /// Twelve files, each about a word of its own. Which of them the walk
    /// reaches first is the filesystem's choice, so nothing below depends on it.
    fn twelve_files(root: &std::path::Path) {
        for (n, word) in WORDS.iter().enumerate() {
            fs::write(
                root.join(format!("f{n}.rs")),
                format!("fn {word}() {{ println!(\"{word} gathers {word} stripes\"); }}\n"),
            )
            .unwrap();
        }
    }

    fn rank(root: &std::path::Path, query: &str) -> crate::lexical::RankedResponse {
        crate::lexical::ranked_search(
            &serde_json::from_value(serde_json::json!({ "root": root, "query": query })).unwrap(),
        )
        .unwrap()
    }

    /// The trigram ceiling bounds the trigram index and nothing else.
    ///
    /// It used to `break` the whole walk, so the ranked index stopped at the
    /// same file and never reached a ceiling of its own: on this repository it
    /// covered 1,139 of 2,507 files while `lexical_limit_reason` read null.
    #[test]
    fn the_trigram_ceiling_does_not_stop_the_ranked_index() {
        let root = scratch("decoupled");
        twelve_files(&root);
        let request = IndexRequest {
            root: root.clone(),
            max_files: 0,
            sparse: None,
        };
        // Room for roughly three files' trigrams, and nowhere near twelve.
        let built = build_with_limits(
            &request,
            100,
            150,
            LEXICAL_POSTINGS,
            LEXICAL_BYTES,
            1 << 20,
            MAX_INDEX_DURATION,
        )
        .unwrap();
        assert_eq!(built.limit_reason, Some("postings"), "{built:?}");
        assert!(
            built.files_indexed > 0 && built.files_indexed < 12,
            "the trigram index must be partial for this to test anything: {built:?}"
        );
        assert_eq!(built.lexical_files, 12, "{built:?}");
        assert_eq!(built.files_seen, 12, "{built:?}");
        assert_eq!(built.lexical_limit_reason, None, "{built:?}");
        for (n, word) in WORDS.iter().enumerate() {
            let ranked = rank(&root, word);
            assert_eq!(ranked.count, 1, "{word}: {ranked:?}");
            assert_eq!(ranked.files[0].path, format!("f{n}.rs"), "{word}");
            // A ranking over every file the walk reached says so.
            assert!(ranked.complete, "{word}: {ranked:?}");
            assert_eq!(ranked.files_seen, 12, "{word}: {ranked:?}");
            assert_eq!(ranked.files_indexed, 12, "{word}: {ranked:?}");
            assert_eq!(ranked.lexical_limit_reason, None, "{word}: {ranked:?}");
        }
        fs::remove_dir_all(root).unwrap();
    }

    /// `search` was always right about a file the trigram index does not hold:
    /// no evidence means no exclusion, so the matcher reads it. This pins that
    /// across the change, because the files that now sit only in the ranked
    /// index are exactly the ones a careless stamp would start excluding.
    #[test]
    fn search_still_finds_a_match_in_a_file_past_the_trigram_ceiling() {
        let root = scratch("search-past");
        twelve_files(&root);
        let request = IndexRequest {
            root: root.clone(),
            max_files: 0,
            sparse: None,
        };
        let built = build_with_limits(
            &request,
            100,
            150,
            LEXICAL_POSTINGS,
            LEXICAL_BYTES,
            1 << 20,
            MAX_INDEX_DURATION,
        )
        .unwrap();
        assert!(built.files_indexed < 12, "{built:?}");
        for (n, word) in WORDS.iter().enumerate() {
            let found = crate::search(
                &serde_json::from_value(serde_json::json!({
                    "root": root, "pattern": format!("{word} gathers")
                }))
                .unwrap(),
            )
            .unwrap();
            assert_eq!(found.count, 1, "{word}: {found:?}");
            assert_eq!(found.matches[0].path, format!("f{n}.rs"), "{word}");
        }
        // Edit every file. Only the trigram index's files may be called stale
        // by a search: it is that snapshot's claim, and the ranked-only files
        // are not in it.
        for n in 0..12 {
            let path = root.join(format!("f{n}.rs"));
            let mut text = fs::read_to_string(&path).unwrap();
            text.push_str("// edited after the index was built\n");
            fs::write(&path, text).unwrap();
        }
        let found = crate::search(
            &serde_json::from_value(serde_json::json!({
                "root": root, "pattern": "gathers"
            }))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(found.count, 12, "{found:?}");
        assert_eq!(
            found.index.files_stale as usize, built.files_indexed,
            "a search reports staleness for the trigram snapshot's files only: {:?}",
            found.index
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// The other direction of the same decoupling: a ranked file the trigram
    /// index never held is still compared with the disk. Without a stamp for
    /// it `rank` could not call it stale, and would not say so.
    #[test]
    fn a_ranked_file_past_the_trigram_ceiling_is_still_called_stale_when_edited() {
        let root = scratch("stale-past");
        twelve_files(&root);
        let request = IndexRequest {
            root: root.clone(),
            max_files: 0,
            sparse: None,
        };
        build_with_limits(
            &request,
            100,
            150,
            LEXICAL_POSTINGS,
            LEXICAL_BYTES,
            1 << 20,
            MAX_INDEX_DURATION,
        )
        .unwrap();
        for (n, word) in WORDS.iter().enumerate() {
            assert!(!rank(&root, word).files[0].stale, "{word} before any edit");
            let path = root.join(format!("f{n}.rs"));
            let mut text = fs::read_to_string(&path).unwrap();
            text.push_str("// edited after the index was built\n");
            fs::write(&path, text).unwrap();
            let after = rank(&root, word);
            assert!(after.files[0].stale, "{word} after an edit: {after:?}");
            assert_eq!(after.stale_files, 1, "{word}");
        }
        fs::remove_dir_all(root).unwrap();
    }

    /// The ranked index's own ceiling is the only thing here that leaves it
    /// short of the tree, and the answer has to say so without arithmetic.
    #[test]
    fn a_ranking_over_a_partial_index_says_so_and_names_the_ceiling() {
        let root = scratch("partial");
        twelve_files(&root);
        let request = IndexRequest {
            root: root.clone(),
            max_files: 0,
            sparse: None,
        };
        // Postings for about four of the twelve one-word-per-file documents.
        let built = build_with_limits(
            &request,
            100,
            1 << 20,
            12,
            LEXICAL_BYTES,
            1 << 20,
            MAX_INDEX_DURATION,
        )
        .unwrap();
        assert_eq!(
            built.lexical_limit_reason,
            Some("lexical_postings"),
            "{built:?}"
        );
        assert!(
            built.lexical_files > 0 && built.lexical_files < 12,
            "{built:?}"
        );
        // The trigram index took everything: this is the ranked ceiling alone.
        assert_eq!(built.files_indexed, 12, "{built:?}");
        assert_eq!(built.limit_reason, None, "{built:?}");
        let mut ranked_somewhere = 0;
        for word in WORDS {
            let reply = rank(&root, word);
            assert!(!reply.complete, "{word}: {reply:?}");
            assert_eq!(
                reply.lexical_limit_reason,
                Some("lexical_postings"),
                "{word}: {reply:?}"
            );
            assert_eq!(reply.files_seen, 12, "{word}: {reply:?}");
            assert_eq!(
                reply.files_indexed, built.lexical_files,
                "{word}: {reply:?}"
            );
            assert!(reply.files_indexed < reply.files_seen, "{word}: {reply:?}");
            ranked_somewhere += reply.count;
        }
        // And the words in files past the ceiling find nothing — which is the
        // silent wrong answer `complete: false` exists to explain.
        assert_eq!(ranked_somewhere, built.lexical_files);
        fs::remove_dir_all(root).unwrap();
    }

    /// The walk's ceilings end the ranked build as surely as the trigram one,
    /// and used to leave `lexical_limit_reason` null while they did.
    #[test]
    fn a_walk_ceiling_is_reported_by_the_ranking_it_cut_short() {
        for (max_files, max_bytes, duration, reason) in [
            (5, 1 << 20, MAX_INDEX_DURATION, "files"),
            (100, 150, MAX_INDEX_DURATION, "input_bytes"),
            (100, 1 << 20, Duration::ZERO, "duration"),
        ] {
            let root = scratch("walk-stop");
            twelve_files(&root);
            let request = IndexRequest {
                root: root.clone(),
                max_files: 0,
                sparse: None,
            };
            let built = build_with_limits(
                &request,
                max_files,
                1 << 20,
                LEXICAL_POSTINGS,
                LEXICAL_BYTES,
                max_bytes,
                duration,
            )
            .unwrap();
            assert_eq!(built.limit_reason, Some(reason), "{built:?}");
            assert_eq!(built.lexical_limit_reason, Some(reason), "{built:?}");
            assert!(built.lexical_files < 12, "{reason}: {built:?}");
            if built.lexical_files > 0 {
                let word = WORDS
                    .iter()
                    .find(|word| rank(&root, word).count == 1)
                    .unwrap_or_else(|| panic!("{reason}: a ranked file is rankable"));
                let reply = rank(&root, word);
                assert!(!reply.complete, "{reason}: {reply:?}");
                assert_eq!(reply.lexical_limit_reason, Some(reason), "{reply:?}");
                assert_eq!(reply.files_seen, built.files_seen, "{reason}: {reply:?}");
            }
            fs::remove_dir_all(root).unwrap();
        }
    }

    /// Once the trigram index is full, the file budget still bounds the ranked
    /// one. It used to count trigram files only.
    #[test]
    fn the_file_budget_bounds_the_ranked_index_after_the_trigram_index_fills() {
        let root = scratch("budget-after-full");
        twelve_files(&root);
        let request = IndexRequest {
            root: root.clone(),
            max_files: 0,
            sparse: None,
        };
        let built = build_with_limits(
            &request,
            6,
            150,
            LEXICAL_POSTINGS,
            LEXICAL_BYTES,
            1 << 20,
            MAX_INDEX_DURATION,
        )
        .unwrap();
        assert_eq!(built.limit_reason, Some("postings"), "{built:?}");
        assert_eq!(built.lexical_limit_reason, Some("files"), "{built:?}");
        assert_eq!(built.lexical_files, 6, "{built:?}");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_admission_rule_forgives_only_a_character_cut_by_a_prefix() {
        use super::is_admitted_text;
        assert!(is_admitted_text(b"fn main() {}\n", false));
        assert!(!is_admitted_text(b"has a \0 nul", false));
        assert!(!is_admitted_text(b"\xef\xbb\xbfbom", false));
        assert!(!is_admitted_text(&[b'c', 0xe9, b' '], false), "latin-1");
        // "é" is 0xc3 0xa9. Cut after the first byte it is a prefix of text; as
        // a whole file it is not valid.
        assert!(is_admitted_text(&[b'c', b'a', b'f', 0xc3], true));
        assert!(!is_admitted_text(&[b'c', b'a', b'f', 0xc3], false));
        // A bad byte in the middle of a prefix is bad whatever the cut.
        assert!(!is_admitted_text(&[b'c', 0xe9, b' ', b'x'], true));
    }

    /// An oversized file is judged by its first bytes, and one that cannot be
    /// read at all is treated as text so the ranking reports a shortfall.
    #[test]
    fn an_oversized_file_is_classified_by_its_prefix_and_fails_closed() {
        use super::oversized_is_text;
        let root = scratch("oversized-prefix");
        let text = root.join("big.txt");
        fs::write(&text, "word ".repeat(4_000)).unwrap();
        assert!(oversized_is_text(&fs::File::open(&text).unwrap()));

        let binary = root.join("big.bin");
        let mut bytes = vec![0x89, b'P', b'N', b'G', 0, 0, 0];
        bytes.extend(std::iter::repeat_n(b'x', 20_000));
        fs::write(&binary, bytes).unwrap();
        assert!(!oversized_is_text(&fs::File::open(&binary).unwrap()));

        // Text whose first 8 KiB ends inside a multi-byte character is text.
        let cut = root.join("cut.txt");
        let mut bytes = "a".repeat(8 * 1024 - 1).into_bytes();
        bytes.extend("é".as_bytes());
        bytes.extend(std::iter::repeat_n(b'x', 20_000));
        fs::write(&cut, bytes).unwrap();
        assert!(oversized_is_text(&fs::File::open(&cut).unwrap()));

        // A descriptor that opens and cannot be read: a directory.
        let unreadable = fs::File::open(&root).unwrap();
        assert!(
            oversized_is_text(&unreadable),
            "an unreadable prefix must count as a shortfall"
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// A cache written before the header grew is refused where the message
    /// says what to do, not by `Reader::open` in the middle of a query.
    #[test]
    fn a_cache_from_the_previous_schema_is_refused_with_the_remedy() {
        let root = scratch("old-schema");
        twelve_files(&root);
        let request = IndexRequest {
            root: root.clone(),
            max_files: 0,
            sparse: None,
        };
        build_with_limits(
            &request,
            100,
            1 << 20,
            LEXICAL_POSTINGS,
            LEXICAL_BYTES,
            1 << 20,
            MAX_INDEX_DURATION,
        )
        .unwrap();
        let pointer = root.join(".devcouncil/dcgrep/current.json");
        let text = fs::read_to_string(&pointer).unwrap();
        assert!(
            text.contains(&format!("\"schema\":{}", super::CACHE_SCHEMA)),
            "{text}"
        );
        fs::write(
            &pointer,
            text.replace(
                &format!("\"schema\":{}", super::CACHE_SCHEMA),
                "\"schema\":3",
            ),
        )
        .unwrap();
        let err = crate::lexical::ranked_search(
            &serde_json::from_value(serde_json::json!({ "root": root, "query": "alpaca" }))
                .unwrap(),
        )
        .expect_err("a schema-3 cache must not be read");
        assert!(err.contains("rebuild with dcgrep index"), "{err}");
        assert_eq!(
            err.matches("dcgrep index").count(),
            1,
            "the remedy is said once, not appended to a cause that already gives it: {err}"
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// The trigram ceiling is derived, not chosen, and the derivation holds.
    ///
    /// The two constants are tgrep's own, kept crate-private there, so they are
    /// pinned against what it actually writes: one posting is the length of an
    /// encoded `PostingEntry`, and `lookup.bin` is sixteen bytes per distinct
    /// trigram.
    #[test]
    fn the_trigram_ceiling_fits_the_cache_file_bound_by_construction() {
        use super::{
            MAX_CACHE_FILE_BYTES, MAX_INDEX_POSTINGS, TRIGRAM_LOOKUP_BYTES, TRIGRAM_POSTING_BYTES,
        };
        assert_eq!(
            tgrep_core::PostingEntry {
                file_id: 0,
                loc_mask: 0,
                next_mask: 0
            }
            .encode()
            .len() as u64,
            TRIGRAM_POSTING_BYTES
        );

        // Worst case for each file at the ceiling: every posting its own
        // trigram for `lookup.bin`, every posting an entry for `index.bin`.
        let ceiling = MAX_INDEX_POSTINGS as u64;
        assert!(ceiling * TRIGRAM_POSTING_BYTES <= MAX_CACHE_FILE_BYTES);
        assert!(ceiling * TRIGRAM_LOOKUP_BYTES <= MAX_CACHE_FILE_BYTES);
        // And it is the largest such number, so nothing is left on the table.
        assert!((ceiling + 1) * TRIGRAM_LOOKUP_BYTES > MAX_CACHE_FILE_BYTES);
        assert_eq!(MAX_INDEX_POSTINGS, 4_194_304);

        // The entry sizes against a real build.
        let root = scratch("sizes");
        twelve_files(&root);
        let request = IndexRequest {
            root: root.clone(),
            max_files: 0,
            sparse: None,
        };
        let built = build_with_limits(
            &request,
            100,
            1 << 20,
            LEXICAL_POSTINGS,
            LEXICAL_BYTES,
            1 << 20,
            MAX_INDEX_DURATION,
        )
        .unwrap();
        let mut distinct = std::collections::HashSet::new();
        for n in 0..12 {
            let bytes = fs::read(root.join(format!("f{n}.rs"))).unwrap();
            for (hash, _) in tgrep_core::trigram::extract_merged_masks(&bytes) {
                distinct.insert(hash);
            }
        }
        let pointer: serde_json::Value = serde_json::from_slice(
            &fs::read(root.join(".devcouncil/dcgrep/current.json")).unwrap(),
        )
        .unwrap();
        let slot = root
            .join(".devcouncil/dcgrep")
            .join(pointer["slot"].as_str().unwrap());
        assert_eq!(
            fs::metadata(slot.join("index.bin")).unwrap().len(),
            built.postings as u64 * TRIGRAM_POSTING_BYTES
        );
        assert_eq!(
            fs::metadata(slot.join("lookup.bin")).unwrap().len(),
            distinct.len() as u64 * TRIGRAM_LOOKUP_BYTES
        );
        fs::remove_dir_all(root).unwrap();
    }
}
