//! A namesake inside a SIBLING module is not visible.
mod s1 {
    pub fn h13() -> u16 { 2 }
}

mod s2 {
    use crate::p_ext::h13;

    pub fn call_c13() -> u8 { h13() }
}
