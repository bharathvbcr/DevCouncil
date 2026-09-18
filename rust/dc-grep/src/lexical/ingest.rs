//! Reading a learned sparse encoder's output into the index.
//!
//! The encoder is a neural network and it does not run here. It runs offline,
//! in Python, against a model the operator downloaded — `scripts/encode-sparse.py`
//! in this repository — and writes one JSONL file. This module reads that file.
//! Nothing in the search path ever loads a model, which is what keeps `dcgrep`
//! a single static binary with no runtime and no `CGO_ENABLED=1`.
//!
//! ## The file
//!
//! Line 1 is the header and describes the model:
//!
//! ```text
//! {"schema":1,"model":"...","vocabulary":"wordpiece-30522",
//!  "vocab":["[PAD]","[UNK]",...],
//!  "query_weights":[0.0,0.0,...],
//!  "parity":[{"text":"parseJSON","ids":[11968,15723]}, ...]}
//! ```
//!
//! Every line after it is one document:
//!
//! ```text
//! {"path":"src/lib.rs","total_terms":812,"terms":[[1996,0.42],[2003,0.31]]}
//! ```
//!
//! ## What this refuses
//!
//! The file is input, and it is input produced by a different program in a
//! different language against a model this build has never seen. So none of it
//! is taken on trust:
//!
//!   - **Paths.** Only files the walk actually admitted are indexed. A path in
//!     the file that the walk did not reach is counted and dropped, never
//!     added. Otherwise a stale or hostile encoding could put entries in the
//!     index for files outside the repository, and a ranking would name them.
//!   - **The tokeniser.** The header's parity samples are replayed through
//!     this build's own WordPiece before anything is written, and a single
//!     disagreement refuses the build. Two tokenisers that differ do not
//!     return fewer results; they return a ranking over ids nothing was
//!     indexed under.
//!   - **Sizes.** Every line, the document count and the term count per
//!     document are bounded during the read rather than checked after it.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use serde::Deserialize;

use super::store::{self, Vocabulary};
use super::wordpiece;

/// The schema of the producer's file. Bumped when its shape changes.
const ENCODED_SCHEMA: u32 = 1;

/// Longest header line accepted.
///
/// The header carries a whole vocabulary and its weight table. BERT WordPiece
/// renders to well under a megabyte of JSON; this leaves room for a much
/// larger vocabulary without leaving room for an unbounded one.
const MAX_HEADER_BYTES: u64 = 16 * 1024 * 1024;

/// Longest document line accepted.
///
/// A document at the term ceiling is roughly 300 KB of JSON, so this is an
/// order of magnitude of headroom and still three hundred times smaller than
/// the header bound. They were one bound, which meant every document line was
/// allowed to be as large as a whole vocabulary — a ceiling that bounds
/// nothing a real document could reach.
const MAX_DOCUMENT_BYTES: u64 = 4 * 1024 * 1024;

/// Documents accepted from one file.
///
/// `crate::MAX_LIST_RESULTS` is defined as this, so the listing a producer uses
/// to decide what to encode can always name at least as many files as this will
/// accept back.
pub(crate) const MAX_DOCUMENTS: usize = 200_000;

/// Terms accepted from one document.
const MAX_TERMS_PER_DOCUMENT: usize = 20_000;

/// Postings accepted from one encoding, across every document in it.
///
/// The two bounds above are each modest and their product is not: 200,000
/// documents of 20,000 terms is four billion postings, all of them parsed and
/// held before the index keeps the first [`store::MAX_POSTINGS`] and drops the
/// rest. Measured at roughly 7 bytes per posting held — 36 million cost 246 MB
/// resident — the declared ceilings come to about 29 GB for an index that can
/// use a thousandth of it. Per-item bounds are not a bound on the fan-out.
///
/// Eight times what the store can hold. A document the walk does not admit
/// still costs memory here, so the budget has to cover a stale encoding whose
/// documents have mostly moved or been deleted; eight times covers an encoding
/// where seven of every eight documents are gone, at about 230 MB. This
/// repository's own encoding is 334,000 postings, two orders of magnitude
/// inside it.
pub(crate) const MAX_INGEST_POSTINGS: usize = 8 * super::store::MAX_POSTINGS;

