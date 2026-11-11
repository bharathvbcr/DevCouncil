//! A crate that exports the name as a `#[proc_macro]` function (not a
//! `macro_rules!`): `use pm::*;` brings its `bail`, which emits a block-level `use`.
use crate::p_da::Dup;
use pm::*;

pub fn pm_bail() -> u8 {
    bail!(crate::p_db::Dup);
    Dup::mk16()
}
