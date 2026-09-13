//! A stat-keyed memo for the `sha256` field `devmap paths` and `devmap doctor`
//! report against every `devmap` binary they find.
//!
//! **Why this exists.** Both commands hash the bytes of every binary in the
//! inventory. Measured 2026-09-13 on this machine that is 63.5 MiB of SHA-256
//! for a single installed release, and up to ~170 MiB when a debug build is on
//! `PATH` beside it. `devmap paths --json` is the first command the generated
//! agent guide tells an agent to run, so that cost lands once per agent turn —
//! forever, for an answer that changes only when a binary is rebuilt.
//!
//! **The key is `(path, size, mtime, ctime)`**, derived by
//! [`devmap_query::stat_memo::stat_key`] — the same derivation the repository
//! content memo uses, because a memo that hashes correctly but keys carelessly
//! returns a stale digest as a fresh one, and no caller can tell.
//!
//! **Advisory in both directions.** An absent, oversized, corrupt, foreign-
//! scheme or unwritable memo costs a rehash and nothing else. Nothing here may
//! turn a cache miss into a missing answer, and nothing here may report a
//! digest it did not either compute or key-match.
//!
//! **The digest is load-bearing, not decorative.** `inspect_devmap_identity`
//! deliberately never executes a discovered binary, so `version` and
//! `build_id` are absent for every row but the running process, and `sha256`
//! is the *only* evidence `binaries_skew_warning` has that two installed
//! `devmap`s differ. A stale digest served as a fresh one would silently hide
//! binary skew — or invent it. That is why the key carries ctime, why every
//! field is revalidated on the way out of the file, and why a digest computed
//! over a file that moved underneath the hash is returned but never recorded.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::sha256;

/// Bumped whenever what is stored changes meaning. A digest written under an
/// older scheme must never be served under a newer one.
const SCHEME: &str = "b1";

/// How many binaries the memo remembers.
///
/// The inventory itself is a handful of rows — `PATH`, the running executable
/// and a few host MCP configs. The cap is what keeps a machine that has moved
/// through hundreds of build directories from accumulating an entry per
/// directory forever; least-recently-used entries fall off the end.
const MAX_ENTRIES: usize = 256;

/// Longest path the memo will key on, so one pathological entry cannot crowd
/// out the rest of the file.
const MAX_PATH_BYTES: usize = 4096;

/// How stale an entry's recency stamp may get before a hit refreshes it.
///
/// Without this, every hit bumped `used` and every `devmap paths` republished
/// the memo — an atomic write per invocation on a read-only query, and 30
/// tmp+renames on one file when a fleet of hooks starts at once. An hour is far
/// finer than eviction needs: the cap is [`MAX_ENTRIES`] binaries, which a
/// machine crosses over weeks, not minutes. The common case is now a pure read
/// that writes nothing.
const USED_REFRESH_SECONDS: u64 = 3600;

/// The JSON around one entry: two field names, a `size:mtime:ctime` key, a
/// 64-character digest, a seconds stamp, and the punctuation between them.
/// Rounded well up — it is a bound, not a measurement.
const ENTRY_OVERHEAD_BYTES: usize = 256;

/// Ceiling on the memo this module will read back.
///
/// **Derived, not chosen.** A hand-picked ceiling is how a memo comes to write
/// files it will not read: at [`MAX_ENTRIES`] entries of [`MAX_PATH_BYTES`]
/// each, a full memo is ~1.1 MB, and the 256 KiB ceiling this started with
/// would have rejected it whole — a permanent, silent cache miss on exactly the
/// machines with the most to gain. The factor of two is headroom for a file
/// something else has grown; past the ceiling the memo reads as absent, which
/// costs a rehash and never a wrong answer.
/// `a_memo_filled_to_capacity_can_be_read_back_again` holds the two ends
/// together.
const MAX_BYTES: u64 = 2 * (MAX_ENTRIES * (MAX_PATH_BYTES + ENTRY_OVERHEAD_BYTES)) as u64;

/// One remembered digest: what the file looked like, and what it hashed to.
///
/// Hand-mapped to and from `serde_json::Value` rather than derived: this crate
/// links `serde_json` and not `serde`, and every other structured payload in it
/// is built the same way.
#[derive(Debug, Clone)]
struct Entry {
    /// `size:mtime_ns:ctime_ns` at the time the digest was computed.
    key: String,
    /// Lowercase hex SHA-256 of the bytes.
    sha256: String,
    /// Unix seconds when this entry was last read or written, for eviction.
    used: u64,
}