// A budget below what the store can hold would trade an out-of-memory for an
// outage: encodings the index could have used in full would be refused.
// Compile-time, because a test can only fail after a binary carrying the wrong
// budget has been built.
const _: () = assert!(MAX_INGEST_POSTINGS >= 8 * super::store::MAX_POSTINGS);
const _: () = assert!(MAX_INGEST_POSTINGS > MAX_TERMS_PER_DOCUMENT);

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    schema: u32,
    model: String,
    vocabulary: String,
    vocab: Vec<String>,
    query_weights: Vec<f32>,
    #[serde(default)]
    parity: Vec<ParitySample>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ParitySample {
    text: String,
    ids: Vec<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    path: String,
    total_terms: u32,
    terms: Vec<(u32, f32)>,
}

/// One encoder run, validated.
pub(crate) struct Encoded {
    pub(crate) model: String,
    pub(crate) vocabulary: Vocabulary,
    pub(crate) vocab: Vec<String>,
    pub(crate) query_weights: Vec<f32>,
    /// Path to `(document length in tokens, (token id, weight) pairs)`.
    pub(crate) documents: HashMap<String, (u32, Vec<(u32, f32)>)>,
    /// The producer's `(text, ids)` samples, kept after the check here so the
    /// caller can replay them against the index it is about to publish. The
    /// check below proves this build agrees with the model; that one proves
    /// the bytes being written still agree after a round trip through the
    /// on-disk format.
    pub(crate) parity: Vec<(String, Vec<u32>)>,
}

impl std::fmt::Debug for Encoded {
    /// Summary, never contents.
    ///
    /// A derived `Debug` would print a thirty-thousand-token vocabulary and
    /// every document's weights into whatever assertion message happened to
    /// fire, which is how a failing test becomes unreadable.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("lexical::Encoded")
            .field("model", &self.model)
            .field("vocabulary", &self.vocabulary)
            .field("vocab", &self.vocab.len())
            .field("documents", &self.documents.len())
            .field("parity", &self.parity.len())
            .finish()
    }
}

/// Reads and validates an encoder's output.
///
/// Every error names what was wrong with the file. None of them produces an
/// empty `Encoded`: an encoding that could not be read and a repository with
/// nothing in it are different facts, and a build that confused them would
/// publish an empty ranked index over a repository full of code.
pub(crate) fn read(path: &Path) -> Result<Encoded, String> {
    read_with_budget(path, MAX_INGEST_POSTINGS)
}

