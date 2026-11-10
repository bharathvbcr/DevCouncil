use crate::handlers::*;

pub fn run_f3() {
    let _h = tauri::generate_handler![cmd_alpha_f3, cmd_beta_f3, cmd_twice_f3];
}

/// Anchors: a qualified call keeps each command live so the dead-code gates
/// see no new candidate whichever way the bare reference above resolves.
pub fn anchor_f3() -> u32 {
    crate::handlers::alpha::cmd_alpha_f3()
        + crate::handlers::beta::cmd_beta_f3()
        + crate::handlers::gamma::cmd_twice_f3()
        + crate::handlers::delta::cmd_twice_f3()
}
