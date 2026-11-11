use crate::g6err::CmdErr;

pub struct SrcErr;

impl From<SrcErr> for CmdErr {
    fn from(_: SrcErr) -> CmdErr {
        CmdErr { m: String::new() }
    }
}
