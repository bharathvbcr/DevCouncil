//! Known gap (rs.s70a, in known_failing.tsv): one name, two cfg-gated imports.
//! Only one of them is live in any configuration, and which one is decided by
//! the target, so `Dup::mk16()` means `p_da::Dup` on unix and `p_db::Dup` on
//! windows. The resolver takes the first binding in source order and emits a
//! 1.0 edge to it, which is wrong for the configuration where the other is live.
//! The true expectation is no single 1.0 edge, with both alternatives candidates.

#[cfg(unix)]
use crate::p_da::Dup;
#[cfg(windows)]
use crate::p_db::Dup;

pub fn alternative_imports() -> u16 {
    Dup::mk16() as u16
}
