//! A declared item shadows a glob import of the same name.
use crate::p_ext::*;

fn h19() -> u8 { 1 }

pub fn call_c19() -> u8 { h19() }
