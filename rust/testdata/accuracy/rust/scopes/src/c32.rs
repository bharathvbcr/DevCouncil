//! B with a glob in a `tests` module (the `anc::tests::child_calls` shape).
pub fn anc32() -> u16 { 2 }

mod tests {
    use crate::p_ext::*;

    pub fn child_calls32() -> u8 { anc32() }
}
