//! The one definition of which characters are escaped wherever text from
//! outside is shown to an agent or a terminal.
//!
//! It lives here, below both `devmap-resolve` (which quotes callee names into
//! its budget note) and `devmap-query` (whose `bounded` module re-exports it
//! and owns the bounded-sentence helpers), so neither needs its own copy.

/// Bidirectional formatting characters (the Unicode `Bidi_Control` property:
/// marks, embeddings, overrides, isolates): not `is_control`, but able to
/// reorder the text around them on a terminal. This is the subset that
/// *validation* may reject (claude.rs); display sinks escape the wider
/// [`is_agent_unsafe`] set, so validation never narrows when the set grows.
pub fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// Whether `c` is escaped wherever text from outside is shown to an agent or a
/// terminal. The one definition of the set; each sink keeps its own spelling.
///
/// - every control (Cc, tab and U+0085 included) and the line separators
///   U+2028/U+2029;
/// - [`is_bidi_control`];
/// - invisible format characters: U+200B-200D (zero-width space, ZWNJ, ZWJ),
///   U+2060, U+FEFF;
/// - the tag block U+E0000-U+E007F, which renders as nothing.
///
/// ZWNJ and ZWJ are escaped, not rejected: they are ordinary in Persian, Indic
/// text and emoji, and a display escape is visible and reversible.
pub fn is_agent_unsafe(c: char) -> bool {
    c.is_control()
        || is_bidi_control(c)
        || matches!(c, '\u{2028}' | '\u{2029}' | '\u{200b}'..='\u{200d}' | '\u{2060}' | '\u{feff}' | '\u{e0000}'..='\u{e007f}')
}

/// Right-to-left letters that no escaper may touch: Hebrew, Arabic, Persian
/// (with a ZWNJ, which is escaped, so it is not here), Devanagari and mixed
/// text. Every sink that escapes agent- or terminal-visible text asserts these
/// come through unchanged.
pub const RTL_LETTER_FIXTURES: &[&str] = &[
    "שלום עולם",
    "مرحبا بالعالم",
    "a שלום b مرحبا",
    "नमस्ते दुनिया",
    "فارسی",
];