/// The memo for one repository's state directory.
///
/// Built once per inventory pass and saved once, so a run that finds the same
/// binary through `PATH`, `current_exe` and three MCP configs stats it once and
/// hashes it at most once.
pub struct BinaryDigests {
    /// `None` disables persistence entirely — see [`BinaryDigests::open`].
    path: Option<PathBuf>,
    /// The state directory `path` hangs off, rechecked before publishing.
    state_dir: Option<PathBuf>,
    /// False for a caller that must read a memo and never write one.
    persist: bool,
    entries: BTreeMap<String, Entry>,
    /// Names this pass looked up, hit or miss. Eviction never drops one.
    touched: std::collections::BTreeSet<String>,
    dirty: bool,
    now: u64,
}

impl BinaryDigests {
    /// Open the memo for `state_dir`, or a memo that never persists.
    ///
    /// `state_dir` is `None`, or names a directory that does not exist, exactly
    /// when this process must not create one. `devmap paths` is a read-only
    /// query that *reports* `state_dir_exists`, and it runs in whatever
    /// directory an agent happens to be in: creating a state directory as a
    /// side effect of answering would both scatter directories across the
    /// filesystem and make the field it reports self-fulfilling. Before a
    /// build there is no memo and every digest is computed — the correct
    /// answer, just not the fast one.
    pub fn open(state_dir: Option<&Path>) -> Self {
        Self::with_persistence(state_dir, true)
    }

    /// A memo that reads what is there and never writes.
    ///
    /// `devmap doctor` is what a host runs *before* it trusts this binary
    /// against a tree it may not own, and its handler already refuses to create
    /// a store for exactly that reason — "is this binary usable?" must not
    /// become a write. It still benefits from a memo `devmap paths` left
    /// behind; it just does not leave one of its own. This is the same split
    /// `freshness::content_fingerprint` draws with its `persist_cache` flag.
    pub fn open_read_only(state_dir: Option<&Path>) -> Self {
        Self::with_persistence(state_dir, false)
    }

    fn with_persistence(state_dir: Option<&Path>, persist: bool) -> Self {
        let now = unix_seconds();
        let Some(state_dir) = state_dir.filter(|dir| dir.is_dir()) else {
            return Self {
                path: None,
                state_dir: None,
                persist,
                entries: BTreeMap::new(),
                touched: std::collections::BTreeSet::new(),
                dirty: false,
                now,
            };
        };
        let path = state_dir.join(devmap_extract::paths::BINARY_DIGEST_CACHE_RELPATH);
        let entries = load(&path, now);
        Self {
            path: Some(path),
            state_dir: Some(state_dir.to_path_buf()),
            persist,
            entries,
            touched: std::collections::BTreeSet::new(),
            dirty: false,
            now,
        }
    }

    /// The SHA-256 of `path`, from the memo when the file is unchanged.
    ///
    /// `None` for anything that cannot be stated or read, which is what every
    /// caller already renders as "no digest available" rather than as a zero
    /// digest.
    pub fn digest(&mut self, path: &Path) -> Option<String> {
        let meta = std::fs::metadata(path).ok()?;
        if !meta.is_file() {
            return None;
        }
        let key = devmap_query::stat_memo::stat_key(&meta);
        let name = memo_key(path);

        if let Some(name) = name.as_deref() {
            self.touched.insert(name.to_string());
            if let Some(entry) = self.entries.get_mut(name) {
                if entry.key == key && is_sha256_hex(&entry.sha256) {
                    // Touch it, or an entry that keeps hitting still ages out —
                    // but only once its stamp has actually gone stale, so a hit
                    // does not turn a read into a write.
                    if self.now.saturating_sub(entry.used) >= USED_REFRESH_SECONDS {
                        entry.used = self.now;
                        self.dirty = true;
                    }
                    return Some(entry.sha256.clone());
                }
            }
        }

        let digest = sha256::sha256_file(path)?;

        // Re-stat before remembering. A file rewritten *while* it was being
        // hashed yields a digest of a torn read: returning it matches what an
        // uncached run would have reported, but writing it into the memo under
        // the key we started with would make that torn value stick until the
        // next rebuild. The uncached path cannot make that mistake, so neither
        // may this one.
        let settled = std::fs::metadata(path)
            .ok()
            .filter(|meta| meta.is_file())
            .is_some_and(|meta| devmap_query::stat_memo::stat_key(&meta) == key);
        if let (Some(name), true) = (name, settled) {
            self.entries.insert(
                name,
                Entry {
                    key,
                    sha256: digest.clone(),
                    used: self.now,
                },
            );
            self.dirty = true;
        }
        Some(digest)
    }

