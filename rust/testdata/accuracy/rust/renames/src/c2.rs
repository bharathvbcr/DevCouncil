//! The manifest's `log = { package = "dep_alt", .. }` makes `log` the package `dep_alt`:
//! `log::trace!` is its macro, not the log crate's.
use crate::rda::Dup;

pub fn renamed_log() -> u8 {
    log::trace!(crate::rdb::Dup);
    Dup::mk16()
}
