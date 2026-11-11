//! A namesake of `pathmod::Quarantine` that the F4 test module can never see.
pub struct Quarantine;

impl Quarantine {
    pub fn new_f4() -> Quarantine {
        Quarantine
    }
}

pub fn make_other_f4() -> Quarantine {
    Quarantine::new_f4()
}
