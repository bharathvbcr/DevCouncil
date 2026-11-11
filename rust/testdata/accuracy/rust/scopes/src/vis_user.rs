//! D: a glob publishes only what is visible to the importing module.
use crate::vis::inner::*;
use crate::vis::pubs::*;

pub fn run_v() -> u8 {
    priv_v1() + sup_v2() + inp_v3() + cr_v4() + pub_v5()
}
