//! The ranked lexical tier: *which files are about this*, rather than *which
//! lines contain this*.
//!
//! The exact matcher answers a question with a yes or a no. This one answers
//! a question that has no yes: a reader asking about "json response parsing"
//! is not asking for a substring, and every file in the repository is a worse
//! or better answer rather than a match or a miss. So this returns files in
//! rank order, and the caller decides how far down to read.
//!
//! ## What it is built on
//!
//! One inverted index, written in the same pass and the same published slot as
//! the trigram index ([`crate::index`]), read under the same shared lock and
//! the same integrity stamps. The weights on disk are the document side of a
//! score; the query side is applied here. That split is the whole reason this
//! module can serve two very different rankers without a branch in the reader:
//!
//!   - **`code-v1`** — this crate's tokeniser, BM25 weights, no model, no
//!     download, nothing to configure. It is what a build produces today.
//!   - **`wordpiece-30522`** — the term space every
//!     `opensearch-neural-sparse-encoding-doc-*` model and SPLADE emit. The
//!     document weights come from running that encoder offline; the query side
//!     is the model's own token weights. Nothing in this module needs to
//!     change to read one, which is the point of storing the vocabulary in
//!     the file rather than assuming it.
//!
//! A reader that finds a vocabulary it cannot supply query weights for refuses
//! the index by name. Scoring model weights with BM25's query side would
//! produce a plausible ranking that means nothing, and no field of the answer
//! would say so.
//!
//! ## What it is not
//!
//! It is not a replacement for the exact matcher and it is not a summariser.
//! It proposes files. Anything that needs lines still reads them, which is
//! also why a hash collision in the term space is survivable here: the worst a
//! collision does is propose a file that turns out not to matter.

pub(crate) mod ingest;
pub(crate) mod store;
pub(crate) mod token;
pub(crate) mod wordpiece;

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::index::Published;
use crate::resolve_roots;
use store::{Reader, Vocabulary};

/// The term spaces a ranked index in this build may be published in.
///
/// A capability of the binary, not a property of any repository: `health` has
/// no root to ask. What one repository's index actually speaks is on every
/// `rank` response, which is where a caller should read it.
pub fn ranked_vocabularies() -> Vec<&'static str> {
    Vocabulary::ALL.iter().map(|v| v.as_str()).collect()
}

/// Where a ranked index's weights can come from, in the same order.
///
/// `code-v1` computes BM25 here from the repository's own text. A model
/// vocabulary carries weights a learned sparse encoder produced offline; no
/// model runs in this process for either.
pub fn ranked_engines() -> Vec<&'static str> {
    Vocabulary::ALL
        .iter()
        .map(|vocabulary| match vocabulary {
            Vocabulary::CodeV1 => "bm25",
            Vocabulary::WordPiece30522 => "learned-sparse",
        })
        .collect()
}

/// Ranked files returned when the caller does not say.
///
/// Smaller than the exact searcher's fifty because a ranked list is read from
/// the top: past twenty the scores are usually indistinguishable and the rows
/// are context spent on files nobody will open.
pub const DEFAULT_MAX_RANKED: usize = 20;

/// The ceiling on `max_results`, whatever the caller asks for.
pub const MAX_MAX_RANKED: usize = 1_000;

/// Longest query accepted, in bytes.
///
/// A ranked query is a phrase, not a program. The exact searcher's four
/// kilobytes exist because a regex can legitimately be long; nothing here can.
pub const MAX_QUERY_BYTES: usize = 1_024;

/// Distinct terms taken from one query.
///
/// Every term walks a posting list, so an unbounded query is an unbounded
/// number of list walks. Terms past this are reported, never dropped quietly.
pub const MAX_QUERY_TERMS: usize = 64;

/// One ranked search.
#[derive(Debug, Deserialize)]
pub struct RankedRequest {
    /// The words to rank by. Not a regular expression: it is tokenised by the
    /// same tokeniser that built the index, because a query split differently
    /// from the documents does not find fewer files, it finds wrong ones.
    pub query: String,
    /// The repository root. Every path is relative to it.
    pub root: PathBuf,
    /// Where under `root` to rank. Empty or "." means the whole repository.
    #[serde(default)]
    pub path: String,
    /// How many files to return before reporting truncation.
    #[serde(default)]
    pub max_results: usize,
}

