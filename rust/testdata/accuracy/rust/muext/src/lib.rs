//! `#[macro_use] extern crate pm;` of an indexed crate that exports no macro named
//! like a std one (`pm` has `with_use`, `body_use` and a derive `Clone`) leaves
//! `println!` the std macro.
#![allow(dead_code, unused_imports)]
#[macro_use]
extern crate pm;

pub mod user;

#[cfg(test)]
mod rt {
    #[test]
    fn println_still_runs() {
        assert_eq!(crate::user::ext_silent(), 3);
    }
}
