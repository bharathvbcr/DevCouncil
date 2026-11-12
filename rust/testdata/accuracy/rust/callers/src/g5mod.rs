//! G5: a module with a PRIVATE `stdin`, published to nobody.
fn stdin() -> u32 {
    1
}

pub fn open_g5() -> u32 {
    stdin()
}
