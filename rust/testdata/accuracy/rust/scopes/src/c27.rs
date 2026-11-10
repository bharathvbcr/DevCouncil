//! Recall: names reached through an inline module, `use checkout::{..}`.
mod checkout {
    pub fn pay27() -> u8 { 1 }
    pub fn refund27() -> u8 { 2 }
}

use checkout::{pay27, refund27};

pub fn call_c27a() -> u8 { pay27() + refund27() }

mod tests {
    use super::checkout::{pay27, refund27};

    pub fn call_c27b() -> u8 { pay27() + refund27() }
}

mod tests2 {
    use super::*;
    use checkout::pay27;

    pub fn call_c27c() -> u8 { pay27() }
}
