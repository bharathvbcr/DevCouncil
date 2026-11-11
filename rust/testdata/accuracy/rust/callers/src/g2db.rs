//! G2: the type a test module imports.
pub struct Dup;

impl Dup {
    pub fn new_g2() -> Dup {
        Dup
    }
}

pub fn use_g2db() -> Dup {
    Dup::new_g2()
}
