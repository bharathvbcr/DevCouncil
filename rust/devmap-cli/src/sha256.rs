//! SHA-256 for build identity and skill receipts.
//!
//! **One owner for the digest.** This module held a hand-rolled FIPS 180-4
//! implementation and `skills.rs` held a second one, both written to avoid a
//! crate dependency — a reason that stopped being true when `sha2` entered the
//! workspace for `dc-evidence`. Three implementations of one hash are three
//! chances to disagree about a receipt, and hand-rolled crypto earns its keep
//! only when nothing vetted is already in the tree.
//!
//! The hand-rolled version also cost more than it saved. `devmap paths --json`
//! is the first command the generated agent guide tells an agent to run, and it
//! hashes the devmap binary to report build identity: measured 2026-09-12
//! against the 61 MiB release binary, that was **34 s in a debug build** — past
//! the 20 s bound `diagnostics_never_execute_discovered_commands` puts on the
//! same call, which is how it first surfaced. An earlier draft of this note
//! also claimed it was 3.67 s of a 3.7 s *release* command; that was read off a
//! single stack sample and is wrong. Release `paths` was blocked on I/O and a
//! subprocess fan-out, not on this function.
//!
//! **Callers memoise this.** `devmap paths` and `devmap doctor` reach it
//! through [`crate::digest_cache`], which keys the result on
//! `(path, size, mtime, ctime)` so an unchanged binary is hashed once rather
//! than once per invocation. Nothing here caches; this function always reads
//! the bytes, which is what makes it usable as the memo's source of truth.

use std::io::Read;
use std::path::Path;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

/// Bytes per read when hashing a file.
const CHUNK: usize = 64 * 1024;

/// Hex digest of a byte slice.
pub fn sha256_hex(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}

/// What a whole inventory pass may spend hashing, shared across every file it
/// reaches rather than granted to each.
///
/// Streaming makes a hash cheap; it does not make it bounded. A large enough
/// file, a slow enough disk or a stalled network mount still runs as long as it
/// likes, and `devmap paths` — the command the generated agent guide tells every
/// agent to run first — reaches this before any real work. A health check that
/// can run for an unbounded time is worse than one that reports "unknown".
///
/// One `Budget` is meant to be shared across a pass rather than handed out per
/// file, for the reason `dc_proc` gives for its own paired waits: *n* files each
/// allowed the whole of it is not the bound the caller was promised.
pub struct Budget {
    started: Instant,
    wall: Duration,
    bytes_left: u64,
    bytes_total: u64,
}

impl Budget {
    pub fn new(wall: Duration, bytes: u64) -> Self {
        Self {
            started: Instant::now(),
            wall,
            bytes_left: bytes,
            bytes_total: bytes,
        }
    }

    pub fn wall(&self) -> Duration {
        self.wall
    }

    pub fn bytes_total(&self) -> u64 {
        self.bytes_total
    }

    pub fn bytes_left(&self) -> u64 {
        self.bytes_left
    }

    pub fn wall_exhausted(&self) -> bool {
        self.started.elapsed() >= self.wall
    }

    fn spend(&mut self, bytes: u64) {
        self.bytes_left = self.bytes_left.saturating_sub(bytes);
    }
}

/// The outcome of one budgeted hash.
///
/// Two states, and the second is the point: a digest that could not be taken
/// carries *why*, so a caller can report it as unavailable rather than as "no
/// hash" — and never as agreement.
#[derive(Debug)]
pub enum FileDigest {
    Hashed(String),
    Unavailable(String),
}

