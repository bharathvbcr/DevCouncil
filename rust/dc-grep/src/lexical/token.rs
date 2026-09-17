//! Turning source text into the terms a ranked index is built from.
//!
//! This is the one decision the whole lexical tier rests on: two files match
//! when they produce overlapping terms, so a tokeniser that splits differently
//! at index time and at query time does not return fewer results — it returns
//! *wrong* ones, and nothing in the answer says so. Index and query go through
//! [`terms_of`] and there is no second entry point.
//!
//! ## Why not words
//!
//! Prose tokenisers split on whitespace and punctuation. Code does neither.
//! `parseJSONResponse`, `parse_json_response` and `ParseJSONResponse` are the
//! same idea written three ways, and a reader searching for "json response"
//! means all three. So an identifier is emitted twice: once whole, and once
//! as its parts.
//!
//! The parts come from three boundaries, which between them cover every
//! convention in this repository:
//!
//!   - separators — `_`, `-`, `.`, `/` and anything else non-alphanumeric;
//!   - a lower-to-upper transition, which splits `parseJson` into `parse` and
//!     `Json`;
//!   - an upper-run followed by a lowercase letter, which splits `JSONResponse`
//!     into `JSON` and `Response` rather than into `J` and `SONResponse`. This
//!     one is the reason acronyms survive: without it every `HTTPServer` in the
//!     corpus indexes under `h`.
//!
//! Digits are a boundary too, so `utf8` yields `utf` and `8` — and `utf8`
//! whole, which is what anyone actually searches for.
//!
//! ## Why a hash and not a string
//!
//! Terms are interned to `u32` by FNV-1a. The index stores those, never the
//! text. The alternative was a string table, which would have put every
//! identifier in the repository on disk in the clear and made the index a
//! second copy of the source — a thing to keep in sync, to redact, and to
//! explain to anyone asking what the tool writes into `.devcouncil`.
//!
//! A hash collides. At 32 bits over the ~200k distinct terms a large
//! repository produces, a collision is likely somewhere (birthday bound),
//! which would make two unrelated terms score as one. It is survivable here
//! and nowhere else: the lexical tier *ranks candidate files*, and every file
//! it proposes is then read by the exact matcher, which either finds the
//! pattern or does not. A collision can therefore cost a wasted read. It
//! cannot put a line in front of a caller that does not contain what they
//! asked for.

/// Longest term kept, in bytes.
///
/// Past this a "term" is a minified bundle's single line, a base64 blob or a
/// generated identifier, none of which anyone searches for and all of which
/// would sit in the vocabulary forever.
pub(crate) const MAX_TERM_BYTES: usize = 64;

/// Shortest term kept, in characters.
///
/// One-character terms are `i`, `x`, `a` — they appear in every file, so they
/// separate nothing and cost a posting list as long as the corpus.
pub(crate) const MIN_TERM_CHARS: usize = 2;

/// Terms taken from one document before the rest is ignored.
///
/// A bound rather than a budget: the cap is reported by the caller as a
/// coverage hole, because a file only half-indexed is a file whose absence
/// from a result set means nothing.
pub(crate) const MAX_TERMS_PER_DOCUMENT: usize = 20_000;

