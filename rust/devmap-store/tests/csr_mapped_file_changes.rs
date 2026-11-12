#![cfg(unix)]

//! A `.csr` file stays memory-mapped for as long as its reader lives. The
//! checks made when it was opened describe the bytes as they were, so what
//! happens to the file afterwards has to be survivable:
//!
//! * a file truncated or rewritten in place under a reader is reported by
//!   `ensure_unchanged`, in a child process here because the alternative is a
//!   `SIGBUS` that would take the test run with it.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use devmap_store::csr::{CsrGraphReader, CsrGraphWriter, CsrNodeRecord};

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "devmap-csr-changes-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A chain of `nodes` nodes, each pointing at the next.
fn chain(generation: u64, nodes: u32) -> CsrGraphWriter {
    let mut writer = CsrGraphWriter::new(generation);
    for _ in 0..nodes {
        writer.add_node(CsrNodeRecord::default());
    }
    for node in 0..nodes - 1 {
        writer.add_edge(node, node + 1, 90);
    }
    writer
}

#[test]
fn a_file_rewritten_in_place_is_reported_by_ensure_unchanged() {
    let dir = scratch("in-place");
    let path = dir.join("graph.csr");
    chain(1, 50).write_to_file(&path).unwrap();
    let reader = CsrGraphReader::open_mmap(&path).unwrap();
    assert!(reader.ensure_unchanged().is_ok());

    let bytes = std::fs::read(&path).unwrap();
    let mapped_at = std::fs::metadata(&path).unwrap().modified().unwrap();
    // Same length, same inode, different modification time: an in-place rewrite.
    // The time is set explicitly rather than waited for, so the test does not
    // depend on the timestamp resolution of the filesystem.
    let mut file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    file.write_all(&bytes).unwrap();
    file.set_modified(mapped_at + std::time::Duration::from_secs(5))
        .unwrap();

    let changed = reader.ensure_unchanged().expect_err("rewritten in place");
    assert_eq!(changed.found_len, Some(bytes.len() as u64));
    std::fs::remove_dir_all(dir).unwrap();
}

const CHILD_PATH: &str = "DEVMAP_CSR_TRUNCATION_CHILD_PATH";

/// The child half of `a_file_truncated_under_a_mapped_reader_is_an_error_not_a_signal`.
/// Does nothing unless the parent started it.
#[test]
fn truncation_child() {
    let Some(path) = std::env::var_os(CHILD_PATH) else {
        return;
    };
    let reader = CsrGraphReader::open_mmap(Path::new(&path)).expect("maps the graph");
    println!("child-ready");
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).unwrap();
    // The file has been truncated. Touching the mapping now would be a
    // signal; asking first must be an error.
    let changed = reader.ensure_unchanged();
    println!("child-saw-change={}", changed.is_err());
}

#[test]
fn a_file_truncated_under_a_mapped_reader_is_an_error_not_a_signal() {
    let dir = scratch("truncate");
    let path = dir.join("graph.csr");
    chain(1, 5000).write_to_file(&path).unwrap();

    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "truncation_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_PATH, &path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("starts the child");
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut seen = Vec::new();
    for line in lines.by_ref() {
        let line = line.unwrap();
        let ready = line.contains("child-ready");
        seen.push(line);
        if ready {
            break;
        }
    }
    assert!(
        seen.iter().any(|line| line.contains("child-ready")),
        "the child never mapped the file: {seen:?}"
    );

    // Truncate the file the child has mapped, then let it ask.
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(100)
        .unwrap();
    child.stdin.take().unwrap().write_all(b"go\n").unwrap();
    for line in lines {
        seen.push(line.unwrap());
    }
    let status = child.wait().unwrap();
    assert_eq!(
        status.signal(),
        None,
        "the child died from a signal: {status:?} {seen:?}"
    );
    assert!(status.success(), "{status:?} {seen:?}");
    assert!(
        seen.iter()
            .any(|line| line.contains("child-saw-change=true")),
        "ensure_unchanged did not report the truncation: {seen:?}"
    );
    std::fs::remove_dir_all(dir).unwrap();
}
