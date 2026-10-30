//! The on-disk shape of the ranked index, and the only code that reads it.
//!
//! One file, `lexical.bin`, written into the same published slot the trigram
//! index uses. It is deliberately not a second cache: the slot, the lock, the
//! two-phase publish and the integrity stamps already exist and already work,
//! and a ranked index that expired on a different schedule from the trigram
//! index would be a second thing to reason about every time either moved.
//!
//! ## Layout
//!
//! Every integer is little-endian and every offset is a count of elements, not
//! of bytes, so an offset cannot address a partial record.
//!
//! ```text
//! magic        8  bytes  "DCLEX\0\0\x01"
//! schema       u32       refuses anything but SCHEMA
//! vocabulary   u32       which term space the ids live in
//! files        u32
//! terms        u32
//! postings     u32
//! avg_doc_len  f32
//! vocab_size   u32       zero when the query side is computed, not stored
//! files_seen   u32       files the build's walk reached, indexed or not
//! stop         u32       why the build stopped short of the tree; 0 = it did not
//! doc_lens     files      x u32
//! paths        files      x (u32 length, that many UTF-8 bytes)
//! term_table   terms      x (u32 term_id, u32 first_posting, u32 posting_count)
//! postings     postings   x (u32 file_id, f32 weight)
//! vocab        vocab_size x (u32 length, that many UTF-8 bytes)
//! query_weights vocab_size x f32
//! vocab_order  vocab_size x u32       token ids, sorted by token text
//! ```
//!
//! The last two sections are what make a learned index self-contained, and
//! they are exactly what "inference-free at query time" means: a query is
//! turned into token ids by the stored vocabulary, and each id's contribution
//! is read from the stored weight table. No model runs. Both are indexed by
//! token id, so both are dense arrays and a lookup is an index rather than a
//! search.
//!
//! They are in this file rather than beside it because the two halves of one
//! ranking must not be replaceable independently. An index that took its
//! document weights from one model and its query weights from another would
//! rank confidently and mean nothing, and every score would still look
//! plausible. One file, one integrity stamp, one answer.
//!
//! The term table is sorted by `term_id` so a lookup is a binary search, and
//! `Reader::open` proves the sort rather than trusting it — an unsorted table
//! would make `postings_for` miss terms that are present, which is the failure
//! this whole crate exists to prevent: a wrong empty answer.
//!
//! ## What a weight means
//!
//! A posting's weight is the *document* side of the score and nothing else.
//! The query side is applied at search time. That split is what lets one
//! format carry two very different producers:
//!
//!   - [`Vocabulary::CodeV1`] stores the BM25 document factor, and the query
//!     side is the IDF this file can compute from `posting_count` and `files`.
//!   - [`Vocabulary::WordPiece30522`] stores whatever a learned sparse encoder
//!     produced for that document, and the query side is that model's own
//!     token weights. The ids are BERT WordPiece ids, shared by every
//!     `opensearch-neural-sparse-encoding-doc-*` model and by SPLADE.
//!
//! A reader that cannot supply the query side for the vocabulary it finds
//! refuses the index by name. It does not score it with the wrong weights and
//! it does not quietly return nothing.

use std::collections::HashSet;
use std::fmt;

/// Identifies the format, and the byte that changes when the layout does.
pub(crate) const MAGIC: [u8; 8] = *b"DCLEX\0\0\x01";

/// Bumped whenever the meaning of the bytes changes, including a change to
/// the tokeniser or the hash — both of which reassign every term id without
/// changing a single field.
///
/// Went to 2 when the query table was added. A schema-1 index has no query
/// section and is refused rather than read with the section assumed empty:
/// "empty" and "absent" would score identically for `code-v1` and differently
/// for a model, which is the kind of agreement that holds until it does not.
///
/// Went to 3 when `vocab_order` joined it. The reader used to build a
/// `HashMap<String, u32>` of the whole vocabulary at open — thirty thousand
/// string allocations, on every query, to serve about ten lookups. Measured by
/// holding the documents and postings fixed and varying only the vocabulary,
/// that cost 1.0 ms per query at 30522 tokens and 3.4 ms at the 100000
/// ceiling. The sorted index is four bytes per token on disk and turns a
/// lookup into a binary search over bytes already in memory.
///
/// Went to 4 when `files_seen` and `stop` joined the header. A ranking over a
/// partial index looked exactly like one over a whole repository — the only hint
/// was a posting count — because the file said nothing about how much of the
/// tree it had read. They are in this file, under its integrity stamp, rather
/// than in the slot's pointer, so the claim cannot be replaced independently of
/// the postings it describes.
pub(crate) const SCHEMA: u32 = 4;

/// Postings kept in one index.
///
/// A count cap and the reader's sanity bound on a claimed posting count — not
/// a bound on the file. Postings are eight bytes each, but the file also holds
/// twelve bytes per *distinct* term and a path per document, and a corpus whose
/// terms are mostly distinct reaches 80 MB at this many postings, over
/// `MAX_FILE_BYTES`. The bound on the file is the builder's byte budget, which
/// counts exactly what `finish` will write and stops at `Limit::Bytes`.
pub(crate) const MAX_POSTINGS: usize = 4_000_000;

/// Vocabulary entries kept in one index.
///
/// BERT WordPiece is 30522, so a model's whole vocabulary fits far inside
/// this. The ceiling exists for the file that claims more.
pub(crate) const MAX_VOCAB: usize = 100_000;

/// Longest single vocabulary token, in bytes.
pub(crate) const MAX_VOCAB_TOKEN_BYTES: usize = 64;

/// Largest `lexical.bin` this will read, and the budget the builder holds
/// itself to.
///
/// The writer cannot exceed it: `Builder::add` refuses the document that would
/// carry the file past it, with `Limit::Bytes`. The reader enforces it anyway,
/// because the file on disk is input and the process that wrote it is not
/// necessarily the process reading it.
pub(crate) const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// Fixed bytes at the front of the file: magic, then the nine u32/f32 header
/// fields. `Reader::open` reads exactly this many before anything else, and a
/// test holds this to what `finish` writes.
pub(crate) const HEADER_BYTES: u64 = 8 + 4 * 9;

/// BM25 term-frequency saturation.
pub(crate) const BM25_K1: f32 = 1.2;

/// BM25 length normalisation.
pub(crate) const BM25_B: f32 = 0.75;

/// Which term space the ids in an index belong to.
///
/// Stored, checked, and never inferred. Two indexes with the same ids and
/// different vocabularies score identically and mean nothing alike.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Vocabulary {
    /// This crate's own code tokeniser, ids interned by FNV-1a, weights BM25.
    CodeV1,
    /// BERT WordPiece, 30522 ids, weights from a learned sparse encoder.
    WordPiece30522,
}

impl Vocabulary {
    /// Every term space this build can read, in code order.
    ///
    /// One list, next to the enum, so a capability report cannot claim a
    /// vocabulary this binary does not have or omit one it does. A test below
    /// walks the codes and fails if a variant is added without joining it.
    pub(crate) const ALL: &'static [Vocabulary] = &[Vocabulary::CodeV1, Vocabulary::WordPiece30522];

    fn code(self) -> u32 {
        match self {
            Vocabulary::CodeV1 => 0,
            Vocabulary::WordPiece30522 => 1,
        }
    }

    fn from_code(code: u32) -> Option<Self> {
        match code {
            0 => Some(Vocabulary::CodeV1),
            1 => Some(Vocabulary::WordPiece30522),
            _ => None,
        }
    }

    /// The one name this term space goes by, on the wire and in a build
    /// report. `Display` renders the same string, so a rename cannot make the
    /// two disagree.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Vocabulary::CodeV1 => "code-v1",
            Vocabulary::WordPiece30522 => "wordpiece-30522",
        }
    }
}

impl fmt::Display for Vocabulary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why a build stopped short of the whole tree.
///
/// The first is this index's own ceiling. The other three are the walk's: they
/// end the ranked build as surely as they end the trigram one, so a ranking has
/// to be able to name them. Their strings are the ones `IndexResponse` already
/// reports as `limit_reason`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Limit {
    Postings,
    Files,
    Duration,
    InputBytes,
    /// The file this index would write has reached `MAX_FILE_BYTES`.
    Bytes,
}