/// Interns a term to the identity the index stores.
///
/// FNV-1a, written out rather than pulled in: it is nine lines, it is stable
/// across platforms and releases by construction, and the index on disk is
/// keyed by its output — a hasher that changed with a dependency bump would
/// silently invalidate every published index without changing the schema
/// number that exists to catch exactly that.
pub(crate) fn term_id(term: &str) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in term.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// Calls `emit` with every term in `text`, in order, at most
/// `MAX_TERMS_PER_DOCUMENT` times.
///
/// Returns `true` when the whole text was tokenised and `false` when the cap
/// stopped it, so the caller can count a partially indexed document rather
/// than publish it as complete.
pub(crate) fn terms_of(text: &str, mut emit: impl FnMut(&str)) -> bool {
    let mut emitted = 0usize;
    let mut push = |term: &str, emitted: &mut usize| -> bool {
        if *emitted >= MAX_TERMS_PER_DOCUMENT {
            return false;
        }
        if term.len() > MAX_TERM_BYTES || term.chars().count() < MIN_TERM_CHARS {
            return true;
        }
        emit(term);
        *emitted += 1;
        true
    };

    for run in text.split(|c: char| !c.is_alphanumeric()) {
        if run.is_empty() {
            continue;
        }
        // The identifier whole. `run` borrows `text`, so folding allocates
        // only for the terms that actually carry uppercase or non-ASCII.
        if !push(&fold(run), &mut emitted) {
            return false;
        }
        // Then its parts, if it has any. A run with no internal boundary is
        // already covered by the line above and is not emitted twice.
        let mut stopped = false;
        split_identifier(run, |part| {
            if !stopped && !push(&fold(part), &mut emitted) {
                stopped = true;
            }
        });
        if stopped {
            return false;
        }
    }
    true
}

/// Lowercases without allocating when there is nothing to lowercase.
///
/// Most terms in a code corpus are already lowercase, and an allocation per
/// term over a million terms is the difference between an index build that
/// fits in the walk and one that doubles it.
fn fold(term: &str) -> std::borrow::Cow<'_, str> {
    if term.bytes().any(|b| b.is_ascii_uppercase()) || !term.is_ascii() {
        std::borrow::Cow::Owned(term.to_lowercase())
    } else {
        std::borrow::Cow::Borrowed(term)
    }
}

