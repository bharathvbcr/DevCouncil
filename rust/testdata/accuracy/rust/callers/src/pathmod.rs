//! F4: the test module lives in a file the `#[path]` attribute names, so its
//! `super` is THIS file, not the directory's `mod.rs`/`lib.rs`.
pub struct Quarantine;

impl Quarantine {
    pub fn new_f4() -> Quarantine {
        Quarantine
    }
}

pub fn make_f4() -> Quarantine {
    Quarantine::new_f4()
}

#[cfg(test)]
#[path = "pathmod_tests.rs"]
mod tests;
