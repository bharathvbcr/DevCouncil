//! `extern crate dep as log;` makes `log::debug!` the dependency's macro. (`debug`,
//! not `info`: another module imports `dep::info` by name, which would already
//! make `info` unusable as the std/log macro for the whole crate and so hide
//! whether the alias itself is read.)
extern crate dep as log;

use crate::p_da::Dup;

pub fn aliased_log() -> u8 {
    log::debug!(crate::p_db::Dup);
    Dup::mk16()
}
