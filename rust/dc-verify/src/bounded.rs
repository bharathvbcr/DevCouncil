//! Bounded reads for the verifier's stdin and coverage file.
//!
//! A stream past the cap is an error, not an allocation of the rest of it.
//! One extra byte is read so a stream that stops exactly on the cap — which
//! is still a complete input — is not refused, and so a stream one byte over
//! can be told from one that ended on the cap. The extra byte is not scanned.

use std::borrow::Cow;
use std::io::Read;

/// The most bytes `dcverify check` will accept on stdin or from a coverage
/// file, and the most it will read back from a binary path the diff did not
/// hunk.
///
/// 32 MiB, the same cap as the Go host's `gitDiffLimit`. A read larger than
/// this is refused with `exceeds N bytes` and is not scanned: a partial scan
/// must not look like a complete one. A buffer of exactly this size is
/// accepted. Valid UTF-8 stays borrowed ([`decode_lossy`]); only a lossy
/// decode allocates a second copy.
pub const MAX_CHECK_BYTES: usize = 32 << 20;

/// Reads at most [`MAX_CHECK_BYTES`] plus one detector byte.
///
/// `what` is the noun in the error (`the diff`, `the coverage file`). The
/// error names the cap and does not include the bytes that were read.
pub fn read_bounded(reader: impl Read, what: &str) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    reader
        .take(MAX_CHECK_BYTES as u64 + 1)
        .read_to_end(&mut buf)
        .map_err(|e| format!("reading {what}: {e}"))?;
    if buf.len() > MAX_CHECK_BYTES {
        return Err(format!("{what} exceeds {MAX_CHECK_BYTES} bytes"));
    }
    Ok(buf)
}

/// Decodes `bytes` as UTF-8, borrowing them when they are already valid.
///
/// Invalid sequences become U+FFFD in an owned copy. Line breaks are not
/// inserted or removed, so a scanner's line numbers survive the substitution.
pub fn decode_lossy(bytes: &[u8]) -> Cow<'_, str> {
    String::from_utf8_lossy(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    struct Fill {
        left: u64,
        read: usize,
    }

    impl Read for Fill {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.left == 0 || buf.is_empty() {
                return Ok(0);
            }
            let n = buf.len().min(self.left as usize);
            buf[..n].fill(b'x');
            self.left -= n as u64;
            self.read += n;
            Ok(n)
        }
    }

    #[test]
    fn a_reader_claiming_a_gibibyte_stops_at_one_past_the_cap() {
        let mut src = Fill {
            left: 1 << 30,
            read: 0,
        };
        let err = read_bounded(&mut src, "the diff").unwrap_err();
        assert!(
            err.contains(&format!("exceeds {MAX_CHECK_BYTES} bytes")),
            "{err}"
        );
        assert!(src.read <= MAX_CHECK_BYTES + 1, "read {} bytes", src.read);
    }

    #[test]
    fn a_buffer_exactly_at_the_cap_is_accepted() {
        let mut src = Fill {
            left: MAX_CHECK_BYTES as u64,
            read: 0,
        };
        let got = read_bounded(&mut src, "the diff").expect("exact cap");
        assert_eq!(got.len(), MAX_CHECK_BYTES);
        assert_eq!(src.read, MAX_CHECK_BYTES);
    }

    #[test]
    fn valid_utf8_is_borrowed_and_a_bad_byte_is_owned() {
        let bytes = b"diff --git a/a b/a\n";
        assert!(matches!(decode_lossy(bytes), Cow::Borrowed(_)));
        let lossy = decode_lossy(b"SF:src/han\xffdler.rs\n");
        assert!(matches!(lossy, Cow::Owned(_)));
        assert!(lossy.contains('\u{FFFD}'));
    }
}
