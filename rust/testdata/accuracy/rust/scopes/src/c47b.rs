//! A derive is the std one only when it is spelled bare or rooted at std/core:
//! `pm::Clone` is a derive of the `pm` crate that happens to share the name, and
//! here it emits a `use` into the block.
use crate::p_da::Dup;

pub fn pathed_derive() -> u8 {
    #[derive(pm::Clone)]
    struct S;
    Dup::mk16()
}