    /// Persist, if anything changed and a state directory was resolved.
    ///
    /// Every failure is silent on purpose: a read-only checkout, a full disk or
    /// a racing sibling must cost the next run a rehash and this run nothing.
    pub fn save(&mut self) {
        if !self.dirty || !self.persist {
            return;
        }
        let Some(path) = self.path.clone() else {
            return;
        };
        self.evict();
        let entries: serde_json::Map<String, Value> = self
            .entries
            .iter()
            .map(|(name, entry)| {
                (
                    name.clone(),
                    json!({"key": entry.key, "sha256": entry.sha256, "used": entry.used}),
                )
            })
            .collect();
        let Ok(mut text) = serde_json::to_string(&json!({
            "scheme": SCHEME,
            "entries": Value::Object(entries),
        })) else {
            return;
        };
        text.push('\n');
        // Rechecked, not remembered from `open`. Between the two the directory
        // can be deleted — `devmap build` republishing state, a cleanup pass, a
        // sibling worktree being torn down — and `create_dir_all` would then
        // put it back, which is the one thing this must never do. It costs one
        // `stat` on a path that is about to be written anyway.
        if !self.state_dir.as_deref().is_some_and(|dir| dir.is_dir()) {
            return;
        }
        if std::fs::create_dir_all(path.parent().unwrap_or(Path::new("."))).is_err() {
            return;
        }
        // Through the shared artifact writer: tmp+rename under a pinned parent
        // directory, so a crashed or concurrent process cannot leave a partial
        // memo for the next run to read as authoritative.
        let _ = devmap_query::artifacts::write_atomic(&path, text.as_bytes());
        self.dirty = false;
    }

    /// Drop entries down to [`MAX_ENTRIES`], newest first.
    ///
    /// **What this pass touched is never dropped.** Recency alone is not
    /// enough: a full memo whose entries all carry the same one-second stamp
    /// falls through to the path tie-break, and a binary whose path sorts late
    /// is then evicted the instant it is hashed — every run, so it never once
    /// hits. That is reachable without any tampering, because the entries the
    /// inventory refreshes together do share a stamp. A memo that discards the
    /// answer it just computed is not a cache.
    ///
    /// Below that, most-recently-used, then path so the outcome does not
    /// depend on `BTreeMap` iteration or on which entries share a timestamp.
    fn evict(&mut self) {
        if self.entries.len() <= MAX_ENTRIES {
            return;
        }
        let mut ranked: Vec<(bool, u64, String)> = self
            .entries
            .iter()
            .map(|(name, entry)| (self.touched.contains(name), entry.used, name.clone()))
            .collect();
        ranked.sort_unstable_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| b.1.cmp(&a.1))
                .then_with(|| a.2.cmp(&b.2))
        });
        for (_, _, name) in ranked.into_iter().skip(MAX_ENTRIES) {
            self.entries.remove(&name);
        }
    }
}

/// The memo key for a path, or `None` when it cannot be one.
///
/// Non-UTF-8 paths are refused rather than lossily rendered: `to_string_lossy`
/// maps distinct paths onto one key, and two binaries sharing a key is the one
/// way this memo could report a digest belonging to a different file. Refusing
/// costs a rehash on every run for such a path, which is correct and rare.
fn memo_key(path: &Path) -> Option<String> {
    let text = path.to_str()?;
    (text.len() <= MAX_PATH_BYTES).then(|| text.to_string())
}

/// 64 lowercase hex characters, and nothing else.
///
/// The memo is a file on disk that anything with write access can edit. Values
/// are checked on the way out, not merely on the way in, so a hand-edited or
/// truncated entry can never be rendered to a caller as a digest.
fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// `size:mtime_ns:ctime_ns`, as [`stat_key`] spells it on this platform.
///
/// Size and mtime must both be integers. `ctime_ns` may be `-`, which is what
/// [`stat_key`] emits off unix, where there is no `st_ctime` — refusing that
/// spelling would silently disable the memo on every non-unix host, which is a
/// check that cannot run reporting the same thing as a check that passed.
///
/// A key whose *mtime* is `-` is refused. That spelling means the platform
/// could not report a modification time, leaving size as the only evidence,
/// and a size-only key turns every same-size rewrite into a wrong hit. A
/// rehash is the correct answer there.
///
/// [`stat_key`]: devmap_query::stat_memo::stat_key
fn is_stat_key(value: &str) -> bool {
    let mut fields = value.split(':');
    let size = fields
        .next()
        .is_some_and(|text| text.parse::<u64>().is_ok());
    let mtime = fields
        .next()
        .is_some_and(|text| text.parse::<i128>().is_ok());
    let ctime = fields
        .next()
        .is_some_and(|text| text == "-" || text.parse::<i128>().is_ok());
    size && mtime && ctime && fields.next().is_none()
}