/// `read`, with the posting budget injectable.
///
/// The seam exists for the same reason `index::build_with_limits` does: proving
/// a ceiling refuses requires an input that exceeds it, and building 32 million
/// postings of JSON to prove that takes seven seconds and 400 MB. A test drives
/// a small budget through the same code; a separate test asserts the real
/// constant is the one `read` passes and that it stays above what the store can
/// hold.
fn read_with_budget(path: &Path, max_postings: usize) -> Result<Encoded, String> {
    // `File::open`, not the walk's hardened `open_for_search`. This path is an
    // argument the operator typed, not a name discovered inside a tree under
    // search, and the encoder's output legitimately lives behind a symlink into
    // a build or cache directory — which `O_NOFOLLOW` would refuse.
    let file = std::fs::File::open(path)
        .map_err(|err| format!("sparse encoding {} is unreadable: {err}", path.display()))?;
    let metadata = file.metadata().map_err(|err| {
        format!(
            "sparse encoding {} could not be stat'ed: {err}",
            path.display()
        )
    })?;
    if metadata.is_dir() {
        return Err(format!("sparse encoding {} is a directory", path.display()));
    }
    let mut reader = BufReader::new(file);

    let header_line = read_line(&mut reader, "header", MAX_HEADER_BYTES)?
        .ok_or_else(|| format!("sparse encoding {} is empty", path.display()))?;
    let header: Header = serde_json::from_str(&header_line)
        .map_err(|err| format!("sparse encoding header is not valid JSON: {err}"))?;
    if header.schema != ENCODED_SCHEMA {
        return Err(format!(
            "sparse encoding is schema {}, this build reads {ENCODED_SCHEMA}",
            header.schema
        ));
    }
    let vocabulary = match header.vocabulary.as_str() {
        "wordpiece-30522" => Vocabulary::WordPiece30522,
        "code-v1" => {
            return Err(
                "code-v1 is this crate's own tokeniser and is built from the repository, \
                 not imported from an encoder"
                    .into(),
            );
        }
        other => {
            return Err(format!(
                "sparse encoding names vocabulary {other:?}, which this build does not know"
            ));
        }
    };
    if header.vocab.len() != header.query_weights.len() {
        return Err(format!(
            "sparse encoding has {} vocabulary tokens and {} query weights; they are \
             indexed by the same id and must agree",
            header.vocab.len(),
            header.query_weights.len()
        ));
    }
    if header.vocab.is_empty() {
        return Err("sparse encoding carries an empty vocabulary".into());
    }
    if header.vocab.len() > store::MAX_VOCAB {
        return Err(format!(
            "sparse encoding carries {} vocabulary tokens, over the {} ceiling",
            header.vocab.len(),
            store::MAX_VOCAB
        ));
    }

    // The gate. Before a byte is written, this build's tokeniser is made to
    // reproduce the model's own output on the producer's samples.
    if header.parity.is_empty() {
        return Err(
            "sparse encoding carries no tokeniser parity samples, so there is no way to \
             show that this build splits a query the way the model split the documents. \
             Regenerate it with a producer that emits them"
                .into(),
        );
    }
    let lookup: HashMap<String, u32> = header
        .vocab
        .iter()
        .enumerate()
        .map(|(id, token)| (token.clone(), id as u32))
        .collect();
    if lookup.len() != header.vocab.len() {
        return Err("sparse encoding vocabulary repeats a token".into());
    }
    let samples: Vec<(String, Vec<u32>)> = header
        .parity
        .into_iter()
        .map(|sample| (sample.text, sample.ids))
        .collect();
    wordpiece::check_parity(&samples, &lookup)?;
    drop(lookup);

    let vocab_size = header.vocab.len() as u32;
    let mut documents = HashMap::new();
    let mut line_number = 1usize;
    let mut postings = 0usize;
    while let Some(line) = read_line(&mut reader, "document", MAX_DOCUMENT_BYTES)? {
        line_number += 1;
        if line.trim().is_empty() {
            continue;
        }
        if documents.len() >= MAX_DOCUMENTS {
            return Err(format!(
                "sparse encoding carries more than {MAX_DOCUMENTS} documents"
            ));
        }
        let document: Document = serde_json::from_str(&line).map_err(|err| {
            format!("sparse encoding line {line_number} is not valid JSON for this schema: {err}")
        })?;
        if document.terms.len() > MAX_TERMS_PER_DOCUMENT {
            return Err(format!(
                "sparse encoding line {line_number} carries {} terms for {}, over the \
                 {MAX_TERMS_PER_DOCUMENT} ceiling",
                document.terms.len(),
                document.path
            ));
        }
        // Checked while reading rather than after, so an encoding past the
        // budget is refused before the memory it would need is allocated. A
        // bound enforced after the allocation is not a bound.
        postings = postings.saturating_add(document.terms.len());
        if postings > max_postings {
            return Err(format!(
                "sparse encoding carries more than {max_postings} postings, \
                 reached at line {line_number}; the ranked index holds at most {} of \
                 them, so this encoding describes far more than any index built from \
                 it could use",
                super::store::MAX_POSTINGS
            ));
        }
        for (id, weight) in &document.terms {
            // An id outside the vocabulary has no token and no query weight,
            // so nothing could ever match it — and its presence means the
            // encoding was produced against a different model from the one in
            // the header, which is worth refusing loudly.
            if *id >= vocab_size {
                return Err(format!(
                    "sparse encoding line {line_number} names token {id}, outside the \
                     {vocab_size}-token vocabulary in its own header"
                ));
            }
            if !weight.is_finite() {
                return Err(format!(
                    "sparse encoding line {line_number} gives token {id} a non-finite weight"
                ));
            }
        }
        if documents
            .insert(
                document.path.clone(),
                (document.total_terms, document.terms),
            )
            .is_some()
        {
            return Err(format!(
                "sparse encoding names {} twice; one of the two would silently win",
                document.path
            ));
        }
    }

    Ok(Encoded {
        model: header.model,
        vocabulary,
        vocab: header.vocab,
        query_weights: header.query_weights,
        documents,
        parity: samples,
    })
}

