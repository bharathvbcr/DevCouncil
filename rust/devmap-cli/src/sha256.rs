//! SHA-256 for doctor binary hashes. No extra crate.
//!
//! Streamed and bounded, because of where it runs. `inventory_devmap_binaries`
//! hashes every `devmap` it discovers — on `PATH`, in host MCP configs, and
//! this process's own executable — and both `devmap paths` and `devmap doctor`
//! call it. `paths` is the command the generated agent guide tells every agent
//! to run first, so the cost of this module is paid before any real work.
//!
//! It used to slurp: `fs::read` the whole file, then `data.to_vec()` a second
//! full copy, then `push` the `0x80` pad byte — which reallocated that copy to
//! twice its capacity. A 109 MB unoptimised binary therefore cost ~330 MB of
//! resident memory and, with the compression loop compiled at `-O0`, more than
//! half a minute of work that nothing bounded. The health check that exists to
//! report a wedged install *was* the wedge.
//!
//! Two properties fix that, and both are needed:
//!
//! * **Streaming.** [`Hasher`] takes the file a chunk at a time, so the cost is
//!   one fixed buffer rather than three copies of the file, whatever its size.
//! * **A wall clock the caller owns.** Streaming makes a hash cheaper; it does
//!   not make it bounded — a big enough file, a slow enough disk or a stalled
//!   network mount still runs as long as it likes. [`Budget`] is checked
//!   between chunks, and an exceeded budget returns
//!   [`FileDigest::Unavailable`] carrying the reason. The caller reports it as
//!   unavailable; it never reports it as "no hash" and never waits.
//!
//! One [`Budget`] is meant to be shared across a whole inventory rather than
//! handed out per file, for the reason `devmap_extract::subprocess` gives for
//! its own paired waits: *n* files each allowed the whole of it is not the
//! bound the caller was promised.

use std::io::Read;
use std::path::Path;
use std::time::{Duration, Instant};

const H0: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// One 64-byte block folded into the running state.
fn compress(h: &mut [u32; 8], block: &[u8; 64]) {
    let mut w = [0u32; 64];
    for i in 0..16 {
        w[i] = u32::from_be_bytes([
            block[i * 4],
            block[i * 4 + 1],
            block[i * 4 + 2],
            block[i * 4 + 3],
        ]);
    }
    for i in 16..64 {
        let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16]
            .wrapping_add(s0)
            .wrapping_add(w[i - 7])
            .wrapping_add(s1);
    }
    let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
        (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
    for i in 0..64 {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ ((!e) & g);
        let temp1 = hh
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(K[i])
            .wrapping_add(w[i]);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let temp2 = s0.wrapping_add(maj);
        hh = g;
        g = f;
        f = e;
        e = d.wrapping_add(temp1);
        d = c;
        c = b;
        b = a;
        a = temp1.wrapping_add(temp2);
    }
    h[0] = h[0].wrapping_add(a);
    h[1] = h[1].wrapping_add(b);
    h[2] = h[2].wrapping_add(c);
    h[3] = h[3].wrapping_add(d);
    h[4] = h[4].wrapping_add(e);
    h[5] = h[5].wrapping_add(f);
    h[6] = h[6].wrapping_add(g);
    h[7] = h[7].wrapping_add(hh);
}

/// Incremental SHA-256: feed it any sized pieces, take the digest at the end.
///
/// The only state is the 32-byte chain value and one partial block, so hashing
/// a file costs the read buffer and nothing that scales with its length.
pub struct Hasher {
    h: [u32; 8],
    /// The bytes of the block still being filled. Always fewer than 64 between
    /// calls — a block that completes is folded in and the count returns to 0.
    block: [u8; 64],
    buffered: usize,
    /// Message length in bytes. SHA-256 appends it in bits, modulo 2^64, which
    /// is what the wrapping arithmetic below is.
    len: u64,
}

