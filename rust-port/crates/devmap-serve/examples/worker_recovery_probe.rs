//! Run as an executable under the shipped profile: `cargo test --release`
//! forces unwinding and cannot detect a release profile that aborts workers.
//! The daemon and transports depend on Tokio returning worker panics as
//! JoinError so their existing error handling can run.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

struct Cleanup(Arc<AtomicBool>);

impl Drop for Cleanup {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let cleaned = Arc::new(AtomicBool::new(false));
    let worker_cleaned = Arc::clone(&cleaned);
    let failed = tokio::task::spawn_blocking(move || {
        let _cleanup = Cleanup(worker_cleaned);
        panic!("intentional worker recovery probe");
    });
    match tokio::time::timeout(Duration::from_secs(5), failed).await? {
        Err(error) if error.is_panic() => {}
        other => anyhow::bail!("worker panic was not returned to its supervisor: {other:?}"),
    }
    anyhow::ensure!(
        cleaned.load(Ordering::SeqCst),
        "worker unwind did not run cleanup"
    );
    let healthy = tokio::task::spawn_blocking(|| 42);
    anyhow::ensure!(
        tokio::time::timeout(Duration::from_secs(5), healthy).await?? == 42,
        "blocking pool did not recover"
    );
    println!("worker panic returned; cleanup ran; subsequent worker completed");
    Ok(())
}
