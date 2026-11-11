//! A glob import of a dependency can bring a macro of the same name into scope:
//! `use dep::*;` brings `bail`, which here emits a block-level `use`.
use crate::p_da::Dup;
use dep::*;

pub fn glob_bail() -> u8 {
    bail!(crate::p_db::Dup);
    Dup::mk16()
}
