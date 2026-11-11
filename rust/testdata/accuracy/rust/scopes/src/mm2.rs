//! A module holding an item macro and a plainly declared function.
macro_rules! mk2 {
    () => {
        pub fn made2() -> u8 { 1 }
    };
}

mk2!();

pub fn plain2() -> u8 { 3 }
