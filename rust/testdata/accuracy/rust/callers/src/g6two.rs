//! G6: two impls of the same trait method on one type in ONE file.
pub struct Twice {
    pub n: u32,
}

impl From<u8> for Twice {
    fn from(n: u8) -> Twice {
        Twice { n: n as u32 }
    }
}

impl From<u16> for Twice {
    fn from(n: u16) -> Twice {
        Twice { n: n as u32 }
    }
}

pub fn conv_two_g6(n: u16) -> Twice {
    Twice::from(n)
}
