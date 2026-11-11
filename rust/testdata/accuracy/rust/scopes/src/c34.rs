//! A block-level glob is nearer than the module's own item, and may provide the
//! name: a macro-made name, and a std module's name.
pub fn gm() -> u16 { 2 }
pub fn swap() -> u16 { 2 }

pub fn macro_glob() -> u8 {
    use crate::mm::*;
    gm()
}

pub fn std_glob() -> u8 {
    use std::mem::*;
    let mut a = 1u8;
    let mut b = 2u8;
    swap(&mut a, &mut b);
    a
}

pub fn std_glob2() -> u8 {
    use std::mem::*;
    let mut x = 5u8;
    let mut y = 6u8;
    swap(&mut x, &mut y);
    y
}
