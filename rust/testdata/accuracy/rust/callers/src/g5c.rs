//! G5: two modules publish `made_g5`, one through a macro. The index sees one.
use crate::g5mchain::*;

pub fn call_made_g5() -> u32 {
    made_g5()
}
