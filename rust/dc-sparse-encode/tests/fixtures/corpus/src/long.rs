//! BERT WordPiece tokenisation, for the query side of a learned index.
//!
//! Every `opensearch-neural-sparse-encoding-doc-*` model and SPLADE encode
//! documents with a neural network and queries with "a tokeniser and a weight
//! look-up table". This is that tokeniser. It runs in the query path, so it
//! must not need a model, a runtime, or a download — the vocabulary it works
//! from is the one stored in the index itself.
//!
//! ## The failure this module is built around
//!
//! Documents are tokenised in Python, by the model's own tokeniser, offline.
//! Queries are tokenised here, in Rust. Two implementations of one algorithm,
//! and the failure mode of a private reimplementation is not a crash: it is
//! the two sides disagreeing about what `parseJSON` splits into, so a query
//! scores against ids no document ever carried and the answer comes back
//! empty, ranked, and wrong.
//!
//! So this is never trusted to agree. The producer emits a sample of
//! `(text, token ids)` pairs from the real tokeniser, and an index build
//! replays every one of them through this code and refuses to publish on the
//! first disagreement — naming the text, the ids expected and the ids
//! produced. The same contract `dc-glob` holds against CPython's `fnmatch`,
//! for the same reason.
//!
//! ## What it implements
//!
//! The uncased pipeline, which is what those models use:
//!
//!   1. drop NUL, U+FFFD, and control/format/surrogate/private-use characters;
//!      turn the four literal whitespace characters and category `Zs` into
//!      spaces;
//!   2. surround each CJK ideograph with spaces, so each is its own token;
//!   3. compose (NFC), then split on whitespace;
//!   4. per word: lowercase, decompose (NFD) and drop every non-spacing mark;
//!   5. split punctuation — ASCII non-alphanumerics and category `P*` — into
//!      single-character tokens;
//!   6. greedy longest-match-first WordPiece over each remaining word, with
//!      continuation pieces prefixed `##`, and `[UNK]` for a word no prefix
//!      of which is in the vocabulary.
//!
//! Step 4 is *decomposition*, not transliteration, and the difference is not
//! academic: `é` is `e` plus a mark and loses it, while `ł`, `ß`, `ø`, `æ`,
//! `ð` and `þ` decompose to themselves and are kept. This was a hand-written
//! fold table that mapped them to `l`, `s`, `o`, `a`, `d` and `p`, and it
//! disagreed with the real tokeniser on 22% of a codepoint sweep.
//!
//! `tests/fixtures/wordpiece-*` hold the real tokeniser's answers for 14810
//! inputs, and `the_tokeniser_reproduces_the_model_s_own` checks every one on
//! an ordinary `cargo test`. Anything that still gets through is caught by the
//! parity gate at build time rather than shipped.
//!
//! One divergence is known, bounded and deliberate. This build's Unicode
//! tables are *newer* than the ones the model's tokeniser was built against,
//! so on 98 codepoints — Arabic Extended-A/B marks, Indic marks, Supplemental
//! Punctuation, all assigned in Unicode 10 through 14 — this build strips a
//! mark or splits a word where the reference sees an unassigned character and
//! keeps it. The reference then emits `[UNK]` for the whole word. The effect
//! is a miss rather than a wrong answer: a query holding one of these produces
//! real tokens while the document that should match it was indexed under
//! `[UNK]`. `TABLE_SKEW` in the test names all 98 and asserts the set both
//! ways — nothing outside it may diverge, and everything inside it still must,
//! so a crate update that closes one fails the test instead of being absorbed.

use std::collections::HashMap;

use unicode_normalization::UnicodeNormalization;
use unicode_properties::{GeneralCategory, GeneralCategoryGroup, UnicodeGeneralCategory};

use super::store::Reader;

/// Anything that can resolve a token to its id.
///
/// A trait rather than `&Reader` because the parity gate has to run *before*
/// an index exists: the whole point of the gate is to refuse to publish one
/// whose query tokeniser disagrees with the model's, and it cannot do that
/// through a reader over the file it is deciding whether to write.
pub(crate) trait Vocab {
    fn token_id(&self, token: &str) -> Option<u32>;
}

impl Vocab for Reader {
    fn token_id(&self, token: &str) -> Option<u32> {
        Reader::token_id(self, token)
    }
}

impl Vocab for HashMap<String, u32> {
    fn token_id(&self, token: &str) -> Option<u32> {
        self.get(token).copied()
    }
}

/// Longest word handed to the WordPiece loop.
///
/// BERT's own tokeniser gives up at 100 characters and emits `[UNK]`, and
/// matching that is not a choice: a longer word tokenised differently here
/// than there is exactly the divergence this module exists to avoid.
pub(crate) const MAX_CHARS_PER_WORD: usize = 100;

/// Tokens taken from one query.
///
/// A query is a phrase; this is the bound on the pathological one.
pub(crate) const MAX_TOKENS: usize = 512;

/// The token a word falls back to. Present in every BERT vocabulary.
const UNKNOWN: &str = "[UNK]";

/// Replays a parity sample and reports the first disagreement.
///
/// The producer emits these from the model's own tokeniser. Replaying them
/// here is the only thing standing between a private reimplementation and the
/// failure it always has: not a crash, but two sides quietly disagreeing about
/// what a word splits into, so every query scores against ids no document
/// carries and the empty answer looks like a real one.
pub(crate) fn check_parity(
    samples: &[(String, Vec<u32>)],
    vocab: &impl Vocab,
) -> Result<(), String> {
    for (text, expected) in samples {
        let got = token_ids(text, vocab);
        if &got != expected {
            return Err(format!(
                "this build's query tokeniser disagrees with the model's on {text:?}: \
                 the model produced {expected:?} and this produced {got:?}. The index was \
                 not published, because a query tokenised differently from the documents \
                 returns a ranking over ids nothing was indexed under"
            ));
        }
    }
    Ok(())
}

/// Turns query text into token ids against the index's stored vocabulary.
///
/// Returns the ids in order, with repeats, because a term repeated in a query
/// weighs more — the same reason the document side counts term frequency.
/// `[UNK]` is emitted as its own id when the vocabulary carries one and
/// dropped when it does not, since a model without an unknown token has no
/// way to represent one.
pub(crate) fn token_ids(text: &str, vocab: &impl Vocab) -> Vec<u32> {
