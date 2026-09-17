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
//!   1. strip control characters, normalise whitespace;
//!   2. lowercase, and fold the accent off a Latin letter that carries one;
//!   3. surround each CJK character with spaces, so each is its own token;
//!   4. split on whitespace, then split punctuation into single-character
//!      tokens;
//!   5. greedy longest-match-first WordPiece over each remaining word, with
//!      continuation pieces prefixed `##`, and `[UNK]` for a word no prefix
//!      of which is in the vocabulary.
//!
//! Anything it gets wrong is caught by the parity gate rather than shipped.

use std::collections::HashMap;

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
    let mut ids = Vec::new();
    for word in basic_tokenize(text) {
        if ids.len() >= MAX_TOKENS {
            break;
        }
        for piece in wordpiece(&word, vocab) {
            if ids.len() >= MAX_TOKENS {
                break;
            }
            ids.push(piece);
        }
    }
    ids
}

/// Steps 1 to 4: everything before WordPiece.
///
/// Returns the words WordPiece will be applied to, already lowercased, accent
/// folded, and split at punctuation and CJK boundaries.
pub(crate) fn basic_tokenize(text: &str) -> Vec<String> {
    let mut cleaned = String::with_capacity(text.len());
    for ch in text.chars() {
        // U+FFFD and NUL are dropped rather than tokenised, matching the
        // reference implementation; other control characters become nothing,
        // and whitespace becomes a plain space.
        if ch == '\u{fffd}' || ch == '\0' {
            continue;
        }
        if ch.is_whitespace() {
            cleaned.push(' ');
            continue;
        }
        if ch.is_control() {
            continue;
        }
        if is_cjk(ch) {
            // Each CJK character is its own token, so it is surrounded by
            // spaces before the whitespace split rather than after.
            cleaned.push(' ');
            cleaned.push(ch);
            cleaned.push(' ');
            continue;
        }
        for lowered in ch.to_lowercase() {
            cleaned.push(fold_accent(lowered));
        }
    }

    let mut words = Vec::new();
    for chunk in cleaned.split_whitespace() {
        let mut current = String::new();
        for ch in chunk.chars() {
            if is_punctuation(ch) {
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
                words.push(ch.to_string());
            } else {
                current.push(ch);
            }
        }
        if !current.is_empty() {
            words.push(current);
        }
    }
    words
}

/// Step 5: greedy longest-match-first over one word.
fn wordpiece(word: &str, vocab: &impl Vocab) -> Vec<u32> {
    let chars: Vec<char> = word.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }
    if chars.len() > MAX_CHARS_PER_WORD {
        return unknown(vocab);
    }

    let mut pieces = Vec::new();
    let mut start = 0usize;
    while start < chars.len() {
        let mut end = chars.len();
        let mut found: Option<u32> = None;
        while start < end {
            let mut candidate: String = chars[start..end].iter().collect();
            if start > 0 {
                candidate.insert_str(0, "##");
            }
            if let Some(id) = vocab.token_id(&candidate) {
                found = Some(id);
                break;
            }
            end -= 1;
        }
        match found {
            Some(id) => {
                pieces.push(id);
                start = end;
            }
            // No prefix of what remains is in the vocabulary, so the *whole*
            // word is unknown — not the part matched so far. Emitting the
            // prefix pieces would be a different token sequence from the one
            // the document side produced.
            None => return unknown(vocab),
        }
    }
    pieces
}

fn unknown(vocab: &impl Vocab) -> Vec<u32> {
    match vocab.token_id(UNKNOWN) {
        Some(id) => vec![id],
        None => Vec::new(),
    }
}

/// Whether a character is one BERT treats as its own token.
///
/// The CJK ranges from the reference implementation, verbatim. Hiragana and
/// katakana are deliberately not here: BERT does not split them per character
/// and neither does this.
fn is_cjk(ch: char) -> bool {
    let code = ch as u32;
    (0x4E00..=0x9FFF).contains(&code)
        || (0x3400..=0x4DBF).contains(&code)
        || (0x20000..=0x2A6DF).contains(&code)
        || (0x2A700..=0x2B73F).contains(&code)
        || (0x2B740..=0x2B81F).contains(&code)
        || (0x2B820..=0x2CEAF).contains(&code)
        || (0xF900..=0xFAFF).contains(&code)
        || (0x2F800..=0x2FA1F).contains(&code)
}

