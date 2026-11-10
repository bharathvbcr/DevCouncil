//! G3: a function-local `Dup` shadows the imported one inside `local_g3` only.
use crate::g3db::Dup;

pub fn local_g3() {
    struct Dup;

    impl Dup {
        pub fn new_g3() -> Dup {
            Dup
        }
    }

    let _ = Dup::new_g3();
}

pub fn imported_g3() -> Dup {
    Dup::new_g3()
}