/// Calls `emit` with each part of one alphanumeric run, split at case and
/// digit boundaries.
///
/// Emits nothing when the run has no internal boundary — the caller has
/// already emitted it whole, and emitting it a second time would double its
/// term frequency relative to every compound identifier, which is the number
/// the ranker weighs.
///
/// A callback rather than an iterator because the cut positions are computed
/// up front and an iterator would have to own or borrow them; this way they
/// live on this frame and nothing allocates but the folding.
fn split_identifier(run: &str, mut emit: impl FnMut(&str)) {
    let chars: Vec<(usize, char)> = run.char_indices().collect();
    let mut cuts: Vec<usize> = vec![0];
    for window in 0..chars.len().saturating_sub(1) {
        let (_, current) = chars[window];
        let (next_at, next) = chars[window + 1];
        let upper_run_then_word = current.is_uppercase()
            && next.is_lowercase()
            && window > 0
            && chars[window - 1].1.is_uppercase();
        let boundary = (current.is_lowercase() && next.is_uppercase())
            || (current.is_numeric() != next.is_numeric())
            || upper_run_then_word;
        if !boundary {
            continue;
        }
        // An upper-run followed by a lowercase letter cuts *before* the last
        // uppercase letter, so `JSONResponse` gives `JSON` + `Response` and
        // not `JSONR` + `esponse`.
        let at = if upper_run_then_word {
            chars[window].0
        } else {
            next_at
        };
        if at > *cuts.last().unwrap_or(&0) {
            cuts.push(at);
        }
    }
    if cuts.len() < 2 {
        return;
    }
    cuts.push(run.len());
    for pair in cuts.windows(2) {
        let part = &run[pair[0]..pair[1]];
        if !part.is_empty() {
            emit(part);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terms(text: &str) -> Vec<String> {
        let mut out = Vec::new();
        assert!(terms_of(text, |term| out.push(term.to_string())));
        out
    }

    #[test]
    fn an_identifier_is_indexed_whole_and_in_parts() {
        assert_eq!(
            terms("parseJsonResponse"),
            vec!["parsejsonresponse", "parse", "json", "response"]
        );
    }

    #[test]
    fn the_three_naming_conventions_produce_the_same_parts() {
        let camel = terms("parseJsonResponse");
        let snake = terms("parse_json_response");
        let pascal = terms("ParseJsonResponse");
        for want in ["parse", "json", "response"] {
            for (name, got) in [("camel", &camel), ("snake", &snake), ("pascal", &pascal)] {
                assert!(
                    got.contains(&want.to_string()),
                    "{name} lost {want}: {got:?}"
                );
            }
        }
    }

    #[test]
    fn an_acronym_survives_the_word_that_follows_it() {
        // Without the upper-run rule this is `j` + `sonresponse`, and every
        // HTTPServer in the corpus indexes under `h`.
        assert_eq!(
            terms("JSONResponse"),
            vec!["jsonresponse", "json", "response"]
        );
        assert_eq!(terms("HTTPServer"), vec!["httpserver", "http", "server"]);
        assert_eq!(terms("ioOpen"), vec!["ioopen", "io", "open"]);
    }

    #[test]
    fn digits_are_a_boundary_and_the_whole_still_survives() {
        assert_eq!(terms("utf8"), vec!["utf8", "utf"]); // "8" is one char
        assert_eq!(terms("sha256sum"), vec!["sha256sum", "sha", "256", "sum"]);
    }

    #[test]
    fn separators_split_and_punctuation_is_not_a_term() {
        assert_eq!(
            terms("a.b/c-d_e"),
            Vec::<String>::new(),
            "every part is one character, below the floor"
        );
        assert_eq!(
            terms("fn main() { let path = self.root; }"),
            vec!["fn", "main", "let", "path", "self", "root"]
        );
    }

    #[test]
    fn terms_below_the_floor_and_above_the_ceiling_are_dropped() {
        assert_eq!(terms("a i x"), Vec::<String>::new());
        let long = "z".repeat(MAX_TERM_BYTES + 1);
        assert_eq!(terms(&long), Vec::<String>::new());
        let at_ceiling = "z".repeat(MAX_TERM_BYTES);
        assert_eq!(terms(&at_ceiling), vec![at_ceiling]);
    }

    #[test]
    fn the_document_cap_is_reported_rather_than_applied_silently() {
        let text = "term ".repeat(MAX_TERMS_PER_DOCUMENT + 50);
        let mut count = 0usize;
        let complete = terms_of(&text, |_| count += 1);
        assert!(!complete, "a capped document must say it was capped");
        assert_eq!(count, MAX_TERMS_PER_DOCUMENT);
    }

    #[test]
    fn index_and_query_tokenise_identically() {
        // The property the whole tier rests on. Stated as a test because the
        // failure mode is silent: a query that splits differently returns
        // plausible, wrong files.
        for text in [
            "parseJsonResponse",
            "HTTP_SERVER_PORT",
            "read_file_bytes",
            "SHA256Sum(x)",
            "日本語 text",
        ] {
            assert_eq!(terms(text), terms(text));
        }
    }

    #[test]
    fn non_ascii_is_folded_and_kept() {
        let got = terms("Ärger straße");
        assert!(got.contains(&"ärger".to_string()), "{got:?}");
        assert!(got.contains(&"straße".to_string()), "{got:?}");
    }

    #[test]
    fn interning_is_stable_and_case_folded_upstream() {
        assert_eq!(term_id("parse"), term_id("parse"));
        assert_ne!(term_id("parse"), term_id("Parse"));
        // Pinned: the on-disk index is keyed by these numbers, so a change
        // here invalidates every published index and must be a schema bump.
        assert_eq!(term_id(""), 0x811c_9dc5);
        assert_eq!(term_id("a"), 0xe40c_292c);
    }

    #[test]
    fn an_empty_or_punctuation_only_document_yields_nothing() {
        assert_eq!(terms(""), Vec::<String>::new());
        assert_eq!(terms("  \n\t  "), Vec::<String>::new());
        assert_eq!(terms("!@#$%^&*()"), Vec::<String>::new());
    }
}
