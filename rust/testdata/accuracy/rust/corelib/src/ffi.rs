//! C23 extern "C" / FFI.

extern "C" {
    fn abs(x: i32) -> i32; // libc abs, declared here
}

pub fn c23_caller(x: i32) -> i32 {
    unsafe { abs(x) }
}

/// Exported to C: no Rust caller, but not dead.
#[no_mangle]
pub extern "C" fn exported_c23(x: i32) -> i32 {
    x + 23
}