/// `now` clamps recency stamps — see the note on the `used` field below.
fn load(path: &Path, now: u64) -> BTreeMap<String, Entry> {
    let Some(text) = devmap_query::stat_memo::read_bounded(path, MAX_BYTES) else {
        return BTreeMap::new();
    };
    let Ok(memo) = serde_json::from_str::<Value>(&text) else {
        return BTreeMap::new();
    };
    if memo["scheme"].as_str() != Some(SCHEME) {
        return BTreeMap::new();
    }
    let Some(entries) = memo["entries"].as_object() else {
        return BTreeMap::new();
    };
    // Per-entry validation, not per-file: one hand-edited row must not throw
    // away the rest, and must not survive either. Every field is checked on the
    // way *out* of the file, because the file is ordinary state on disk and
    // nothing guarantees this process is what last wrote it.
    let mut admitted: Vec<(String, Entry)> = entries
        .iter()
        .filter_map(|(name, value)| {
            let key = value["key"].as_str()?;
            let sha256 = value["sha256"].as_str()?;
            // Clamped to now. An entry claiming it was last used in the
            // future is not credible, and left as written it sorts ahead of
            // every real entry forever: `evict` would keep it and drop the
            // binaries actually in use. Reachable from a corrupted file or a
            // clock that moved, so it is handled rather than trusted.
            let used = value["used"].as_u64()?.min(now);
            (name.len() <= MAX_PATH_BYTES && is_stat_key(key) && is_sha256_hex(sha256)).then(|| {
                (
                    name.clone(),
                    Entry {
                        key: key.to_string(),
                        sha256: sha256.to_string(),
                        used,
                    },
                )
            })
        })
        .collect();
    // A memo this code wrote is already capped, so this only bites on a
    // foreign or hand-edited file — where taking the lexicographically first
    // 256 of 10,000 would discard exactly the entries in daily use and keep
    // whatever sorts early. Recency is the same order `evict` uses.
    if admitted.len() > MAX_ENTRIES {
        admitted.sort_unstable_by(|a, b| b.1.used.cmp(&a.1.used).then_with(|| a.0.cmp(&b.0)));
        admitted.truncate(MAX_ENTRIES);
    }
    admitted.into_iter().collect()
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0)
}

/// Helpers both test modules use.
#[cfg(test)]
mod tests_support {
    use super::*;

    /// The memo's path under a state directory, for tests that plant one.
    pub const RELPATH: &str = devmap_extract::paths::BINARY_DIGEST_CACHE_RELPATH;

    pub fn scratch(label: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQUENCE: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "devmap-digestcache-{label}-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::*;
    use super::*;

    #[test]
    fn only_64_lowercase_hex_is_a_digest() {
        let good = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        assert!(is_sha256_hex(good));
        assert!(!is_sha256_hex(&good.to_uppercase()), "uppercase");
        assert!(!is_sha256_hex(&good[1..]), "63 characters");
        assert!(!is_sha256_hex(&format!("{good}0")), "65 characters");
        assert!(!is_sha256_hex(""), "empty");
        assert!(!is_sha256_hex(&"g".repeat(64)), "not hex");
        assert!(!is_sha256_hex(&"0".repeat(63).to_string()), "short");
        // A 64-character string of digits *is* valid hex, and must stay valid:
        // refusing it would silently drop one digest in 10^-13 of them.
        assert!(is_sha256_hex(&"1".repeat(64)));
    }

    #[test]
    fn a_key_must_carry_a_size_and_an_mtime() {
        assert!(is_stat_key("0:0:0"));
        assert!(is_stat_key(
            "63500000:1789276480712334647:1789276480712334647"
        ));
        assert!(is_stat_key("12:-1000:-2000"), "pre-epoch timestamps");
        // Off unix there is no ctime, and refusing that spelling would disable
        // the memo on every non-unix host without saying so.
        assert!(is_stat_key("12:1789276480712334647:-"));

        assert!(!is_stat_key("12:-:-"), "size-only keys are refused");
        assert!(!is_stat_key("-:1:1"), "a size must be a size");
        assert!(!is_stat_key("-12:1:1"), "and never negative");
        assert!(!is_stat_key("12:1"), "too few fields");
        assert!(!is_stat_key("12:1:1:1"), "too many fields");
        assert!(!is_stat_key(""), "empty");
        assert!(!is_stat_key("../../etc/passwd"), "not a key at all");
        assert!(!is_stat_key("12:1e9:1"), "not an integer");
    }

    #[test]
    fn a_path_the_memo_cannot_key_on_is_refused_not_mangled() {
        assert_eq!(
            memo_key(Path::new("/usr/local/bin/devmap")).as_deref(),
            Some("/usr/local/bin/devmap")
        );
        let long = PathBuf::from(format!("/{}", "a".repeat(MAX_PATH_BYTES)));
        assert!(memo_key(&long).is_none(), "past the path ceiling");

        // Two distinct non-UTF-8 paths must not collapse onto one key.
        // `to_string_lossy` maps both of these to "/tmp/\u{fffd}", and a shared
        // key is the one way this memo could serve another file's digest.
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let a = Path::new(std::ffi::OsStr::from_bytes(b"/tmp/\xff"));
            let b = Path::new(std::ffi::OsStr::from_bytes(b"/tmp/\xfe"));
            assert_eq!(a.to_string_lossy(), b.to_string_lossy(), "the trap");
            assert!(memo_key(a).is_none());
            assert!(memo_key(b).is_none());
        }
    }

