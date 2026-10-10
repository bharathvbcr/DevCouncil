//! C50/C51 (added for the dead-policy card): function-local items.
//! A local RAII guard runs through `Drop`; an inner `extern` block declares a
//! foreign function the enclosing body calls. Neither is a dead-code candidate.

pub fn with_guard_c50() -> u8 {
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {}
    }
    let _g = Guard;
    50
}

pub fn inner_extern_c51(x: i32) -> i32 {
    extern "C" {
        fn abs(x: i32) -> i32;
    }
    unsafe { abs(x) }
}
