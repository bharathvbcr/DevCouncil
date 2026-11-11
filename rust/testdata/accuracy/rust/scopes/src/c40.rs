//! A macro imported by name (`use dep::info;`) is the dependency's, and it
//! emits a block-level `use`.
use crate::p_da::Dup;
use dep::info;

pub fn imported_info() -> u8 {
    info!(crate::p_db::Dup);
    Dup::mk16()
}
