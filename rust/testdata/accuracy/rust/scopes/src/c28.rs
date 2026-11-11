//! Recall: `#[cfg]` twins are one method.
pub struct StatKey;

#[cfg(unix)]
impl StatKey {
    pub fn of(a: u8) -> u8 { a }
}

#[cfg(not(unix))]
impl StatKey {
    pub fn of(a: u8, b: u8) -> u8 { a + b }
}

pub fn call_c28() -> u8 { StatKey::of(1) }