impl Limit {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Limit::Postings => "lexical_postings",
            Limit::Files => "files",
            Limit::Duration => "duration",
            Limit::InputBytes => "input_bytes",
            Limit::Bytes => "lexical_bytes",
        }
    }

    /// Nonzero, because zero is "did not stop".
    fn code(self) -> u32 {
        match self {
            Limit::Postings => 1,
            Limit::Files => 2,
            Limit::Duration => 3,
            Limit::InputBytes => 4,
            Limit::Bytes => 5,
        }
    }

    fn from_code(code: u32) -> Option<Option<Self>> {
        match code {
            0 => Some(None),
            1 => Some(Some(Limit::Postings)),
            2 => Some(Some(Limit::Files)),
            3 => Some(Some(Limit::Duration)),
            4 => Some(Some(Limit::InputBytes)),
            5 => Some(Some(Limit::Bytes)),
            _ => None,
        }
    }
}

/// Accumulates documents, then renders the file.
///
/// Term frequencies are counted per document and folded into an inverted map
/// at the end rather than appended as they arrive, because BM25's document
/// factor needs the average document length — a number that is not known
/// until the last document has been seen.
pub(crate) struct Builder {
    vocabulary: Vocabulary,
    paths: Vec<String>,
    doc_lens: Vec<u32>,
    /// `(term_id, file_id, raw_weight)` — for `CodeV1` the raw weight is the
    /// term frequency, which becomes the BM25 factor in `finish`. For a model
    /// vocabulary it is the model's weight and passes through untouched.
    raw: Vec<(u32, u32, f32)>,
    /// The model's token strings, indexed by token id. Empty for `CodeV1`,
    /// whose terms are hashed and never stored.
    vocab: Vec<String>,
    /// The model's query weight per token id, parallel to `vocab`. Empty for
    /// `CodeV1`, whose query side is IDF computed from the postings.
    query_weights: Vec<f32>,
    /// This index's own ceiling, once it has bitten. Stops `add`.
    limit: Option<Limit>,
    /// Postings this builder will hold. `MAX_POSTINGS` outside a test, which
    /// lowers it to prove what happens when it bites without writing four
    /// million postings to do so.
    ceiling: usize,
    /// Exactly the length `finish` will return for what has been added so far:
    /// the header, the query-side block, and per document a doc length, a path
    /// and its length prefix, eight bytes per posting and twelve per distinct
    /// term. Kept exact rather than estimated, because the bound it enforces is
    /// the reader's refusal of a larger file, and an estimate that is ever low
    /// publishes an index nothing can open.
    bytes: u64,
    /// The most `bytes` may reach. `MAX_FILE_BYTES` outside a test.
    byte_ceiling: u64,
    /// Terms already counted at twelve bytes. Postings for a term seen before
    /// cost eight and not twenty.
    terms: HashSet<u32>,
    /// How many files the walk reached, and the walk's own ceiling if one
    /// ended it. Reported by the finished file; neither stops `add`.
    files_seen: usize,
    walk_stop: Option<Limit>,
}

impl Builder {
    /// A builder at the format's own ceiling. Production names its ceiling —
    /// `build_with_limits` takes it as a parameter — so only tests ask for this.
    #[cfg(test)]
    pub(crate) fn new(vocabulary: Vocabulary) -> Self {
        Self::with_ceiling(vocabulary, MAX_POSTINGS, MAX_FILE_BYTES)
    }

    pub(crate) fn with_ceiling(vocabulary: Vocabulary, ceiling: usize, byte_ceiling: u64) -> Self {
        Builder {
            vocabulary,
            paths: Vec::new(),
            doc_lens: Vec::new(),
            raw: Vec::new(),
            vocab: Vec::new(),
            query_weights: Vec::new(),
            limit: None,
            ceiling: ceiling.min(MAX_POSTINGS),
            bytes: HEADER_BYTES,
            byte_ceiling: byte_ceiling.min(MAX_FILE_BYTES),
            terms: HashSet::new(),
            files_seen: 0,
            walk_stop: None,
        }
    }

    /// Records how far the build's walk got, for the finished file to report.
    ///
    /// Separate from `limit`, which is this index's own and stops `add`: a
    /// walk that ended early did not make this index full, it made it short,
    /// and the two have different readers. `stop` is what ended the walk.
    pub(crate) fn record_walk(&mut self, files_seen: usize, stop: Option<Limit>) {
        self.files_seen = files_seen;
        self.walk_stop = stop;
    }

    /// Why the finished index covers less than the tree, if it does: its own
    /// ceiling first, because the walk ending afterwards is a consequence of
    /// nothing having been left to add, not a second cause.
    pub(crate) fn stopped_by(&self) -> Option<Limit> {
        self.limit.or(self.walk_stop)
    }

    /// Stores the query side of a learned ranking: the token strings and the
    /// weight each contributes when it appears in a query.
    ///
    /// Both are indexed by token id, so they must be the same length — an
    /// index where they were not would assign one token's weight to another's
    /// text, silently, for every query.
    ///
    /// Refused for `CodeV1`, whose query side is derived rather than supplied:
    /// an index carrying both would have two answers to the same question and
    /// no rule about which wins.
    ///
    /// Called before the first `add`, because its bytes are part of the file
    /// and a document admitted before they were counted could carry the file
    /// past `MAX_FILE_BYTES` once they were. Refused after, and refused twice.
    pub(crate) fn set_query_side(
        &mut self,
        vocab: Vec<String>,
        weights: Vec<f32>,
    ) -> Result<(), String> {
        if self.vocabulary == Vocabulary::CodeV1 {
            return Err("code-v1 derives its query weights and cannot be given them".into());
        }
        if !self.paths.is_empty() || !self.vocab.is_empty() {
            return Err(
                "the query side must be stored once, before the first document, so its \
                 bytes are reserved against the file's budget"
                    .into(),
            );
        }
        if vocab.len() != weights.len() {
            return Err(format!(
                "vocabulary has {} tokens and the query table has {} weights; \
                 they are indexed by the same id and must agree",
                vocab.len(),
                weights.len()
            ));
        }
        if vocab.len() > MAX_VOCAB {
            return Err(format!(
                "{} vocabulary tokens, over the {MAX_VOCAB} ceiling",
                vocab.len()
            ));
        }
        if vocab.is_empty() {
            return Err("a model vocabulary cannot be empty".into());
        }
        for (id, token) in vocab.iter().enumerate() {
            if token.is_empty() {
                return Err(format!("vocabulary token {id} is empty"));
            }
            if token.len() > MAX_VOCAB_TOKEN_BYTES {
                return Err(format!(
                    "vocabulary token {id} is {} bytes, over the \
                     {MAX_VOCAB_TOKEN_BYTES}-byte limit",
                    token.len()
                ));
            }
        }
        // A weight that cannot rank is stored as zero rather than refused: a
        // learned encoder legitimately assigns most of its vocabulary nothing,
        // and the dense table has to have an entry for every id regardless.
        self.query_weights = weights
            .into_iter()
            .map(|weight| {
                if weight.is_finite() && weight > 0.0 {
                    weight
                } else {
                    0.0
                }
            })
            .collect();
        // The token strings with their length prefixes, then a query weight and
        // an order entry for every id: what `finish` writes after the postings.
        let block: u64 = vocab
            .iter()
            .map(|token| 4 + token.len() as u64)
            .sum::<u64>()
            + 8 * vocab.len() as u64;
        if self.bytes + block > self.byte_ceiling {
            return Err(format!(
                "the query side alone is {block} bytes, which leaves no room under the \
                 {}-byte file budget",
                self.byte_ceiling
            ));
        }
        self.bytes += block;
        self.vocab = vocab;
        Ok(())
    }

    /// The length `finish` will return for what has been added, which the
    /// budget is held against and a test holds to the written file.
    #[cfg(test)]
    pub(crate) fn predicted_len(&self) -> u64 {
        self.bytes
    }

    pub(crate) fn is_full(&self) -> bool {
        self.limit.is_some()
    }

    /// This index's own ceiling alone; `stopped_by` is the answer a build
    /// reports, because it includes the walk's.
    #[cfg(test)]
    pub(crate) fn limit(&self) -> Option<Limit> {
        self.limit
    }

    pub(crate) fn documents(&self) -> usize {
        self.paths.len()
    }

    pub(crate) fn postings(&self) -> usize {
        self.raw.len()
    }

