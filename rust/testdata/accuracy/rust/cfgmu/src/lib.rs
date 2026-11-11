//! `#[cfg_attr(all(), macro_use)] extern crate dep;` is `#[macro_use]` (the
//! predicate holds), so `vec!` is the dependency's macro.
#![allow(dead_code, unused_imports)]
#[cfg_attr(all(), macro_use)]
extern crate dep;

pub mod mda;
pub mod mdb;
pub mod user;

#[cfg(test)]
mod rt {
    #[test]
    fn the_dependency_macro_rebinds_the_type() {
        assert_eq!(crate::user::via_cfg_attr(), 1);
    }
}
