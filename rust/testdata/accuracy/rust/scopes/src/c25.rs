//! `super::name()` names the parent module's item.
fn h25() -> u8 { 1 }

mod m {
    pub fn call_c25() -> u8 { super::h25() }
}