    /// Adds one document.
    ///
    /// `weights` is `(term_id, weight)` with no duplicate ids; for `CodeV1`
    /// the weight is a term count. `total_terms` is the document's length in
    /// terms including repeats, which is what BM25 normalises by — it is not
    /// the number of distinct terms and the two differ by a lot in code.
    ///
    /// Returns false when a ceiling stopped it — the posting count, or the
    /// file's byte budget — leaving the document out entirely rather than half
    /// in. A document with some of its terms indexed would rank below where it
    /// belongs and there would be nothing in the index to say so.
    pub(crate) fn add(&mut self, path: String, total_terms: u32, weights: &[(u32, f32)]) -> bool {
        if self.limit.is_some() {
            return false;
        }
        if self.raw.len().saturating_add(weights.len()) > self.ceiling {
            self.limit = Some(Limit::Postings);
            return false;
        }
        let Ok(file_id) = u32::try_from(self.paths.len()) else {
            self.limit = Some(Limit::Postings);
            return false;
        };
        // A non-finite weight would poison every score the term takes part in
        // and would compare false against every threshold, so it is dropped
        // here where the cause is still visible — and costs nothing, because
        // nothing is written for it.
        let kept = weights
            .iter()
            .filter(|(_, weight)| weight.is_finite() && *weight > 0.0);
        // Counted by inserting, and undone if the document is refused: a term
        // new to the index costs a term-table entry as well as its posting, and
        // two postings for one new term in the same document cost one entry.
        let mut fresh: Vec<u32> = Vec::new();
        let mut postings = 0u64;
        for (term, _) in kept.clone() {
            postings += 1;
            if self.terms.insert(*term) {
                fresh.push(*term);
            }
        }
        let cost = 8 + path.len() as u64 + 8 * postings + 12 * fresh.len() as u64;
        if self.bytes + cost > self.byte_ceiling {
            for term in &fresh {
                self.terms.remove(term);
            }
            self.limit = Some(Limit::Bytes);
            return false;
        }
        self.bytes += cost;
        for (term, weight) in kept {
            self.raw.push((*term, file_id, *weight));
        }
        self.paths.push(path);
        self.doc_lens.push(total_terms);
        true
    }

    /// Renders the index.
    pub(crate) fn finish(mut self) -> Vec<u8> {
        let files = self.paths.len();
        let total_len: u64 = self.doc_lens.iter().map(|len| u64::from(*len)).sum();
        let avg_doc_len = if files == 0 {
            0.0
        } else {
            (total_len as f64 / files as f64) as f32
        };

        if self.vocabulary == Vocabulary::CodeV1 {
            let avg = if avg_doc_len > 0.0 { avg_doc_len } else { 1.0 };
            for (_, file_id, weight) in &mut self.raw {
                let doc_len = f32::from(
                    u16::try_from(self.doc_lens[*file_id as usize].min(u32::from(u16::MAX)))
                        .unwrap_or(u16::MAX),
                );
                let tf = *weight;
                let norm = BM25_K1 * (1.0 - BM25_B + BM25_B * (doc_len / avg));
                *weight = (tf * (BM25_K1 + 1.0)) / (tf + norm);
            }
        }

        // Sorted by term, then by file, so the term table is contiguous and a
        // posting list is in ascending file order — which lets a reader stop
        // early and makes two builds of the same tree byte-identical.
        self.raw
            .sort_unstable_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

        let mut term_table: Vec<(u32, u32, u32)> = Vec::new();
        for (index, (term, _, _)) in self.raw.iter().enumerate() {
            match term_table.last_mut() {
                Some((last, _, count)) if last == term => *count += 1,
                _ => term_table.push((*term, index as u32, 1)),
            }
        }

        let mut out = Vec::with_capacity(
            64 + self.raw.len() * 8 + self.vocab.iter().map(|t| t.len() + 8).sum::<usize>(),
        );
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&SCHEMA.to_le_bytes());
        out.extend_from_slice(&self.vocabulary.code().to_le_bytes());
        out.extend_from_slice(&(files as u32).to_le_bytes());
        out.extend_from_slice(&(term_table.len() as u32).to_le_bytes());
        out.extend_from_slice(&(self.raw.len() as u32).to_le_bytes());
        out.extend_from_slice(&avg_doc_len.to_le_bytes());
        out.extend_from_slice(&(self.vocab.len() as u32).to_le_bytes());
        // Never below the documents it holds: a walk that saw fewer files than
        // the index contains is a build that mislabelled itself.
        let seen = u32::try_from(self.files_seen.max(files)).unwrap_or(u32::MAX);
        out.extend_from_slice(&seen.to_le_bytes());
        out.extend_from_slice(&self.stopped_by().map_or(0, Limit::code).to_le_bytes());
        for len in &self.doc_lens {
            out.extend_from_slice(&len.to_le_bytes());
        }
        for path in &self.paths {
            out.extend_from_slice(&(path.len() as u32).to_le_bytes());
            out.extend_from_slice(path.as_bytes());
        }
        for (term, first, count) in &term_table {
            out.extend_from_slice(&term.to_le_bytes());
            out.extend_from_slice(&first.to_le_bytes());
            out.extend_from_slice(&count.to_le_bytes());
        }
        for (_, file_id, weight) in &self.raw {
            out.extend_from_slice(&file_id.to_le_bytes());
            out.extend_from_slice(&weight.to_le_bytes());
        }
        for token in &self.vocab {
            out.extend_from_slice(&(token.len() as u32).to_le_bytes());
            out.extend_from_slice(token.as_bytes());
        }
        for weight in &self.query_weights {
            out.extend_from_slice(&weight.to_le_bytes());
        }
        // Token ids in the order their text sorts. Written here rather than
        // derived at open because deriving it is the cost this exists to
        // remove, and because a sort proved once by the writer can be *checked*
        // by the reader in one pass without allocating anything.
        let mut order: Vec<u32> = (0..self.vocab.len() as u32).collect();
        order.sort_unstable_by(|a, b| self.vocab[*a as usize].cmp(&self.vocab[*b as usize]));
        for id in &order {
            out.extend_from_slice(&id.to_le_bytes());
        }
        out
    }
}

/// One posting: a file and the document side of its score for some term.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Posting {
    pub(crate) file_id: u32,
    pub(crate) weight: f32,
}

/// A validated index, held in memory.
///
/// Every field is checked once here, at open, so the accessors below cannot
/// be handed an offset that runs off the end. The alternative — checking in
/// the accessors — puts the bounds test on the hot path and, worse, leaves
/// "this index is usable" a thing no single place decides.
pub(crate) struct Reader {
    bytes: Vec<u8>,
    vocabulary: Vocabulary,
    files: usize,
    terms: usize,
    postings: usize,
    term_table_at: usize,
    postings_at: usize,
    path_offsets: Vec<(usize, usize)>,
    /// Where each token's length prefix starts, indexed by token id.
    ///
    /// One bulk allocation, not one per token. This replaced a
    /// `HashMap<String, u32>`, which cost thirty thousand string allocations
    /// at every open to answer about ten lookups — 1.0 ms per query at BERT's
    /// vocabulary size, 3.4 ms at the format's ceiling.
    vocab_starts: Vec<u32>,
    /// Offset of `vocab_order`: token ids sorted by their text, which
    /// `token_id` binary-searches without allocating.
    vocab_order_at: usize,
    query_weights: Vec<f32>,
    /// What the build that wrote this file says about how much of the tree it
    /// read. Carried so a ranking can say whether it ranked all of it.
    files_seen: usize,
    stopped_by: Option<Limit>,
}

impl fmt::Debug for Reader {
    /// Summary, never contents.
    ///
    /// A derived `Debug` would print the whole index — up to 64 MiB of file
    /// paths and weights — into whatever assertion message happened to fire.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("lexical::Reader")
            .field("vocabulary", &self.vocabulary)
            .field("files", &self.files)
            .field("terms", &self.terms)
            .field("postings", &self.postings)
            .field("vocab", &self.vocab_starts.len())
            .field("bytes", &self.bytes.len())
            .finish()
    }
}

