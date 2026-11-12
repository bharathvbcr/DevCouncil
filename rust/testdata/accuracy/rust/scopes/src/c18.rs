//! A glob inside a block shadows the enclosing module's item of the same name.
pub fn h18() -> u16 { 2 }

pub fn call_c18() -> u8 {
    use crate::p_ext::*;
    h18()
}