/// One ranked file.
#[derive(Debug, Serialize)]
pub struct RankedHit {
    pub path: String,
    /// Higher is a better answer. Scores are comparable within one response
    /// and meaningless across two: they depend on the corpus, so a hit at 8.1
    /// here and one at 8.1 from another repository say nothing about each
    /// other. Deliberately not normalised to 0..1, which would imply exactly
    /// the comparison that does not hold.
    pub score: f32,
    /// How many of the query's terms this file holds at all.
    pub terms_present: usize,
    /// The indexed copy of this file is no longer what is on disk. The file
    /// is still returned — it was about the query when it was read — but a
    /// caller quoting it without reading it first would be quoting history.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub stale: bool,
}

/// The answer to one `RankedRequest`.
#[derive(Debug, Serialize)]
pub struct RankedResponse {
    pub ok: bool,
    pub query: String,
    pub count: usize,
    pub files: Vec<RankedHit>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    /// The term space the index speaks, echoed so a caller can tell a BM25
    /// ranking from a learned one without asking a second question.
    pub vocabulary: String,
    /// Documents the ranking drew on.
    pub files_indexed: usize,
    /// Postings in the index, so a caller can tell a ranking over a thin
    /// index from one over a whole repository. An index truncated by its
    /// posting ceiling ranks only the files that fit, and the count is the
    /// only thing in the answer that says so.
    pub index_postings: usize,
    /// Query terms the index had never seen. Not an error — a word nothing
    /// contains simply scores nothing — but a query where this equals
    /// `terms_total` returned zero files for a reason worth printing.
    pub terms_unknown: usize,
    pub terms_total: usize,
    /// Query terms past `MAX_QUERY_TERMS`, which were not used.
    #[serde(skip_serializing_if = "crate::is_zero_usize")]
    pub terms_dropped: usize,
    /// Returned files whose indexed copy no longer matches the disk.
    pub stale_files: usize,
}

