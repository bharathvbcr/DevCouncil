//! A nearer glob of an unindexed external module, of a module with an item macro,
//! and a plainly declared name of that module.
pub fn max(a: u8, b: u8) -> u16 { (a + b) as u16 }
pub fn made2() -> u16 { 2 }

pub fn ext_glob() -> u8 {
    use std::cmp::*;
    max(1, 2)
}

pub fn macro_item_glob() -> u8 {
    use crate::mm2::*;
    made2()
}

pub fn plain_via_glob() -> u8 {
    use crate::mm2::*;
    plain2()
}
