//! C39 Tauri commands: reachable only from the generate_handler! list (and the
//! TS `invoke("cmd_c39")`). C40 serde derive attribute-driven calls.

mod commands;

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            commands::cmd_c39,
            commands::cmd_async_c39,
            commands::cmd_private_c39,
        ])
        .run(tauri::generate_context!())
        .expect("run");
}