/// Runs one ranked search.
///
/// Every `Err` names a fault, and "there is no index" is one of them. It is
/// deliberately not an empty result: a repository that has never been indexed
/// and a repository where nothing is about the query are different facts, and
/// the whole reason this crate exists is that a caller told the second when it
/// means the first will act on it.
pub fn ranked_search(request: &RankedRequest) -> Result<RankedResponse, String> {
    if request.query.trim().is_empty() {
        return Err("query is required".to_string());
    }
    if request.query.len() > MAX_QUERY_BYTES {
        return Err(format!(
            "query is {} bytes, over the {MAX_QUERY_BYTES}-byte limit — \
             this is not a negative result, no search ran",
            request.query.len()
        ));
    }
    let (root, target) = resolve_roots(&request.root, &request.path)?;
    let scope = target
        .strip_prefix(&root)
        .ok()
        .and_then(crate::slashed)
        .unwrap_or_default();

    let limit = match request.max_results {
        0 => DEFAULT_MAX_RANKED,
        n => n.min(MAX_MAX_RANKED),
    };

    let published = Published::open(&root).map_err(|err| {
        format!("no ranked index for this repository ({err}); build one with `dcgrep index`")
    })?;
    let bytes =
        crate::index::read_bounded(&published.slot.join("lexical.bin"), store::MAX_FILE_BYTES)?;
    let reader = Reader::open(bytes)?;

    // The query side is a property of the vocabulary. Both paths end in the
    // same `(term_id, query_weight)` list, and everything after this point is
    // shared — which is the point of storing the vocabulary rather than
    // inferring it. Applying one vocabulary's query side to the other's
    // document weights would rank confidently and mean nothing.
    let mut query_terms: HashMap<u32, f32> = HashMap::new();
    let mut terms_total = 0usize;
    let mut terms_dropped = 0usize;
    match reader.vocabulary() {
        Vocabulary::CodeV1 => {
            token::terms_of(&request.query, |term| {
                terms_total += 1;
                let id = token::term_id(term);
                if query_terms.len() >= MAX_QUERY_TERMS && !query_terms.contains_key(&id) {
                    terms_dropped += 1;
                    return;
                }
                *query_terms.entry(id).or_insert(0.0) += 1.0;
            });
            if query_terms.is_empty() {
                return Err(format!(
                    "query {:?} produced no indexable terms — every word was shorter than \
                     {} characters or longer than {} bytes, so no ranking was produced",
                    request.query,
                    token::MIN_TERM_CHARS,
                    token::MAX_TERM_BYTES
                ));
            }
        }
        Vocabulary::WordPiece30522 => {
            // No model runs here. The query is split by the vocabulary the
            // index carries, and each token's contribution is read from the
            // weight table beside it — which is the whole of what a doc-only
            // sparse encoder means by "inference-free at query time".
            for id in wordpiece::token_ids(&request.query, &reader) {
                terms_total += 1;
                if query_terms.len() >= MAX_QUERY_TERMS && !query_terms.contains_key(&id) {
                    terms_dropped += 1;
                    continue;
                }
                *query_terms.entry(id).or_insert(0.0) += 1.0;
            }
            if query_terms.is_empty() {
                return Err(format!(
                    "query {:?} produced no tokens in this index's vocabulary, so no \
                     ranking was produced",
                    request.query
                ));
            }
        }
    }

    // Summed in term order, not hash order. Floating-point addition is not
    // associative: the same three contributions added in two orders differ in
    // the last bits, and two files whose scores differ only there change
    // places between runs of the same query over the same index. The stress
    // harness found this before a caller did, and it is exactly the class of
    // bug that a test comparing only the *paths* of a ranking cannot see.
    let mut ordered: Vec<(u32, f32)> = query_terms.into_iter().collect();
    ordered.sort_unstable_by_key(|(term, _)| *term);

    let mut scores: HashMap<u32, (f32, usize)> = HashMap::new();
    let mut terms_unknown = 0usize;
    for (term, query_tf) in &ordered {
        let Some((postings, df)) = reader.postings_for(*term) else {
            terms_unknown += 1;
            continue;
        };
        // `code-v1` derives its query weight from how rare the term is;
        // a learned index reads the model's own. Either way it is one
        // number, applied the same way.
        let query_side = match reader.vocabulary() {
            Vocabulary::CodeV1 => reader.idf(df),
            Vocabulary::WordPiece30522 => reader.query_weight(*term),
        };
        if query_side <= 0.0 {
            // The model assigns this token nothing. Not unknown — the index
            // has it — simply weightless, which is most of the vocabulary for
            // any given query.
            continue;
        }
        for posting in postings {
            let entry = scores.entry(posting.file_id).or_insert((0.0, 0));
            entry.0 += query_side * posting.weight * query_tf;
            entry.1 += 1;
        }
    }

    // Scope, then rank. Filtering after scoring rather than before keeps the
    // scores comparable: IDF is a property of the whole corpus, and
    // recomputing it per subdirectory would make the same file score
    // differently depending on how the caller asked.
    let mut ranked: Vec<(String, f32, usize)> = Vec::new();
    for (file_id, (score, present)) in scores {
        let Some(path) = reader.path(file_id) else {
            continue;
        };
        if !scope.is_empty() && !in_scope(path, &scope) {
            continue;
        }
        if score > 0.0 {
            ranked.push((path.to_string(), score, present));
        }
    }

    // Descending by score, then ascending by path. The tie-break is not
    // cosmetic: BM25 produces exact ties routinely — two files holding the
    // same terms the same number of times — and without a second key the
    // order would come from a hash map's iteration order and change between
    // runs of the same query over the same index.
    ranked.sort_unstable_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    let truncated = ranked.len() > limit;
    ranked.truncate(limit);

    let evidence = tgrep_core::meta::read_file_evidence(&published.slot)
        .map_err(|err| format!("ranked index evidence is unreadable: {err}"))?;
    let mut stale_files = 0usize;
    let files: Vec<RankedHit> = ranked
        .into_iter()
        .map(|(path, score, terms_present)| {
            let stale = is_stale(&root, &path, &evidence);
            if stale {
                stale_files += 1;
            }
            RankedHit {
                path,
                score,
                terms_present,
                stale,
            }
        })
        .collect();

    Ok(RankedResponse {
        ok: true,
        query: request.query.clone(),
        count: files.len(),
        files,
        truncated,
        limit: truncated.then_some(limit),
        vocabulary: reader.vocabulary().to_string(),
        files_indexed: reader.files(),
        index_postings: reader.postings_len(),
        terms_unknown,
        terms_total,
        terms_dropped,
        stale_files,
    })
}

/// Whether `path` is inside `scope`, by path component and not by prefix.
///
/// A plain `starts_with` puts `src/lib_old.rs` inside a scope of `src/lib`,
/// which is a different directory and usually a different answer.
fn in_scope(path: &str, scope: &str) -> bool {
    path == scope
        || path
            .strip_prefix(scope)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Whether the file on disk still matches what the index read.
///
/// A file the index has no evidence for is not called stale: the index never
/// claimed to have read it, and reporting "changed" about something never seen
/// would be an assertion from an absence.
fn is_stale(root: &std::path::Path, path: &str, evidence: &tgrep_core::meta::FileEvidence) -> bool {
    let Some(recorded) = evidence.version(path) else {
        return false;
    };
    let Ok(file) = crate::open_for_search(&root.join(path)) else {
        // Gone, or no longer openable. It is not what the index read.
        return true;
    };
    match file.metadata() {
        Ok(metadata) => *recorded != tgrep_core::meta::file_version(&metadata),
        Err(_) => true,
    }
}
