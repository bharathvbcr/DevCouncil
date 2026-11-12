pub mod kid;

fn priv_k() -> u8 { 1 }
fn priv_v1() -> u16 { 2 }
pub(super) fn sup_v2() -> u16 { 2 }
pub(in crate::vis) fn inp_v3() -> u16 { 2 }
pub(crate) fn cr_v4() -> u8 { 1 }
pub fn pub_v5() -> u8 { 1 }
pub(super) fn sup_w2() -> u8 { 1 }
pub(in crate::vis) fn inp_w3() -> u8 { 1 }

pub fn touch_v() -> u16 { priv_v1() }
