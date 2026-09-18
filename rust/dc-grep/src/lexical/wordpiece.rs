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
/// The order is the reference implementation's, and it is load-bearing rather
/// than incidental. Cleaning happens before the CJK padding so a control
/// character cannot separate two ideographs; the lowercase and accent pass
/// happens per whitespace-delimited word rather than per character, because
/// `İ` lowercases to two characters and only the second is the mark that gets
/// dropped; and punctuation is split last, so a word is already folded when it
/// is cut. Reordering any of these changes the answer for some input, and the
/// parity gate is what would find out.
pub(crate) fn basic_tokenize(text: &str) -> Vec<String> {
    let mut cleaned = String::with_capacity(text.len());
    for ch in text.chars() {
        // The control test comes first, and its three exceptions are the
        // reason: `\t`, `\n` and `\r` are `Cc` but survive to become spaces,
        // while `\x0b` and `\x0c` are dropped entirely. Asking "is this
        // whitespace?" first turns those two into spaces and splits a word
        // the reference keeps whole.
        if ch == '\0' || ch == '\u{fffd}' {
            continue;
        }
        if is_control(ch) {
            continue;
        }
        if is_space(ch) {
            cleaned.push(' ');
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
        cleaned.push(ch);
    }

    // Composed before splitting, so `e` + U+0301 and `é` are one word either
    // way. The decomposition below then treats them identically, which is the
    // property that makes a query typed with combining marks find a document
    // written with precomposed ones.
    let composed: String = cleaned.nfc().collect();

    let mut words = Vec::new();
    for chunk in composed.split_whitespace() {
        let folded = lower_and_strip_accents(chunk);
        let mut current = String::new();
        for ch in folded.chars() {
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

/// Lowercases, then removes every non-spacing mark.
///
/// This is `unicodedata.normalize("NFD", token)` followed by dropping category
/// `Mn`, which is *canonical decomposition* and not transliteration. The
/// distinction is the whole point: `é` decomposes to `e` plus an acute mark
/// and loses the mark, while `ł`, `ß`, `ø`, `æ`, `ð` and `þ` decompose to
/// themselves and are kept. A table that mapped those to `l`, `s`, `o`, `a`,
/// `d` and `p` — which is what this used to be — produces ids the model never
/// assigned, for every word containing one.
///
/// Hangul, Greek, Cyrillic, Hebrew, Arabic, Devanagari and Thai all fall out
/// of the same rule rather than needing cases of their own.
fn lower_and_strip_accents(word: &str) -> String {
    word.chars()
        .flat_map(char::to_lowercase)
        .nfd()
        .filter(|ch| ch.general_category() != GeneralCategory::NonspacingMark)
        .collect()
}

/// Control, format, surrogate and private-use — but *not* unassigned.
///
/// Tab, newline and carriage return are `Cc` and are deliberately excluded, so
/// they reach the whitespace branch and become spaces.
///
/// The exclusion of `Cn` is the part worth explaining, because the whole
/// category group `C*` is the obvious reading and it is wrong. The reference
/// implementation these models ship with keeps an unassigned codepoint, lets
/// it join the word around it, and lets WordPiece fail on it — so `x\u{378}y`
/// is `[UNK]` there and was nothing at all here. Measured: 109 of the 110
/// codepoints still disagreeing after the rest of this rewrite were `Cn`.
///
/// It also means a codepoint assigned after the version of the tables linked
/// here will be treated as unassigned. That is a real skew and it is survivable
/// for exactly one reason: the parity gate compares this build against the
/// producer's own tokeniser on every build, so the skew becomes a named
/// refusal rather than a ranking nobody can explain.
fn is_control(ch: char) -> bool {
    if matches!(ch, '\t' | '\n' | '\r') {
        return false;
    }
    matches!(
        ch.general_category(),
        GeneralCategory::Control
            | GeneralCategory::Format
            | GeneralCategory::Surrogate
            | GeneralCategory::PrivateUse
    )
}

/// The four literal whitespace characters, plus category `Zs`.
///
/// Narrower than `char::is_whitespace`, which also answers yes for `\x0b`,
/// `\x0c` and the line/paragraph separators — all of which the reference has
/// already discarded as control characters by this point.
fn is_space(ch: char) -> bool {
    matches!(ch, ' ' | '\t' | '\n' | '\r')
        || ch.general_category() == GeneralCategory::SpaceSeparator
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
/// BERT's rule is the union of two things: every ASCII character that is not
/// alphanumeric — which is why `_`, `` ` `` and `$` count — and the Unicode
/// general category `P*`. The second half used to be a list of ranges I wrote
/// out, and it missed `§`, which is `Po` and appears in this repository's own
/// prose. Asking the category is both shorter and right.
fn is_punctuation(ch: char) -> bool {
    let code = ch as u32;
    if (33..=47).contains(&code)
        || (58..=64).contains(&code)
        || (91..=96).contains(&code)
        || (123..=126).contains(&code)
    {
        return true;
    }
    ch.general_category_group() == GeneralCategoryGroup::Punctuation
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

    /// No text makes the tokeniser panic, overrun its ceiling, or invent an id.
    ///
    /// This is the function that turns an untrusted query into the ids a
    /// ranking is computed from, so its input is whatever a caller typed. The
    /// tests around it check specific decisions; this checks that the
    /// decisions hold for text nobody chose — including the shapes that break
    /// tokenisers: lone combining marks with no base, a hundred marks on one
    /// letter, CJK abutting punctuation, words past the 100-character
    /// reference ceiling, and the isolated surrogate range's neighbours.
    ///
    /// Seeded, so a failure reproduces.
    #[test]
    fn no_text_makes_the_tokeniser_panic_overrun_or_invent() {
        const TOKENS: &[&str] = &[
            "[UNK]", "parse", "json", "##json", "##s", "a", "##a", "日", "本", "-", ".", "'",
        ];
        let reader = reader_with(TOKENS);

        // Blocks chosen to include what a random codepoint sweep reaches
        // rarely and what breaks tokenisers often.
        const BLOCKS: &[(u32, u32)] = &[
            (0x0009, 0x007F),
            (0x0080, 0x00FF),
            (0x0300, 0x036F),
            (0x0590, 0x08FF),
            (0x0900, 0x0DFF),
            (0x1DC0, 0x1DFF),
            (0x2000, 0x206F),
            (0x2E00, 0x2E7F),
            (0x3000, 0x30FF),
            (0x4E00, 0x4F00),
            (0xFE00, 0xFEFF),
            (0xFF00, 0xFFEF),
            (0x1F300, 0x1F5FF),
            (0x10FF00, 0x10FFFF),
        ];

        let mut state = 0x2026_0917u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let pick = move |rng: &mut dyn FnMut() -> u64| -> char {
            loop {
                let (lo, hi) = BLOCKS[(rng() as usize) % BLOCKS.len()];
                let cp = lo + (rng() as u32) % (hi - lo + 1);
                if let Some(ch) = char::from_u32(cp) {
                    return ch;
                }
            }
        };

        let mut longest = 0usize;
        for round in 0..30_000 {
            let mut text = String::new();
            match round % 5 {
                // Free-form text across the blocks.
                0 => {
                    for _ in 0..(next() as usize % 40) {
                        text.push(pick(&mut next));
                    }
                }
                // A base letter buried under marks, which is where NFD
                // reordering and mark-stripping meet.
                1 => {
                    text.push('a');
                    for _ in 0..(next() as usize % 120) {
                        text.push(char::from_u32(0x0300 + (next() as u32) % 0x70).expect("a mark"));
                    }
                }
                // Marks with no base at all.
                2 => {
                    for _ in 0..(next() as usize % 30) {
                        text.push(char::from_u32(0x0300 + (next() as u32) % 0x70).expect("a mark"));
                    }
                }
                // Past MAX_CHARS_PER_WORD, where the reference gives up.
                3 => {
                    let len = MAX_CHARS_PER_WORD + (next() as usize % 40);
                    for _ in 0..len {
                        text.push(if next() % 2 == 0 { 'a' } else { 'j' });
                    }
                }
                // Known words interleaved with random separators, so real
                // tokens and adversarial ones meet — and enough of them to
                // carry the run past MAX_TOKENS.
                //
                // The word count was `% 12` at first, which cannot produce 512
                // tokens however the pieces fall. Deleting the ceiling check
                // in `token_ids` then left this test green, which is how the
                // bound came to be asserted by a case that never reached it.
                _ => {
                    // Mostly short, occasionally long enough to pass the
                    // ceiling. Making every case long reached it too, at five
                    // times the runtime for the same one fact.
                    let words = if next() % 8 == 0 {
                        400 + next() as usize % 400
                    } else {
                        next() as usize % 12
                    };
                    for _ in 0..words {
                        text.push_str(["parse", "json", "日本", "a"][(next() as usize) % 4]);
                        text.push(pick(&mut next));
                    }
                }
            }

            let got = token_ids(&text, &reader);
            assert!(
                got.len() <= MAX_TOKENS,
                "{} tokens from {:?}, over the {MAX_TOKENS} ceiling",
                got.len(),
                text
            );
            for id in &got {
                assert!(
                    (*id as usize) < TOKENS.len(),
                    "id {id} is outside the {}-token vocabulary, from {:?}",
                    TOKENS.len(),
                    text
                );
            }
            // A query tokenised twice must rank the same twice. Sampled: the
            // second pass costs as much as the first, and determinism is a
            // property of the code rather than of a particular input, so every
            // tenth case buys the same confidence for a tenth of the time.
            if round % 10 == 0 {
                assert_eq!(
                    got,
                    token_ids(&text, &reader),
                    "not deterministic: {text:?}"
                );
            }
            longest = longest.max(got.len());
        }
        // The ceiling has to be *reached* for the assertion above to mean
        // anything. Without this, removing the bound in `token_ids` leaves
        // this test green.
        assert_eq!(
            longest, MAX_TOKENS,
            "the generator never reached the {MAX_TOKENS}-token ceiling \
             (longest was {longest}), so the check that it holds never ran"
        );
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
    fn a_mark_is_stripped_and_a_distinct_letter_is_not() {
        // The rule is canonical decomposition with every non-spacing mark
        // dropped — not transliteration. `é` decomposes into `e` plus an acute
        // that goes; `ł` decomposes into itself and stays.
        //
        // This test used to assert `Łódź` → `lodz`, and passed, because it was
        // written from the same wrong idea of the algorithm as the code it was
        // checking. The model's own tokeniser says `łodz`. A test can only
        // falsify a belief the author did not already share with the code.
        assert_eq!(ids("café", &["[UNK]", "cafe"]), vec!["cafe"]);
        assert_eq!(ids("ÄRGER", &["[UNK]", "arger"]), vec!["arger"]);
        assert_eq!(ids("Łódź", &["[UNK]", "łodz"]), vec!["łodz"]);

        // Letters that are not a base plus a mark, and so are never folded.
        for word in ["ß", "ø", "æ", "ð", "þ", "đ", "ı"] {
            assert_eq!(
                ids(word, &["[UNK]", word]),
                vec![word],
                "{word} is its own letter, not an accented one"
            );
        }

        // A mark typed separately lands where the precomposed character does,
        // which is what lets a query match a document that spelled it the
        // other way.
        assert_eq!(ids("e\u{301}cole", &["[UNK]", "ecole"]), vec!["ecole"]);
        assert_eq!(ids("École", &["[UNK]", "ecole"]), vec!["ecole"]);
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

    /// The tokeniser, judged against the real thing.
    ///
    /// `tests/fixtures/wordpiece-*` were recorded from
    /// `opensearch-neural-sparse-encoding-doc-v2-mini`'s own HuggingFace
    /// tokeniser: its full 30522-token vocabulary, and what it produced for
    /// 14810 inputs — every codepoint across 30 Unicode ranges both alone and
    /// inside a word, plus curated cases and a sample of lines out of this
    /// repository.
    ///
    /// The ranges were 22 and covered Devanagari but no other Indic block,
    /// Arabic but not Arabic Extended-A, and no Supplemental Punctuation. That
    /// is where 97 of the 98 `TABLE_SKEW` characters live, so the fixture
    /// reported one known divergence while a sweep of the whole space found
    /// 442. Widening the ranges is what turned that into a number this test
    /// can hold.
    ///
    /// Recorded rather than computed, so this runs on `cargo test` with no
    /// PyTorch, no model download and no network. A check that needs a 23 MB
    /// download to run is a check that stops running.
    ///
    /// It is worth saying what this replaced. The accent handling here was a
    /// hand-written transliteration table, which is a different algorithm from
    /// the canonical decomposition the reference performs: it turned `ł` into
    /// `l` and `ß` into `s`, where the reference keeps both. Against this
    /// corpus that was 1754 disagreements out of 7904 — 22%, over 95% inside
    /// Latin Extended Additional, and 100% of combining marks. A curated
    /// corpus had put it at 0.3%, because I had chosen the examples.
    ///
    /// `DCGREP_WORDPIECE_CONFORMANCE` points this at a different corpus. The
    /// committed one is sized to keep `cargo test` fast; a sweep large enough
    /// to be worth running is too large to commit, and a sweep that cannot be
    /// re-run against this code is a sweep whose result expires. Generate one
    /// with `scripts/encode-sparse.py --sweep`.
    #[test]
    fn the_tokeniser_reproduces_the_model_s_own() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/");
        let vocab_text =
            std::fs::read_to_string(format!("{dir}wordpiece-vocab.txt")).expect("vocab fixture");
        let vocab: HashMap<String, u32> = vocab_text
            .lines()
            .enumerate()
            .map(|(id, token)| (token.to_string(), id as u32))
            .collect();
        assert_eq!(vocab.len(), 30522, "the fixture is not a BERT vocabulary");

        let corpus_path = std::env::var("DCGREP_WORDPIECE_CONFORMANCE")
            .unwrap_or_else(|_| format!("{dir}wordpiece-conformance.jsonl"));
        let corpus = std::fs::read_to_string(&corpus_path)
            .unwrap_or_else(|err| panic!("conformance corpus {corpus_path}: {err}"));
        let mut lines = corpus.lines();
        lines.next().expect("header");

        // Where this build's Unicode tables and the model tokeniser's disagree
        // about what a character *is*.
        //
        // `unicode-properties` 0.1.4 knows codepoints assigned through roughly
        // Unicode 15. The tokeniser that encoded the documents was built
        // against older tables, so for each character below it sees an
        // unassigned codepoint where this build sees a mark or a punctuation
        // mark. This build then strips the mark or splits the word; the model
        // keeps the character and the whole word becomes `[UNK]`.
        //
        // The direction matters for judging the severity. The model's side is
        // the *less* specific one, so the effect is a miss and never a wrong
        // file: a query containing one of these produces real tokens while the
        // document it should find was indexed under `[UNK]`.
        //
        // Measured, not guessed. A 37,846-case sweep across every Unicode
        // block produced 442 disagreements, and bisecting each input down to
        // single characters accounted for all 442 with exactly these. The
        // fixture's earlier 22-range sweep found one of them, which is why
        // this was a single `char` until the sweep replaced it.
        //
        // Not fixed by matching the model's tables: that would mean pinning a
        // copy of another project's Unicode version and tracking it, to change
        // behaviour on 82 codepoints that are marks and medieval punctuation.
        // Written down instead, and asserted both ways below — nothing outside
        // this set may diverge, and everything in it must still diverge, so a
        // crate update that fixes one fails this test rather than widening it.
        const TABLE_SKEW: &[char] = &[
            '\u{61d}', '\u{7fd}', '\u{890}', '\u{891}', '\u{897}', '\u{898}', '\u{899}', '\u{89a}',
            '\u{89b}', '\u{89c}', '\u{89d}', '\u{89e}', '\u{89f}', '\u{8ca}', '\u{8cb}', '\u{8cc}',
            '\u{8cd}', '\u{8ce}', '\u{8cf}', '\u{8d0}', '\u{8d1}', '\u{8d2}', '\u{8d3}', '\u{8d4}',
            '\u{8d5}', '\u{8d6}', '\u{8d7}', '\u{8d8}', '\u{8d9}', '\u{8da}', '\u{8db}', '\u{8dc}',
            '\u{8dd}', '\u{8de}', '\u{8df}', '\u{8e0}', '\u{8e1}', '\u{8e2}', '\u{9fd}', '\u{9fe}',
            '\u{a76}', '\u{afa}', '\u{afb}', '\u{afc}', '\u{afd}', '\u{afe}', '\u{aff}', '\u{b55}',
            '\u{c04}', '\u{c3c}', '\u{c77}', '\u{c84}', '\u{d00}', '\u{d3b}', '\u{d3c}', '\u{d81}',
            '\u{eba}', '\u{ece}', '\u{166d}', '\u{180f}', '\u{1885}', '\u{1886}', '\u{1b4e}',
            '\u{1b4f}', '\u{1b7d}', '\u{1b7e}', '\u{1b7f}', '\u{1df6}', '\u{1df7}', '\u{1df8}',
            '\u{1df9}', '\u{1dfa}', '\u{1dfb}', '\u{2e43}', '\u{2e44}', '\u{2e45}', '\u{2e46}',
            '\u{2e47}', '\u{2e48}', '\u{2e49}', '\u{2e4a}', '\u{2e4b}', '\u{2e4c}', '\u{2e4d}',
            '\u{2e4e}', '\u{2e4f}', '\u{2e52}', '\u{2e53}', '\u{2e54}', '\u{2e55}', '\u{2e56}',
            '\u{2e57}', '\u{2e58}', '\u{2e59}', '\u{2e5a}', '\u{2e5b}', '\u{2e5c}', '\u{2e5d}',
        ];

        assert!(
            TABLE_SKEW.windows(2).all(|pair| pair[0] < pair[1]),
            "TABLE_SKEW must be sorted and free of duplicates; it is searched"
        );

        let mut unexpected = Vec::new();
        let mut blamed: Vec<char> = Vec::new();
        let mut skewed = 0usize;
        let mut checked = 0usize;
        for line in lines {
            let pair: serde_json::Value = serde_json::from_str(line).expect("pair");
            let text = pair["text"].as_str().expect("text");
            let want: Vec<u32> = pair["ids"]
                .as_array()
                .expect("ids")
                .iter()
                .map(|v| v.as_u64().expect("id") as u32)
                .collect();
            // The recorder truncates at the model's 512; so does this.
            if want.len() > MAX_TOKENS {
                continue;
            }
            checked += 1;
            if token_ids(text, &vocab) == want {
                continue;
            }
            let causes: Vec<char> = text
                .chars()
                .filter(|ch| TABLE_SKEW.binary_search(ch).is_ok())
                .collect();
            if causes.is_empty() {
                unexpected.push(text.to_string());
            } else {
                skewed += 1;
                blamed.extend(causes);
            }
        }

        // Over a large sweep the message below can only show a handful, and a
        // handful is not enough to tell a systematic skew from a real bug.
        // `DCGREP_WORDPIECE_DUMP` writes every disagreement for analysis.
        if let Ok(dump) = std::env::var("DCGREP_WORDPIECE_DUMP") {
            let rows: String = unexpected
                .iter()
                .map(|text| format!("{}\n", serde_json::json!({ "text": text })))
                .collect();
            std::fs::write(&dump, rows).expect("write the disagreement dump");
        }

        // Raised from 8,000 when the recorded ranges were widened to cover the
        // blocks TABLE_SKEW lives in. A floor that trails the fixture lets it
        // shrink back to the curated corpus without anything saying so.
        assert!(checked > 14_000, "the fixture shrank: only {checked} cases");
        assert!(
            unexpected.is_empty(),
            "{} of {checked} inputs tokenise differently from the model, \
             starting with {:?}",
            unexpected.len(),
            &unexpected[..unexpected.len().min(8)]
        );
        // The set must stay minimal. An allowlist is the natural place for a
        // real bug to hide: add a character here and every disagreement
        // involving it becomes invisible. So every listed character has to
        // still be diverging — if a crate update fixes one, this fails and
        // says to remove it rather than letting the list quietly become a
        // blanket over behaviour nobody is checking any more.
        // Minimality is a property of the *committed* fixture, which is built
        // to contain every character in the set. An override corpus is a
        // sweep, and a sweep's job is the assertion above — finding a
        // divergence nothing accounts for. Asking a random corpus to exercise
        // all 98 would fail for the uninteresting reason that it happened not
        // to contain one.
        if std::env::var_os("DCGREP_WORDPIECE_CONFORMANCE").is_none() {
            blamed.sort_unstable();
            blamed.dedup();
            let healed: Vec<char> = TABLE_SKEW
                .iter()
                .copied()
                .filter(|ch| blamed.binary_search(ch).is_err())
                .collect();
            assert!(
                healed.is_empty(),
                "{} characters in TABLE_SKEW no longer diverge and must be \
                 removed from it: {:?}",
                healed.len(),
                &healed[..healed.len().min(12)]
            );
            assert!(
                skewed > 0,
                "no skewed input was seen at all, so the fixture no longer \
                 covers the blocks TABLE_SKEW is about"
            );
        }
    }
}