/// Reads one line, bounded during the read.
///
/// One byte past the limit is taken so the cap can be *detected*: a reader
/// stopped exactly at the limit cannot tell a line that fit from one that was
/// cut, and a cut JSON line is very likely still parseable with fewer terms in
/// it — which would index a document by part of itself and say nothing.
fn read_line<R: BufRead>(reader: &mut R, what: &str, limit: u64) -> Result<Option<String>, String> {
    let mut bytes = Vec::new();
    // Spelled as a call rather than `reader.take(..)`, which resolves through
    // the reference and tries to move the reader itself.
    let mut limited = Read::take(reader, limit + 1);
    let read = limited
        .read_until(b'\n', &mut bytes)
        .map_err(|err| format!("sparse encoding {what} could not be read: {err}"))?;
    if read == 0 {
        return Ok(None);
    }
    if read as u64 > limit {
        return Err(format!(
            "sparse encoding {what} is longer than the {limit}-byte line limit"
        ));
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| format!("sparse encoding {what} is not valid UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn scratch(contents: &str) -> std::path::PathBuf {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "dcgrep-ingest-{}-{}.jsonl",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        fs::write(&path, contents).expect("write fixture");
        path
    }

    /// A header whose parity samples this build genuinely reproduces.
    fn header(parity: &str) -> String {
        // Not a raw string: the vocabulary deliberately contains `##json`, the
        // WordPiece continuation shape, and `"#` closes a raw literal.
        format!(
            "{{\"schema\":1,\"model\":\"test\",\"vocabulary\":\"wordpiece-30522\",\
             \"vocab\":[\"[UNK]\",\"parse\",\"##json\",\"server\"],\
             \"query_weights\":[0.0,1.5,0.5,0.25],\
             \"parity\":[{parity}]}}"
        )
    }

    #[test]
    fn a_well_formed_encoding_reads_back() {
        let path = scratch(&format!(
            "{}\n{}\n{}\n",
            header(r#"{"text":"parse","ids":[1]},{"text":"server","ids":[3]}"#),
            r#"{"path":"src/a.rs","total_terms":10,"terms":[[1,0.9],[2,0.4]]}"#,
            r#"{"path":"src/b.rs","total_terms":5,"terms":[[3,0.2]]}"#,
        ));
        let encoded = read(&path).expect("reads");
        assert_eq!(encoded.vocabulary, Vocabulary::WordPiece30522);
        assert_eq!(encoded.model, "test");
        assert_eq!(encoded.vocab.len(), 4);
        assert_eq!(encoded.documents.len(), 2);
        assert_eq!(encoded.documents["src/a.rs"].0, 10);
        assert_eq!(encoded.documents["src/a.rs"].1.len(), 2);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn a_tokeniser_disagreement_refuses_the_whole_encoding() {
        // The model claims "parse" is token 3. This build resolves it to 1.
        // Publishing anyway would rank every query against ids no document
        // carries, and the empty answer would look like a real one.
        let path = scratch(&format!(
            "{}\n{}\n",
            header(r#"{"text":"parse","ids":[3]}"#),
            r#"{"path":"src/a.rs","total_terms":10,"terms":[[1,0.9]]}"#,
        ));
        let err = read(&path).expect_err("must refuse");
        assert!(err.contains("disagrees with the model"), "{err}");
        assert!(err.contains("not published"), "{err}");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn an_encoding_without_parity_samples_is_refused() {
        let path = scratch(&format!(
            "{}\n{}\n",
            header(""),
            r#"{"path":"src/a.rs","total_terms":10,"terms":[[1,0.9]]}"#,
        ));
        let err = read(&path).expect_err("must refuse");
        assert!(err.contains("no tokeniser parity samples"), "{err}");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn a_token_outside_the_headers_own_vocabulary_is_refused() {
        let path = scratch(&format!(
            "{}\n{}\n",
            header(r#"{"text":"parse","ids":[1]}"#),
            r#"{"path":"src/a.rs","total_terms":10,"terms":[[99,0.9]]}"#,
        ));
        let err = read(&path).expect_err("must refuse");
        assert!(err.contains("outside the"), "{err}");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn a_file_written_on_windows_reads_the_same_as_one_written_on_unix() {
        // CRLF is not a corruption to reject: a producer run on Windows emits
        // it, and refusing would make the tier unusable there while looking
        // like a malformed-file error. The trailing carriage return sits
        // outside each line's JSON document, so both spellings must parse to
        // exactly the same encoding.
        let body = format!(
            "{}\n{}\n",
            header(r#"{"text":"parse","ids":[1]}"#),
            r#"{"path":"src/a.rs","total_terms":10,"terms":[[1,0.9]]}"#,
        );
        let unix = scratch(&body);
        let windows = scratch(&body.replace('\n', "\r\n"));
        let left = read(&unix).expect("unix reads");
        let right = read(&windows).expect("windows reads");
        assert_eq!(left.documents.len(), right.documents.len());
        assert_eq!(left.vocab, right.vocab);
        // And the path carries no carriage return, which would make it name a
        // file the walk can never match.
        assert!(right.documents.contains_key("src/a.rs"), "{right:?}");
        let _ = fs::remove_file(unix);
        let _ = fs::remove_file(windows);
    }

    #[test]
    fn a_line_past_its_own_bound_is_refused_while_being_read() {
        // The header and a document have different bounds, because a document
        // that is allowed to be as large as a whole vocabulary is not bounded
        // by anything a real document could reach.
        let mut line = String::from(r#"{"path":"src/a.rs","total_terms":10,"terms":["#);
        // Comfortably past MAX_DOCUMENT_BYTES, built from repeated postings so
        // it is the size that refuses it rather than the shape.
        while line.len() as u64 <= MAX_DOCUMENT_BYTES {
            line.push_str("[1,0.9],");
        }
        line.push_str("[1,0.9]]}");
        let path = scratch(&format!(
            "{}\n{line}\n",
            header(r#"{"text":"parse","ids":[1]}"#)
        ));
        let err = read(&path).expect_err("must refuse");
        assert!(err.contains("line limit"), "{err}");
        assert!(err.contains("document"), "{err}");
        let _ = fs::remove_file(path);
    }

    /// Per-item bounds do not bound their product.
    ///
    /// `MAX_DOCUMENTS` and `MAX_TERMS_PER_DOCUMENT` each look modest and
    /// multiply to four billion postings, every one of which is parsed and
    /// held in memory before the index that can hold four *million* of them
    /// discards the rest. Measured at about 7 bytes per posting held, the
    /// declared ceilings come to roughly 29 GB of resident memory for an index
    /// that cannot use a thousandth of it.
    ///
    /// Not a security bound — the encoding path is operator-typed — but a
    /// robustness one: a monorepo encoding that is merely large should be
    /// refused by name rather than by the machine running out of memory.
    #[test]
    fn an_encoding_whose_postings_exceed_the_ingest_budget_is_refused() {
        // Documents each well inside every per-item bound, so only the total
        // can refuse this. Driven through a small budget rather than the real
        // one: the behaviour is identical and the fixture is 300 bytes instead
        // of 400 MB.
        let budget = 20usize;
        let terms: String = (0..8)
            .map(|i| format!("[{},0.5]", i % 4))
            .collect::<Vec<_>>()
            .join(",");
        let mut text = format!("{}\n", header(r#"{"text":"parse","ids":[1]}"#));
        for i in 0..4 {
            text.push_str(&format!(
                "{{\"path\":\"src/f{i}.rs\",\"total_terms\":10,\"terms\":[{terms}]}}\n"
            ));
        }
        let path = scratch(&text);
        let err = read_with_budget(&path, budget).expect_err("must refuse");
        assert!(
            err.contains("postings") && err.contains(&budget.to_string()),
            "the refusal must name the budget and what exceeded it: {err}"
        );
        // The same file inside the budget reads, so it is the total that
        // refused and not the shape of the documents.
        let ok = read_with_budget(&path, 1_000).expect("inside the budget it reads");
        assert_eq!(ok.documents.len(), 4);
        let _ = fs::remove_file(path);
    }

    /// `read` must pass the real constant, not a smaller one.
    ///
    /// The seam above makes the budget injectable, which is also how a seam
    /// becomes a way for production to run unbounded while the tests all pass.
    #[test]
    fn the_budget_read_enforces_is_the_declared_one() {
        let terms: String = (0..8)
            .map(|i| format!("[{},0.5]", i % 4))
            .collect::<Vec<_>>()
            .join(",");
        let path = scratch(&format!(
            "{}\n{{\"path\":\"src/a.rs\",\"total_terms\":10,\"terms\":[{terms}]}}\n",
            header(r#"{"text":"parse","ids":[1]}"#)
        ));
        // Eight postings: refused by a budget of four, accepted by the real
        // one. If `read` had been left on a token budget this would refuse.
        assert!(read_with_budget(&path, 4).is_err());
        assert!(read(&path).is_ok());
        let _ = fs::remove_file(path);
    }

    // The budget's relationship to what the store can hold is a fact about two
    // constants, so it sits beside them as a `const` assertion and fails the
    // build rather than a test run. What remains a test is the behaviour:
    // `an_encoding_whose_postings_exceed_the_ingest_budget_is_refused` and
    // `the_budget_read_enforces_is_the_declared_one`.

    #[test]
    fn a_key_this_schema_does_not_know_refuses_rather_than_being_ignored() {
        // Silently dropping an unrecognised key means reading a file from a
        // producer this build does not understand and reporting success. The
        // schema number is how a producer asks for new keys.
        let path = scratch(&format!(
            "{}\n{}\n",
            header(r#"{"text":"parse","ids":[1]}"#),
            r#"{"path":"src/a.rs","total_terms":10,"terms":[[1,0.9]],"weights_v2":[1]}"#,
        ));
        let err = read(&path).expect_err("must refuse");
        assert!(err.contains("weights_v2"), "{err}");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn a_duplicate_path_is_refused_rather_than_letting_one_win() {
        let path = scratch(&format!(
            "{}\n{}\n{}\n",
            header(r#"{"text":"parse","ids":[1]}"#),
            r#"{"path":"src/a.rs","total_terms":10,"terms":[[1,0.9]]}"#,
            r#"{"path":"src/a.rs","total_terms":20,"terms":[[2,0.1]]}"#,
        ));
        let err = read(&path).expect_err("must refuse");
        assert!(err.contains("twice"), "{err}");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn a_mismatched_vocabulary_and_weight_table_is_refused() {
        let path = scratch(
            r#"{"schema":1,"model":"t","vocabulary":"wordpiece-30522","vocab":["[UNK]","a"],"query_weights":[0.0],"parity":[{"text":"a","ids":[1]}]}"#,
        );
        let err = read(&path).expect_err("must refuse");
        assert!(err.contains("must agree"), "{err}");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn a_wrong_schema_or_unknown_vocabulary_is_refused_by_name() {
        let path = scratch(
            r#"{"schema":9,"model":"t","vocabulary":"wordpiece-30522","vocab":["a"],"query_weights":[1.0],"parity":[]}"#,
        );
        assert!(read(&path).unwrap_err().contains("schema 9"));
        let _ = fs::remove_file(path);

        let path = scratch(
            r#"{"schema":1,"model":"t","vocabulary":"martian","vocab":["a"],"query_weights":[1.0],"parity":[]}"#,
        );
        assert!(read(&path).unwrap_err().contains("does not know"));
        let _ = fs::remove_file(path);

        // code-v1 is built from the repository, never imported.
        let path = scratch(
            r#"{"schema":1,"model":"t","vocabulary":"code-v1","vocab":["a"],"query_weights":[1.0],"parity":[]}"#,
        );
        assert!(read(&path).unwrap_err().contains("not imported"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn an_empty_or_unparseable_file_is_refused_rather_than_read_as_nothing() {
        let path = scratch("");
        assert!(read(&path).unwrap_err().contains("is empty"));
        let _ = fs::remove_file(path);

        let path = scratch("not json at all\n");
        assert!(read(&path).unwrap_err().contains("not valid JSON"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn a_non_finite_weight_is_refused() {
        // JSON has no NaN or infinity literal, so a producer that computed one
        // emits an enormous number instead. 1e39 parses as a perfectly ordinary
        // f64 and becomes `inf` on the way into f32 — silently, since a cast
        // does not fail — which is the case this check exists for.
        let path = scratch(&format!(
            "{}\n{}\n",
            header(r#"{"text":"parse","ids":[1]}"#),
            r#"{"path":"src/a.rs","total_terms":10,"terms":[[1,1e39]]}"#,
        ));
        let err = read(&path).expect_err("must refuse");
        assert!(err.contains("non-finite"), "{err}");
        let _ = fs::remove_file(path);

        // And one too large for f64 is refused a step earlier, by the parser.
        // Different message, same outcome: nothing is published.
        let path = scratch(&format!(
            "{}\n{}\n",
            header(r#"{"text":"parse","ids":[1]}"#),
            r#"{"path":"src/a.rs","total_terms":10,"terms":[[1,1e400]]}"#,
        ));
        assert!(
            read(&path).is_err(),
            "an unrepresentable weight must refuse"
        );
        let _ = fs::remove_file(path);
    }
}
