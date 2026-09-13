//! `devmap paths` and `devmap doctor` must not re-hash an unchanged binary.
//!
//! The regression this pins: both commands report a `sha256` for every `devmap`
//! they find, and computed it from the bytes on **every** invocation. Measured
//! 2026-09-13 on this machine that is 63.5 MiB of SHA-256 for a single
//! installed release, and `devmap paths --json` is the first command the
//! generated agent guide tells an agent to run — so the cost landed once per
//! agent turn, forever, for an answer that changes only when a binary is
//! rebuilt.
//!
//! **Nothing here is timed.** A wall-clock assertion on this machine measures
//! the sibling build running next to it. A hit is proved instead by *planting a
//! digest the bytes could not produce* and watching it come back; a miss by
//! watching the real digest come back over the top of a planted one. Neither
//! can pass by accident, and both fail against the pre-fix binary.

#![cfg(unix)]

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use serde_json::{json, Value};

/// The binary this build produced, never one found on PATH: an installed
/// `devmap` is routinely a different version from the tree under test.
const DEVMAP: &str = env!("CARGO_BIN_EXE_devmap");

/// A digest that is well-formed and impossible: 64 hex characters no input
/// hashes to, so seeing it can only mean the memo was read.
const PLANTED: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// A repository, a private HOME, and a private `bin` holding a fake `devmap`.
struct Fixture {
    base: PathBuf,
    repo: PathBuf,
    home: PathBuf,
    bin: PathBuf,
    fake: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

/// pid + a process-local counter, not a timestamp: these run in parallel and
/// macOS hands out coarse enough clock readings that two nanosecond-stamped
/// names in one process do collide.
fn fixture(label: &str, body: &[u8], with_state_dir: bool) -> Fixture {
    static SEQUENCE: AtomicU32 = AtomicU32::new(0);
    let base = std::env::temp_dir().join(format!(
        "devmap-bindigest-{label}-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&base);
    let repo = base.join("repo");
    let home = base.join("home");
    let bin = base.join("bin");
    if with_state_dir {
        std::fs::create_dir_all(repo.join(".devmap")).expect("mkdir state");
    } else {
        std::fs::create_dir_all(&repo).expect("mkdir repo");
    }
    std::fs::create_dir_all(&home).expect("mkdir home");
    std::fs::create_dir_all(&bin).expect("mkdir bin");
    let fake = bin.join("devmap");
    std::fs::write(&fake, body).expect("write fake devmap");
    // Deliberately not executable: the inventory probes what it finds by
    // running it, and a fixture this test controls must never be runnable.
    Fixture {
        base,
        repo,
        home,
        bin,
        fake,
    }
}

impl Fixture {
    fn memo(&self) -> PathBuf {
        self.repo.join(".devmap/cache/binary_digests.json")
    }

    /// The canonical path the inventory keys the fake binary under.
    fn fake_key(&self) -> String {
        self.fake
            .canonicalize()
            .expect("canonical fixture path")
            .display()
            .to_string()
    }

    /// `size:mtime_ns:ctime_ns` for the fake binary, exactly as the memo spells
    /// it. Recomputed from the live inode rather than remembered, so a planted
    /// entry always carries the key of the file as it is right now.
    fn stat_key(&self) -> String {
        let meta = std::fs::metadata(&self.fake).expect("stat fixture");
        format!(
            "{}:{}:{}",
            meta.len(),
            meta.mtime() as i128 * 1_000_000_000 + meta.mtime_nsec() as i128,
            meta.ctime() as i128 * 1_000_000_000 + meta.ctime_nsec() as i128
        )
    }

    /// Write a memo claiming `digest` for the fake binary as it stands now.
    fn plant(&self, digest: &str) {
        self.plant_raw(
            &json!({
                "scheme": "b1",
                "entries": {
                    self.fake_key(): {
                        "key": self.stat_key(),
                        "sha256": digest,
                        "used": 1_789_000_000u64,
                    }
                }
            })
            .to_string(),
        );
    }

    /// Write the memo file verbatim, for the shapes a struct cannot express.
    fn plant_raw(&self, text: &str) {
        let memo = self.memo();
        std::fs::create_dir_all(memo.parent().expect("memo parent")).expect("mkdir cache");
        std::fs::write(&memo, text).expect("plant memo");
    }

    /// `devmap --root <repo> --json paths`, with PATH, HOME and the
    /// state-directory override all pinned so the answer describes the fixture.
    fn paths_json(&self) -> Value {
        let out = Command::new(DEVMAP)
            .args(["--root", self.repo.to_str().unwrap(), "--json", "paths"])
            .current_dir(&self.repo)
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
            .env("HOME", &self.home)
            .env_remove("DEVMAP_HOME")
            .output()
            .expect("run devmap paths");
        assert!(
            out.status.success(),
            "paths failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).expect("paths emitted JSON")
    }

    /// The `sha256` the answer reports for the fake binary.
    ///
    /// Matched on the fixture's own name fragment: the inventory canonicalizes,
    /// and on macOS the temp tree is reached through `/var` -> `/private/var`.
    fn reported_digest(&self) -> Option<String> {
        let answer = self.paths_json();
        let rows = answer["binaries"].as_array().expect("binaries array");
        let row = rows
            .iter()
            .find(|row| {
                row["path"]
                    .as_str()
                    .is_some_and(|path| path.contains("devmap-bindigest-"))
            })
            .expect("the fixture binary is in the inventory");
        row["sha256"].as_str().map(str::to_string)
    }

    /// What the memo currently records for the fake binary.
    fn recorded(&self) -> Option<Value> {
        let text = std::fs::read_to_string(self.memo()).ok()?;
        let memo: Value = serde_json::from_str(&text).ok()?;
        memo["entries"].get(self.fake_key()).cloned()
    }
}

/// The digest the bytes actually hash to, computed independently of the code
/// under test so an agreeing answer means agreement and not a shared bug.
fn shasum(path: &Path) -> String {
    let out = Command::new("shasum")
        .args(["-a", "256"])
        .arg(path)
        .output()
        .expect("shasum");
    assert!(out.status.success(), "shasum failed");
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .expect("shasum printed a digest")
        .to_string()
}

// ---------------------------------------------------------------- hits

/// A hit is served without reading the bytes.
///
/// The planted digest is well-formed and unreachable: no input hashes to it, so
/// an answer carrying it can only have come from the memo. Against the pre-fix
/// binary the memo is not consulted and the real digest comes back instead.
#[test]
fn a_memoised_digest_is_served_without_reading_the_binary() {
    let fx = fixture("hit", b"first contents of a pretend devmap binary", true);
    fx.plant(PLANTED);
    assert_eq!(
        fx.reported_digest().as_deref(),
        Some(PLANTED),
        "an unchanged binary must be answered from the memo"
    );
}

/// The first run records what it hashed, under the key it hashed it at.
#[test]
fn a_first_run_records_what_it_hashed() {
    let fx = fixture("record", b"contents worth remembering", true);
    let truth = shasum(&fx.fake);
    let key = fx.stat_key();

    assert_eq!(fx.reported_digest().as_deref(), Some(truth.as_str()));

    let entry = fx.recorded().expect("the memo names the fixture binary");
    assert_eq!(entry["sha256"].as_str(), Some(truth.as_str()));
    assert_eq!(
        entry["key"].as_str(),
        Some(key.as_str()),
        "the recorded key must be the one the file had when it was hashed"
    );
}

// --------------------------------------------------------------- misses

/// A rebuilt binary is rehashed even when its size and mtime are put back.
///
/// This is the `cp -p` / `tar -x` / `rsync --times` shape, and the reason the
/// key carries ctime. The memo is planted *before* the rewrite and the file is
/// then restored to its old mtime; only ctime distinguishes the two states.
#[test]
fn a_rewrite_that_restores_the_old_mtime_is_still_rehashed() {
    let fx = fixture("backdate", b"aaaaaaaaaaaaaaaa", true);
    let reference = fx.base.join("reference");
    std::fs::write(&reference, b"").expect("write reference");

    // Park the original mtime, plant a hit for the current state, then rewrite
    // to different bytes of the same length and put the mtime back.
    assert!(touch_from(&fx.fake, &reference), "touch -r unavailable");
    fx.plant(PLANTED);
    let original = std::fs::metadata(&fx.fake).expect("stat").modified().ok();
    std::fs::write(&fx.fake, b"bbbbbbbbbbbbbbbb").expect("rewrite");
    assert!(touch_from(&reference, &fx.fake), "touch -r unavailable");

    // Both halves of the premise, checked rather than assumed.
    assert_eq!(
        std::fs::metadata(&fx.fake).expect("stat").len(),
        16,
        "the rewrite must keep the size identical"
    );
    if std::fs::metadata(&fx.fake).expect("stat").modified().ok() != original {
        eprintln!("skipped: `touch -r` did not restore mtime exactly");
        return;
    }

    assert_eq!(
        fx.reported_digest().as_deref(),
        Some(shasum(&fx.fake).as_str()),
        "a back-dated rewrite must be rehashed, not served from the memo"
    );
}

/// A plain rebuild — different bytes, different length — is rehashed.
#[test]
fn a_changed_binary_is_rehashed() {
    let fx = fixture("changed", b"before", true);
    fx.plant(PLANTED);
    std::fs::write(&fx.fake, b"after, and longer than before").expect("rewrite");
    assert_eq!(
        fx.reported_digest().as_deref(),
        Some(shasum(&fx.fake).as_str())
    );
}

/// A memo naming a *different* file never answers for this one.
#[test]
fn a_memo_keyed_on_another_path_is_not_consulted() {
    let fx = fixture("otherpath", b"contents", true);
    fx.plant_raw(
        &json!({
            "scheme": "b1",
            "entries": {
                "/somewhere/else/devmap": {
                    "key": fx.stat_key(),
                    "sha256": PLANTED,
                    "used": 1_789_000_000u64,
                }
            }
        })
        .to_string(),
    );
    assert_eq!(
        fx.reported_digest().as_deref(),
        Some(shasum(&fx.fake).as_str())
    );
}

// ------------------------------------------------------- refusing garbage

/// Every unusable memo costs a rehash and nothing else.
///
/// The property under test is uniform across the shapes: whatever the file
/// contains, the answer is the digest of the bytes. A memo is an optimisation,
/// and an optimisation that can make an answer *wrong or missing* is a defect,
/// not a trade-off.
#[test]
fn an_unusable_memo_costs_a_rehash_and_nothing_else() {
    for (label, body) in unusable_memos() {
        let fx = fixture("garbage", b"the bytes that must win", true);
        let text = body.replace("__KEY__", &fx.stat_key());
        let text = text.replace("__PATH__", &fx.fake_key());
        fx.plant_raw(&text);
        assert_eq!(
            fx.reported_digest().as_deref(),
            Some(shasum(&fx.fake).as_str()),
            "memo shape {label:?} changed the answer"
        );
    }
}

/// Every way a memo can be unusable, each paired with what makes it so.
fn unusable_memos() -> Vec<(&'static str, String)> {
    let oversized_entries: Vec<String> = (0..4000)
        .map(|i| {
            format!(
                "\"/pad/{i}\":{{\"key\":\"1:1:1\",\"sha256\":\"{}\",\"used\":1}}",
                "a".repeat(64)
            )
        })
        .collect();
    vec![
        ("empty file", String::new()),
        ("not JSON at all", "\u{0}\u{1}not json{{{".to_string()),
        ("JSON, wrong type", "[1,2,3]".to_string()),
        ("truncated mid-object", "{\"scheme\":\"b1\",\"entr".to_string()),
        (
            "a newer scheme",
            json!({"scheme": "b99", "entries": {"__PATH__": {"key": "__KEY__", "sha256": PLANTED, "used": 1}}})
                .to_string(),
        ),
        (
            "no scheme at all",
            json!({"entries": {"__PATH__": {"key": "__KEY__", "sha256": PLANTED, "used": 1}}})
                .to_string(),
        ),
        (
            "a digest that is not hex",
            json!({"scheme": "b1", "entries": {"__PATH__": {"key": "__KEY__", "sha256": "not a digest", "used": 1}}})
                .to_string(),
        ),
        (
            "a digest of the wrong length",
            json!({"scheme": "b1", "entries": {"__PATH__": {"key": "__KEY__", "sha256": "abc123", "used": 1}}})
                .to_string(),
        ),
        (
            "an uppercase digest",
            json!({"scheme": "b1", "entries": {"__PATH__": {"key": "__KEY__", "sha256": PLANTED.to_uppercase(), "used": 1}}})
                .to_string(),
        ),
        (
            "a digest that is a number",
            json!({"scheme": "b1", "entries": {"__PATH__": {"key": "__KEY__", "sha256": 12345, "used": 1}}})
                .to_string(),
        ),
        (
            "a key with too few fields",
            json!({"scheme": "b1", "entries": {"__PATH__": {"key": "123:456", "sha256": PLANTED, "used": 1}}})
                .to_string(),
        ),
        (
            "a key with too many fields",
            json!({"scheme": "b1", "entries": {"__PATH__": {"key": "1:2:3:4", "sha256": PLANTED, "used": 1}}})
                .to_string(),
        ),
        (
            "a size-only key",
            json!({"scheme": "b1", "entries": {"__PATH__": {"key": "23:-:-", "sha256": PLANTED, "used": 1}}})
                .to_string(),
        ),
        (
            "a key that is not a key",
            json!({"scheme": "b1", "entries": {"__PATH__": {"key": "../../etc/passwd", "sha256": PLANTED, "used": 1}}})
                .to_string(),
        ),
        (
            "entries that are not an object",
            json!({"scheme": "b1", "entries": ["__PATH__"]}).to_string(),
        ),
        (
            "an entry that is not an object",
            json!({"scheme": "b1", "entries": {"__PATH__": PLANTED}}).to_string(),
        ),
        (
            "a missing used stamp",
            json!({"scheme": "b1", "entries": {"__PATH__": {"key": "__KEY__", "sha256": PLANTED}}})
                .to_string(),
        ),
        (
            "a negative used stamp",
            json!({"scheme": "b1", "entries": {"__PATH__": {"key": "__KEY__", "sha256": PLANTED, "used": -1}}})
                .to_string(),
        ),
        (
            "past the byte ceiling",
            format!(
                "{{\"scheme\":\"b1\",\"entries\":{{{}}}}}",
                oversized_entries.join(",")
            ),
        ),
    ]
}

// -------------------------------------------------- the no-state-dir rule

/// `devmap paths` must not create a state directory to hold a memo.
///
/// It is a read-only query that *reports* `state_dir_exists`, and it runs in
/// whatever directory an agent happens to be in. Creating one as a side effect
/// would scatter directories across the filesystem and make the field it
/// reports self-fulfilling. Before a build there is no memo and the digest is
/// computed — the correct answer, just not the fast one.
#[test]
fn the_memo_never_creates_a_state_directory() {
    let fx = fixture("nostate", b"no state directory here", false);
    assert_eq!(
        fx.reported_digest().as_deref(),
        Some(shasum(&fx.fake).as_str()),
        "the digest must still be right without a memo"
    );

    let mut created: Vec<String> = std::fs::read_dir(&fx.repo)
        .expect("read repo")
        .filter_map(|entry| Some(entry.ok()?.file_name().to_string_lossy().into_owned()))
        .collect();
    created.sort();
    assert!(
        created.is_empty(),
        "answering created {created:?} in a repository that had nothing"
    );
}

/// `devmap doctor` must not leave a memo behind.
///
/// Its handler already refuses to create a store, on the grounds that a
/// usability probe runs against a tree the caller may not own. Writing a memo
/// there would be the same write by another name. It still *reads* one that
/// `devmap paths` left, which the unit tests cover.
#[test]
fn doctor_reads_a_memo_but_never_writes_one() {
    let fx = fixture("doctor", b"a binary doctor will hash", true);
    let out = Command::new(DEVMAP)
        .args(["--root", fx.repo.to_str().unwrap(), "--json", "doctor"])
        .current_dir(&fx.repo)
        .env("PATH", format!("{}:/usr/bin:/bin", fx.bin.display()))
        .env("HOME", &fx.home)
        .env_remove("DEVMAP_HOME")
        .output()
        .expect("run devmap doctor");
    assert!(
        out.status.success(),
        "doctor failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let answer: Value = serde_json::from_slice(&out.stdout).expect("doctor emitted JSON");
    let row = answer["binaries"]
        .as_array()
        .expect("binaries array")
        .iter()
        .find(|row| {
            row["path"]
                .as_str()
                .is_some_and(|path| path.contains("devmap-bindigest-"))
        })
        .expect("the fixture binary is in the inventory");
    assert_eq!(
        row["sha256"].as_str(),
        Some(shasum(&fx.fake).as_str()),
        "doctor must still report the right digest"
    );
    assert!(
        !fx.memo().exists(),
        "doctor wrote a memo into a tree it only probed"
    );
}

/// A memo left by `paths` survives `doctor` unchanged.
#[test]
fn doctor_leaves_an_existing_memo_alone() {
    let fx = fixture("doctor-keep", b"a binary paths already hashed", true);
    fx.reported_digest().expect("paths writes a memo");
    let before = std::fs::read(fx.memo()).expect("read memo");

    let out = Command::new(DEVMAP)
        .args(["--root", fx.repo.to_str().unwrap(), "--json", "doctor"])
        .current_dir(&fx.repo)
        .env("PATH", format!("{}:/usr/bin:/bin", fx.bin.display()))
        .env("HOME", &fx.home)
        .env_remove("DEVMAP_HOME")
        .output()
        .expect("run devmap doctor");
    assert!(out.status.success());
    assert_eq!(
        std::fs::read(fx.memo()).expect("read memo"),
        before,
        "doctor rewrote the memo"
    );
}

/// `touch -r source target`, which calls the same `utimensat` that `cp -p`
/// does. False when the tool is missing or refuses.
fn touch_from(source: &Path, target: &Path) -> bool {
    Command::new("touch")
        .arg("-r")
        .arg(source)
        .arg(target)
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}
