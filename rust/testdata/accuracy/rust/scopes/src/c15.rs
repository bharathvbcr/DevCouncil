//! C: a `use` inside one function belongs to that function's block only.
pub fn h15() -> u16 { 2 }

pub fn call_c15() -> u8 {
    use crate::p_ext::h15;
    h15()
}

pub fn other_c15() -> u16 { h15() }
