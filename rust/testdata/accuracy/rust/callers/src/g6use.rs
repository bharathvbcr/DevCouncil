//! G6: which `From` impl runs depends on the argument's type, which is not known
//! here.
use crate::g6err::CmdErr;
use crate::g6src::SrcErr;

pub fn conv_g6(e: SrcErr) -> CmdErr {
    CmdErr::from(e)
}
