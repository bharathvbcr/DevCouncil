//! A full path names its module; the same-file namesake is not the callee.
pub fn h24() -> u16 { 2 }

pub fn call_c24() -> u8 { crate::p_ext::h24() }
