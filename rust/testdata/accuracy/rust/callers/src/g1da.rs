//! G1: the type a caller imports.
pub struct Dup;

impl Dup {
    pub fn new_g1() -> Dup {
        Dup
    }
}

pub fn use_g1da() -> Dup {
    Dup::new_g1()
}
