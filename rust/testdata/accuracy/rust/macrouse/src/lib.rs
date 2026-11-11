//! `#[macro_use] extern crate dep;` makes the dependency's `vec!` the one the
//! crate means, so no macro of this crate can be taken as the std one.
#![allow(dead_code, unused_imports)]
#[macro_use]
extern crate dep;

pub mod mda;
pub mod mdb;
pub mod user;

#[cfg(test)]
mod rt {
    #[test]
    fn the_dependency_macro_rebinds_the_type() {
        assert_eq!(crate::user::via_macro_use(), 1);
    }
}
