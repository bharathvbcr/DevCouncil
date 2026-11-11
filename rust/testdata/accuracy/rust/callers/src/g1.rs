//! G1: `Dup` is imported at file level; a namesake `Dup` is declared only inside
//! `mod inner`, which is not an ancestor of `top_g1`.
use crate::g1da::Dup;

mod inner {
    pub struct Dup;

    impl Dup {
        pub fn new_g1() -> Dup {
            Dup
        }
    }

    pub fn inner_g1() -> Dup {
        Dup::new_g1()
    }
}

pub fn top_g1() -> Dup {
    let _ = inner::inner_g1();
    Dup::new_g1()
}
