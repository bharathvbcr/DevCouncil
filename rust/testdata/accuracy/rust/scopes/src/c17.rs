//! A (the GitPulse `verify_status.rs::evidence_store` shape): the module has its
//! own `record`; the function imports another one, with other arguments, from a
//! test-support module.
fn record17(a: u8, b: u8) -> u16 { (a + b) as u16 }

pub fn evidence_store17() -> u8 {
    use crate::p_gt::{head17, record17, store17};
    head17() + store17() + record17(1, 2, 3, 4, 5)
}

pub fn plain17() -> u16 { record17(1, 2) }
