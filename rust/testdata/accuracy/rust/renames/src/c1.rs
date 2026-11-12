//! `extern crate dep as anyhow;` makes `anyhow` the dependency `dep`, so the
//! `bail!` imported from it is not anyhow's.
extern crate dep as anyhow;

use crate::rda::Dup;
use anyhow::bail;

pub fn aliased_bail() -> u8 {
    bail!(crate::rdb::Dup);
    Dup::mk16()
}