    #[test]
    fn a_memo_without_a_state_directory_still_answers() {
        let dir = scratch("nostate");
        let file = dir.join("payload");
        std::fs::write(&file, b"contents").expect("write");

        // Both spellings of "no memo": no directory named, and one named that
        // does not exist. Neither may create anything.
        let absent = dir.join("does-not-exist");
        for state in [None, Some(absent.as_path())] {
            let mut memo = BinaryDigests::open(state);
            assert_eq!(
                memo.digest(&file).as_deref(),
                Some(sha256::sha256_file(&file).expect("digest").as_str())
            );
            memo.save();
        }
        assert!(!absent.exists(), "opening a memo created a state directory");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_digest_survives_a_round_trip_and_a_change_does_not() {
        let dir = scratch("roundtrip");
        let state = dir.join("state");
        std::fs::create_dir_all(&state).expect("mkdir state");
        let file = dir.join("payload");
        std::fs::write(&file, b"first").expect("write");

        let mut writer = BinaryDigests::open(Some(&state));
        let first = writer.digest(&file).expect("digest");
        writer.save();

        // Reopened in a fresh memo, the entry is a hit: same key, same digest.
        let mut reader = BinaryDigests::open(Some(&state));
        assert_eq!(reader.entries.len(), 1, "the memo persisted one entry");
        assert_eq!(reader.digest(&file).as_deref(), Some(first.as_str()));

        // Changed bytes, changed answer — through a fresh memo, so the result
        // cannot come from in-process state.
        std::fs::write(&file, b"second and different").expect("rewrite");
        let mut after = BinaryDigests::open(Some(&state));
        let second = after.digest(&file).expect("digest");
        assert_ne!(second, first);
        assert_eq!(
            second,
            sha256::sha256_file(&file).expect("digest"),
            "the answer must be the digest of the bytes on disk"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_memo_keeps_the_most_recently_used_entries_and_no_more() {
        let dir = scratch("evict");
        let state = dir.join("state");
        std::fs::create_dir_all(&state).expect("mkdir state");

        let mut memo = BinaryDigests::open(Some(&state));
        // Deliberately past the cap, with recency running the other way from
        // insertion order so "kept the last N inserted" cannot pass by luck.
        let overflow = MAX_ENTRIES + 64;
        for index in 0..overflow {
            memo.entries.insert(
                format!("/bin/devmap-{index:04}"),
                Entry {
                    key: "1:1:1".to_string(),
                    sha256: "a".repeat(64),
                    used: index as u64,
                },
            );
        }
        memo.dirty = true;
        memo.save();

        let reloaded = BinaryDigests::open(Some(&state));
        assert_eq!(reloaded.entries.len(), MAX_ENTRIES, "capped on the way out");
        // The oldest went; the newest stayed.
        assert!(reloaded
            .entries
            .contains_key(&format!("/bin/devmap-{:04}", overflow - 1)));
        assert!(!reloaded.entries.contains_key("/bin/devmap-0000"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_memo_that_cannot_be_written_costs_nothing() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("readonly");
        let state = dir.join("state");
        std::fs::create_dir_all(&state).expect("mkdir state");
        let file = dir.join("payload");
        std::fs::write(&file, b"contents").expect("write");

        // A checkout an agent can read and not write is an ordinary situation,
        // and it must cost a rehash rather than an error.
        std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o555)).expect("chmod");
        let mut memo = BinaryDigests::open(Some(&state));
        let digest = memo.digest(&file);
        memo.save();
        std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o755)).expect("chmod");

        assert_eq!(
            digest.as_deref(),
            Some(sha256::sha256_file(&file).expect("digest").as_str()),
            "a read-only state directory must not change the answer"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_directory_and_a_missing_file_have_no_digest() {
        let dir = scratch("shapes");
        let mut memo = BinaryDigests::open(None);
        assert!(memo.digest(&dir).is_none(), "a directory is not a binary");
        assert!(memo.digest(&dir.join("nope")).is_none(), "missing");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_second_lookup_in_one_pass_does_not_rewrite_the_memo() {
        // The inventory reaches the same binary through PATH, `current_exe` and
        // several MCP configs. The pass must settle after the first hash.
        let dir = scratch("onepass");
        let state = dir.join("state");
        std::fs::create_dir_all(&state).expect("mkdir state");
        let file = dir.join("payload");
        std::fs::write(&file, b"contents").expect("write");

        let mut memo = BinaryDigests::open(Some(&state));
        let first = memo.digest(&file);
        memo.save();
        assert!(!memo.dirty, "saving settles the memo");
        let second = memo.digest(&file);
        assert_eq!(first, second);
        assert!(
            !memo.dirty,
            "a same-second hit must not dirty the memo again"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod adversarial {
    use super::tests_support::*;
    use super::*;

    /// A memo filled to capacity must still be readable.
    ///
    /// The read ceiling and the write cap are two constants that have to agree,
    /// and when they drifted apart the failure was invisible: every write
    /// succeeded, every read after it returned nothing, and the only symptom
    /// was that the memo never hit. This is the test that holds them together.
    #[test]
    fn a_memo_filled_to_capacity_can_be_read_back_again() {
        let dir = scratch("capacity");
        let state = dir.join("state");
        std::fs::create_dir_all(&state).expect("mkdir state");

        let mut memo = BinaryDigests::open(Some(&state));
        for index in 0..MAX_ENTRIES {
            // The longest path the memo will accept, so the file lands at the
            // largest size this code can produce.
            let name = format!("/{:0>width$}", index, width = MAX_PATH_BYTES - 1);
            assert_eq!(name.len(), MAX_PATH_BYTES);
            memo.entries.insert(
                name,
                Entry {
                    key: "1:1:1".to_string(),
                    sha256: "a".repeat(64),
                    used: index as u64,
                },
            );
        }
        memo.dirty = true;
        memo.save();

        let written = std::fs::metadata(state.join(RELPATH))
            .expect("stat memo")
            .len();
        assert!(
            written <= MAX_BYTES,
            "a full memo is {written} bytes, past its own {MAX_BYTES}-byte read ceiling"
        );
        assert_eq!(
            BinaryDigests::open(Some(&state)).entries.len(),
            MAX_ENTRIES,
            "a memo this code wrote must be one it can read"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A memo that is a directory, a device, or a dangling symlink is absent.
    #[test]
    fn a_memo_that_is_not_a_regular_file_is_absent() {
        let dir = scratch("shapes");
        let state = dir.join("state");
        let memo = state.join(RELPATH);
        std::fs::create_dir_all(memo.parent().expect("parent")).expect("mkdir");

        // A directory where the memo should be.
        std::fs::create_dir(&memo).expect("mkdir memo");
        assert!(BinaryDigests::open(Some(&state)).entries.is_empty());
        std::fs::remove_dir(&memo).expect("rmdir");

        // A symlink to a character device: `File::open` succeeds and
        // `read_to_string` would block forever on /dev/zero without the
        // is_file() guard.
        std::os::unix::fs::symlink("/dev/zero", &memo).expect("symlink");
        assert!(BinaryDigests::open(Some(&state)).entries.is_empty());
        std::fs::remove_file(&memo).expect("unlink");

        // A dangling symlink.
        std::os::unix::fs::symlink(dir.join("nowhere"), &memo).expect("symlink");
        assert!(BinaryDigests::open(Some(&state)).entries.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Deeply nested JSON must not recurse the parser into the stack.
    #[test]
    fn a_deeply_nested_memo_is_refused_not_recursed() {
        let dir = scratch("nested");
        let state = dir.join("state");
        let memo = state.join(RELPATH);
        std::fs::create_dir_all(memo.parent().expect("parent")).expect("mkdir");
        let depth = 20_000;
        std::fs::write(&memo, format!("{}{}", "[".repeat(depth), "]".repeat(depth)))
            .expect("write");
        assert!(BinaryDigests::open(Some(&state)).entries.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// One bad row must not discard the good ones beside it.
    #[test]
    fn a_hand_edited_row_is_dropped_and_its_neighbours_kept() {
        let dir = scratch("mixed");
        let state = dir.join("state");
        let memo = state.join(RELPATH);
        std::fs::create_dir_all(memo.parent().expect("parent")).expect("mkdir");
        let good = "a".repeat(64);
        std::fs::write(
            &memo,
            serde_json::json!({
                "scheme": SCHEME,
                "entries": {
                    "/bin/good-one": {"key": "1:2:3", "sha256": good, "used": 5},
                    "/bin/bad-digest": {"key": "1:2:3", "sha256": "nope", "used": 5},
                    "/bin/bad-key": {"key": "oops", "sha256": good, "used": 5},
                    "/bin/good-two": {"key": "9:8:-", "sha256": good, "used": 6},
                }
            })
            .to_string(),
        )
        .expect("write");

        let loaded = BinaryDigests::open(Some(&state));
        let mut names: Vec<&str> = loaded.entries.keys().map(String::as_str).collect();
        names.sort_unstable();
        assert_eq!(names, ["/bin/good-one", "/bin/good-two"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A foreign memo far past the cap keeps the entries most recently used.
    #[test]
    fn an_oversized_entry_count_is_trimmed_by_recency() {
        let dir = scratch("trim");
        let state = dir.join("state");
        let memo = state.join(RELPATH);
        std::fs::create_dir_all(memo.parent().expect("parent")).expect("mkdir");
        let good = "a".repeat(64);
        let mut entries = serde_json::Map::new();
        let total = MAX_ENTRIES + 40;
        for index in 0..total {
            // Names sort the *opposite* way from recency, so a load that
            // trimmed lexicographically would keep exactly the wrong ones.
            entries.insert(
                format!("/bin/z{:04}", total - index),
                serde_json::json!({"key": "1:2:3", "sha256": good, "used": index}),
            );
        }
        std::fs::write(
            &memo,
            serde_json::json!({"scheme": SCHEME, "entries": entries}).to_string(),
        )
        .expect("write");

        let loaded = BinaryDigests::open(Some(&state));
        assert_eq!(loaded.entries.len(), MAX_ENTRIES);
        assert!(
            loaded
                .entries
                .contains_key(&format!("/bin/z{:04}", total - (total - 1))),
            "the most recently used entry must survive"
        );
        assert!(
            !loaded.entries.contains_key(&format!("/bin/z{total:04}")),
            "the least recently used entry must not"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod lifecycle {
    use super::tests_support::*;
    use super::*;

    /// A run that only hits must not write.
    ///
    /// `devmap paths` is a query, and a fleet of agent hooks starting together
    /// runs dozens of them at once. Republishing the memo on every hit made
    /// each of those an atomic write to one file for no new information.
    #[test]
    fn a_pure_hit_writes_nothing() {
        let dir = scratch("purehit");
        let state = dir.join("state");
        std::fs::create_dir_all(&state).expect("mkdir state");
        let file = dir.join("payload");
        std::fs::write(&file, b"contents").expect("write");

        let mut first = BinaryDigests::open(Some(&state));
        first.digest(&file).expect("digest");
        first.save();
        let memo = state.join(RELPATH);
        let before = std::fs::metadata(&memo).expect("stat memo");

        let mut second = BinaryDigests::open(Some(&state));
        second.digest(&file).expect("digest");
        assert!(!second.dirty, "a fresh hit must not dirty the memo");
        second.save();

        let after = std::fs::metadata(&memo).expect("stat memo");
        assert_eq!(
            before.modified().ok(),
            after.modified().ok(),
            "a hit republished the memo"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A stamp that has gone stale is refreshed, or nothing ever ages out.
    #[test]
    fn a_stale_stamp_is_refreshed_so_eviction_still_orders() {
        let dir = scratch("refresh");
        let state = dir.join("state");
        std::fs::create_dir_all(&state).expect("mkdir state");
        let file = dir.join("payload");
        std::fs::write(&file, b"contents").expect("write");

        let mut memo = BinaryDigests::open(Some(&state));
        let digest = memo.digest(&file).expect("digest");
        memo.save();

        // Age the stored stamp past the refresh window and look again.
        let key = memo_key(&file).expect("key");
        let mut aged = BinaryDigests::open(Some(&state));
        aged.entries.get_mut(&key).expect("entry").used =
            aged.now.saturating_sub(USED_REFRESH_SECONDS + 1);
        aged.dirty = false;
        assert_eq!(aged.digest(&file).as_deref(), Some(digest.as_str()));
        assert!(aged.dirty, "a stale stamp must be refreshed");
        assert_eq!(aged.entries[&key].used, aged.now);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A state directory that vanishes after `open` is not put back.
    #[test]
    fn a_state_directory_deleted_mid_run_is_not_recreated() {
        let dir = scratch("vanish");
        let state = dir.join("state");
        std::fs::create_dir_all(&state).expect("mkdir state");
        let file = dir.join("payload");
        std::fs::write(&file, b"contents").expect("write");

        let mut memo = BinaryDigests::open(Some(&state));
        let digest = memo.digest(&file);
        // `devmap build` republishing state, a cleanup pass, a worktree torn
        // down — the directory is gone by the time the answer is ready.
        std::fs::remove_dir_all(&state).expect("rmdir state");
        memo.save();

        assert!(digest.is_some(), "the answer must still be right");
        assert!(
            !state.exists(),
            "saving recreated a deleted state directory"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod recency {
    use super::tests_support::*;
    use super::*;

    /// An entry stamped in the future must not outrank every real one.
    ///
    /// Left as written, a `used` of `u64::MAX` sorts first in `evict` forever,
    /// so a memo carrying [`MAX_ENTRIES`] of them would drop every binary
    /// actually in use and keep the garbage. Clamping to now makes eviction
    /// well-ordered whatever the file claims.
    #[test]
    fn a_stamp_from_the_future_is_clamped_to_now() {
        let dir = scratch("future");
        let state = dir.join("state");
        let memo = state.join(RELPATH);
        std::fs::create_dir_all(memo.parent().expect("parent")).expect("mkdir");
        let digest = "a".repeat(64);
        std::fs::write(
            &memo,
            json!({
                "scheme": SCHEME,
                "entries": {
                    "/bin/from-the-future": {"key": "1:2:3", "sha256": digest, "used": u64::MAX},
                }
            })
            .to_string(),
        )
        .expect("write");

        let loaded = BinaryDigests::open(Some(&state));
        let entry = &loaded.entries["/bin/from-the-future"];
        assert!(
            entry.used <= loaded.now,
            "a future stamp ({}) outranks now ({})",
            entry.used,
            loaded.now
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// And the clamp actually changes who survives eviction.
    #[test]
    fn future_stamps_do_not_crowd_out_the_binaries_in_use() {
        let dir = scratch("crowd");
        let state = dir.join("state");
        let memo = state.join(RELPATH);
        std::fs::create_dir_all(memo.parent().expect("parent")).expect("mkdir");
        let digest = "a".repeat(64);
        let mut entries = serde_json::Map::new();
        // A full memo of future-stamped junk, plus one real recent entry.
        for index in 0..MAX_ENTRIES {
            entries.insert(
                format!("/bin/junk-{index:04}"),
                json!({"key": "1:2:3", "sha256": digest, "used": u64::MAX - index as u64}),
            );
        }
        std::fs::write(
            &memo,
            json!({"scheme": SCHEME, "entries": entries}).to_string(),
        )
        .expect("write");

        let mut loaded = BinaryDigests::open(Some(&state));
        let real = dir.join("real-binary");
        std::fs::write(&real, b"a binary that is actually in use").expect("write");
        loaded.digest(&real).expect("digest");
        loaded.save();

        let after = BinaryDigests::open(Some(&state));
        assert_eq!(after.entries.len(), MAX_ENTRIES);
        assert!(
            after.entries.contains_key(&memo_key(&real).expect("key")),
            "the binary in use was evicted in favour of future-stamped junk"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod eviction_ordering {
    use super::tests_support::*;
    use super::*;

    /// The untampered version of the same failure.
    ///
    /// No poisoned stamps here: a full memo whose entries all carry one
    /// ordinary timestamp — which is what the inventory produces when it
    /// refreshes several binaries together — plus a real binary whose path
    /// sorts *after* all of them. Ordered by recency and path alone, the real
    /// binary loses the tie-break and is evicted the instant it is hashed,
    /// every run, so it never once hits.
    #[test]
    fn a_binary_hashed_this_pass_outranks_a_full_memo_of_equal_stamps() {
        let dir = scratch("tiebreak");
        let state = dir.join("state");
        std::fs::create_dir_all(&state).expect("mkdir state");
        // Sorts after every "/aaa-…" name below.
        let real = dir.join("zzz-real-binary");
        std::fs::write(&real, b"a binary that is actually in use").expect("write");

        let mut memo = BinaryDigests::open(Some(&state));
        let stamp = memo.now;
        for index in 0..MAX_ENTRIES {
            memo.entries.insert(
                format!("/aaa-{index:04}"),
                Entry {
                    key: "1:2:3".to_string(),
                    sha256: "a".repeat(64),
                    used: stamp,
                },
            );
        }
        memo.digest(&real).expect("digest");
        memo.save();

        let after = BinaryDigests::open(Some(&state));
        assert_eq!(after.entries.len(), MAX_ENTRIES);
        assert!(
            after.entries.contains_key(&memo_key(&real).expect("key")),
            "the binary hashed this pass was evicted on a path tie-break"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod read_only {
    use super::tests_support::*;
    use super::*;

    /// A read-only memo answers from what is there and leaves nothing behind.
    #[test]
    fn a_read_only_memo_never_writes() {
        let dir = scratch("readonly-mode");
        let state = dir.join("state");
        std::fs::create_dir_all(&state).expect("mkdir state");
        let file = dir.join("payload");
        std::fs::write(&file, b"contents").expect("write");

        let mut probe = BinaryDigests::open_read_only(Some(&state));
        let digest = probe.digest(&file).expect("digest");
        probe.save();
        assert_eq!(
            digest,
            sha256::sha256_file(&file).expect("digest"),
            "read-only must not change the answer"
        );
        assert!(
            !state.join(RELPATH).exists(),
            "a read-only memo wrote a file"
        );

        // But it does read one another caller left.
        let mut writer = BinaryDigests::open(Some(&state));
        writer.digest(&file).expect("digest");
        writer.save();
        let planted = "b".repeat(64);
        let mut tampered = BinaryDigests::open(Some(&state));
        let key = memo_key(&file).expect("key");
        tampered.entries.get_mut(&key).expect("entry").sha256 = planted.clone();
        tampered.dirty = true;
        tampered.save();

        let mut reader = BinaryDigests::open_read_only(Some(&state));
        assert_eq!(
            reader.digest(&file).as_deref(),
            Some(planted.as_str()),
            "a read-only memo must still consult what is on disk"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