/// Hex digest of a file's contents, read in chunks and charged to `budget`.
///
/// Streamed rather than `fs::read`: the largest thing this hashes is the devmap
/// binary itself, and pulling 61 MiB into memory to hash it is a spike no
/// caller asked for — in a workspace that bounds every other payload it reads.
///
/// Every failure is an [`FileDigest::Unavailable`] carrying its reason. Nothing
/// here returns a bare `None`, because "could not" and "nothing to hash" are
/// different answers and the caller has to be able to tell them apart.
pub fn sha256_file_within(path: &Path, budget: &mut Budget) -> FileDigest {
    if budget.wall_exhausted() {
        return FileDigest::Unavailable(format!(
            "not hashed: the {:?} shared time budget for hashing discovered binaries was already \
             spent",
            budget.wall()
        ));
    }
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) => return FileDigest::Unavailable(format!("not hashed: cannot open: {error}")),
    };
    // Asked before a byte is read, so a file too large for what is left costs
    // nothing at all — neither the read nor the clock.
    let size = match file.metadata() {
        Ok(meta) => meta.len(),
        Err(error) => {
            return FileDigest::Unavailable(format!("not hashed: cannot size the file: {error}"));
        }
    };
    if size > budget.bytes_left() {
        return FileDigest::Unavailable(format!(
            "not hashed: {size} bytes exceeds the {} bytes left of the {} byte shared budget for \
             hashing discovered binaries",
            budget.bytes_left(),
            budget.bytes_total()
        ));
    }
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    loop {
        if budget.wall_exhausted() {
            return FileDigest::Unavailable(format!(
                "not hashed: exceeded the {:?} shared time budget for hashing discovered binaries",
                budget.wall()
            ));
        }
        match file.read(&mut buf) {
            Ok(0) => break,
            Ok(read) => {
                // Charged as read, not as sized: a file that grew under us must
                // not overrun the ceiling just because it was small when asked.
                budget.spend(read as u64);
                hasher.update(&buf[..read]);
            }
            // `read` may report `Interrupted` before transferring anything;
            // treating that as the end would silently hash a prefix.
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => {
                return FileDigest::Unavailable(format!(
                    "not hashed: read stopped before the end of the file: {error}"
                ));
            }
        }
    }
    FileDigest::Hashed(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_string_digest() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn abc_digest() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    /// Distinct temp directory per test.
    ///
    /// Nanosecond stamps collide when cargo runs these in parallel on macOS —
    /// the clock is coarser than the spawn — so the name is the pid plus a
    /// sequence this process owns.
    fn scratch() -> std::path::PathBuf {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let seq = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("devmap-sha256-{}-{seq}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn generous() -> Budget {
        Budget::new(Duration::from_secs(60), u64::MAX)
    }

    fn hashed(digest: FileDigest) -> String {
        match digest {
            FileDigest::Hashed(hex) => hex,
            FileDigest::Unavailable(why) => panic!("expected a digest, got: {why}"),
        }
    }

    fn unavailable(digest: FileDigest) -> String {
        match digest {
            FileDigest::Unavailable(why) => why,
            FileDigest::Hashed(hex) => panic!("expected a refusal, got: {hex}"),
        }
    }

    /// A file spanning several chunks must hash as its whole contents.
    ///
    /// The streaming loop is the part that can silently hash a prefix, and a
    /// fixture smaller than one chunk would never exercise it.
    #[test]
    fn a_multi_chunk_file_matches_its_contents() {
        let dir = scratch();
        let path = dir.join("payload.bin");
        let body: Vec<u8> = (0..(CHUNK * 3 + 517)).map(|i| (i % 251) as u8).collect();
        std::fs::write(&path, &body).unwrap();

        assert_eq!(
            hashed(sha256_file_within(&path, &mut generous())),
            sha256_hex(&body)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_file_says_why_rather_than_nothing() {
        let why = unavailable(sha256_file_within(
            Path::new("/nonexistent/devmap/sha256/fixture"),
            &mut generous(),
        ));
        assert!(why.starts_with("not hashed: cannot open"), "{why}");
    }

    /// A file larger than what the budget has left is refused before a byte is
    /// read, and the refusal names both numbers.
    ///
    /// Asked from the size rather than discovered mid-read, so an oversized file
    /// costs neither the read nor the clock.
    #[test]
    fn a_file_past_the_byte_ceiling_is_refused_before_it_is_read() {
        let dir = scratch();
        let path = dir.join("big.bin");
        std::fs::write(&path, vec![7u8; 4096]).unwrap();

        let mut budget = Budget::new(Duration::from_secs(60), 1024);
        let why = unavailable(sha256_file_within(&path, &mut budget));
        assert!(why.contains("4096 bytes exceeds"), "{why}");
        assert!(why.contains("1024 bytes left"), "{why}");
        // Nothing was read, so nothing was charged.
        assert_eq!(budget.bytes_left(), 1024, "an untouched file spent budget");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The ceiling is shared, not per file: the second of two files that each
    /// fit alone is refused once the first has spent the allowance.
    ///
    /// This is the property a per-file bound cannot give. Before the budget
    /// existed there was no bound at all, so this test cannot be written against
    /// the previous code.
    #[test]
    fn the_byte_ceiling_is_spent_across_files_not_granted_to_each() {
        let dir = scratch();
        let first = dir.join("a.bin");
        let second = dir.join("b.bin");
        std::fs::write(&first, vec![1u8; 3000]).unwrap();
        std::fs::write(&second, vec![2u8; 3000]).unwrap();

        let mut budget = Budget::new(Duration::from_secs(60), 4096);
        hashed(sha256_file_within(&first, &mut budget));
        assert_eq!(budget.bytes_left(), 1096, "the first file was not charged");
        let why = unavailable(sha256_file_within(&second, &mut budget));
        assert!(why.contains("1096 bytes left"), "{why}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An already-spent wall clock refuses without opening the file.
    #[test]
    fn an_exhausted_clock_refuses_before_opening_anything() {
        let mut budget = Budget::new(Duration::ZERO, u64::MAX);
        assert!(budget.wall_exhausted());
        let why = unavailable(sha256_file_within(Path::new("/etc/hosts"), &mut budget));
        assert!(why.contains("was already spent"), "{why}");
    }
}
