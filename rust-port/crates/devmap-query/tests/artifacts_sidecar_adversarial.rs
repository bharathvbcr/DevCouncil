//! The artifact sidecar, attacked.
//!
//! `ArtifactStamp::still_current` decides whether `devmap build --manifest`
//! skips regenerating `repo_map.json` and `code_graph.json` and reports
//! `artifacts_unchanged: true`. A skip is a claim that the files on disk are
//! exactly the ones this generation would produce, so anything that can move
//! the bytes without moving the stamp's evidence turns that claim into a lie
//! that no later run corrects — the next build compares against the same
//! doctored stat and skips again.
//!
//! These tests attack the evidence rather than the decision.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use devmap_query::artifacts::ArtifactStamp;

fn workspace(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "devmap-sidecar-{tag}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn inputs() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("generation_id".to_string(), "41".to_string()),
        ("content_fingerprint".to_string(), "c2:deadbeef".to_string()),
    ])
}

/// Overwrite `path` in place with `bytes` (which must be the same length as the
/// file already there), then restore the modification time it had before.
///
/// This is not exotic: it is what `cp -p`, `rsync --times`, `tar -x`, `git
/// checkout` of a same-size blob, and any editor that writes in place followed
/// by a timestamp-preserving restore all do. The file keeps its inode and its
/// length, and its mtime is the one it had — the three facts the stamp records.
fn rewrite_preserving_mtime(path: &Path, bytes: &[u8]) {
    use std::io::{Seek, SeekFrom, Write};

    let before = fs::metadata(path).expect("stat before");
    assert_eq!(
        before.len(),
        bytes.len() as u64,
        "the attack only works at equal length; that is the point"
    );
    let times = fs::FileTimes::new()
        .set_accessed(before.accessed().expect("atime"))
        .set_modified(before.modified().expect("mtime"));

    let mut file = fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("open for in-place rewrite");
    file.seek(SeekFrom::Start(0)).expect("seek");
    file.write_all(bytes).expect("write");
    file.sync_all().expect("sync");
    file.set_times(times).expect("restore mtime");

    let after = fs::metadata(path).expect("stat after");
    assert_eq!(after.len(), before.len(), "length must be unchanged");
    assert_eq!(
        after.modified().unwrap(),
        before.modified().unwrap(),
        "mtime must be restored"
    );
}

/// A skip may not survive a rewrite that restores the timestamp it was
/// compared against.
#[test]
fn a_rewritten_artifact_with_a_restored_mtime_is_not_current() {
    let dir = workspace("mtime-restore");
    let map = dir.join("repo_map.json");
    let graph = dir.join("code_graph.json");
    fs::write(&map, br#"{"map_engine":"devmap-rust","files":[]}"#).unwrap();
    fs::write(&graph, br#"{"nodes":[],"edges":[],"generation":41}"#).unwrap();

    let outputs: Vec<&Path> = vec![map.as_path(), graph.as_path()];
    let stamp = ArtifactStamp::of(inputs(), &outputs).expect("stamp the artifacts");
    assert!(
        stamp.still_current(&inputs(), &outputs),
        "nothing has moved yet"
    );

    // Same length, same inode, mtime put back where it was; only the bytes
    // differ — a graph that now claims a generation it was not built from.
    rewrite_preserving_mtime(&graph, br#"{"nodes":[],"edges":[],"generation":99}"#);

    assert!(
        !stamp.still_current(&inputs(), &outputs),
        "the artifacts on disk are not the ones this stamp describes, so the \
         skip that quotes it would report artifacts_unchanged over stale bytes"
    );

    let _ = fs::remove_dir_all(&dir);
}

/// The same attack against the file the whole map hangs off.
#[test]
fn a_rewritten_repo_map_with_a_restored_mtime_is_not_current() {
    let dir = workspace("mtime-restore-map");
    let map = dir.join("repo_map.json");
    fs::write(&map, br#"{"map_engine":"devmap-rust","files":["a.py"]}"#).unwrap();

    let outputs: Vec<&Path> = vec![map.as_path()];
    let stamp = ArtifactStamp::of(inputs(), &outputs).expect("stamp");
    assert!(stamp.still_current(&inputs(), &outputs));

    rewrite_preserving_mtime(&map, br#"{"map_engine":"devmap-rust","files":["b.py"]}"#);

    assert!(
        !stamp.still_current(&inputs(), &outputs),
        "a repo map whose file list was swapped under a restored mtime must not \
         be reported as the map this generation wrote"
    );

    let _ = fs::remove_dir_all(&dir);
}
