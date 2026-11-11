//! `cfg_attr(pred, attr)` is `attr` whenever `pred` holds (here it does): an
//! attribute macro written through it can emit a `use` into the block as much as
//! one written plainly.
use crate::p_da::Dup;

pub fn cfg_attr_use() -> u8 {
    #[cfg_attr(all(), pm::with_use(crate::p_db::Dup))]
    fn _x() {}
    Dup::mk16()
}
