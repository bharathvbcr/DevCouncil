//! Document ids and the header's query ids, both through `dcgrep`'s WordPiece.
//!
//! One implementation, on purpose. The header's parity block is replayed
//! through that same function at index time; a second tokeniser here would
//! be the thing the parity gate exists to catch, and it would catch it on
//! every build.

use std::collections::HashMap;

/// The query tokeniser's cap (`wordpiece::MAX_TOKENS`, not re-exported).
/// Parity ids are compared against that cap, so emitting more than this
/// makes every index build refuse.
pub const QUERY_CAP: usize = 512;

/// `[CLS] + content + [SEP]`, content truncated so the whole sequence fits
/// in `max_positions`. No padding: the model pads inside the batch, and a
/// stored id list that contained pad ids would disagree with the fixture.
pub fn document_ids(
    text: &str,
    vocab: &HashMap<String, u32>,
    max_positions: usize,
) -> Result<Vec<u32>, String> {
    if max_positions < 2 {
        return Err(format!(
            "the model has {max_positions} positions, which is not enough for [CLS] and [SEP]"
        ));
    }
    let cls = *vocab.get("[CLS]").ok_or("vocabulary has no [CLS]")?;
    let sep = *vocab.get("[SEP]").ok_or("vocabulary has no [SEP]")?;
    let content = dc_grep::wordpiece_ids(text, vocab, max_positions - 2);
    let mut ids = Vec::with_capacity(content.len() + 2);
    ids.push(cls);
    ids.extend(content);
    ids.push(sep);
    Ok(ids)
}

/// Query-side ids: no `[CLS]` or `[SEP]`. This is what the header records.
pub fn query_ids(text: &str, vocab: &HashMap<String, u32>) -> Vec<u32> {
    dc_grep::wordpiece_ids(text, vocab, QUERY_CAP)
}