impl Reader {
    /// Validates and adopts `bytes`.
    ///
    /// Every failure names what was wrong. None of them returns an empty
    /// index: a corrupt file and a repository with no matches are different
    /// facts, and a caller that cannot tell them apart will report the second
    /// when it means the first.
    pub(crate) fn open(bytes: Vec<u8>) -> Result<Self, String> {
        let head = HEADER_BYTES as usize;
        if bytes.len() < head {
            return Err(format!(
                "lexical index is {} bytes, shorter than its {head}-byte header",
                bytes.len()
            ));
        }
        if bytes[..8] != MAGIC {
            return Err("lexical index does not start with the format magic".into());
        }
        let schema = u32_at(&bytes, 8);
        if schema != SCHEMA {
            return Err(format!(
                "lexical index is schema {schema}, this build reads {SCHEMA}"
            ));
        }
        let vocabulary_code = u32_at(&bytes, 12);
        let Some(vocabulary) = Vocabulary::from_code(vocabulary_code) else {
            return Err(format!(
                "lexical index names vocabulary {vocabulary_code}, which this build does not know"
            ));
        };
        let files = u32_at(&bytes, 16) as usize;
        let terms = u32_at(&bytes, 20) as usize;
        let postings = u32_at(&bytes, 24) as usize;
        // Validated and not retained. It is part of the format because a
        // re-scorer would need it, but nothing in this build reads it back and
        // a field carried for a caller that does not exist is a field that
        // rots. The check stays: a non-finite average is a corrupt header.
        let avg_doc_len = f32_at(&bytes, 28);
        if !avg_doc_len.is_finite() || avg_doc_len < 0.0 {
            return Err("lexical index carries a non-finite average document length".into());
        }
        if postings > MAX_POSTINGS {
            return Err(format!(
                "lexical index claims {postings} postings, over the {MAX_POSTINGS} ceiling"
            ));
        }
        let vocab_size = u32_at(&bytes, 32) as usize;
        let files_seen = u32_at(&bytes, 36) as usize;
        let stop_code = u32_at(&bytes, 40);
        let Some(stopped_by) = Limit::from_code(stop_code) else {
            return Err(format!(
                "lexical index names stop reason {stop_code}, which this build does not know"
            ));
        };
        // A walk cannot have seen fewer files than the index holds. Refused
        // rather than clamped, because the number is what a caller divides by.
        if files_seen < files {
            return Err(format!(
                "lexical index holds {files} files but says its walk saw {files_seen}"
            ));
        }
        if vocab_size > MAX_VOCAB {
            return Err(format!(
                "lexical index claims a {vocab_size}-token vocabulary, over the \
                 {MAX_VOCAB} ceiling"
            ));
        }
        // The two vocabularies disagree about where the query side lives, and
        // an index that gets this wrong ranks confidently with half a model.
        if vocabulary == Vocabulary::CodeV1 && vocab_size != 0 {
            return Err(
                "lexical index is code-v1 and carries a stored vocabulary, which it derives".into(),
            );
        }
        if vocabulary != Vocabulary::CodeV1 && vocab_size == 0 {
            return Err(format!(
                "lexical index speaks {vocabulary} but carries no vocabulary, so half of \
                 its ranking is missing"
            ));
        }

        let doc_lens_at = head;
        let paths_at = doc_lens_at
            .checked_add(
                files
                    .checked_mul(4)
                    .ok_or("lexical index file count overflows")?,
            )
            .ok_or("lexical index doc table overflows")?;
        if paths_at > bytes.len() {
            return Err("lexical index is truncated inside its document table".into());
        }

        // The path table is variable-length, so its extent is discovered
        // rather than computed — and discovering it is also what proves every
        // length prefix is inside the file.
        let mut cursor = paths_at;
        let mut path_offsets = Vec::with_capacity(files);
        for index in 0..files {
            if cursor + 4 > bytes.len() {
                return Err(format!(
                    "lexical index is truncated before the length of path {index}"
                ));
            }
            let len = u32_at(&bytes, cursor) as usize;
            cursor += 4;
            let end = cursor
                .checked_add(len)
                .ok_or("lexical index path length overflows")?;
            if end > bytes.len() {
                return Err(format!("lexical index is truncated inside path {index}"));
            }
            if std::str::from_utf8(&bytes[cursor..end]).is_err() {
                return Err(format!("lexical index path {index} is not valid UTF-8"));
            }
            path_offsets.push((cursor, end));
            cursor = end;
        }

        let term_table_at = cursor;
        let term_bytes = terms
            .checked_mul(12)
            .ok_or("lexical index term count overflows")?;
        let postings_at = term_table_at
            .checked_add(term_bytes)
            .ok_or("lexical index term table overflows")?;
        if postings_at > bytes.len() {
            return Err("lexical index is truncated inside its term table".into());
        }
        let posting_bytes = postings
            .checked_mul(8)
            .ok_or("lexical index posting count overflows")?;
        let vocab_at = postings_at
            .checked_add(posting_bytes)
            .ok_or("lexical index posting table overflows")?;
        if vocab_at > bytes.len() {
            return Err(format!(
                "lexical index is truncated inside its postings: needs {vocab_at} bytes, has {}",
                bytes.len()
            ));
        }
        // Variable-length again, so the extent is discovered — and discovering
        // it is what proves every length prefix lands inside the file.
        let mut cursor = vocab_at;
        let mut vocab_starts: Vec<u32> = Vec::with_capacity(vocab_size);
        for id in 0..vocab_size {
            if cursor + 4 > bytes.len() {
                return Err(format!(
                    "lexical index is truncated before the length of vocabulary token {id}"
                ));
            }
            vocab_starts.push(u32::try_from(cursor).map_err(|_| {
                "lexical index vocabulary is larger than an offset can address".to_string()
            })?);
            let len = u32_at(&bytes, cursor) as usize;
            cursor += 4;
            if len == 0 || len > MAX_VOCAB_TOKEN_BYTES {
                return Err(format!(
                    "lexical index vocabulary token {id} claims {len} bytes"
                ));
            }
            let end = cursor
                .checked_add(len)
                .ok_or("lexical index vocabulary token overflows")?;
            if end > bytes.len() {
                return Err(format!(
                    "lexical index is truncated inside vocabulary token {id}"
                ));
            }
            // Validated here and nowhere else, so the accessors below can hand
            // out `&str` without re-checking. No `String` is built: the bytes
            // are already in memory and stay there.
            std::str::from_utf8(&bytes[cursor..end])
                .map_err(|_| format!("lexical index vocabulary token {id} is not valid UTF-8"))?;
            cursor = end;
        }
        let end = cursor
            .checked_add(
                vocab_size
                    .checked_mul(4)
                    .ok_or("lexical index query count overflows")?,
            )
            .ok_or("lexical index query table overflows")?;
        if end > bytes.len() {
            return Err(format!(
                "lexical index is truncated inside its query table: needs {end} bytes, has {}",
                bytes.len()
            ));
        }
        let mut query_weights = Vec::with_capacity(vocab_size);
        for id in 0..vocab_size {
            let weight = f32_at(&bytes, cursor + id * 4);
            if !weight.is_finite() || weight < 0.0 {
                return Err(format!(
                    "lexical index query weight for token {id} is {weight}, which cannot rank"
                ));
            }
            query_weights.push(weight);
        }
        let vocab_order_at = end;
        let order_end = vocab_order_at
            .checked_add(
                vocab_size
                    .checked_mul(4)
                    .ok_or("lexical index order count overflows")?,
            )
            .ok_or("lexical index order table overflows")?;
        if order_end > bytes.len() {
            return Err(format!(
                "lexical index is truncated inside its vocabulary order table: \
                 needs {order_end} bytes, has {}",
                bytes.len()
            ));
        }

        let reader = Reader {
            bytes,
            vocabulary,
            files,
            terms,
            postings,
            term_table_at,
            postings_at,
            path_offsets,
            vocab_starts,
            vocab_order_at,
            query_weights,
            files_seen,
            stopped_by,
        };

        // The order table is what `token_id` binary-searches, so an unsorted or
        // repeating one does not fail loudly — it makes a token that is present
        // resolve to nothing, and the query then scores against ids no document
        // carries. Proved in one pass here: every id in range, and every token
        // strictly greater than the one before, which is also what rules out a
        // duplicate token and a repeated id.
        let mut previous: Option<&str> = None;
        for slot in 0..vocab_size {
            let id = u32_at(&reader.bytes, reader.vocab_order_at + slot * 4);
            if id as usize >= vocab_size {
                return Err(format!(
                    "lexical index vocabulary order names token {id}, outside the \
                     {vocab_size}-token vocabulary"
                ));
            }
            let token = reader
                .token_text(id)
                .ok_or("lexical index vocabulary order points outside the vocabulary")?;
            // Equal and out-of-order are both fatal and are reported apart,
            // because they are different mistakes: a repeat means one id is
            // unreachable and takes the other's weight, and a mis-sort means
            // the binary search misses tokens that are present. An operator
            // reading one message should not have to guess which happened.
            match previous {
                Some(last) if last == token => {
                    return Err(format!(
                        "lexical index vocabulary repeats the token {token:?} at slot {slot}"
                    ));
                }
                Some(last) if last > token => {
                    return Err(format!(
                        "lexical index vocabulary order is not ascending at slot {slot}: \
                         {token:?} follows {last:?}"
                    ));
                }
                _ => {}
            }
            previous = Some(token);
        }

        // Proven, not assumed. An unsorted table makes the binary search in
        // `postings_for` miss terms that are present, and the symptom is an
        // empty result set that looks exactly like a repository with no match.
        let mut previous: Option<u32> = None;
        for slot in 0..reader.terms {
            let (term, first, count) = reader.term_entry(slot);
            if previous.is_some_and(|last| last >= term) {
                return Err(format!(
                    "lexical index term table is not strictly ascending at entry {slot}"
                ));
            }
            previous = Some(term);
            let last = (first as usize)
                .checked_add(count as usize)
                .ok_or("lexical index term slice overflows")?;
            if count == 0 || last > reader.postings {
                return Err(format!(
                    "lexical index term {term} names postings {first}..{last} of {}",
                    reader.postings
                ));
            }
        }
        for index in 0..reader.postings {
            let posting = reader.posting(index);
            if posting.file_id as usize >= reader.files {
                return Err(format!(
                    "lexical index posting {index} names file {} of {}",
                    posting.file_id, reader.files
                ));
            }
            if !posting.weight.is_finite() {
                return Err(format!(
                    "lexical index posting {index} has a non-finite weight"
                ));
            }
        }
        Ok(reader)
    }

