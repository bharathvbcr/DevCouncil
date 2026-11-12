//! An attribute macro on a block item can emit a `use` into the block.
use crate::p_da::Dup;

pub fn attr_use() -> u8 {
    #[pm::with_use(crate::p_db::Dup)]
    fn _x() {}
    Dup::mk16()
}
