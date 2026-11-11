//! The same through a glob chain: `use crate::p_prelude::*;` reaches `dep`'s
//! `bail` by way of the prelude's `pub use dep::*;`.
use crate::p_da::Dup;
use crate::p_prelude::*;

pub fn prelude_bail() -> u8 {
    bail!(crate::p_db::Dup);
    Dup::mk16()
}
