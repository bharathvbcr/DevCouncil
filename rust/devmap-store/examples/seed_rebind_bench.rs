//! Seed-rebind cost: what staging a donor store for a worktree writes.
//!
//! ```text
//! seed_rebind_bench <src.sqlite> <worktree_root> <mode> <out_dir>
//! ```
//!
//! `<src.sqlite>` must be a private copy (`VACUUM INTO`), never a live store:
//! staging may checkpoint its source. Each run stages a clone of it into
//! `<out_dir>/staged.sqlite`, rebinds it for the mode, settles it, and prints
//! one JSON object: wall time per phase, `generations_rows`, and
//! `pages_changed` -- the pages of the staged file that differ from the source,
//! compared at `PRAGMA page_size`. On APFS the clone shares every page until it
//! is written, so that count is the copy-on-write write amplification.
//!
//! Modes:
//! * `full`: the pre-81b1372e rebind, `UPDATE generations SET repo_root` on
//!   every row plus `pending_state`, reproduced inline as the reference.
//! * `latest`: the current `rebind_staged` (latest row, `seed_rebind`,
//!   `pending_state`).
//! * `firstbuild`: `latest`, then the normalisation the first build or restamp
//!   runs (`Store::restamp_latest_head`), which rewrites every retained row:
//!   the cost `latest` defers.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{bail, Context, Result};
use devmap_store::seed::{rebind_staged, settle_staged, stage_seed_healing_wal};
use devmap_store::Store;
use rusqlite::Connection;

fn rebind_full(staged: &Path, root: &str) -> Result<()> {
    let mut conn = Connection::open(staged)?;
    conn.busy_timeout(std::time::Duration::from_secs(10))?;
    let tx = conn.transaction()?;
    tx.execute("UPDATE generations SET repo_root = ?1", [root])?;
    let bound = tx.execute(
        "UPDATE pending_state SET repo_root = ?1 WHERE singleton = 1",
        [root],
    )?;
    if bound != 1 {
        bail!("the seed store has no pending_state row to rebind");
    }
    tx.commit()?;
    conn.close().map_err(|(_, error)| error)?;
    Ok(())
}

fn pages_changed(src: &Path, staged: &Path, page_size: usize) -> Result<(u64, u64)> {
    let mut a = File::open(src).with_context(|| format!("open {}", src.display()))?;
    let mut b = File::open(staged).with_context(|| format!("open {}", staged.display()))?;
    let a_len = a.seek(SeekFrom::End(0))?;
    let b_len = b.seek(SeekFrom::End(0))?;
    a.seek(SeekFrom::Start(0))?;
    b.seek(SeekFrom::Start(0))?;
    let total_pages = a_len.max(b_len).div_ceil(page_size as u64);
    let mut buf_a = vec![0u8; page_size];
    let mut buf_b = vec![0u8; page_size];
    let mut changed = 0u64;
    for _ in 0..total_pages {
        let na = read_page(&mut a, &mut buf_a)?;
        let nb = read_page(&mut b, &mut buf_b)?;
        if na != nb || buf_a[..na] != buf_b[..nb] {
            changed += 1;
        }
    }
    Ok((changed, total_pages))
}

fn read_page(file: &mut File, buf: &mut [u8]) -> Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        let n = file.read(&mut buf[filled..])?;
        if n == 0 {
            break;
        }
        filled += n;
    }
    Ok(filled)
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 5 {
        bail!("usage: seed_rebind_bench <src.sqlite> <worktree_root> <mode> <out_dir>");
    }
    let src = PathBuf::from(&args[1]);
    let worktree = PathBuf::from(&args[2]);
    let mode = args[3].as_str();
    let out_dir = PathBuf::from(&args[4]);
    if !matches!(mode, "full" | "latest" | "firstbuild") {
        bail!("mode must be full, latest or firstbuild, got {mode:?}");
    }
    std::fs::create_dir_all(&worktree)?;
    std::fs::create_dir_all(&out_dir)?;
    let root = worktree.canonicalize()?;
    let root_str = root.to_str().context("worktree root is not UTF-8")?;
    let staged = out_dir.join("staged.sqlite");
    let _ = std::fs::remove_file(&staged);

    let page_size: usize = {
        let conn = Connection::open_with_flags(&src, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let size: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
        usize::try_from(size)?
    };

    let t = Instant::now();
    let method = stage_seed_healing_wal(&src, &staged)?;
    let stage_ms = t.elapsed().as_secs_f64() * 1000.0;

    let t = Instant::now();
    match mode {
        "full" => rebind_full(&staged, root_str)?,
        _ => rebind_staged(&staged, &root)?,
    }
    let rebind_ms = t.elapsed().as_secs_f64() * 1000.0;

    let t = Instant::now();
    settle_staged(&staged)?;
    let settle_ms = t.elapsed().as_secs_f64() * 1000.0;

    let mut normalize_ms = 0.0;
    if mode == "firstbuild" {
        let t = Instant::now();
        let store = Store::open(&staged)?;
        let head = store
            .latest_generation_head_sha()?
            .context("the source store has no generation")?;
        store.restamp_latest_head(&head)?;
        drop(store);
        settle_staged(&staged)?;
        normalize_ms = t.elapsed().as_secs_f64() * 1000.0;
    }

    let generations_rows: i64 = {
        let conn =
            Connection::open_with_flags(&staged, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        conn.query_row("SELECT COUNT(*) FROM generations", [], |r| r.get(0))?
    };
    let (changed, total_pages) = pages_changed(&src, &staged, page_size)?;
    println!(
        "{}",
        serde_json::json!({
            "mode": mode,
            "seed_method": format!("{method:?}"),
            "page_size": page_size,
            "total_pages": total_pages,
            "pages_changed": changed,
            "bytes_changed": changed * page_size as u64,
            "generations_rows": generations_rows,
            "stage_ms": stage_ms,
            "rebind_ms": rebind_ms,
            "settle_ms": settle_ms,
            "normalize_ms": normalize_ms,
        })
    );
    Ok(())
}
