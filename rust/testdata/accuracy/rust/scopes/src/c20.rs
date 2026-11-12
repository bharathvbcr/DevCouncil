//! A named import shadows a glob import of the same name.
use crate::p_ext::*;
use crate::p_ext2::h20;

pub fn call_c20() -> u8 { h20() }
