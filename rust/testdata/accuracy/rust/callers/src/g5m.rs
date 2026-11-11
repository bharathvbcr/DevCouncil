//! G5: publishes `made_g5` through a macro the index cannot see.
macro_rules! mk {
    () => {
        pub fn made_g5() -> u32 {
            1
        }
    };
}

mk!();