/// Whether a character is split off as its own token.
///
/// BERT's rule is wider than Unicode's punctuation category: every ASCII
/// character that is not alphanumeric counts, which is why `_` and `` ` ``
/// are punctuation here and are not in `char::is_ascii_punctuation` terms
/// alone.
fn is_punctuation(ch: char) -> bool {
    let code = ch as u32;
    if (33..=47).contains(&code)
        || (58..=64).contains(&code)
        || (91..=96).contains(&code)
        || (123..=126).contains(&code)
    {
        return true;
    }
    ch.is_ascii_punctuation() || unicode_punctuation(ch)
}

fn unicode_punctuation(ch: char) -> bool {
    // The general categories P* and S*, approximated by the ranges that carry
    // them. Anything this misses is caught by the parity gate.
    let code = ch as u32;
    (0x2000..=0x206F).contains(&code)
        || (0x2E00..=0x2E7F).contains(&code)
        || (0x3000..=0x303F).contains(&code)
        || (0xFF00..=0xFF0F).contains(&code)
        || (0xFF1A..=0xFF20).contains(&code)
        || (0xFF3B..=0xFF40).contains(&code)
        || (0xFF5B..=0xFF65).contains(&code)
        || matches!(ch, '\u{00A1}' | '\u{00BF}' | '\u{00AB}' | '\u{00BB}')
}

