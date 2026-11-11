//! A nearer glob of an enum's variants.
#![allow(non_snake_case)]
pub enum E {
    Mk(u8),
}

pub fn Mk(x: u8) -> u16 { x as u16 }

pub fn enum_glob() -> E {
    use E::*;
    Mk(1)
}
