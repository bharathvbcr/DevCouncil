//! F4 regression shape: a glob of a module whose chain publishes `shared_f4r`,
//! and a function of that name declared in this file's inline test module.
use crate::handlers::*;

pub fn outer_f4r() -> u32 {
    cmd_alpha_f3()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared_f4r() -> u32 {
        6
    }

    #[test]
    fn call_f4r() {
        assert_eq!(shared_f4r(), 6);
    }
}
