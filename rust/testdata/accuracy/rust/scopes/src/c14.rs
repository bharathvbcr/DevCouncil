//! A glob in the module reaches the one file it names; an unrelated file's
//! namesake (`p_dec`) is not imported.
use crate::p_ext::*;

pub fn call_c14() -> u8 { h14() }
