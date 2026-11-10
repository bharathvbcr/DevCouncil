//! `extern crate dep as log;` makes `log::info!` the dependency's macro.
extern crate dep as log;

use crate::p_da::Dup;

pub fn aliased_log() -> u8 {
    log::info!(crate::p_db::Dup);
    Dup::mk16()
}
