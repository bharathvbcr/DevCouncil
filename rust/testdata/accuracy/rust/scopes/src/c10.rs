//! B: a namesake declared in an ANCESTOR module is not visible to a child module.
pub fn h10() -> u16 { 2 }

pub mod m {
    use crate::p_ext::h10;

    pub fn call_c10() -> u8 { h10() }
}
