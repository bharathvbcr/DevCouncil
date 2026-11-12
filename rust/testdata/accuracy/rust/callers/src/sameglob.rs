//! F4b: the parent publishes `helper_f4b`; the child module declares its own.
pub fn helper_f4b() -> u32 {
    1
}

pub fn parent_f4b() -> u32 {
    helper_f4b()
}

#[cfg(test)]
mod tests;
