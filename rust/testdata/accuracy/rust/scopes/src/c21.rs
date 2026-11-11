//! `use super::*` in an inline module brings the parent's names in.
fn h21() -> u8 { 1 }

mod t {
    use super::*;

    pub fn call_c21() -> u8 { h21() }
}
