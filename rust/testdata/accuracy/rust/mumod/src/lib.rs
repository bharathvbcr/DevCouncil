//! `#[macro_use] mod macros;` publishes only the module's own `macro_rules!`,
//! which the index already reads as the crate's definitions: it makes no std
//! macro of another name unusable.
#![allow(dead_code, unused_imports)]
#[macro_use]
mod macros;

pub mod user;

#[cfg(test)]
mod rt {
    #[test]
    fn the_module_macro_and_println_both_run() {
        assert_eq!(crate::user::mod_silent(), 2);
    }
}
