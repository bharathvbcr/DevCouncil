//! B: the same, three modules deep (the `n3::a::b::c::call_c` shape).
pub fn h11() -> u16 { 2 }

pub mod a {
    pub fn h11() -> u16 { 2 }

    pub mod b {
        pub mod c {
            use crate::p_ext::h11;

            pub fn call_c11() -> u8 { h11() }
        }
    }
}
