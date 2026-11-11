use super::*;
use crate::otherq::Quarantine;

#[test]
fn mid_f4s() {
    let _s = Scoped::new_f4s();
    let _q = Quarantine::new_f4();
}

#[cfg(test)]
#[path = "leaf_f4s.rs"]
mod leaf;
