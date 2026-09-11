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

fn inputs() -> BTreeMap<String, serde_json::Value> {
    BTreeMap::from([
        ("generation_id".to_string(), 41.into()),
        ("content_fingerprint".to_string(), "c2:deadbeef".into()),
    ])
}

/// Write a sidecar describing the two consumer artifacts.
///
/// The one place these tests name `ArtifactStamp`'s constructor, so the shape of
/// the call is stated once.
fn write_stamp(sidecar: &Path, map: &Path, graph: &Path) {
    let outputs: Vec<(&str, &Path)> = vec![("repo_map", map), ("code_graph", graph)];
    ArtifactStamp::of(inputs(), Some("deadbeef".to_string()), &outputs)
        .expect("stamp the artifacts")
        .write(sidecar)
        .expect("write the sidecar");
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

    let outputs: Vec<(&str, &Path)> =
        vec![("repo_map", map.as_path()), ("code_graph", graph.as_path())];
    let stamp = ArtifactStamp::of(inputs(), None, &outputs).expect("stamp the artifacts");
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

    let outputs: Vec<(&str, &Path)> = vec![("repo_map", map.as_path())];
    let stamp = ArtifactStamp::of(inputs(), None, &outputs).expect("stamp");
    assert!(stamp.still_current(&inputs(), &outputs));

    rewrite_preserving_mtime(&map, br#"{"map_engine":"devmap-rust","files":["b.py"]}"#);

    assert!(
        !stamp.still_current(&inputs(), &outputs),
        "a repo map whose file list was swapped under a restored mtime must not \
         be reported as the map this generation wrote"
    );

    let _ = fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// The sidecar is also the only cheap way to answer "did the kernel write this".
// A consumer that cannot read it has to parse the artifact instead — 34.5 MB of
// `code_graph.json`, ~150 ms, every `dev map status` and every doctor run.
// ---------------------------------------------------------------------------

/// The sidecar must be readable by a consumer that is not this crate.
///
/// `devmap_health._artifact` answers "which engine wrote this artifact" by
/// `json.loads`-ing the artifact itself. For `code_graph.json` that is a 34.5 MB
/// parse — measured at 150.0 ms p50 against 1.3 ms for `repo_map.json` — paid on
/// every status and every doctor run, to read one string out of `meta`.
///
/// The sidecar is written next to the store at exactly the moment that string is
/// known, so it could answer instead. Three things stopped it:
///
///  * it did not record the engine at all;
///  * it identified each output by path only, and the paths are stored *as
///    resolved* (`/repo/./.devcouncil/repo_map.json`, with the `./` the CLI's
///    default argument leaves in), so a consumer joining `root` with the
///    relative path builds a string that names the same file and does not
///    compare equal — path is not a usable key, role is;
///  * its `inputs` values were Rust `Debug` renderings — `"Some(\"c2:8261…\")"`,
///    `"None"`, `"1"` — so the file was JSON in syntax only, and nothing outside
///    this crate could take a value out of it without reimplementing
///    `Debug for Option<String>`.
///
/// This asserts the sidecar is a data file: engine named, outputs keyed by role,
/// values typed.
#[test]
fn the_sidecar_names_its_engine_and_keys_its_outputs_by_role() {
    let dir = workspace("readable");
    let map = dir.join("repo_map.json");
    let graph = dir.join("code_graph.json");
    let sidecar = dir.join("devmap.sqlite.artifacts.json");
    fs::write(&map, br#"{"map_engine":"devmap-rust"}"#).unwrap();
    fs::write(&graph, br#"{"meta":{"map_engine":"devmap-rust"}}"#).unwrap();

    write_stamp(&sidecar, &map, &graph);

    let raw = fs::read_to_string(&sidecar).expect("the sidecar is there");
    let value: serde_json::Value = serde_json::from_str(&raw).expect("the sidecar is JSON");

    assert_eq!(
        value["map_engine"], "devmap-rust",
        "the sidecar must name the engine that wrote the artifacts, so a \
         consumer does not have to parse 34.5 MB of code_graph.json to learn it"
    );

    let outputs = value["outputs"].as_array().expect("outputs is an array");
    let roles: Vec<&str> = outputs
        .iter()
        .map(|record| {
            record["role"]
                .as_str()
                .expect("every output record names its role; path is not a key")
        })
        .collect();
    assert!(
        roles.contains(&"repo_map") && roles.contains(&"code_graph"),
        "the roles a consumer looks up by must be the ones it knows: {roles:?}"
    );

    assert!(
        value["inputs"]["generation_id"].is_number(),
        "`generation_id` is a number, not the Rust rendering of one: {}",
        value["inputs"]["generation_id"]
    );
    assert!(
        value["inputs"]["content_fingerprint"].is_string()
            || value["inputs"]["content_fingerprint"].is_null(),
        "a digest is a string, and one that could not be computed is null — \
         never the four characters `None`: {}",
        value["inputs"]["content_fingerprint"]
    );

    let _ = fs::remove_dir_all(&dir);
}
