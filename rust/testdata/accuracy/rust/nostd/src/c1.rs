//! `#![no_std]` has no `println!` in its macro prelude (rustc: a glob of `kio`
//! that exports one wins, no E0659), so the glob's `println!` is called here and
//! emits a block-level `use` of the other Dup. The core macros (`assert!`) are
//! still in the prelude, where a glob of the same name is E0659 and cannot exist
//! in a compiling crate, so `assert!` stays silent.
use crate::na::Dup;
use kio::*;

pub fn h55() -> u16 {
    2
}

pub fn nostd_println() -> u8 {
    println!(crate::nb::Dup);
    Dup::mk16()
}

pub fn core_assert_silent() -> u16 {
    assert!(true);
    h55()
}

#[cfg(test)]
mod tests {
    extern crate std;

    #[test]
    fn the_glob_println_is_the_one_called() {
        std::assert_eq!(super::nostd_println(), 1);
        std::assert_eq!(super::core_assert_silent(), 2);
    }
}
