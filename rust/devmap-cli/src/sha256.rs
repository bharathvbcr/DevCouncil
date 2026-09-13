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
//! hashes the devmap binary to report build identity: measured 2026-09-12, that
//! was 3.67 s of a 3.7 s command against the 61 MiB release binary (~17 MiB/s),
//! and 34 s in a debug build — past the 20 s bound
//! `diagnostics_never_execute_discovered_commands` puts on the same call.

use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};

/// Bytes per read when hashing a file.
const CHUNK: usize = 64 * 1024;

/// Hex digest of a byte slice.
pub fn sha256_hex(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}

/// Hex digest of a file's contents, read in chunks.
///
/// Streamed rather than `fs::read`: the largest thing this hashes is the devmap
/// binary itself, and pulling 61 MiB into memory to hash it is a spike no
/// caller asked for — in a workspace that bounds every other payload it reads.
///
/// `None` on any read error, which is what every caller already renders as "no
/// digest available" rather than as a zero digest.
pub fn sha256_file(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    loop {
        match file.read(&mut buf) {
            Ok(0) => break,
            Ok(read) => hasher.update(&buf[..read]),
            // `read` may report `Interrupted` before transferring anything;
            // treating that as the end would silently hash a prefix.
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return None,
        }
    }
    Some(format!("{:x}", hasher.finalize()))
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

    /// A file spanning several chunks must hash as its whole contents.
    ///
    /// The streaming loop is the part that can silently hash a prefix, and a
    /// fixture smaller than one chunk would never exercise it.
    #[test]
    fn a_multi_chunk_file_matches_its_contents() {
        let dir = std::env::temp_dir().join(format!("devmap-sha256-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("payload.bin");
        let body: Vec<u8> = (0..(CHUNK * 3 + 517)).map(|i| (i % 251) as u8).collect();
        std::fs::write(&path, &body).unwrap();

        assert_eq!(sha256_file(&path).unwrap(), sha256_hex(&body));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_file_has_no_digest() {
        assert!(sha256_file(Path::new("/nonexistent/devmap/sha256/fixture")).is_none());
    }
}
