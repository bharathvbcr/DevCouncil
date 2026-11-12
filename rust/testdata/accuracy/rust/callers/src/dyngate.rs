//! F2 with a namesake: `Gate` is also a struct in `gateother.rs`, so the trait's
//! bare name is not one declaration. The parameter's type is spelled in this
//! file, where `Gate` is the trait.
use std::sync::Arc;

pub trait Gate {
    fn open_f2n(&self) -> bool;
}

pub struct Door;

impl Gate for Door {
    fn open_f2n(&self) -> bool {
        true
    }
}

pub fn pass_f2n(gate: &Arc<dyn Gate>) -> bool {
    gate.open_f2n()
}