/// Removes the accent from a precomposed Latin letter.
///
/// The uncased pipeline decomposes and drops combining marks. Doing that
/// properly needs a Unicode normalisation table, which is a dependency; this
/// covers Latin-1 Supplement and Latin Extended-A, which is where essentially
/// every accented Latin letter in source code and documentation lives.
///
/// What it does not cover is not guessed at: it passes the character through
/// unchanged, the word then tokenises differently from the model's own
/// tokeniser, and the parity gate refuses the build and names the text. A
/// silent near-miss here is the one outcome that must not happen.
fn fold_accent(ch: char) -> char {
    const LATIN1: &str =
        "aaaaaaaceeeeiiiidnooooo\u{00d7}ouuuuypsaaaaaaaceeeeiiiidnooooo\u{00f7}ouuuuypy";
    let code = ch as u32;
    if (0x00C0..=0x00FF).contains(&code) {
        return LATIN1.chars().nth((code - 0x00C0) as usize).unwrap_or(ch);
    }
    if (0x0100..=0x017F).contains(&code) {
        // Latin Extended-A is laid out in upper/lower pairs by base letter.
        const EXTENDED: &[(u32, u32, char)] = &[
            (0x0100, 0x0105, 'a'),
            (0x0106, 0x010D, 'c'),
            (0x010E, 0x0111, 'd'),
            (0x0112, 0x011B, 'e'),
            (0x011C, 0x0123, 'g'),
            (0x0124, 0x0127, 'h'),
            (0x0128, 0x0131, 'i'),
            (0x0134, 0x0135, 'j'),
            (0x0136, 0x0138, 'k'),
            (0x0139, 0x0142, 'l'),
            (0x0143, 0x014B, 'n'),
            (0x014C, 0x0153, 'o'),
            (0x0154, 0x0159, 'r'),
            (0x015A, 0x0161, 's'),
            (0x0162, 0x0167, 't'),
            (0x0168, 0x0173, 'u'),
            (0x0174, 0x0175, 'w'),
            (0x0176, 0x0178, 'y'),
            (0x0179, 0x017E, 'z'),
        ];
        for (low, high, base) in EXTENDED {
            if (*low..=*high).contains(&code) {
                return *base;
            }
        }
    }
    ch
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexical::store::{Builder, Vocabulary};

    fn reader_with(tokens: &[&str]) -> Reader {
        let vocab: Vec<String> = tokens.iter().map(|t| t.to_string()).collect();
        let weights = vec![1.0f32; vocab.len()];
        let mut builder = Builder::new(Vocabulary::WordPiece30522);
        builder.set_query_side(vocab, weights).expect("query side");
        builder.add("a.rs".into(), 1, &[(0, 1.0)]);
        Reader::open(builder.finish()).expect("opens")
    }

    fn ids(text: &str, tokens: &[&str]) -> Vec<String> {
        let reader = reader_with(tokens);
        token_ids(text, &reader)
            .into_iter()
            // The builder assigned ids by position in `tokens`, so this
            // decodes the answer without asking the reader to confirm it.
            .map(|id| tokens[id as usize].to_string())
            .collect()
    }

    #[test]
    fn a_word_in_the_vocabulary_is_one_token() {
        assert_eq!(ids("parse", &["[UNK]", "parse"]), vec!["parse"]);
    }

    #[test]
    fn a_word_out_of_it_is_split_longest_match_first() {
        assert_eq!(
            ids("parsing", &["[UNK]", "parse", "par", "##sing", "##s"]),
            vec!["par", "##sing"],
            "the longest prefix wins, then the longest continuation"
        );
    }

    #[test]
    fn a_word_with_no_usable_prefix_is_wholly_unknown() {
        // Not "par" plus a failure: the reference emits one [UNK] for the
        // whole word, and a partial split would be a different id sequence
        // from the one the document side produced.
        assert_eq!(ids("parzival", &["[UNK]", "par", "##zi"]), vec!["[UNK]"]);
    }

    #[test]
    fn case_is_folded_before_lookup() {
        assert_eq!(
            ids("PARSE Parse", &["[UNK]", "parse"]),
            vec!["parse", "parse"]
        );
    }

    #[test]
    fn accents_are_folded_off_latin_letters() {
        assert_eq!(ids("café", &["[UNK]", "cafe"]), vec!["cafe"]);
        assert_eq!(ids("ÄRGER", &["[UNK]", "arger"]), vec!["arger"]);
        assert_eq!(ids("Łódź", &["[UNK]", "lodz"]), vec!["lodz"]);
    }

    #[test]
    fn punctuation_is_split_into_single_tokens() {
        assert_eq!(
            ids("parse(json)", &["[UNK]", "parse", "json", "(", ")"]),
            vec!["parse", "(", "json", ")"]
        );
        // Underscore is punctuation to BERT even though it is not to Unicode.
        assert_eq!(
            ids("parse_json", &["[UNK]", "parse", "json", "_"]),
            vec!["parse", "_", "json"]
        );
    }

    #[test]
    fn each_cjk_character_is_its_own_token() {
        assert_eq!(
            ids("日本語", &["[UNK]", "日", "本", "語"]),
            vec!["日", "本", "語"]
        );
    }

    #[test]
    fn whitespace_and_control_characters_do_not_become_tokens() {
        assert_eq!(
            ids("  parse\t\n\r parse\u{0}  ", &["[UNK]", "parse"]),
            vec!["parse", "parse"]
        );
    }

    #[test]
    fn an_over_long_word_is_unknown_exactly_as_the_reference_has_it() {
        let long = "a".repeat(MAX_CHARS_PER_WORD + 1);
        assert_eq!(ids(&long, &["[UNK]", "a", "##a"]), vec!["[UNK]"]);
        let at_limit = "a".repeat(MAX_CHARS_PER_WORD);
        assert_ne!(ids(&at_limit, &["[UNK]", "a", "##a"]), vec!["[UNK]"]);
    }

    #[test]
    fn a_vocabulary_with_no_unknown_token_drops_rather_than_invents() {
        assert_eq!(ids("zzzz", &["parse"]), Vec::<String>::new());
    }

    #[test]
    fn repeats_are_kept_because_a_repeated_query_term_weighs_more() {
        assert_eq!(
            ids("parse parse parse", &["[UNK]", "parse"]),
            vec!["parse", "parse", "parse"]
        );
    }

    #[test]
    fn the_token_ceiling_stops_a_pathological_query() {
        let reader = reader_with(&["[UNK]", "parse"]);
        let text = "parse ".repeat(MAX_TOKENS + 100);
        assert_eq!(token_ids(&text, &reader).len(), MAX_TOKENS);
    }
}
