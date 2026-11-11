//! Known gap (rs.s70b, in known_failing.tsv): two globs that are cfg
//! alternatives. `Dup` comes from `p_da` on unix and from `p_db` on windows, so
//! `Dup::mk16()` has two candidates and neither is certain. The resolver takes
//! the first glob in source order and emits a 1.0 edge to it. The true
//! expectation is no single 1.0 edge, with both alternatives candidates.
#[cfg(unix)]
use crate::p_da::*;
#[cfg(windows)]
use crate::p_db::*;

pub fn alternative_globs() -> u16 {
    Dup::mk16() as u16
}
