//! Artifact writers with tmp+rename (V14) and fingerprint skip-on-unchanged.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::escape::{html_escape, json_script_escape};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactFingerprint {
    pub generated_head: String,
    pub built_at: u64,
    pub fingerprint: String,
}

/// Write bytes via tmp+rename; returns true when content changed on disk.
/// Distinguishes concurrent temp files written by one process.
static WRITE_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn write_atomic(path: &Path, content: &[u8]) -> std::io::Result<bool> {
    let parent = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;

    // A *unique* temp name per writer. `path.with_extension("tmp")` is shared by
    // every concurrent process writing the same artifact: two `dev map` runs
    // against one repository both create `repo_map.tmp`, the first rename moves
    // it away, and the second fails with ENOENT. Measured at 24-way
    // concurrency: 8 of 24 workers died in `manifest` with
    // `No such file or directory (os error 2)`. The store itself survived —
    // SC28 hardened it — so this was the last unguarded writer.
    //
    // pid separates processes; the counter separates the two artifacts one
    // process writes in a single `manifest` run.
    let stamp = WRITE_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let unique = format!(
        "{}.{}.{}.tmp",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("artifact"),
        std::process::id(),
        stamp
    );
    let tmp = parent.join(unique);

    // Any early return past this point must not strand the temp file, so the
    // body is run once and the temp cleaned on failure.
    let result = (|| -> std::io::Result<bool> {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(content)?;
        file.sync_all()?;
        if path.exists() {
            let existing = fs::read(path)?;
            if existing == content {
                return Ok(false);
            }
        }
        fs::rename(&tmp, path)?;
        Ok(true)
    })();
    if !matches!(result, Ok(true)) {
        fs::remove_file(&tmp).ok();
    }
    result
}

// ---- consumer-artifact stamp -------------------------------------------------
//
// `should_regenerate` below answers the same question for the HTML artifacts by
// reading the file back and looking for a marker in it. That is affordable for a
// visualizer page and is not for the pair `manifest` writes: `code_graph.json`
// is 22 MB on this repository, and by the time the marker could be compared the
// generation has already been read out of SQLite and serialized — 0.43 s of a
// 1.39 s `dev hook post-tool-use` spent producing bytes identical to the ones
// already on disk. `write_atomic` then declines the rename, so nothing changed
// and nothing was saved.
//
// The stamp moves the decision in front of all of that. It is a small sidecar
// naming (a) the binary that wrote the artifacts, (b) every input their content
// derives from, and (c) what each output looked like when it was written; when
// all three still hold, the store is never read.

/// What one written artifact looked like immediately after it was written.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactRecord {
    pub path: String,
    pub len: u64,
    pub mtime_ns: i128,
    /// Inode, 0 where the platform has none. Catches a file swapped for another
    /// of the same length whose mtime was restored with it.
    #[serde(default)]
    pub ino: u64,
    /// Inode change time, the field userspace cannot back-date. `-1` off unix,
    /// where there is none.
    ///
    /// `len`+`mtime_ns`+`ino` alone are all restorable: an in-place rewrite of
    /// the same number of bytes keeps the length and the inode, and
    /// `utimensat` — which is what `cp -p`, `rsync --times`, `tar -x` and
    /// `File::set_times` all reach for — puts the modification time back. The
    /// stamp then matched a file whose contents had changed, and the skip that
    /// quotes it reported `artifacts_unchanged` over bytes belonging to another
    /// generation, permanently: every later run compared against the same
    /// doctored stat. `ctime` moves on any write and cannot be set, which is
    /// the same reason `freshness::stat_key` carries it.
    #[serde(default = "unknown_ctime")]
    pub ctime_ns: i128,
}

/// A stamp written before `ctime_ns` existed has no value for it. `-1` is the
/// same value an unreadable clock yields and matches no real `ctime`, so such a
/// stamp is a miss and its artifacts regenerate once — which is the fail-closed
/// direction.
fn unknown_ctime() -> i128 {
    -1
}

impl ArtifactRecord {
    fn of(path: &Path) -> std::io::Result<Self> {
        let meta = fs::metadata(path)?;
        Ok(Self {
            path: path.to_string_lossy().into_owned(),
            len: meta.len(),
            mtime_ns: mtime_ns(&meta),
            ino: ino_of(&meta),
            ctime_ns: ctime_ns(&meta),
        })
    }
}

