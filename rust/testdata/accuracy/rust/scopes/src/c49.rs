//! Control: a glob of a std module cannot bring a different `println!`, so the
//! macro stays silent and the module's own `h49` answers the call.
use std::collections::*;

pub fn h49() -> u16 { 2 }

pub fn std_glob_silent() -> u16 {
    println!("x");
    h49()
}
