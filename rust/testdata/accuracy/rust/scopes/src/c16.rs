//! C (the `fnuse` shape): a block-level `use` of a type shadows the module-level
//! import of the same name inside that block only.
use crate::p_da::Dup;

pub fn fl_c16() -> u8 {
    use crate::p_db::Dup;
    Dup::mk16()
}

pub fn other_c16() -> u16 { Dup::mk16() }
