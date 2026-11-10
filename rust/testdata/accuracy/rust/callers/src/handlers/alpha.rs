#[tauri::command]
pub fn cmd_alpha_f3() -> u32 {
    1
}

/// Published by `handlers::*`; `inlinemod.rs` declares its own of this name in
/// an inline module.
pub fn shared_f4r() -> u32 {
    5
}
