/// Published by two globs (also in `delta`): a bare `cmd_twice_f3` names two
/// symbols and must not resolve to either.
#[tauri::command]
pub fn cmd_twice_f3() -> u32 {
    3
}
