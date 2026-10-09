//! C14 tokio::select!, C16 async fn / .await, thread::spawn closures (C38).

use tokio::sync::mpsc;

pub async fn fetch_c16() -> u32 {
    16
}
pub struct Client;
impl Client {
    pub async fn get_c16(&self) -> u32 {
        fetch_c16().await
    }
}
pub async fn c16_caller(c: &Client) -> u32 {
    let a = fetch_c16().await;
    let b = c.get_c16().await;
    a + b
}

pub struct Signals {
    rx: mpsc::Receiver<u8>,
}
impl Signals {
    pub async fn recv_c14(&mut self) -> Option<u8> {
        self.rx.recv().await
    }
}
pub fn handle_c14(v: Option<u8>) -> u8 {
    v.unwrap_or(0)
}
pub async fn tick_c14() {}

pub async fn c14_caller(mut signals: Signals) -> u8 {
    tokio::select! {
        v = signals.recv_c14() => handle_c14(v),
        _ = tick_c14() => 0,
    }
}

pub fn work_c38() -> u8 {
    38
}
pub fn c38_caller() -> u8 {
    let h = std::thread::spawn(|| work_c38());
    let h2 = std::thread::spawn(work_c38);
    h.join().unwrap() + h2.join().unwrap()
}