fn mtime_ns(meta: &fs::Metadata) -> i128 {
    meta.modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|since| since.as_nanos() as i128)
        .unwrap_or(-1)
}

#[cfg(unix)]
fn ino_of(meta: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    meta.ino()
}

#[cfg(not(unix))]
fn ino_of(_meta: &fs::Metadata) -> u64 {
    0
}

#[cfg(unix)]
fn ctime_ns(meta: &fs::Metadata) -> i128 {
    use std::os::unix::fs::MetadataExt;
    meta.ctime() as i128 * 1_000_000_000 + meta.ctime_nsec() as i128
}

/// No `st_ctime` off unix. `-1` costs a regeneration on every run there and
/// never a wrong skip, which is the trade the whole sidecar is built on.
#[cfg(not(unix))]
fn ctime_ns(_meta: &fs::Metadata) -> i128 {
    -1
}

/// The layout of the sidecar. A stamp written under a different layout is not
/// read as though it were this one; it is a miss, and the artifacts regenerate.
const ARTIFACT_STAMP_VERSION: u32 = 2;

/// The sidecar: what produced the consumer artifacts, and from what.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactStamp {
    pub version: u32,
    /// Identity of the kernel that wrote these artifacts — see
    /// [`writer_identity`]. A rebuilt kernel regenerates once, on purpose: the
    /// same generation emitted by a different binary is a different artifact,
    /// and that is exactly what an extractor or emitter change *is*.
    pub writer: String,
    /// Every input the artifacts' bytes derive from, named. A map rather than a
    /// struct so that adding an input can only ever cause a regeneration:
    /// an unknown key on either side makes the maps unequal, where a new struct
    /// field would quietly default and compare equal to a stamp that never
    /// carried it.
    pub inputs: BTreeMap<String, String>,
    pub outputs: Vec<ArtifactRecord>,
}

/// `path:len:mtime_ns` of the running binary.
///
/// Two builds of this workspace both report `devmap 0.1.0`, so the version
/// string is not an identity. The executable's own stat is: same bytes on disk,
/// same emitter. Unreadable — a binary deleted or replaced under a running
/// process — yields a value that matches nothing, so the artifacts regenerate
/// rather than being trusted to a writer that cannot be identified.
pub fn writer_identity() -> String {
    let Ok(exe) = std::env::current_exe() else {
        return "unidentified-writer".to_string();
    };
    match fs::metadata(&exe) {
        Ok(meta) => format!("{}:{}:{}", exe.display(), meta.len(), mtime_ns(&meta)),
        Err(_) => "unidentified-writer".to_string(),
    }
}

impl ArtifactStamp {
    /// Stamp `outputs` as they are on disk right now.
    pub fn of(inputs: BTreeMap<String, String>, outputs: &[&Path]) -> std::io::Result<Self> {
        Ok(Self {
            version: ARTIFACT_STAMP_VERSION,
            writer: writer_identity(),
            inputs,
            outputs: outputs
                .iter()
                .map(|path| ArtifactRecord::of(path))
                .collect::<std::io::Result<Vec<_>>>()?,
        })
    }

    pub fn read(path: &Path) -> Option<Self> {
        let text = fs::read_to_string(path).ok()?;
        let stamp: Self = serde_json::from_str(&text).ok()?;
        (stamp.version == ARTIFACT_STAMP_VERSION).then_some(stamp)
    }

    pub fn write(&self, path: &Path) -> std::io::Result<()> {
        let json = serde_json::to_vec(self)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        write_atomic(path, &json).map(|_| ())
    }

    /// True when the artifacts this stamp describes are still exactly the ones
    /// the current inputs would produce.
    ///
    /// Fail-closed in every direction: an unreadable sidecar, a missing output,
    /// a stat that will not answer, a different writer, one differing input —
    /// each is a miss, and a miss regenerates. The only way to skip is for every
    /// question to have been asked and answered the same.
    pub fn still_current(&self, inputs: &BTreeMap<String, String>, outputs: &[&Path]) -> bool {
        if self.writer != writer_identity() || &self.inputs != inputs {
            return false;
        }
        if self.outputs.len() != outputs.len() {
            return false;
        }
        outputs
            .iter()
            .zip(self.outputs.iter())
            .all(|(path, record)| {
                record.path == path.to_string_lossy()
                    && ArtifactRecord::of(path).is_ok_and(|current| &current == record)
            })
    }
}

