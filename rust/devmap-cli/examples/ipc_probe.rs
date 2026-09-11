//! Bounded native transport probe for the worktree_stress example.
//! Kept outside the production CLI; reuses its request schema and Tokio runtime.
use std::io::Write;
use std::time::Duration;

use anyhow::{bail, Context};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args_os().skip(1);
    let endpoint = args.next().context("missing IPC endpoint")?;
    let frame = args.next().context("missing request")?;
    let frame = frame.to_str().context("request is not UTF-8")?;
    if args.next().is_some() || frame.len() > 1_048_576 {
        bail!("expected endpoint and a request no larger than 1 MiB");
    }
    let _: devmap_serve::IpcRequest = serde_json::from_str(frame)?;
    let reply = tokio::time::timeout(Duration::from_secs(30), async {
        #[cfg(unix)]
        let connection = tokio::net::UnixStream::connect(&endpoint).await;
        #[cfg(windows)]
        let connection = loop {
            let result = tokio::net::windows::named_pipe::ClientOptions::new().open(&endpoint);
            if result
                .as_ref()
                .is_err_and(|error| error.raw_os_error() == Some(231))
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
                continue;
            }
            break result;
        };
        let mut stream = match connection {
            Ok(stream) => stream,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                ) =>
            {
                // The harness distinguishes an absent endpoint from a broken
                // probe, timeout, malformed response or server refusal.
                std::process::exit(2);
            }
            Err(error) => return Err(error.into()),
        };
        stream.write_all(frame.as_bytes()).await?;
        stream.write_all(b"\n").await?;
        stream.flush().await?;
        let mut reply = Vec::new();
        stream.take(2_000_001).read_to_end(&mut reply).await?;
        if reply.len() > 2_000_000 {
            bail!("IPC response exceeded 2 MB");
        }
        let _: serde_json::Value = serde_json::from_slice(&reply)?;
        Ok::<_, anyhow::Error>(reply)
    })
    .await
    .context("IPC exchange exceeded 30 seconds")??;
    std::io::stdout().lock().write_all(&reply)?;
    Ok(())
}
