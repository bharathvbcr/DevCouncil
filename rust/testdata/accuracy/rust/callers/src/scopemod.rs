//! F4 nested shape: `scopemod/mid.rs` is a conventional child that globs this
//! module, and its own `#[path]` child globs `mid`. The type reaches the leaf
//! through `mid`'s private `use super::*;`.
pub struct Scoped;

impl Scoped {
    pub fn new_f4s() -> Scoped {
        Scoped
    }
}

pub fn make_f4s() -> Scoped {
    Scoped::new_f4s()
}

#[cfg(test)]
mod mid;
