//! `extern crate dep as anyhow;` and a Cargo `log = { package = "dep_alt" }` make
//! `bail!` and `log::trace!` the dependency's macros, which emit a block-level
//! `use`; the names are the real crates' and the crates are not.
#![allow(dead_code, unused_imports)]

pub mod c1;
pub mod c2;
pub mod rda;
pub mod rdb;

#[cfg(test)]
mod rt {
    #[test]
    fn a_renamed_crate_rebinds_the_type() {
        assert_eq!(crate::c1::aliased_bail(), 1);
        assert_eq!(crate::c2::renamed_log(), 1);
    }
}
