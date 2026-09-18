//! Optional, bounded tgrep snapshots. The live walker still owns membership,
//! and a file is excluded only when its opened descriptor matches the version
//! read into the index. Missing evidence always means running the matcher.

use std::collections::{HashMap, HashSet};
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
const CACHE_SCHEMA: u32 = 3;

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
const MAX_INDEX_POSTINGS: usize = 2_000_000;
const MAX_INDEX_INPUT_BYTES: u64 = 128 * 1024 * 1024;
const MAX_INDEX_DURATION: Duration = Duration::from_secs(30);
const MAX_CACHE_FILE_BYTES: u64 = 64 * 1024 * 1024;
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
    pub files_seen: usize,
    pub files_indexed: usize,
    pub files_unindexed: usize,
    pub walk_errors: usize,
    /// The walk finished; this does not assert every file was indexable.
    pub traversal_complete: bool,
    pub limit_reason: Option<&'static str>,
    pub input_bytes: u64,
    pub postings: usize,
    pub cache_warning: Option<String>,
    /// Documents in the ranked index. Never more than `files_indexed`, and
    /// lower whenever a file's text was too repetitive to tokenise whole.
    pub lexical_files: usize,
    pub lexical_postings: usize,
    /// Files read for the trigram index but left out of the ranked one,
    /// because a document tokenised only in part would rank by a length it
    /// does not have. Counted rather than indexed short.
    pub lexical_unindexed: usize,
    /// The ceiling that stopped the ranked build, if one did.
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
        let evidence = tgrep_core::meta::read_file_evidence(&slot).map_err(error)?;
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
        MAX_INDEX_INPUT_BYTES,
        MAX_INDEX_DURATION,
    )
}

fn build_with_limits(
    request: &IndexRequest,
    max_files: usize,
    max_postings: usize,
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
    let encoded = match &request.sparse {
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
    let mut lexical = lexical::store::Builder::new(match &encoded {
        Some(encoded) => encoded.vocabulary,
        None => lexical::store::Vocabulary::CodeV1,
    });
    let mut lexical_terms: HashMap<u32, f32> = HashMap::new();
    // Which of the encoder's documents this walk actually reached. The
    // difference between this and the encoding's own count is how stale it is,
    // and reporting it is the only way that fact reaches anyone: a ranking
    // over a subset does not look different from a ranking over the whole.
    let mut lexical_used: usize = 0;
    for entry in build_walker(&root, true)?.build() {
        if started.elapsed() >= max_duration {
            response.limit_reason = Some("duration");
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
        if paths.len() >= max_files {
            response.limit_reason = Some("files");
            break;
        }
        let Some(path) = entry.path().strip_prefix(&root).ok().and_then(slashed) else {
            continue;
        };
        let file = match open_for_search(entry.path()) {
            Ok(file) => file,
            Err(_) => continue,
        };
        let metadata = match file.metadata() {
            Ok(metadata) if metadata.is_file() && metadata.len() < DEFAULT_MAX_FILE_BYTES => {
                metadata
            }
            _ => continue,
        };
        if response.input_bytes.saturating_add(metadata.len()) > max_bytes {
            response.limit_reason = Some("input_bytes");
            break;
        }
        let before = file_version(&metadata);
        if !before.is_trusted() {
            continue;
        }
        let mut bytes = Vec::new();
        let read = (&file).take(DEFAULT_MAX_FILE_BYTES).read_to_end(&mut bytes);
        response.input_bytes += bytes.len() as u64;
        if bytes.len() as u64 >= DEFAULT_MAX_FILE_BYTES || response.input_bytes > max_bytes {
            response.limit_reason = Some("input_bytes");
            break;
        }
        if read.is_err() {
            continue;
        }
        // Only unchanged plain UTF-8 is eligible for negative evidence. Other
        // encodings, NULs, and BOMs remain the live searcher's responsibility.
        if bytes.contains(&0)
            || bytes.starts_with(&[0xef, 0xbb, 0xbf])
            || std::str::from_utf8(&bytes).is_err()
        {
            continue;
        }
        let trigrams = tgrep_core::trigram::extract_merged_masks(&bytes);
        let after = match file.metadata() {
            Ok(meta) => file_version(&meta),
            Err(_) => continue,
        };
        if before != after {
            continue;
        }
        // Checked above: this byte slice is plain UTF-8 with no NUL and no
        // BOM, which is the same admission rule the trigram side applies.
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
                        if !lexical.add(path.clone(), *total_terms, weights) {
                            response.lexical_limit_reason =
                                lexical.limit().map(|limit| limit.as_str());
                        }
                    }
                    // Reached by the walk, absent from the encoding: the file
                    // is newer than the model run. It is searchable and simply
                    // unranked, which `lexical_unindexed` already means.
                    None => response.lexical_unindexed += 1,
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
                        if !lexical.add(path.clone(), total_terms, &weights) {
                            response.lexical_limit_reason =
                                lexical.limit().map(|limit| limit.as_str());
                        }
                    } else {
                        // The tokeniser stopped at its per-document ceiling, so
                        // this document's length is not its length. Ranking it
                        // against documents that were measured whole would put
                        // it wherever the ceiling happened to fall.
                        response.lexical_unindexed += 1;
                    }
                }
            }
        }
        if response.postings.saturating_add(trigrams.len()) > max_postings {
            response.limit_reason = Some("postings");
            break;
        }
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
            // The query half. Without it the index carries document weights in
            // a learned term space and no way to weigh a query in that space,
            // which `Reader::open` refuses below rather than letting it publish.
            lexical
                .set_query_side(encoded.vocab, encoded.query_weights)
                .map_err(|err| format!("the encoder's query side cannot be stored: {err}"))?;
            encoded.parity
        }
        None => Vec::new(),
    };
    response.lexical_files = lexical.documents();
    response.lexical_postings = lexical.postings();
    if response.lexical_limit_reason.is_none() {
        response.lexical_limit_reason = lexical.limit().map(|limit| limit.as_str());
    }
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
            let built = build_with_limits(&request, 100, postings, bytes, duration).unwrap();
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
}
