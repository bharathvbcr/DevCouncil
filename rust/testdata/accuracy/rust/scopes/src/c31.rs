//! B with a glob: ancestors' namesakes are not visible to a module that globs
//! the real one (the `n3::a::b::c::call_c` shape).
pub fn h31() -> u16 { 2 }

pub mod a {
    pub fn h31() -> u16 { 2 }

    pub mod b {
        pub mod c {
            use crate::p_ext::*;

            pub fn call_c31() -> u8 { h31() }
        }
    }
}
