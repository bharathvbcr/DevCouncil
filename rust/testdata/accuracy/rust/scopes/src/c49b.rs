//! Control: a glob of an indexed crate that exports no `println!` leaves the macro
//! silent (`pm` exports only attribute and derive macros of other names).
use pm::*;

pub fn h49b() -> u16 {
    3
}

pub fn indexed_glob_silent() -> u16 {
    println!("y");
    h49b()
}
