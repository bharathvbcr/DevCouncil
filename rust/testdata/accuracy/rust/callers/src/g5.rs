//! G5: the callee is `std::io::stdin`, imported by name; the glob chain brings
//! only `open_g5` (the private `stdin` is not visible here).
use crate::g5chain::*;
use std::io::stdin;

pub fn read_g5() -> u32 {
    let _ = stdin();
    open_g5()
}