impl Hasher {
    pub fn new() -> Self {
        Self {
            h: H0,
            block: [0u8; 64],
            buffered: 0,
            len: 0,
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.len = self.len.wrapping_add(data.len() as u64);
        if self.buffered > 0 {
            let take = (64 - self.buffered).min(data.len());
            self.block[self.buffered..self.buffered + take].copy_from_slice(&data[..take]);
            self.buffered += take;
            data = &data[take..];
            if self.buffered == 64 {
                compress(&mut self.h, &self.block);
                self.buffered = 0;
            }
        }
        while data.len() >= 64 {
            let (block, rest) = data.split_at(64);
            compress(
                &mut self.h,
                block.try_into().expect("split_at(64) yields 64 bytes"),
            );
            data = rest;
        }
        if !data.is_empty() {
            self.block[..data.len()].copy_from_slice(data);
            self.buffered = data.len();
        }
    }

    pub fn finish(mut self) -> [u8; 32] {
        let bit_len = self.len.wrapping_mul(8);
        // `buffered` is < 64 here — `update` never leaves a full block — so the
        // pad byte always has room.
        self.block[self.buffered] = 0x80;
        self.buffered += 1;
        if self.buffered > 56 {
            self.block[self.buffered..].fill(0);
            compress(&mut self.h, &self.block);
            self.buffered = 0;
        }
        self.block[self.buffered..56].fill(0);
        self.block[56..].copy_from_slice(&bit_len.to_be_bytes());
        compress(&mut self.h, &self.block);
        let mut out = [0u8; 32];
        for (i, word) in self.h.iter().enumerate() {
            out[i * 4..(i + 1) * 4].copy_from_slice(&word.to_be_bytes());
        }
        out
    }
}

impl Default for Hasher {
    fn default() -> Self {
        Self::new()
    }
}

fn hex(digest: [u8; 32]) -> String {
    use std::fmt::Write as _;
    digest.iter().fold(String::with_capacity(64), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

/// What an inventory of hashes may spend, shared across every file taken under
/// it rather than granted to each.
///
/// Two bounds, because they answer different failures and only one of them is
/// deterministic:
///
/// * **Bytes** are the primary bound, and they are the reason a result does not
///   depend on how busy the machine is. Hashing is O(size), so a byte ceiling
///   caps the work exactly — and a file that cannot fit under it is refused
///   *without being read*, so an enormous one costs nothing rather than costing
///   the whole clock.
/// * **The clock** is the backstop for what bytes cannot see: a filesystem that
///   answers slowly or not at all, where the work is small and the waiting is
///   not.
///
/// Bounding this by clock alone was tried and is wrong: the machine this was
/// measured on runs at a load average of ~280 across 18 cores, where an
/// unoptimised hash of 104 MiB costs 3.5 s of CPU but 32 s of wall clock. A
/// clock-only bound turns "is this binary the one I think it is" into
/// "unavailable" whenever the machine is busy, which is both nondeterministic
/// and the least useful moment to stop answering.
///
/// The clock is deliberately not an `Instant` deadline computed up front:
/// `Instant::now() + total` can overflow, and the failure mode of that is a
/// budget that reads as already spent. Elapsed-against-total cannot.
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

/// What a bounded hash attempt produced.
///
/// There is no third case that means "nothing to say". A file that was hashed
/// has a digest; a file that could not be hashed within the budget, or whose
/// bytes stopped arriving, says so and why. The caller must be able to tell a
/// hash that matched from a hash that never ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileDigest {
    Hashed(String),
    Unavailable(String),
}

/// Read size per clock check. The budget's resolution is therefore "one chunk",
/// which is well under a millisecond on a local disk and a few milliseconds in
/// an unoptimised build — fine against budgets measured in whole seconds, and
/// far cheaper than consulting the clock per 64-byte block.
const READ_CHUNK: usize = 1 << 20;

/// Hash `path`, giving up inside `budget` rather than taking as long as the
/// file and the filesystem require.
///
/// Errors are not swallowed. A read that stops before EOF leaves a *prefix*,
/// and a prefix's digest is not the file's digest — so it is reported
/// unavailable rather than returned as if it were an answer. `Interrupted` is
/// resumed, for the reason `devmap_extract::subprocess::drain` gives: `read`
/// may return it having transferred nothing, and `std` retries it only inside
/// `read_to_end`/`read_exact`, never on a bare `read`.
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
            return FileDigest::Unavailable(format!("not hashed: cannot size the file: {error}"))
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
    let mut hasher = Hasher::new();
    let mut chunk = vec![0u8; READ_CHUNK];
    loop {
        if budget.wall_exhausted() {
            return FileDigest::Unavailable(format!(
                "not hashed: exceeded the {:?} shared time budget for hashing discovered binaries",
                budget.wall()
            ));
        }
        match file.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => {
                // Charged as read, not as sized: a file that grew under us must
                // not overrun the ceiling just because it was small when asked.
                budget.spend(read as u64);
                hasher.update(&chunk[..read]);
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => {
                return FileDigest::Unavailable(format!(
                    "not hashed: read stopped before the end of the file: {error}"
                ))
            }
        }
    }
    FileDigest::Hashed(hex(hasher.finish()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One-shot hashing, kept for the known-answer vectors below. Nothing in
    /// the binary hashes a whole slice any more — a file is streamed — so this
    /// lives with the tests that need it rather than as public surface with no
    /// caller.
    fn sha256_hex(data: &[u8]) -> String {
        let mut hasher = Hasher::new();
        hasher.update(data);
        hex(hasher.finish())
    }

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

    /// The streaming rewrite must agree with the one-shot answer at every
    /// boundary the block and length padding can land on — a chunk that ends
    /// mid-block, exactly on one, and in the 56..64 window that forces the
    /// length into an extra block.
    #[test]
    fn streaming_in_pieces_agrees_with_hashing_in_one_go() {
        let data: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
        for length in [0usize, 1, 55, 56, 57, 63, 64, 65, 119, 120, 128, 129, 1000] {
            let whole = sha256_hex(&data[..length]);
            for split in [1usize, 7, 64, 63, 100] {
                let mut hasher = Hasher::new();
                for piece in data[..length].chunks(split) {
                    hasher.update(piece);
                }
                assert_eq!(
                    hex(hasher.finish()),
                    whole,
                    "length {length} fed in {split}-byte pieces"
                );
            }
        }
    }

    #[test]
    fn a_hashable_file_reports_its_digest() {
        let path = std::env::temp_dir().join(format!("devmap-sha-{}", std::process::id()));
        std::fs::write(&path, b"abc").unwrap();
        let digest = sha256_file_within(&path, &mut Budget::new(Duration::from_secs(30), 1 << 20));
        std::fs::remove_file(&path).ok();
        assert_eq!(
            digest,
            FileDigest::Hashed(
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into()
            )
        );
    }

    /// The property the hang was missing: a spent budget returns, and says so.
    #[test]
    fn a_spent_budget_reports_unavailable_instead_of_hashing() {
        let path = std::env::temp_dir().join(format!("devmap-sha-spent-{}", std::process::id()));
        std::fs::write(&path, b"abc").unwrap();
        let digest = sha256_file_within(&path, &mut Budget::new(Duration::ZERO, 1 << 20));
        std::fs::remove_file(&path).ok();
        match digest {
            FileDigest::Unavailable(reason) => assert!(reason.contains("time budget"), "{reason}"),
            FileDigest::Hashed(hash) => panic!("a spent budget must not hash: {hash}"),
        }
    }

    /// The bound that does not depend on how busy the machine is: a file too
    /// large for what is left is refused on its size, before a byte is read, so
    /// an enormous one costs nothing instead of costing the whole clock.
    #[test]
    fn a_file_larger_than_the_byte_budget_is_refused_without_being_read() {
        let path = std::env::temp_dir().join(format!("devmap-sha-big-{}", std::process::id()));
        std::fs::write(&path, vec![7u8; 4096]).unwrap();
        let mut budget = Budget::new(Duration::from_secs(30), 1024);
        let digest = sha256_file_within(&path, &mut budget);
        std::fs::remove_file(&path).ok();
        match digest {
            FileDigest::Unavailable(reason) => {
                assert!(reason.contains("4096 bytes exceeds"), "{reason}")
            }
            FileDigest::Hashed(hash) => panic!("4 KiB must not fit a 1 KiB budget: {hash}"),
        }
        assert_eq!(
            budget.bytes_left(),
            1024,
            "a refused file must not have spent any of the budget"
        );
    }

    /// …and the ceiling is shared, so the second file sees what the first spent.
    #[test]
    fn the_byte_budget_is_spent_across_files_not_granted_to_each() {
        let dir = std::env::temp_dir().join(format!("devmap-sha-share-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (first, second) = (dir.join("first"), dir.join("second"));
        std::fs::write(&first, vec![1u8; 800]).unwrap();
        std::fs::write(&second, vec![2u8; 800]).unwrap();
        let mut budget = Budget::new(Duration::from_secs(30), 1000);
        let one = sha256_file_within(&first, &mut budget);
        let two = sha256_file_within(&second, &mut budget);
        std::fs::remove_dir_all(&dir).ok();
        assert!(matches!(one, FileDigest::Hashed(_)), "{one:?}");
        match two {
            FileDigest::Unavailable(reason) => assert!(reason.contains("exceeds"), "{reason}"),
            FileDigest::Hashed(_) => {
                panic!("800 + 800 must not both fit a 1000 byte shared budget")
            }
        }
    }

    #[test]
    fn a_file_that_cannot_be_opened_is_unavailable_not_silently_absent() {
        let digest = sha256_file_within(
            Path::new("/no/such/devmap-binary"),
            &mut Budget::new(Duration::from_secs(30), 1 << 20),
        );
        assert!(matches!(digest, FileDigest::Unavailable(_)));
    }
}
