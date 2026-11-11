//! An explicit import that sits under `#[cfg]` does not shadow the glob when the
//! cfg is off. `bail_feature` is off in this crate, so `bail!` is `dep`'s (the glob
//! brings it) and emits a block-level `use` of the other Dup.
use crate::p_da::Dup;
#[cfg(feature = "bail_feature")]
use anyhow::bail;
use dep::*;

pub fn cfg_import() -> u8 {
    bail!(crate::p_db::Dup);
    Dup::mk16()
}
