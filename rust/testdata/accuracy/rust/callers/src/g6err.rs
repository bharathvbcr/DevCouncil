//! G6: `CmdErr` has an `impl From<..>` here and another in `g6src.rs`.
pub struct CmdErr {
    pub m: String,
}

impl From<String> for CmdErr {
    fn from(m: String) -> CmdErr {
        CmdErr { m }
    }
}

pub struct CmdOne {
    pub m: u8,
}

impl From<u8> for CmdOne {
    fn from(m: u8) -> CmdOne {
        CmdOne { m }
    }
}

pub fn conv_one_g6(m: u8) -> CmdOne {
    CmdOne::from(m)
}