    pub(crate) fn vocabulary(&self) -> Vocabulary {
        self.vocabulary
    }

    pub(crate) fn files(&self) -> usize {
        self.files
    }

    pub(crate) fn postings_len(&self) -> usize {
        self.postings
    }

    /// Files the build's walk reached, whether or not they were indexed.
    pub(crate) fn files_seen(&self) -> usize {
        self.files_seen
    }

    /// The ceiling that ended the build short of the tree. `None` means none
    /// did — not that every file was admissible, only that nothing stopped the
    /// walk or filled the index.
    pub(crate) fn stopped_by(&self) -> Option<Limit> {
        self.stopped_by
    }

    pub(crate) fn path(&self, file_id: u32) -> Option<&str> {
        let (start, end) = *self.path_offsets.get(file_id as usize)?;
        // Checked at open; `from_utf8` here only to get a `&str` back.
        std::str::from_utf8(&self.bytes[start..end]).ok()
    }

    fn term_entry(&self, slot: usize) -> (u32, u32, u32) {
        let at = self.term_table_at + slot * 12;
        (
            u32_at(&self.bytes, at),
            u32_at(&self.bytes, at + 4),
            u32_at(&self.bytes, at + 8),
        )
    }

    /// The text of one token, by id.
    ///
    /// Borrowed straight out of the index's own bytes. Both the length and the
    /// UTF-8 were proved at open, so this cannot fail for an id in range and
    /// costs nothing but two reads.
    fn token_text(&self, id: u32) -> Option<&str> {
        let start = *self.vocab_starts.get(id as usize)? as usize;
        let len = u32_at(&self.bytes, start) as usize;
        let from = start + 4;
        // Checked at open; `from_utf8` here only to get a `&str` back.
        std::str::from_utf8(self.bytes.get(from..from + len)?).ok()
    }

