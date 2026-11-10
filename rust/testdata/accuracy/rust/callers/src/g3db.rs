//! G3: the type a function-local `struct Dup` shadows.
pub struct Dup;

impl Dup {
    pub fn new_g3() -> Dup {
        Dup
    }
}

pub fn use_g3db() -> Dup {
    Dup::new_g3()
}
