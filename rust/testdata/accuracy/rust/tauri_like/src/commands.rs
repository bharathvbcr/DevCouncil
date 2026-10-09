use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct Payload {
    #[serde(default = "default_c40")]
    pub n: u32,
    #[serde(serialize_with = "ser_c40")]
    pub m: u32,
}

fn default_c40() -> u32 {
    40
}
fn ser_c40<S: serde::Serializer>(v: &u32, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_u32(*v)
}

fn helper_c39() -> u32 {
    39
}

#[tauri::command]
pub fn cmd_c39() -> u32 {
    helper_c39()
}

#[tauri::command]
pub async fn cmd_async_c39(p: Payload) -> Result<u32, String> {
    Ok(p.n + helper_c39())
}

#[tauri::command]
fn cmd_private_c39() -> u32 {
    1
}
