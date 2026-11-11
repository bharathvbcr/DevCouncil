//! A namesake inside a CHILD module is not visible to its parent.
mod ch {
    pub fn h12() -> u16 { 2 }
}

use crate::p_ext::h12;

pub fn call_c12() -> u8 { h12() }