    /// The id of one token, if the stored vocabulary holds it.
    ///
    /// A binary search over the stored order, not a hash lookup. The map this
    /// replaced had to be built from thirty thousand freshly allocated strings
    /// before the first query term could be resolved; this touches about
    /// fifteen tokens and allocates nothing.
    pub(crate) fn token_id(&self, token: &str) -> Option<u32> {
        let count = self.vocab_starts.len();
        let (mut lo, mut hi) = (0usize, count);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let id = u32_at(&self.bytes, self.vocab_order_at + mid * 4);
            match self.token_text(id)?.cmp(token) {
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
                std::cmp::Ordering::Equal => return Some(id),
            }
        }
        None
    }

    /// What one token contributes when it appears in a query.
    ///
    /// Zero is an ordinary answer, not an absence: a learned sparse encoder's
    /// whole economy is that most of its vocabulary weighs nothing for any
    /// given query.
    pub(crate) fn query_weight(&self, token_id: u32) -> f32 {
        self.query_weights
            .get(token_id as usize)
            .copied()
            .unwrap_or(0.0)
    }

    fn posting(&self, index: usize) -> Posting {
        let at = self.postings_at + index * 8;
        Posting {
            file_id: u32_at(&self.bytes, at),
            weight: f32_at(&self.bytes, at + 4),
        }
    }

    /// The postings for one term, and the number of documents holding it.
    ///
    /// `None` means the term is not in this index — which, for a ranked tier,
    /// is an ordinary answer rather than a fault: a query term nothing
    /// contains simply contributes nothing to any score.
    pub(crate) fn postings_for(&self, term: u32) -> Option<(Vec<Posting>, u32)> {
        let mut low = 0usize;
        let mut high = self.terms;
        while low < high {
            let mid = low + (high - low) / 2;
            let (candidate, first, count) = self.term_entry(mid);
            match candidate.cmp(&term) {
                std::cmp::Ordering::Less => low = mid + 1,
                std::cmp::Ordering::Greater => high = mid,
                std::cmp::Ordering::Equal => {
                    let postings = (first as usize..first as usize + count as usize)
                        .map(|index| self.posting(index))
                        .collect();
                    return Some((postings, count));
                }
            }
        }
        None
    }

    /// Inverse document frequency for a term with `df` documents.
    ///
    /// The Robertson/Sparck-Jones form, with the `1 +` that keeps it positive:
    /// without it a term present in more than half the corpus scores negative
    /// and *subtracts* from the files that contain it.
    pub(crate) fn idf(&self, df: u32) -> f32 {
        let n = self.files as f32;
        let df = df as f32;
        (1.0 + (n - df + 0.5) / (df + 0.5)).ln()
    }
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn f32_at(bytes: &[u8], at: usize) -> f32 {
    f32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_vocabulary_this_build_can_read_is_one_it_advertises() {
        // `ALL` is what `dcgrep health` reports as a capability. A variant
        // added to the enum and not to `ALL` would make the binary able to
        // open an index it tells its callers it cannot read — a disagreement
        // with no symptom until a build fails for a reason health denies.
        for (code, vocabulary) in Vocabulary::ALL.iter().enumerate() {
            assert_eq!(
                Vocabulary::from_code(code as u32),
                Some(*vocabulary),
                "ALL is out of code order at {code}"
            );
            assert_eq!(vocabulary.code(), code as u32);
        }
        assert_eq!(
            Vocabulary::from_code(Vocabulary::ALL.len() as u32),
            None,
            "a readable vocabulary is missing from ALL"
        );
        // And the names are distinct, since they are how a caller selects one.
        let mut names: Vec<&str> = Vocabulary::ALL.iter().map(|v| v.as_str()).collect();
        names.sort_unstable();
        let total = names.len();
        names.dedup();
        assert_eq!(names.len(), total, "two vocabularies share a name");
    }

    fn built() -> Vec<u8> {
        let mut builder = Builder::new(Vocabulary::CodeV1);
        builder.add("a.rs".into(), 10, &[(7, 3.0), (9, 1.0)]);
        builder.add("b.rs".into(), 20, &[(7, 1.0), (11, 5.0)]);
        builder.finish()
    }

    #[test]
    fn a_built_index_reads_back_with_every_posting() {
        let reader = Reader::open(built()).expect("opens");
        assert_eq!(reader.vocabulary(), Vocabulary::CodeV1);
        assert_eq!(reader.files(), 2);
        assert_eq!(reader.postings_len(), 4);
        assert_eq!(reader.path(0), Some("a.rs"));
        assert_eq!(reader.path(1), Some("b.rs"));
        assert_eq!(reader.path(2), None);

        let (postings, df) = reader.postings_for(7).expect("term 7 is present");
        assert_eq!(df, 2);
        assert_eq!(postings.len(), 2);
        assert_eq!(postings[0].file_id, 0);
        assert_eq!(postings[1].file_id, 1);
        assert!(
            reader.postings_for(8).is_none(),
            "absent term is not a fault"
        );
    }

    #[test]
    fn bm25_rewards_frequency_and_penalises_length() {
        let reader = Reader::open(built()).expect("opens");
        let (postings, _) = reader.postings_for(7).unwrap();
        // a.rs says term 7 three times in ten terms; b.rs once in twenty.
        assert!(
            postings[0].weight > postings[1].weight,
            "{:?} should outrank {:?}",
            postings[0],
            postings[1]
        );
    }

    #[test]
    fn idf_never_goes_negative_on_a_common_term() {
        let reader = Reader::open(built()).expect("opens");
        // Present in every document — the case where the textbook formula
        // without the `1 +` turns negative and subtracts from its own matches.
        assert!(reader.idf(reader.files() as u32) > 0.0);
        assert!(reader.idf(1) > reader.idf(2));
    }

    #[test]
    fn an_empty_index_is_valid_and_answers_nothing() {
        let reader = Reader::open(Builder::new(Vocabulary::CodeV1).finish()).expect("opens");
        assert_eq!(reader.files(), 0);
        assert_eq!(reader.postings_len(), 0);
        assert!(reader.postings_for(1).is_none());
    }

    #[test]
    fn two_builds_of_the_same_input_are_byte_identical() {
        assert_eq!(built(), built(), "the index must not depend on map order");
    }

    #[test]
    fn every_truncation_is_refused_by_name() {
        let full = built();
        for cut in 0..full.len() {
            let err =
                Reader::open(full[..cut].to_vec()).expect_err("a truncated index must never open");
            assert!(
                err.contains("truncated") || err.contains("shorter") || err.contains("magic"),
                "cut at {cut} gave an unhelpful error: {err}"
            );
        }
    }

    #[test]
    fn a_corrupt_header_is_refused_rather_than_read() {
        let mut bytes = built();
        bytes[0] = b'X';
        assert!(Reader::open(bytes).unwrap_err().contains("magic"));

        let mut bytes = built();
        bytes[8] = 99;
        assert!(Reader::open(bytes).unwrap_err().contains("schema"));

        let mut bytes = built();
        bytes[12] = 42;
        assert!(Reader::open(bytes).unwrap_err().contains("vocabulary"));

        let mut bytes = built();
        // A posting count larger than the file can hold.
        bytes[24..28].copy_from_slice(&9_999u32.to_le_bytes());
        assert!(Reader::open(bytes).unwrap_err().contains("truncated"));

        let mut bytes = built();
        bytes[28..32].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(Reader::open(bytes).unwrap_err().contains("non-finite"));
    }

    #[test]
    fn the_file_says_how_much_of_the_tree_it_read() {
        // Unfinished: nothing recorded, so it claims no more than it holds.
        let reader = Reader::open(built()).expect("opens");
        assert_eq!(reader.files_seen(), 2);
        assert_eq!(reader.stopped_by(), None);

        for stop in [
            Limit::Postings,
            Limit::Files,
            Limit::Duration,
            Limit::InputBytes,
        ] {
            let mut builder = Builder::new(Vocabulary::CodeV1);
            builder.add("a.rs".into(), 10, &[(7, 3.0)]);
            builder.record_walk(41, Some(stop));
            let reader = Reader::open(builder.finish()).expect("opens");
            assert_eq!(reader.files(), 1);
            assert_eq!(reader.files_seen(), 41);
            assert_eq!(reader.stopped_by(), Some(stop));
        }
    }

    #[test]
    fn its_own_ceiling_is_named_before_the_walks() {
        let mut builder = Builder::with_ceiling(Vocabulary::CodeV1, 3, MAX_FILE_BYTES);
        assert!(builder.add("a.rs".into(), 4, &[(1, 1.0), (2, 1.0)]));
        assert!(!builder.add("b.rs".into(), 4, &[(1, 1.0), (2, 1.0)]));
        // The walk then ran out of time with the index already full.
        builder.record_walk(9, Some(Limit::Duration));
        let reader = Reader::open(builder.finish()).expect("opens");
        assert_eq!(reader.files(), 1);
        assert_eq!(reader.files_seen(), 9);
        assert_eq!(reader.stopped_by(), Some(Limit::Postings));
    }

    #[test]
    fn the_header_constant_is_the_header_finish_writes() {
        // An empty index is the header and nothing else.
        let empty = Builder::new(Vocabulary::CodeV1);
        assert_eq!(empty.predicted_len(), HEADER_BYTES);
        assert_eq!(empty.finish().len() as u64, HEADER_BYTES);
    }

    /// The accounting is exact, not an estimate: the length predicted from the
    /// adds equals the length written, for every sequence of adds, and a refused
    /// document leaves the prediction where it was.
    #[test]
    fn the_byte_budget_counts_exactly_what_finish_writes() {
        // Shared terms across documents, a repeated term inside one document,
        // a dropped non-finite weight, and paths of different lengths: each is
        // a place an off-by-one would hide.
        let mut builder = Builder::new(Vocabulary::CodeV1);
        let before = builder.predicted_len();
        assert!(builder.add("a.rs".into(), 5, &[(1, 1.0), (2, 2.0)]));
        assert!(builder.add(
            "deeply/nested/b.rs".into(),
            5,
            &[(2, 1.0), (3, 1.0), (4, f32::NAN)]
        ));
        assert!(builder.add("c.rs".into(), 5, &[(9, 1.0), (9, 2.0)]));
        // 8 + path per document, 8 per kept posting, 12 per distinct term
        // {1, 2, 3, 9}.
        let expected = before + (8 + 4) + (8 + 18) + (8 + 4) + 8 * (2 + 2 + 2) + 12 * 4;
        assert_eq!(builder.predicted_len(), expected);
        assert_eq!(builder.finish().len() as u64, expected);
    }

    #[test]
    fn a_document_that_would_cross_the_byte_budget_is_refused_whole() {
        // Room for the header and one small document, not two.
        let one = HEADER_BYTES + (8 + 4) + 8 * 2 + 12 * 2;
        let mut builder = Builder::with_ceiling(Vocabulary::CodeV1, MAX_POSTINGS, one + 10);
        assert!(builder.add("a.rs".into(), 5, &[(1, 1.0), (2, 1.0)]));
        let kept = builder.predicted_len();
        assert_eq!(kept, one);
        // New terms 7 and 8 would be counted while checking and must not stay.
        assert!(!builder.add("b.rs".into(), 5, &[(7, 1.0), (8, 1.0)]));
        assert_eq!(
            builder.predicted_len(),
            kept,
            "a refused document costs nothing"
        );
        assert_eq!(builder.documents(), 1);
        assert_eq!(builder.limit(), Some(Limit::Bytes));
        assert_eq!(Limit::Bytes.as_str(), "lexical_bytes");
        builder.record_walk(2, None);
        let bytes = builder.finish();
        assert_eq!(bytes.len() as u64, one);
        let reader = Reader::open(bytes).expect("opens");
        assert_eq!(reader.stopped_by(), Some(Limit::Bytes));
        assert_eq!(reader.files_seen(), 2);
    }

    /// The worst case the attack found: every term distinct, so each posting
    /// costs twenty bytes and four million of them would be eighty megabytes.
    /// Long paths and the largest vocabulary a learned index may carry are
    /// added on top. The builder must stop short of the bound and the file it
    /// writes must be exactly as long as it said.
    #[test]
    fn a_corpus_of_distinct_terms_stops_under_the_file_bound_exactly() {
        for vocabulary in [Vocabulary::CodeV1, Vocabulary::WordPiece30522] {
            let mut builder = Builder::new(vocabulary);
            if vocabulary != Vocabulary::CodeV1 {
                let tokens: Vec<String> = (0..MAX_VOCAB).map(|n| format!("{n:064}")).collect();
                builder
                    .set_query_side(tokens, vec![1.0; MAX_VOCAB])
                    .expect("the largest vocabulary a model may carry");
            }
            let mut next_term = 0u32;
            let mut added = 0usize;
            loop {
                let weights: Vec<(u32, f32)> = (0..20_000)
                    .map(|_| {
                        next_term += 1;
                        (next_term, 1.0)
                    })
                    .collect();
                let path = format!("{added:04}/{}", "p".repeat(1_000));
                if !builder.add(path, 20_000, &weights) {
                    break;
                }
                added += 1;
                assert!(added < 1_000, "the byte budget should have stopped this");
            }
            assert_eq!(builder.limit(), Some(Limit::Bytes), "{vocabulary}");
            assert!(
                builder.postings() < MAX_POSTINGS,
                "this corpus must reach the byte budget before the count cap: {vocabulary}"
            );
            let predicted = builder.predicted_len();
            assert!(predicted <= MAX_FILE_BYTES, "{vocabulary}: {predicted}");
            // Tight: one more document's worth of postings would not have fit.
            assert!(
                predicted + 8 + 1_005 + 20_000 * 20 > MAX_FILE_BYTES,
                "{vocabulary}: stopped with room to spare"
            );
            builder.record_walk(added + 1, None);
            let bytes = builder.finish();
            assert_eq!(bytes.len() as u64, predicted, "{vocabulary}");
            Reader::open(bytes).unwrap_or_else(|err| panic!("{vocabulary}: must open: {err}"));
        }
    }

    #[test]
    fn the_query_side_is_stored_once_and_before_any_document() {
        let mut builder = Builder::new(Vocabulary::WordPiece30522);
        builder.add("a.rs".into(), 1, &[(0, 1.0)]);
        let err = builder
            .set_query_side(vec!["a".into()], vec![1.0])
            .expect_err("after a document");
        assert!(err.contains("before the first document"), "{err}");

        let mut builder = Builder::new(Vocabulary::WordPiece30522);
        builder.set_query_side(vec!["a".into()], vec![1.0]).unwrap();
        assert!(builder.set_query_side(vec!["a".into()], vec![1.0]).is_err());

        // A budget with no room for the vocabulary is refused, not published.
        let mut tight =
            Builder::with_ceiling(Vocabulary::WordPiece30522, MAX_POSTINGS, HEADER_BYTES + 5);
        assert!(tight.set_query_side(vec!["a".into()], vec![1.0]).is_err());
    }

    #[test]
    fn a_header_that_contradicts_itself_about_coverage_is_refused() {
        let mut bytes = built();
        // A stop reason this build has never heard of.
        bytes[40..44].copy_from_slice(&77u32.to_le_bytes());
        assert!(Reader::open(bytes).unwrap_err().contains("stop reason 77"));

        let mut bytes = built();
        // Two documents held, one file seen: the walk cannot have seen fewer
        // files than the index contains, and a caller divides by this.
        bytes[36..40].copy_from_slice(&1u32.to_le_bytes());
        assert!(Reader::open(bytes).unwrap_err().contains("saw 1"));
    }

    #[test]
    fn a_posting_naming_a_file_that_does_not_exist_is_refused() {
        let mut builder = Builder::new(Vocabulary::CodeV1);
        builder.add("only.rs".into(), 5, &[(3, 1.0)]);
        let mut bytes = builder.finish();
        let len = bytes.len();
        // The single posting's file_id is the last 8 bytes; point it at file 9.
        bytes[len - 8..len - 4].copy_from_slice(&9u32.to_le_bytes());
        let err = Reader::open(bytes).expect_err("must refuse");
        assert!(err.contains("names file 9"), "{err}");
    }

    #[test]
    fn an_unsorted_term_table_is_refused_rather_than_searched() {
        let mut builder = Builder::new(Vocabulary::CodeV1);
        builder.add("a.rs".into(), 4, &[(1, 1.0), (2, 1.0)]);
        let mut bytes = builder.finish();
        // Swap the two term ids so the table descends. A binary search over
        // it would report term 1 absent, which is the silent wrong answer.
        let table = bytes.len() - 2 * 8 - 2 * 12;
        bytes[table..table + 4].copy_from_slice(&2u32.to_le_bytes());
        bytes[table + 12..table + 16].copy_from_slice(&1u32.to_le_bytes());
        let err = Reader::open(bytes).expect_err("must refuse");
        assert!(err.contains("ascending"), "{err}");
    }

    #[test]
    fn the_posting_ceiling_stops_the_build_and_says_so() {
        let mut builder = Builder::new(Vocabulary::CodeV1);
        let wide: Vec<(u32, f32)> = (0..50_000u32).map(|term| (term, 1.0)).collect();
        let mut added = 0;
        while builder.add(format!("f{added}.rs"), 50_000, &wide) {
            added += 1;
            assert!(added < 1_000, "the ceiling should have stopped this");
        }
        assert!(builder.is_full());
        assert_eq!(builder.limit(), Some(Limit::Postings));
        assert!(builder.postings() <= MAX_POSTINGS);
        // The document that did not fit is absent entirely, not half in.
        assert_eq!(builder.documents(), added);
    }

    #[test]
    fn a_non_finite_or_zero_weight_never_reaches_the_index() {
        let mut builder = Builder::new(Vocabulary::WordPiece30522);
        builder
            .set_query_side(model_vocab(), vec![0.0, 0.0, 0.0, 0.0, 0.0, 1.0])
            .expect("a model index may carry a query side");
        builder.add(
            "a.rs".into(),
            4,
            &[
                (1, f32::NAN),
                (2, f32::INFINITY),
                (3, 0.0),
                (4, -1.0),
                (5, 0.5),
            ],
        );
        let reader = Reader::open(builder.finish()).expect("opens");
        assert_eq!(reader.postings_len(), 1, "only the usable weight survives");
        assert!(reader.postings_for(5).is_some());
        for poisoned in [1, 2, 3, 4] {
            assert!(reader.postings_for(poisoned).is_none(), "term {poisoned}");
        }
    }

    fn model_vocab() -> Vec<String> {
        ["[UNK]", "parse", "##json", "response", "server", "quetzal"]
            .iter()
            .map(|token| token.to_string())
            .collect()
    }

    #[test]
    fn a_model_index_carries_its_query_side_and_looks_it_up() {
        let mut builder = Builder::new(Vocabulary::WordPiece30522);
        builder
            .set_query_side(model_vocab(), vec![0.0, 1.5, 0.5, 0.0, f32::NAN, -2.0])
            .expect("accepted");
        builder.add("a.rs".into(), 4, &[(1, 1.0), (2, 1.0)]);
        let reader = Reader::open(builder.finish()).expect("opens");

        assert_eq!(reader.token_id("parse"), Some(1));
        assert_eq!(reader.token_id("##json"), Some(2));
        assert_eq!(reader.token_id("absent"), None);
        assert_eq!(reader.query_weight(1), 1.5);
        assert_eq!(reader.query_weight(2), 0.5);
        // A weight that cannot rank is stored as zero, not refused: the table
        // is dense and every id needs an entry, and a learned encoder gives
        // most of its vocabulary nothing.
        assert_eq!(reader.query_weight(4), 0.0, "NaN becomes no contribution");
        assert_eq!(reader.query_weight(5), 0.0, "negative becomes none");
        assert_eq!(reader.query_weight(999), 0.0, "out of range is not a panic");
        // Every token the builder was given survives the round trip to its
        // own id, which is the property the on-disk vocabulary exists for.
        for (id, token) in model_vocab().iter().enumerate() {
            assert_eq!(
                reader.token_id(token),
                Some(id as u32),
                "{token:?} did not round trip"
            );
        }
    }

    #[test]
    fn the_two_vocabularies_disagree_about_the_query_side_and_both_are_checked() {
        // code-v1 derives its query side, so being handed one is a fault
        // rather than an extra.
        let mut derived = Builder::new(Vocabulary::CodeV1);
        assert!(
            derived.set_query_side(model_vocab(), vec![0.0; 6]).is_err(),
            "code-v1 must refuse a stored query side"
        );

        // And a model index without one is half a ranking, refused at open
        // rather than scored with whatever the other half implies.
        let mut headless = Builder::new(Vocabulary::WordPiece30522);
        headless.add("a.rs".into(), 4, &[(3, 1.0)]);
        let err = Reader::open(headless.finish()).expect_err("must refuse");
        assert!(err.contains("no vocabulary"), "{err}");
    }

    #[test]
    fn the_vocabulary_and_its_weights_must_be_the_same_length() {
        // They are indexed by the same id. A mismatch assigns one token's
        // weight to another's text, for every query, silently.
        let mut builder = Builder::new(Vocabulary::WordPiece30522);
        let err = builder
            .set_query_side(model_vocab(), vec![0.0; 3])
            .expect_err("must refuse");
        assert!(err.contains("must agree"), "{err}");
    }

    #[test]
    fn a_repeated_vocabulary_token_is_refused() {
        let mut builder = Builder::new(Vocabulary::WordPiece30522);
        builder
            .set_query_side(vec!["parse".into(), "parse".into()], vec![1.0, 2.0])
            .expect("the builder does not police duplicates; the reader does");
        builder.add("a.rs".into(), 2, &[(0, 1.0)]);
        let err = Reader::open(builder.finish()).expect_err("must refuse");
        assert!(err.contains("repeats the token"), "{err}");
    }

    #[test]
    fn a_vocabulary_order_that_would_mislead_the_search_is_refused() {
        // `token_id` binary-searches this table, so a corrupted one does not
        // fail loudly: it makes a token that is present resolve to nothing,
        // and the query then scores against ids no document carries. Every
        // corruption below is therefore a refusal, not a degraded answer.
        let good = || {
            let mut builder = Builder::new(Vocabulary::WordPiece30522);
            builder
                .set_query_side(model_vocab(), vec![1.0; 6])
                .expect("query side");
            builder.add("a.rs".into(), 2, &[(0, 1.0)]);
            builder.finish()
        };
        let bytes = good();
        let reader = Reader::open(bytes.clone()).expect("the unmodified index opens");
        for (id, token) in model_vocab().iter().enumerate() {
            assert_eq!(reader.token_id(token), Some(id as u32), "{token:?}");
        }
        assert_eq!(reader.token_id("absent"), None);

        // The order table is the last section, four bytes per token.
        let order_at = bytes.len() - model_vocab().len() * 4;

        // An id outside the vocabulary.
        let mut wrong = bytes.clone();
        wrong[order_at..order_at + 4].copy_from_slice(&999u32.to_le_bytes());
        let err = Reader::open(wrong).expect_err("must refuse");
        assert!(err.contains("outside the"), "{err}");

        // Two slots naming the same token: one id becomes unreachable and
        // takes the other's weight.
        let mut repeated = bytes.clone();
        let first = u32_at(&repeated, order_at);
        repeated[order_at + 4..order_at + 8].copy_from_slice(&first.to_le_bytes());
        let err = Reader::open(repeated).expect_err("must refuse");
        assert!(err.contains("repeats the token"), "{err}");

        // Out of order, which makes the binary search miss present tokens.
        let mut swapped = bytes.clone();
        let a = u32_at(&swapped, order_at);
        let b = u32_at(&swapped, order_at + 4);
        swapped[order_at..order_at + 4].copy_from_slice(&b.to_le_bytes());
        swapped[order_at + 4..order_at + 8].copy_from_slice(&a.to_le_bytes());
        let err = Reader::open(swapped).expect_err("must refuse");
        assert!(err.contains("not ascending"), "{err}");

        // And a file that simply stops before the table.
        let err = Reader::open(bytes[..bytes.len() - 4].to_vec()).expect_err("must refuse");
        assert!(err.contains("order table"), "{err}");
    }

    #[test]
    fn a_model_vocabulary_passes_its_weights_through_untouched() {
        let mut builder = Builder::new(Vocabulary::WordPiece30522);
        builder
            .set_query_side(model_vocab(), vec![0.0; 6])
            .expect("a model index may carry a query side");
        builder.add("a.rs".into(), 10, &[(7, 0.25)]);
        builder.add("b.rs".into(), 99, &[(7, 0.25)]);
        let reader = Reader::open(builder.finish()).expect("opens");
        let (postings, _) = reader.postings_for(7).unwrap();
        // No BM25 length normalisation: the encoder already decided.
        assert_eq!(postings[0].weight, 0.25);
        assert_eq!(postings[1].weight, 0.25);
    }

    /// Every accessor a query reaches, driven past its edges.
    ///
    /// A reader that opened has asserted its own consistency, so this asserts
    /// the same properties from outside: if `open` let something through, the
    /// symptom is here rather than in a ranking nobody is checking.
    fn exercise(reader: &Reader) {
        // Paths: in range, past the end, and at the u32 boundary.
        for file_id in 0..reader.files as u32 {
            let path = reader.path(file_id).expect("an indexed file has a path");
            assert!(!path.is_empty(), "file {file_id} opened with an empty path");
        }
        assert!(reader.path(reader.files as u32).is_none());
        assert!(reader.path(u32::MAX).is_none());

        // The vocabulary side, including ids nothing was published under.
        for id in 0..reader.vocab_starts.len() as u32 {
            let weight = reader.query_weight(id);
            assert!(
                weight.is_finite() && weight >= 0.0,
                "token {id} carries query weight {weight}"
            );
        }
        assert_eq!(reader.query_weight(u32::MAX), 0.0);
        for token in ["parse", "", "\u{0}", "quetzal", "\u{10FFFF}", "zzzzzzzz"] {
            if let Some(id) = reader.token_id(token) {
                assert!(
                    (id as usize) < reader.vocab_starts.len(),
                    "token {token:?} resolved to {id}, outside the vocabulary"
                );
            }
        }

        // Every posting reachable by a term lookup must name a real file and
        // carry a weight that can participate in a sum.
        //
        // Deliberately *not* asserted: that the term slices tile the posting
        // table exactly. A corrupted table can leave postings no term reaches,
        // or point two terms at the same ones, and the first draft of this
        // called both a fault. They are not. Every index read is still inside
        // the table — `open` proves `first + count <= postings` per entry — so
        // the worst case is a posting nobody scores or one scored twice, in a
        // file whose weights are already arbitrary because it was corrupted.
        // Requiring exact tiling would buy no safety and would fail on inputs
        // the format permits.
        for slot in 0..reader.terms {
            let (term, _, _) = reader.term_entry(slot);
            let (postings, df) = reader
                .postings_for(term)
                .expect("a term in the table must be findable by the search that reads it");
            assert_eq!(
                df as usize,
                postings.len(),
                "term {term} reports df {df} and returned {} postings",
                postings.len()
            );
            for posting in &postings {
                assert!(
                    (posting.file_id as usize) < reader.files,
                    "term {term} names file {} of {}",
                    posting.file_id,
                    reader.files
                );
                assert!(posting.weight.is_finite());
            }
            assert!(reader.idf(df).is_finite());
        }
    }

    /// No single-byte change to a published index produces a reader that
    /// panics or hands out something outside itself.
    ///
    /// `every_truncation_is_refused_by_name` covers prefixes and
    /// `a_corrupt_header_is_refused_rather_than_read` covers five header
    /// fields. Neither covers the tables, which is where most of the file is
    /// and where a wrong offset stops being a length check and starts being an
    /// index into a slice. Exhaustive over every byte and every value it could
    /// take, because "we tried some corruptions" is not the same claim.
    ///
    /// Refusing is always a correct outcome. Opening is only correct if the
    /// reader that comes back is internally consistent, which `exercise`
    /// is what decides.
    #[test]
    fn no_single_byte_change_makes_a_reader_that_panics_or_escapes_itself() {
        let mut model = Builder::new(Vocabulary::WordPiece30522);
        model
            .set_query_side(model_vocab(), vec![0.0, 1.5, 0.5, 0.25, 2.0, 0.75])
            .expect("accepted");
        model.add("a.rs".into(), 4, &[(1, 1.0), (2, 1.0)]);
        model.add("b.rs".into(), 9, &[(2, 0.5), (4, 2.0)]);

        let mut opened = 0usize;
        for original in [built(), model.finish()] {
            for position in 0..original.len() {
                for value in 0u16..=255 {
                    let mut bytes = original.clone();
                    if bytes[position] == value as u8 {
                        continue;
                    }
                    bytes[position] = value as u8;
                    if let Ok(reader) = Reader::open(bytes) {
                        exercise(&reader);
                        opened += 1;
                    }
                }
            }
        }
        // Not an assertion about how many survive — that number is allowed to
        // move. It asserts the test is doing work: if a change made every
        // mutation refuse, this would pass while proving nothing.
        // Not an upper bound — more survivors is fine, and the number moves
        // with the fixtures. It guards the case that would make this test pass
        // while proving nothing: a change that makes every mutation refuse at
        // the header, so `exercise` never runs. It was 27,610 when written.
        assert!(
            opened > 10_000,
            "only {opened} mutations opened; this test proves nothing if they \
             are all refused before a reader exists"
        );
    }

    /// The same property under many bytes changing at once.
    ///
    /// Single-byte coverage is exhaustive but cannot reach a state that needs
    /// two fields to agree — a length and the offset that follows it, say.
    /// Seeded so a failure reproduces exactly.
    #[test]
    fn no_multi_byte_corruption_makes_a_reader_that_panics_or_escapes_itself() {
        let mut state = 0x2026_0917u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };

        let mut model = Builder::new(Vocabulary::WordPiece30522);
        model
            .set_query_side(model_vocab(), vec![0.0, 1.5, 0.5, 0.25, 2.0, 0.75])
            .expect("accepted");
        model.add("src/a.rs".into(), 40, &[(1, 1.0), (2, 1.0), (5, 0.5)]);
        model.add("src/b.rs".into(), 90, &[(2, 0.5), (4, 2.0)]);
        model.add("c.rs".into(), 7, &[(1, 3.0)]);
        let corpora = [built(), model.finish()];

        let mut opened = 0usize;
        for round in 0..20_000 {
            let original = &corpora[round % corpora.len()];
            let mut bytes = original.clone();
            let changes = 1 + (next() as usize % 8);
            for _ in 0..changes {
                let at = next() as usize % bytes.len();
                bytes[at] = next() as u8;
            }
            // Also exercise lengths the writer would never produce.
            match next() % 8 {
                0 => bytes.truncate(next() as usize % original.len().max(1)),
                1 => bytes.extend(std::iter::repeat_n(next() as u8, next() as usize % 64)),
                _ => {}
            }
            if let Ok(reader) = Reader::open(bytes) {
                exercise(&reader);
                opened += 1;
            }
        }
        // See the single-byte test: a floor, not a target. It was 897.
        assert!(
            opened > 200,
            "only {opened} corruptions opened; this test proves nothing if they \
             are all refused before a reader exists"
        );
    }
}