/// Skip regeneration when fingerprint matches existing artifact header (V14).
pub fn should_regenerate(path: &Path, fp: &ArtifactFingerprint) -> bool {
    if !path.exists() {
        return true;
    }
    let Ok(text) = fs::read_to_string(path) else {
        return true;
    };
    let marker = format!("fingerprint:{}", fp.fingerprint);
    let escaped_marker = format!("fingerprint:{}", html_escape(&fp.fingerprint));
    !text.contains(&marker) && !text.contains(&escaped_marker)
}

/// Symbol explorer payload embedded in script tag (V2) with escaped title (V1).
pub fn render_symbol_explorer_html(
    title: &str,
    payload_json: &str,
    fp: &ArtifactFingerprint,
) -> String {
    let safe_title = html_escape(title);
    let safe_json = json_script_escape(payload_json);
    format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>{safe_title}</title></head><body>\
         <!-- fingerprint:{} -->\
         <h1>{safe_title}</h1>\
         <p>staleness: head={} fp={}</p>\
         <script type=\"application/json\" id=\"payload\">{safe_json}</script>\
         </body></html>",
        html_escape(&fp.fingerprint),
        html_escape(&fp.generated_head),
        html_escape(&fp.fingerprint)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fp() -> ArtifactFingerprint {
        ArtifactFingerprint {
            generated_head: "abc123".into(),
            built_at: 1,
            fingerprint: "fp-test".into(),
        }
    }

    #[test]
    fn test_v14_atomic_write_and_fingerprint_skip() {
        // closes V14
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("devmap-artifact-{stamp}.html"));
        let html = format!("<!-- fingerprint:{} -->\n<p>artifact</p>", fp().fingerprint);
        assert!(write_atomic(&path, html.as_bytes()).unwrap());
        assert!(!should_regenerate(&path, &fp()));
        let fp2 = ArtifactFingerprint {
            fingerprint: "other".into(),
            ..fp()
        };
        assert!(should_regenerate(&path, &fp2));
        let _ = fs::remove_file(&path);
    }

    fn stamp_dir(tag: &str) -> std::path::PathBuf {
        let base = std::env::temp_dir().join(format!(
            "devmap-stamp-{tag}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&base).expect("temp dir");
        base
    }

    fn inputs(generation: &str) -> std::collections::BTreeMap<String, String> {
        let mut map = std::collections::BTreeMap::new();
        map.insert("generation_id".to_string(), generation.to_string());
        map.insert("content_fingerprint".to_string(), "c2:abc".to_string());
        map
    }

    /// The whole point of the sidecar: after a write, the same inputs skip; any
    /// change to an input, or to an artifact on disk, does not.
    #[test]
    fn the_artifact_stamp_skips_only_when_every_question_answers_the_same() {
        let dir = stamp_dir("current");
        let map = dir.join("repo_map.json");
        let graph = dir.join("code_graph.json");
        let sidecar = dir.join("devmap.sqlite.artifacts.json");
        fs::write(&map, br#"{"map_engine":"devmap-rust"}"#).unwrap();
        fs::write(&graph, br#"{"meta":{}}"#).unwrap();
        let outputs = [map.as_path(), graph.as_path()];

        // Nothing written yet: there is no stamp, so nothing may be skipped.
        assert!(ArtifactStamp::read(&sidecar).is_none());

        ArtifactStamp::of(inputs("7"), &outputs)
            .unwrap()
            .write(&sidecar)
            .unwrap();
        let stamp = ArtifactStamp::read(&sidecar).expect("the stamp reads back");
        assert!(stamp.still_current(&inputs("7"), &outputs));

        // A moved generation is a different artifact.
        assert!(!stamp.still_current(&inputs("8"), &outputs));

        // An input this run does not know about must not compare equal to a
        // stamp that never carried it.
        let mut extra = inputs("7");
        extra.insert("pending_count".to_string(), "3".to_string());
        assert!(!stamp.still_current(&extra, &outputs));

        // An artifact edited or replaced under us is not the one we wrote.
        fs::write(&graph, br#"{"meta":{"tampered":true}}"#).unwrap();
        assert!(!stamp.still_current(&inputs("7"), &outputs));

        // …and one that is simply gone certainly is not.
        fs::remove_file(&graph).unwrap();
        assert!(!stamp.still_current(&inputs("7"), &outputs));

        let _ = fs::remove_dir_all(&dir);
    }

    /// A corrupt, truncated or foreign-layout sidecar must read as "no stamp",
    /// never as a stamp that happens to match.
    #[test]
    fn an_unreadable_stamp_is_a_miss_not_a_match() {
        let dir = stamp_dir("corrupt");
        let sidecar = dir.join("stamp.json");
        for body in [
            "".as_bytes(),
            b"not json at all",
            br#"{"version": 999, "writer": "x", "inputs": {}, "outputs": []}"#,
            br#"{"version": 1, "writer": "x"}"#,
        ] {
            fs::write(&sidecar, body).unwrap();
            assert!(
                ArtifactStamp::read(&sidecar).is_none(),
                "must not parse: {}",
                String::from_utf8_lossy(body)
            );
        }
        let _ = fs::remove_dir_all(&dir);
    }

    /// A stamp written by another binary is not this binary's evidence.
    #[test]
    fn a_stamp_from_a_different_writer_never_matches() {
        let dir = stamp_dir("writer");
        let artifact = dir.join("repo_map.json");
        fs::write(&artifact, b"{}").unwrap();
        let outputs = [artifact.as_path()];
        let mut stamp = ArtifactStamp::of(inputs("1"), &outputs).unwrap();
        assert!(stamp.still_current(&inputs("1"), &outputs));
        stamp.writer = "/some/other/devmap:123:456".to_string();
        assert!(!stamp.still_current(&inputs("1"), &outputs));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_v1_v2_symbol_payload_is_inert_and_remains_valid_json() {
        let payload = serde_json::json!({
            "name": "</script><img src=x onerror=alert(1)>",
            "ampersand": "a&b"
        });
        let raw = serde_json::to_string(&payload).unwrap();
        let html = render_symbol_explorer_html("Symbols", &raw, &fp());
        let marker = "<script type=\"application/json\" id=\"payload\">";
        let start = html.find(marker).unwrap() + marker.len();
        let end = html[start..].find("</script>").unwrap() + start;
        let embedded = &html[start..end];

        assert_eq!(
            serde_json::from_str::<serde_json::Value>(embedded).unwrap(),
            payload
        );
        assert_eq!(html.matches("</script>").count(), 1);
        assert!(!html.contains("<img"));
    }
}

#[cfg(test)]
mod concurrency_tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    /// Concurrent writers to one artifact must all succeed.
    ///
    /// `path.with_extension("tmp")` gave every writer the same temp name: the
    /// first rename moved it away and the rest failed with ENOENT. Measured
    /// through the CLI at 24-way concurrency, **8 of 24** `dev map` workers died
    /// in `manifest` with `No such file or directory (os error 2)`. The store
    /// itself was already safe (SC28); this was the last unguarded writer.
    ///
    /// Threads, not processes, so the test is cheap — the pid component of the
    /// temp name is constant here, which means this exercises exactly the
    /// same-process collision the sequence counter exists to prevent.
    #[test]
    fn concurrent_writers_to_one_artifact_all_succeed() {
        let dir = tempdir();
        let target = Arc::new(dir.join("repo_map.json"));

        let handles: Vec<_> = (0..16)
            .map(|worker| {
                let target = Arc::clone(&target);
                thread::spawn(move || {
                    let body = format!("{{\"worker\": {worker}}}");
                    write_atomic(&target, body.as_bytes())
                })
            })
            .collect();

        for (worker, handle) in handles.into_iter().enumerate() {
            let outcome = handle.join().expect("writer panicked");
            assert!(
                outcome.is_ok(),
                "writer {worker} failed: {:?}",
                outcome.err()
            );
        }

        // Exactly one payload survives, and it is one a writer actually wrote —
        // never a truncated or interleaved file.
        let final_text = fs::read_to_string(target.as_path()).expect("artifact must exist");
        assert!(
            (0..16).any(|worker| final_text == format!("{{\"worker\": {worker}}}")),
            "surviving artifact is not any writer's complete payload: {final_text}"
        );

        // No temp file may outlive the write; a stray one is what the next run
        // would trip over.
        let strays: Vec<_> = fs::read_dir(&dir)
            .expect("readable dir")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(".tmp"))
            .collect();
        assert!(strays.is_empty(), "temp files left behind: {strays:?}");
    }

    fn tempdir() -> std::path::PathBuf {
        let base = std::env::temp_dir().join(format!(
            "devmap-artifacts-{}-{}",
            std::process::id(),
            WRITE_SEQUENCE.load(std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir_all(&base).expect("temp dir");
        base
    }
}
