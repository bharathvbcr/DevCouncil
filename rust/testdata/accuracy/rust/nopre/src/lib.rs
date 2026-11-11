//! `#![no_implicit_prelude]`: no `println!` in the macro prelude (rustc: a glob of
//! `kio` that exports one wins, no E0659), but `assert!` is a builtin and stays
//! E0659 against a glob of the same name.
#![no_implicit_prelude]

extern crate kio;

pub mod pa;
pub mod pb;

use crate::pa::Dup;
use kio::*;

pub fn h56() -> u16 { 2 }

pub fn nopre_println() -> u8 {
    println!(crate::pb::Dup);
    Dup::mk16()
}

pub fn builtin_assert_silent() -> u16 {
    assert!(true);
    h56()
}
