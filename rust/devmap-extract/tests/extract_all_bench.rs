//! Not a test: an A/B timing probe for `extract_all_with_progress`.
//!
//! Run with `BENCH_CORPUS=<dir> cargo test -p devmap-extract --test it
//! extract_all_bench -- --ignored --nocapture` on each arm and compare the
//! `BENCH min_ms=` lines. It reads every `.rs`/`.py`/`.go`/`.ts` file under the
//! directory, so both arms must be pointed at the same frozen tree.

use devmap_extract::{extract_all_with_progress, FileRef};
use std::path::Path;

fn collect(dir: &Path, out: &mut Vec<(String, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name == "target") {
                continue;
            }
            collect(&path, out);
        } else if path
            .extension()
            .is_some_and(|ext| ["rs", "py", "go", "ts"].iter().any(|e| ext == *e))
        {
            if let Ok(text) = std::fs::read_to_string(&path) {
                out.push((path.to_string_lossy().into_owned(), text));
            }
        }
    }
}

#[test]
#[ignore = "timing probe; set BENCH_CORPUS and run with --ignored --nocapture"]
fn extract_all_bench() {
    let root = std::env::var("BENCH_CORPUS").expect("BENCH_CORPUS names the frozen corpus");
    let mut sources = Vec::new();
    collect(Path::new(&root), &mut sources);
    let files: Vec<FileRef> = sources
        .iter()
        .map(|(path, source)| FileRef { path, source })
        .collect();
    let mut best = u128::MAX;
    let mut failed = 0;
    for _ in 0..3 {
        let started = std::time::Instant::now();
        let out = extract_all_with_progress(&files, None);
        best = best.min(started.elapsed().as_millis());
        failed = out.iter().filter(|e| e.is_parse_failure()).count();
    }
    println!("BENCH files={} failed={failed} min_ms={best}", files.len());
}
