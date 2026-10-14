//! The `engine_core` library root, at `[lib] path = "lib/core.rs"`.

pub mod parts;

pub fn run_cp() -> u8 {
    parts::assemble_cp()
}
