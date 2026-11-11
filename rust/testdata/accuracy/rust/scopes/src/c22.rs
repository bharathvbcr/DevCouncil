//! A named `use super::name` in an inline module.
fn h22() -> u8 { 1 }

mod t {
    use super::h22;

    pub fn call_c22() -> u8 { h22() }
}
