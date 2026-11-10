//! A struct named like `dyngate::Gate`; it implements no trait.
pub struct Gate {
    pub shut: bool,
}

impl Gate {
    pub fn open_f2n(&self) -> bool {
        !self.shut
    }
}

pub fn make_gate_f2n() -> Gate {
    Gate { shut: false }
}
